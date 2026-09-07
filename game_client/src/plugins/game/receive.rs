//! Transport receive, authoritative snapshots, and reconciliation dispatch.
use super::*;

pub(super) fn network_update(
    real_time: Res<Time<Real>>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
    mut world: ResMut<WorldView>,
    mut net_stats: ResMut<NetStats>,
    mut snapshot_buffer: ResMut<SnapshotBuffer>,
    mut local_sim: ResMut<LocalSimulation>,
    mut pending_actions: ResMut<PendingActions>,
    mut smoothing: ResMut<ReconciliationSmoothing>,
    mut hud_state: ResMut<HudState>,
    mut debug_overlay: ResMut<DebugOverlayState>,
    mut debug_bridge: ResMut<ClientDebugBridgeState>,
    render_index: Res<RenderIndex>,
    mut commands: Commands,
    scene_assets: Res<SceneAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };
    let runtime_inner: &mut NetworkRuntime = &mut runtime;

    // A terminal disconnect otherwise emits the same transport error every frame.
    if runtime_inner.renet.disconnect_reason().is_some() {
        return;
    }
    let dt_secs = real_time.delta_secs();
    let dt = real_time.delta();
    runtime_inner.renet.update(dt);
    if let Err(err) = runtime_inner.transport_update(dt) {
        debug_bridge.replication.transport_errors += 1;
        log::warn!("client transport update failed: {err}");
    }

    // Advance the interpolation render clock
    snapshot_buffer.advance_render_time(dt_secs);

    while let Some(bytes) = runtime_inner
        .renet
        .receive_message(DefaultChannel::ReliableOrdered)
    {
        match game_shared::decode::<ReliableServerMessage>(&bytes) {
            Ok(ReliableServerMessage::JoinSnapshot(snapshot)) => {
                acknowledge_pending_moves(
                    &mut runtime_inner.pending_moves,
                    snapshot.world.your_last_input_seq,
                );
                runtime_inner.client_id = snapshot.you;
                runtime_inner.match_epoch =
                    snapshot.world.sim_meta.map_or(0, |meta| meta.match_epoch);
                snapshot_buffer.clear();
                snapshot_buffer.push(TimestampedSnapshot {
                    server_tick: snapshot.world.tick,
                    receive_time: real_time.elapsed_secs(),
                    world: snapshot.world.clone(),
                });
                smoothing.hero_offset = [0.0, 0.0];
                smoothing.enemy_offsets.clear();
                smoothing.render_pos = None;

                // Initialize local simulation from join snapshot
                if let Some(meta) = snapshot.world.sim_meta {
                    local_sim.sim = Simulation::from_snapshot(&snapshot.world, &meta);
                    local_sim.predicted_tick = snapshot.world.tick;
                    local_sim.input_buffer.clear();
                    local_sim.initialized = true;
                }

                world.apply_join(snapshot);
                hud_state.last_event = "Joined authoritative match".to_owned();
            }
            Ok(ReliableServerMessage::Event(event)) => {
                hud_state.last_event = describe_event(event, runtime_inner.client_id);
            }
            Err(err) => {
                debug_bridge.replication.decode_errors += 1;
                log::warn!("failed to decode reliable message: {err}");
            }
        }
    }

    let wall_time = real_time.elapsed_secs();

    while let Some(bytes) = runtime_inner
        .renet
        .receive_message(DefaultChannel::Unreliable)
    {
        match game_shared::decode::<ServerWorldMessage>(&bytes) {
            Ok(msg) => {
                // Track message type for debug overlay
                let is_full = matches!(&msg, ServerWorldMessage::Full(_));
                debug_bridge.latest_server_message = Some(if is_full {
                    game_shared::DebugWorldMessageKind::Full
                } else {
                    game_shared::DebugWorldMessageKind::Patch
                });
                if debug_overlay.recent_msg_types.len() >= 32 {
                    debug_overlay.recent_msg_types.pop_front();
                }
                debug_overlay.recent_msg_types.push_back(is_full);

                let delta = match msg {
                    ServerWorldMessage::Full(d) => {
                        debug_bridge.replication.full_snapshots += 1;
                        d
                    }
                    ServerWorldMessage::Patch(patch) => {
                        debug_bridge.replication.patches += 1;
                        match reconstruct_from_patch(&snapshot_buffer, patch) {
                            Some(d) => d,
                            None => {
                                debug_bridge.replication.baseline_misses += 1;
                                continue;
                            }
                        }
                    }
                };

                // Unreliable traffic may beat the reliable join. Wait for initialization.
                if !local_sim.initialized
                    || !snapshot_buffer.push(TimestampedSnapshot {
                        server_tick: delta.tick,
                        receive_time: wall_time,
                        world: delta.clone(),
                    })
                {
                    continue;
                }
                if let Some(meta) = delta.sim_meta {
                    if meta.match_epoch != runtime_inner.match_epoch {
                        runtime_inner.match_epoch = meta.match_epoch;
                        runtime_inner.pending_moves.clear();
                        runtime_inner.unacked_actions.clear();
                        runtime_inner.last_generated_move = None;
                        pending_actions.0.clear();
                        local_sim.input_buffer.clear();
                        local_sim.enemy_hit_feedback = EnemyHitFeedback::default();
                        *smoothing = ReconciliationSmoothing::default();
                        net_stats.last_seq_send_times.clear();
                    }
                }
                net_stats.last_snapshot_received_secs = Some(real_time.elapsed_secs_f64());
                let hit_enemy_ids = detect_hit_enemy_ids(&world.enemies, &delta);
                let hit_hero_ids = detect_hit_hero_ids(&world.heroes, &delta);
                let fired_tower_ids = detect_fired_tower_ids(&world.towers, &delta);
                let mut attack_starts =
                    detect_regular_attack_starts(&world, &delta, runtime_inner.client_id);

                if let Some(hero) = delta
                    .heroes
                    .iter()
                    .find(|h| h.client_id == runtime_inner.client_id)
                {
                    if let Some(mut attack) = take_local_attack_effect(&mut local_sim, hero) {
                        attack.pos = smoothing.render_pos.unwrap_or(hero.pos);
                        attack_starts.push(attack);
                    }
                }

                for attack in attack_starts {
                    spawn_regular_attack_effect(
                        &mut commands,
                        &mut materials,
                        &scene_assets,
                        attack.pos,
                        attack.facing,
                        attack.range,
                        attack.enemy,
                    );
                }

                for enemy in &delta.enemies {
                    // Seed old authority when this actor has not been predicted yet.
                    if let Some(previous) = world.enemies.get(&enemy.id) {
                        local_sim.enemy_hit_feedback.seed(enemy.id, previous.hp);
                    }
                    if !local_sim.enemy_hit_feedback.observe(enemy.id, enemy.hp) {
                        continue;
                    }

                    spawn_hit_effect(&mut commands, &mut materials, &scene_assets, enemy.pos);
                    if let Some(entity) = render_index.by_id.get(&ActorKey::World(enemy.id)) {
                        commands.entity(*entity).insert(EnemyHitReaction::default());
                    }
                }

                local_sim.enemy_hit_feedback.retain(&delta.enemies);

                for hero in &delta.heroes {
                    if !hit_hero_ids.contains(&hero.client_id) {
                        continue;
                    }

                    spawn_hit_effect(&mut commands, &mut materials, &scene_assets, hero.pos);
                    if let Some(entity) = render_index.by_id.get(&ActorKey::Hero(hero.client_id)) {
                        commands.entity(*entity).insert(HeroHitReaction::default());
                    }
                }

                for tower in &delta.towers {
                    if !fired_tower_ids.contains(&tower.id) {
                        continue;
                    }

                    if let Some(entity) = render_index.by_id.get(&ActorKey::World(tower.id)) {
                        commands
                            .entity(*entity)
                            .insert(TowerFireReaction::default());
                    }

                    if let Some(target_pos) =
                        pick_tower_shot_target(tower, &delta.enemies, &hit_enemy_ids)
                    {
                        spawn_tower_shot_effect(
                            &mut commands,
                            &mut materials,
                            &scene_assets,
                            tower.pos,
                            target_pos,
                        );
                    }
                }

                // RTT measurement: match your_last_input_seq to recorded send times
                if let Some(ack_seq) = delta.your_last_input_seq {
                    if let Some(idx) = net_stats
                        .last_seq_send_times
                        .iter()
                        .position(|(seq, _)| *seq == ack_seq)
                    {
                        let (_, send_time) = net_stats.last_seq_send_times[idx];
                        let rtt_sample = wall_time - send_time;
                        if rtt_sample > 0.0 && rtt_sample < 2.0 {
                            net_stats.update_rtt_sample(rtt_sample);
                            net_stats.last_rtt_sample_secs = Some(real_time.elapsed_secs_f64());
                        }
                        net_stats.last_seq_send_times.drain(..=idx);
                    }
                }

                acknowledge_pending_moves(
                    &mut runtime_inner.pending_moves,
                    delta.your_last_input_seq,
                );
                if let Some(ack) = delta
                    .heroes
                    .iter()
                    .find(|h| h.client_id == runtime_inner.client_id)
                    .and_then(|h| h.last_action_seq)
                {
                    runtime_inner
                        .unacked_actions
                        .retain(|a| is_newer_input_seq(a.command.seq(), ack));
                }
                debug_bridge.latest_acked_input_seq = delta.your_last_input_seq;
                debug_bridge.latest_sim_meta = delta.sim_meta;

                // Push into snapshot buffer for remote hero interpolation
                snapshot_buffer.update_interpolation_delay(&net_stats);
                snapshot_buffer.sync_render_clock();

                // --- Server reconciliation ---
                if local_sim.initialized {
                    if let Some(meta) = delta.sim_meta {
                        reconcile_local_sim(
                            &mut local_sim,
                            &mut smoothing,
                            &delta,
                            &meta,
                            runtime_inner.client_id,
                        );
                    }
                }

                // Apply to world view for HUD scalars
                world.apply_delta(delta);
            }
            Err(err) => {
                debug_bridge.replication.decode_errors += 1;
                log::warn!("failed to decode ServerWorldMessage: {err}");
            }
        }
    }
}

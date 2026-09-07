//! Fixed-step local prediction and authoritative rollback/replay.
use super::*;

/// Rollback the local simulation to server-confirmed state and replay unacknowledged inputs.
pub(super) fn reconcile_local_sim(
    local_sim: &mut LocalSimulation,
    smoothing: &mut ReconciliationSmoothing,
    delta: &WorldDelta,
    meta: &SimMeta,
    my_id: u64,
) {
    let ack_seq = delta.your_last_input_seq;

    // Movement ACKs cannot discard delayed reliable actions in the same input frame.
    let action_ack = delta
        .heroes
        .iter()
        .find(|h| h.client_id == my_id)
        .and_then(|h| h.last_action_seq);
    for entry in &mut local_sim.input_buffer {
        entry.commands.retain(|command| {
            let ack = if matches!(command, ClientCommand::Move { .. }) {
                ack_seq
            } else {
                action_ack
            };
            ack.is_none_or(|ack| is_newer_input_seq(command.seq(), ack))
        });
    }
    local_sim
        .input_buffer
        .retain(|entry| !entry.commands.is_empty());

    // Save pre-reconciliation positions for visual smoothing
    let old_hero_delta = local_sim.sim.world_delta_for(my_id);
    let old_hero_pos = old_hero_delta
        .heroes
        .iter()
        .find(|h| h.client_id == my_id)
        .map(|h| h.pos);
    let mut old_enemy_positions: HashMap<u64, [f32; 2]> = HashMap::new();
    for enemy in &old_hero_delta.enemies {
        old_enemy_positions.insert(enemy.id, enemy.pos);
    }

    // Reset sim to server-confirmed state
    local_sim.sim.apply_snapshot(delta, meta);

    // Replay unacknowledged inputs
    for entry in &local_sim.input_buffer {
        for cmd in &entry.commands {
            local_sim.sim.queue_command(my_id, *cmd);
        }
        local_sim.sim.step();
    }
    local_sim.predicted_tick = local_sim.sim.tick();

    // Compute smoothing offsets (old predicted - new predicted)
    let new_hero_delta = local_sim.sim.world_delta_for(my_id);
    if let Some(old_pos) = old_hero_pos {
        if let Some(new_hero) = new_hero_delta.heroes.iter().find(|h| h.client_id == my_id) {
            smoothing.hero_offset[0] += old_pos[0] - new_hero.pos[0];
            smoothing.hero_offset[1] += old_pos[1] - new_hero.pos[1];
        }
    }

    // Update enemy smoothing offsets
    for enemy in &new_hero_delta.enemies {
        if let Some(old_pos) = old_enemy_positions.get(&enemy.id) {
            let offset = smoothing
                .enemy_offsets
                .entry(enemy.id)
                .or_insert([0.0, 0.0]);
            offset[0] += old_pos[0] - enemy.pos[0];
            offset[1] += old_pos[1] - enemy.pos[1];
        }
    }
    // Remove offsets for enemies no longer in sim
    let new_enemy_ids: HashSet<u64> = new_hero_delta.enemies.iter().map(|e| e.id).collect();
    smoothing
        .enemy_offsets
        .retain(|id, _| new_enemy_ids.contains(id));
}

pub(super) fn advance_local_simulation(
    time: Res<Time<Real>>,
    input_state: Res<InputState>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
    mut local_sim: ResMut<LocalSimulation>,
    mut pending_actions: ResMut<PendingActions>,
    mut net_stats: ResMut<NetStats>,
    mut effects: Commands,
    render_index: Res<RenderIndex>,
    smoothing: Res<ReconciliationSmoothing>,
    scene_assets: Res<SceneAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };

    if !runtime.renet.is_connected() || !local_sim.initialized {
        return;
    }

    let my_id = runtime.client_id;
    let seq = runtime.next_command_seq();
    runtime.last_generated_move = Some(seq);

    // Record send time for RTT measurement
    let wall_time = time.elapsed_secs();
    net_stats.last_seq_send_times.push_back((seq, wall_time));
    while net_stats.last_seq_send_times.len() > 256 {
        net_stats.last_seq_send_times.pop_front();
    }

    // Collect all commands for this tick: actions first, then move
    let mut commands: Vec<ClientCommand> = pending_actions.0.drain(..).collect();
    let move_cmd = ClientCommand::Move {
        seq,
        dir: input_state.dir,
    };
    commands.push(move_cmd);

    // Apply all commands to local sim
    for cmd in &commands {
        local_sim.sim.queue_command(my_id, *cmd);
    }
    let before = local_sim.sim.world_delta_for(my_id);
    for enemy in &before.enemies {
        local_sim.enemy_hit_feedback.seed(enemy.id, enemy.hp);
    }
    let tick_output = local_sim.sim.step();
    let predicted = local_sim.sim.world_delta_for(my_id);
    if let Some(hero) = predicted.heroes.iter().find(|h| h.client_id == my_id) {
        if let Some(attack) = take_local_attack_effect(&mut local_sim, hero) {
            spawn_regular_attack_effect(
                &mut effects,
                &mut materials,
                &scene_assets,
                attack.pos,
                attack.facing,
                attack.range,
                false,
            );
        }
    }

    for enemy in &predicted.enemies {
        if local_sim.enemy_hit_feedback.observe(enemy.id, enemy.hp) {
            let offset = smoothing
                .enemy_offsets
                .get(&enemy.id)
                .copied()
                .unwrap_or([0.0; 2]);
            spawn_hit_effect(
                &mut effects,
                &mut materials,
                &scene_assets,
                [enemy.pos[0] + offset[0], enemy.pos[1] + offset[1]],
            );
            if let Some(entity) = render_index.by_id.get(&ActorKey::World(enemy.id)) {
                effects.entity(*entity).insert(EnemyHitReaction::default());
            }
        }
    }
    // Lethal hits remove the enemy during the step, so use its pre-step location.
    for event in &tick_output.reliable_events {
        if let ReliableGameEvent::EnemyKilled { enemy_id, .. } = event {
            if local_sim.enemy_hit_feedback.observe(*enemy_id, 0.0) {
                if let Some(enemy) = before.enemies.iter().find(|e| e.id == *enemy_id) {
                    spawn_hit_effect(&mut effects, &mut materials, &scene_assets, enemy.pos);
                    if let Some(entity) = render_index.by_id.get(&ActorKey::World(*enemy_id)) {
                        effects.entity(*entity).insert(EnemyHitReaction::default());
                    }
                }
            }
        }
    }

    // Store in input buffer for reconciliation replay
    local_sim.input_buffer.push_back(InputEntry { commands });
    // Prevent unbounded buffer growth
    while local_sim.input_buffer.len() > 256 {
        local_sim.input_buffer.pop_front();
    }
    local_sim.predicted_tick = local_sim.predicted_tick.wrapping_add(1);

    // Retain movement for the independent sender and packet-loss redundancy.
    runtime.pending_moves.push_back(PendingMove {
        seq,
        dir: input_state.dir,
    });
    while runtime.pending_moves.len() > 256 {
        runtime.pending_moves.pop_front();
    }
}

/// Effect identity survives reconciliation; replay itself never spawns effects.
pub(super) fn take_local_attack_effect(
    local: &mut LocalSimulation,
    hero: &HeroSnapshot,
) -> Option<RegularAttackStart> {
    let seq = hero.last_attack_seq?;
    if hero.regular_attack.phase != AttackPhase::Windup
        || local
            .last_attack_effect_seq
            .is_some_and(|last| !is_newer_input_seq(seq, last))
    {
        return None;
    }
    local.last_attack_effect_seq = Some(seq);
    Some(RegularAttackStart {
        pos: hero.pos,
        facing: hero.facing.dir,
        range: hero_regular_attack_effect_range(hero),
        enemy: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walking_attack_keeps_predicted_position_through_partial_acknowledgments() {
        for walk_ticks in [1, 5, 10, 16] {
            let mut server = Simulation::new();
            server.add_player(1);
            let mut local = LocalSimulation::default();
            local.sim = Simulation::from_snapshot(&server.world_delta_for(1), &server.sim_meta());
            local.initialized = true;
            for seq in 1..=walk_ticks {
                let cmd = ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                };
                local.sim.queue_command(1, cmd);
                local.sim.step();
                local.input_buffer.push_back(InputEntry {
                    commands: vec![cmd],
                });
            }
            let attack = ClientCommand::BasicAttack {
                seq: walk_ticks + 1,
            };
            let movement = ClientCommand::Move {
                seq: walk_ticks + 2,
                dir: [1.0, 0.0],
            };
            local.sim.queue_command(1, attack);
            local.sim.queue_command(1, movement);
            local.sim.step();
            local.input_buffer.push_back(InputEntry {
                commands: vec![attack, movement],
            });
            let expected = local.sim.world_delta_for(1).heroes[0].pos;
            let envelope = game_shared::ClientAction {
                view_tick: None,
                match_epoch: 0,
                command: attack,
                after_move_seq: Some(walk_ticks),
            };
            // Action arrives first; movements arrive as one redundant burst.
            server.queue_action(1, envelope);
            for seq in 1..=walk_ticks {
                server.queue_command(
                    1,
                    ClientCommand::Move {
                        seq,
                        dir: [1.0, 0.0],
                    },
                );
            }
            server.queue_command(1, movement);
            for _ in 0..=walk_ticks {
                server.queue_action(1, envelope);
                server.step();
                let delta = server.world_delta_for(1);
                reconcile_local_sim(
                    &mut local,
                    &mut ReconciliationSmoothing::default(),
                    &delta,
                    &server.sim_meta(),
                    1,
                );
                assert_eq!(
                    local.sim.world_delta_for(1).heroes[0].pos,
                    expected,
                    "walk length {walk_ticks}, server tick {}",
                    server.tick()
                );
            }
            assert!(local.input_buffer.is_empty());
        }
    }

    #[test]
    fn predicted_effect_is_not_repeated_by_authority_or_other_actions() {
        let mut local = LocalSimulation::default();
        local.sim.add_player(1);
        local
            .sim
            .queue_command(1, ClientCommand::BasicAttack { seq: 1 });
        local.sim.step();
        let hero = local.sim.world_delta_for(1).heroes.remove(0);
        assert!(take_local_attack_effect(&mut local, &hero).is_some());
        assert!(take_local_attack_effect(&mut local, &hero).is_none());
        let mut updated = hero.clone();
        updated.last_action_seq = Some(2);
        assert!(take_local_attack_effect(&mut local, &updated).is_none());
        let mut fresh = LocalSimulation::default();
        assert!(take_local_attack_effect(&mut fresh, &hero).is_some()); // authority wins at low latency
    }
}

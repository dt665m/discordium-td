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
    let ack_seq = delta.last_move_seq_for(my_id);
    if delta.phase != MatchPhase::InProgress
        || local_sim.sim.sim_meta().match_epoch != meta.match_epoch
    {
        local_sim.input_buffer.clear();
        local_sim.sim.apply_snapshot(delta, meta);
        local_sim.predicted_tick = delta.tick;
        *smoothing = ReconciliationSmoothing::default();
        return;
    }

    // Movement ACKs cannot discard delayed reliable actions in the same input frame.
    let action_ack = delta
        .heroes
        .iter()
        .find(|h| h.client_id == my_id)
        .and_then(|h| h.last_action_seq);
    for entry in &mut local_sim.input_buffer {
        if entry.movement.as_ref().is_some_and(|movement| {
            ack_seq.is_some_and(|ack| !is_newer_input_seq(movement.seq, ack))
        }) {
            entry.movement = None;
        }
        entry.actions.retain(|action| {
            action_ack.is_none_or(|ack| is_newer_input_seq(action.command.seq(), ack))
        });
    }
    // Retire stale movement time while preserving reliable action envelopes and
    // their original ordering barriers through restore, replay, and recovery.
    let mut moves = 0;
    for entry in local_sim.input_buffer.iter_mut().rev() {
        if entry.movement.is_some() {
            moves += 1;
            if moves > game_shared::MAX_PREDICTION_TICKS {
                entry.movement = None;
            }
        }
    }
    local_sim.input_buffer.retain(|entry| !entry.is_empty());

    // Save pre-reconciliation positions for visual smoothing
    let old_hero_delta = local_sim.sim.world_delta();
    let old_generation = old_hero_delta
        .heroes
        .iter()
        .find(|h| h.client_id == my_id)
        .map(|h| h.respawn_generation);
    let old_hero_pos = old_hero_delta
        .heroes
        .iter()
        .find(|h| h.client_id == my_id)
        .map(|h| h.pos);
    let mut old_enemy_positions = HashMap::new();
    for enemy in &old_hero_delta.enemies {
        old_enemy_positions.insert(enemy.id, (enemy.spawn, enemy.pos));
    }

    // Reset sim to server-confirmed state
    local_sim.sim.apply_snapshot(delta, meta);

    // Replay unacknowledged inputs
    let mut replayed_movement = false;
    for entry in &local_sim.input_buffer {
        entry.queue(&mut local_sim.sim, my_id);
        // Retained actions do not represent additional movement time.
        if entry.movement.is_some() {
            local_sim.sim.step();
            replayed_movement = true;
        }
    }
    // With no newer movement, preview all still-pending actions in one future
    // tick. The action count must never extend the replay horizon by itself.
    if !replayed_movement && !local_sim.input_buffer.is_empty() {
        local_sim.sim.step();
    }
    local_sim.predicted_tick = local_sim.sim.tick();

    // Compute smoothing offsets (old predicted - new predicted)
    let new_hero_delta = local_sim.sim.world_delta();
    if let Some(old_pos) = old_hero_pos {
        if let Some(new_hero) = new_hero_delta.heroes.iter().find(|h| h.client_id == my_id) {
            if old_generation == Some(new_hero.respawn_generation) {
                smoothing.hero_offset[0] += old_pos[0] - new_hero.pos[0];
                smoothing.hero_offset[1] += old_pos[1] - new_hero.pos[1];
            } else {
                smoothing.hero_offset = [0.0; 2];
                smoothing.render_pos = None;
            }
        }
    }

    // Update enemy smoothing offsets
    for enemy in &new_hero_delta.enemies {
        if let Some((_, old_pos)) = old_enemy_positions
            .get(&enemy.id)
            .filter(|(spawn, _)| *spawn == enemy.spawn)
        {
            let offset = smoothing
                .enemy_offsets
                .entry(enemy.id)
                .or_insert([0.0, 0.0]);
            offset[0] += old_pos[0] - enemy.pos[0];
            offset[1] += old_pos[1] - enemy.pos[1];
        } else {
            smoothing.enemy_offsets.remove(&enemy.id);
            local_sim.enemy_hit_feedback.forget(enemy.id);
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
    world: Res<WorldView>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
    mut local_sim: ResMut<LocalSimulation>,
    mut pending_actions: ResMut<PendingActions>,
    mut net_stats: ResMut<NetStats>,
    mut effects: Commands,
    render_index: Res<NetEntityIndex>,
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

    if world.phase != MatchPhase::InProgress {
        pending_actions.0.clear();
        return;
    }
    if local_sim.predicted_tick.wrapping_sub(world.tick) >= game_shared::MAX_PREDICTION_TICKS as u32
    {
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

    let entry = InputEntry {
        actions: pending_actions.0.drain(..).collect(),
        movement: Some(PendingMove {
            seq,
            dir: input_state.dir,
        }),
    };
    entry.queue(&mut local_sim.sim, my_id);
    let before = local_sim.sim.world_delta();
    for enemy in &before.enemies {
        local_sim.enemy_hit_feedback.seed(enemy.id, enemy.hp);
    }
    let tick_output = local_sim.sim.step();
    let predicted = local_sim.sim.world_delta();
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
            if let Some(entity) = render_index.get(&NetId::new(
                world.match_epoch,
                game_shared::actor_namespace::ENEMY,
                enemy.id,
            )) {
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
                    if let Some(entity) = render_index.get(&NetId::new(
                        world.match_epoch,
                        game_shared::actor_namespace::ENEMY,
                        *enemy_id,
                    )) {
                        effects.entity(*entity).insert(EnemyHitReaction::default());
                    }
                }
            }
        }
    }

    // Store in input buffer for reconciliation replay
    local_sim.input_buffer.push_back(entry);
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
    fn different_spawn_under_the_same_predicted_id_clears_correction() {
        let mut local = LocalSimulation::default();
        local.sim.add_player(1);
        for _ in 0..=game_shared::WAVE_PREP_TICKS {
            local.sim.step();
        }
        let mut authoritative = local.sim.world_delta();
        let id = authoritative.enemies[0].id;
        authoritative.enemies[0].spawn.ordinal += 1;
        authoritative.enemies[0].pos[1] += 8.0;
        let mut smoothing = ReconciliationSmoothing::default();
        smoothing.enemy_offsets.insert(id, [2.0, 0.0]);
        let meta = local.sim.sim_meta();
        reconcile_local_sim(&mut local, &mut smoothing, &authoritative, &meta, 1);
        assert!(!smoothing.enemy_offsets.contains_key(&id));
        assert_eq!(local.sim.world_delta(), authoritative);
    }

    #[test]
    fn recovered_actions_use_the_same_ordered_barriers_as_the_server() {
        let mut server = Simulation::new();
        server.add_player(1);
        server.queue_command(
            1,
            ClientCommand::Move {
                seq: 2,
                dir: [0.0; 2],
            },
        );
        server.step();
        let a = game_shared::ClientAction {
            match_epoch: 0,
            view_tick: None,
            after_move_seq: Some(2),
            command: ClientCommand::CastAbility {
                seq: 3,
                ability: AbilityId::ArcBurst,
            },
        };
        let b = game_shared::ClientAction {
            match_epoch: 0,
            view_tick: None,
            after_move_seq: Some(2),
            command: ClientCommand::SetCharging {
                seq: 5,
                active: true,
            },
        };
        server.queue_action(1, a);
        let mut predicted = Simulation::from_snapshot(&server.world_delta(), &server.sim_meta());
        let entry = InputEntry {
            movement: Some(PendingMove {
                seq: 6,
                dir: [0.0; 2],
            }),
            actions: vec![a, b],
        };
        entry.queue(&mut predicted, 1);
        predicted.step();
        server.queue_action(1, b);
        server.queue_command(
            1,
            ClientCommand::Move {
                seq: 6,
                dir: [0.0; 2],
            },
        );
        server.step();
        assert_eq!(predicted.world_delta(), server.world_delta());
        assert_eq!(
            predicted.presentations().len(),
            1,
            "the earlier ability cannot be discarded by the newer action ACK"
        );
    }
    #[test]
    fn new_epoch_does_not_reintroduce_the_old_position_offset() {
        let mut local = LocalSimulation::default();
        local.sim.add_player(1);
        let mut delta = local.sim.world_delta();
        delta.heroes[0].pos = [15.0, 0.0];
        let mut meta = local.sim.sim_meta();
        meta.match_epoch += 1;
        delta.sim_meta = Some(meta);
        let mut smoothing = ReconciliationSmoothing::default();
        reconcile_local_sim(&mut local, &mut smoothing, &delta, &meta, 1);
        assert_eq!(smoothing.hero_offset, [0.0; 2]);
        assert_eq!(local.sim.world_delta(), delta);
    }

    #[test]
    fn terminal_authority_cannot_be_replayed_into_a_new_epoch() {
        let mut server = Simulation::new();
        server.add_player(1);
        let mut delta = server.world_delta();
        delta.phase = MatchPhase::Defeat;
        delta.match_restart_ticks_remaining = Some(2);
        let mut local = LocalSimulation::default();
        for seq in 1..200 {
            local
                .input_buffer
                .push_back(InputEntry::for_test(vec![ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                }]));
        }
        reconcile_local_sim(
            &mut local,
            &mut ReconciliationSmoothing::default(),
            &delta,
            &server.sim_meta(),
            1,
        );
        assert_eq!(local.sim.tick(), delta.tick);
        assert_eq!(
            local.sim.sim_meta().match_epoch,
            server.sim_meta().match_epoch
        );
        assert!(local.input_buffer.is_empty());
    }
    #[test]
    fn replay_work_is_bounded_after_missing_authority() {
        let mut server = Simulation::new();
        server.add_player(1);
        let mut local = LocalSimulation::default();
        for seq in 1..200 {
            local
                .input_buffer
                .push_back(InputEntry::for_test(vec![ClientCommand::Move {
                    seq,
                    dir: [0.0; 2],
                }]));
        }
        reconcile_local_sim(
            &mut local,
            &mut ReconciliationSmoothing::default(),
            &server.world_delta(),
            &server.sim_meta(),
            1,
        );
        assert_eq!(local.sim.tick(), game_shared::MAX_PREDICTION_TICKS as u32);
        assert_eq!(local.input_buffer.len(), game_shared::MAX_PREDICTION_TICKS);
    }
    #[test]
    fn respawn_clears_local_correction_history() {
        let mut server = Simulation::new();
        server.add_player(1);
        let mut delta = server.world_delta();
        let mut local = LocalSimulation::default();
        local.sim = Simulation::from_snapshot(&delta, &server.sim_meta());
        delta.heroes[0].respawn_generation += 1;
        delta.heroes[0].pos = [10.0, 10.0];
        let mut smoothing = ReconciliationSmoothing::default();
        smoothing.hero_offset = [3.0, 1.0];
        smoothing.render_pos = Some([0.0; 2]);
        reconcile_local_sim(&mut local, &mut smoothing, &delta, &server.sim_meta(), 1);
        assert_eq!(smoothing.hero_offset, [0.0; 2]);
        assert!(smoothing.render_pos.is_none());
    }

    #[test]
    fn delayed_action_does_not_add_a_replay_tick_after_its_move_was_acked() {
        let mut server = Simulation::new();
        server.add_player(1);
        let action = ClientCommand::SetLockTarget {
            seq: 1,
            target_id: None,
        };
        let moves = (2..=4)
            .map(|seq| ClientCommand::Move {
                seq,
                dir: [1.0, 0.0],
            })
            .collect::<Vec<_>>();
        let mut local = LocalSimulation::default();
        local.initialized = true;
        local.sim = Simulation::from_snapshot(&server.world_delta(), &server.sim_meta());
        for (index, movement) in moves.iter().enumerate() {
            let commands = if index == 0 {
                vec![action, *movement]
            } else {
                vec![*movement]
            };
            for command in &commands {
                local.sim.queue_command(1, *command);
            }
            local.sim.step();
            local.input_buffer.push_back(InputEntry::for_test(commands));
        }
        server.queue_command(1, moves[0]);
        server.step();
        let delta = server.world_delta();
        reconcile_local_sim(
            &mut local,
            &mut ReconciliationSmoothing::default(),
            &delta,
            &server.sim_meta(),
            1,
        );
        assert_eq!(
            local.sim.tick(),
            delta.tick + 2,
            "only the two unacknowledged movement frames advance time"
        );
        assert_eq!(
            local.input_buffer.len(),
            3,
            "the delayed action remains available for a later snapshot replay"
        );
    }

    #[test]
    fn walking_attack_keeps_predicted_position_through_partial_acknowledgments() {
        // Bursts within the input-delay budget preserve every movement. Larger
        // bursts intentionally retire stale time (covered by recovery tests).
        for walk_ticks in [1, 2] {
            let mut server = Simulation::new();
            server.add_player(1);
            let mut local = LocalSimulation::default();
            local.sim = Simulation::from_snapshot(&server.world_delta(), &server.sim_meta());
            local.initialized = true;
            for seq in 1..=walk_ticks {
                let cmd = ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                };
                local.sim.queue_command(1, cmd);
                local.sim.step();
                local
                    .input_buffer
                    .push_back(InputEntry::for_test(vec![cmd]));
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
            local
                .input_buffer
                .push_back(InputEntry::for_test(vec![attack, movement]));
            let expected = local.sim.world_delta().heroes[0].pos;
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
                let delta = server.world_delta();
                reconcile_local_sim(
                    &mut local,
                    &mut ReconciliationSmoothing::default(),
                    &delta,
                    &server.sim_meta(),
                    1,
                );
                assert_eq!(
                    local.sim.world_delta().heroes[0].pos,
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
        let hero = local.sim.world_delta().heroes.remove(0);
        assert!(take_local_attack_effect(&mut local, &hero).is_some());
        assert!(take_local_attack_effect(&mut local, &hero).is_none());
        let mut updated = hero.clone();
        updated.last_action_seq = Some(2);
        assert!(take_local_attack_effect(&mut local, &updated).is_none());
        let mut fresh = LocalSimulation::default();
        assert!(take_local_attack_effect(&mut fresh, &hero).is_some()); // authority wins at low latency
    }
}

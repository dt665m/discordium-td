use crate::combat::*;
use crate::*;
fn arena() -> DreamSimulation {
    let mut sim = super::combat::arena(1);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.action.due = false;
        enemy.action.recovery = 100.0;
        enemy.motor.position = [0.0, -3.0];
    }
    sim
}
fn online(sequence: u64, query: u32) -> CombatAction {
    CombatAction::Ray(ValidatedRay {
        command_fraction: None,
        query_fraction: 0,
        key: RayActionKey {
            match_epoch: 1,
            connection_epoch: 9,
            command_stream: 1,
            ownership_epoch: 1,
            actor: 1,
            actor_generation: 1,
            command_sequence: sequence,
            action_slot: 0,
        },
        execution_server_tick: u64::from(query) + 1,
        query_server_tick: u64::from(query),
        query_gameplay_tick: query,
        aim: [0.0, -1.0],
    })
}
#[test]
fn dreamlance_current_pose_is_authoritative_and_cooldown_duplicate_safe() {
    let mut sim = arena();
    let action = CombatAction::RayCurrent {
        actor: 1,
        aim: [0.0, -1.0],
    };
    let result = sim
        .step_multiplayer_with_actions(&[], &[action.clone()])
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].reason, ActionReason::Accepted);
    let combat = result[0].combat.as_ref().unwrap();
    assert_eq!(combat.reason, CombatReason::Hit);
    assert_eq!(combat.damage, 32.0);
    assert_eq!(combat.execution_gameplay_tick, 1);
    assert_eq!(combat.query_gameplay_tick, 1);
    assert!(combat.transaction.is_some());
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 5);
    let again = sim.step_multiplayer_with_actions(&[], &[action]).unwrap();
    assert_eq!(again[0].reason, ActionReason::Cooldown);
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 5);
    assert_eq!(sim.snapshot().enemies[0].hp, 168.0);
}
#[test]
fn duplicated_online_action_has_one_transaction_and_cannot_fire_after_replay() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    let action = online(11, 1);
    let result = sim
        .step_multiplayer_with_actions(&[], &[action.clone(), action.clone()])
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].reason, ActionReason::Accepted);
    let before = sim.snapshot();
    let replayed = sim.step_multiplayer_with_actions(&[], &[action]).unwrap();
    assert_eq!(replayed[0].reason, ActionReason::InvalidAction);
    assert_eq!(
        sim.snapshot().hero.dreamlance_ammo,
        before.hero.dreamlance_ammo
    );
    assert_eq!(sim.snapshot().enemies[0].hp, before.enemies[0].hp);
}
#[test]
fn historical_target_and_current_defense_follow_the_declared_mixed_time_policy() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [4.0, -3.0];
        enemy.combat.grant_shield(
            50.0,
            7.0,
            engine_core::ShieldPolicy::Replace,
            engine_core::DurationPolicy::Reset,
        );
        enemy
            .combat
            .grant_invulnerability(1.0, engine_core::DurationPolicy::Reset);
    }
    let result = sim
        .step_multiplayer_with_actions(&[], &[online(1, 1)])
        .unwrap();
    assert_eq!(result[0].combat.as_ref().unwrap().reason, CombatReason::Hit);
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.enemies[0].hp, 168.0);
    assert_eq!(
        snapshot.state.enemies[0].combat.shield, 50.0,
        "a newer defensive credit is untouched by old-time damage"
    );
}
#[test]
fn historical_invulnerability_blocks_even_after_it_ends() {
    let mut sim = arena();
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy
            .combat
            .grant_invulnerability(1.0, engine_core::DurationPolicy::Reset);
    }
    sim.step(DreamInput::default());
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.combat.invulnerability_remaining = 0.0;
    }
    let result = sim
        .step_multiplayer_with_actions(&[], &[online(1, 1)])
        .unwrap();
    assert_eq!(
        result[0].combat.as_ref().unwrap().reason,
        CombatReason::HistoricalInvulnerability
    );
    assert_eq!(sim.snapshot().enemies[0].hp, 200.0);
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 5);
}
#[test]
fn thin_static_wall_blocks_ray_without_spending_a_target_transaction() {
    let mut sim = arena();
    let original = sim
        .world
        .resource::<collision::CollisionWorld>()
        .manifest()
        .clone();
    let mut colliders = original.colliders().to_vec();
    colliders.push(engine_core::StaticCollider::cuboid(
        engine_core::ColliderKey {
            index: 1000,
            generation: 1,
        },
        [0.0, 1.0, -1.2],
        [1.0, 1.0, 0.001],
    ));
    let manifest =
        collision::CollisionManifest::from_parts(1, original.config(), colliders).unwrap();
    sim.world.insert_resource(manifest.build().unwrap());
    let result = sim
        .step_multiplayer_with_actions(
            &[],
            &[CombatAction::RayCurrent {
                actor: 1,
                aim: [0.0, -1.0],
            }],
        )
        .unwrap();
    let result = result[0].combat.as_ref().unwrap();
    assert_eq!(result.reason, CombatReason::StaticBlocker);
    assert_eq!(result.transaction, None);
    assert_eq!(sim.snapshot().enemies[0].hp, 200.0);
}
#[test]
fn missing_history_and_respawn_reject_without_spending_ammo() {
    let mut sim = arena();
    let result = sim
        .step_multiplayer_with_actions(&[], &[online(1, 0)])
        .unwrap();
    assert_eq!(result[0].reason, ActionReason::MissingHistory);
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 6);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.combat_identity.discontinuity(true).unwrap();
    }
    let result = sim
        .step_multiplayer_with_actions(&[], &[online(2, 1)])
        .unwrap();
    assert_eq!(
        result[0].combat.as_ref().unwrap().reason,
        CombatReason::WrongLifecycle
    );
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 6);
}
#[test]
fn checkpoint_restore_keeps_late_ray_history_ammo_and_transaction_identity() {
    let mut sim = arena();
    for _ in 0..3 {
        sim.step(DreamInput::default());
    }
    let checkpoint = DreamCheckpoint::new(sim.snapshot(), 1).unwrap();
    let mut replay = DreamSimulation::try_from_checkpoint(
        &DreamCheckpoint::decode(&checkpoint.encode().unwrap(), 1).unwrap(),
        1,
    )
    .unwrap();
    let request = online(123, 1);
    let a = sim
        .step_multiplayer_with_actions(&[], &[request.clone()])
        .unwrap();
    let b = replay
        .step_multiplayer_with_actions(&[], &[request])
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(sim.snapshot(), replay.snapshot());
}
#[test]
fn explicit_cast_dash_and_extra_edge_have_gameplay_receipts() {
    let mut sim = arena();
    let CombatAction::Ray(ray) = online(1, 1) else {
        unreachable!()
    };
    let cast = CombatAction::Cast {
        key: ray.key,
        execution_server_tick: 1,
        slot: 3,
        aim: [0.0, -1.0],
    };
    let mut dash_key = ray.key;
    dash_key.action_slot = 1;
    let dash = CombatAction::Dash {
        key: dash_key,
        execution_server_tick: 1,
        direction: [1.0, 0.0],
    };
    let result = sim
        .step_multiplayer_with_actions(&[], &[dash, cast])
        .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].reason, ActionReason::Accepted);
    assert_eq!(result[1].reason, ActionReason::Conflict);
    assert!(result.iter().all(|v| v.execution_gameplay_tick == 1));
}
#[test]
fn restricted_owner_predicts_only_ray_cost_and_replays_same_duplicate_rules() {
    use crate::replication::{OwnerPredictionState, ReplicationStamp};
    let mut sim = arena();
    sim.step(DreamInput::default());
    let stamp = ReplicationStamp {
        match_epoch: 1,
        server_tick: 1,
        gameplay_tick: 1,
        scene_revision: 1,
        revision: 1,
    };
    let cp = sim
        .capture_replication(stamp)
        .unwrap()
        .owner_checkpoint(1, 1)
        .unwrap();
    let collision = sim.world.resource::<collision::CollisionWorld>().clone();
    let bases = sim
        .snapshot()
        .state
        .platforms
        .iter()
        .map(|p| p.presentation(1))
        .collect::<Vec<_>>();
    let mut predicted = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    let CombatAction::Ray(ray) = online(123, 1) else {
        unreachable!()
    };
    assert_eq!(predicted.combat_generation(), ray.key.actor_generation);
    predicted
        .step_restricted_combat_with_bases(
            DreamInput::default(),
            &collision,
            &bases,
            Some(crate::replication::OwnerCombatAction::Ray {
                key: ray.key,
                aim: ray.aim,
            }),
            None,
        )
        .unwrap();
    sim.step_multiplayer_with_actions(&[], &[CombatAction::Ray(ray)])
        .unwrap();
    assert_eq!(
        predicted.hero_view().dreamlance_ammo,
        sim.snapshot().hero.dreamlance_ammo
    );
    assert_eq!(
        predicted.hero_view().dreamlance_cooldown,
        sim.snapshot().hero.dreamlance_cooldown
    );
    for _ in 0..40 {
        predicted
            .step_restricted_combat_with_bases(
                DreamInput::default(),
                &collision,
                &bases,
                Some(crate::replication::OwnerCombatAction::Ray {
                    key: ray.key,
                    aim: ray.aim,
                }),
                None,
            )
            .unwrap();
    }
    assert_eq!(predicted.hero_view().dreamlance_ammo, 5);
    assert_eq!(predicted.hero_view().dreamlance_cooldown, 0.0);
}

fn beam_sample(sequence: u64, query: u32) -> ValidatedRay {
    let CombatAction::Ray(ray) = online(sequence, query) else {
        unreachable!()
    };
    ray
}
#[test]
fn held_beam_uses_latest_accepted_sample_and_has_no_recurring_edge_receipts() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    let begin = beam_sample(1, 1);
    let result = sim
        .step_multiplayer_with_actions(
            &[],
            &[CombatAction::BeamBegin(begin), CombatAction::BeamAim(begin)],
        )
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].reason, ActionReason::Accepted);
    assert_eq!(sim.snapshot().enemies[0].hp, 196.0);
    let graphic = sim.snapshot().presentations[0];
    assert!(matches!(
        graphic.kind,
        engine_core::GraphicsKind::Beam { .. }
    ));
    for sequence in 2..=7 {
        let sample = beam_sample(sequence, sim.snapshot().tick);
        let result = sim
            .step_multiplayer_with_actions(&[], &[CombatAction::BeamAim(sample)])
            .unwrap();
        assert!(result.is_empty());
    }
    assert_eq!(sim.snapshot().enemies[0].hp, 192.0);
    assert_eq!(sim.snapshot().presentations[0].id, graphic.id);
    let mut stop = begin.key;
    stop.command_sequence = 8;
    let result = sim
        .step_multiplayer_with_actions(
            &[],
            &[CombatAction::BeamStop {
                key: stop,
                execution_server_tick: 9,
            }],
        )
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].reason, ActionReason::Accepted);
    assert!(sim.snapshot().presentations.is_empty());
    assert!(sim.snapshot().state.heroes[0].ray.beam.is_none());
}
#[test]
fn beam_checkpoint_restore_retains_episode_timing_identity_and_resource_cost() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(beam_sample(1, 1))])
        .unwrap();
    let checkpoint = DreamCheckpoint::new(sim.snapshot(), 1).unwrap();
    let mut restored = DreamSimulation::try_from_checkpoint(
        &DreamCheckpoint::decode(&checkpoint.encode().unwrap(), 1).unwrap(),
        1,
    )
    .unwrap();
    for sequence in 2..=120 {
        let sample = CombatAction::BeamAim(beam_sample(sequence, sim.snapshot().tick));
        assert_eq!(
            sim.step_multiplayer_with_actions(&[], &[sample.clone()])
                .unwrap(),
            restored
                .step_multiplayer_with_actions(&[], &[sample])
                .unwrap()
        );
    }
    assert_eq!(sim.snapshot(), restored.snapshot());
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 2);
    sim.step_multiplayer_with_actions(
        &[],
        &[CombatAction::BeamAim(beam_sample(121, sim.snapshot().tick))],
    )
    .unwrap();
    assert!(sim.snapshot().state.heroes[0].ray.beam.is_none());
    assert!(sim.snapshot().presentations.is_empty());
}
#[test]
fn beam_stale_sample_expires_and_rejected_begin_does_not_create_graphics() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    let mut invalid = beam_sample(1, 1);
    invalid.aim = [0.0; 2];
    let result = sim
        .step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(invalid)])
        .unwrap();
    assert_eq!(result[0].reason, ActionReason::InvalidAction);
    assert!(sim.snapshot().presentations.is_empty());
    sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(beam_sample(2, 2))])
        .unwrap();
    for _ in 0..10 {
        sim.step_multiplayer_with_actions(&[], &[]).unwrap();
    }
    assert!(sim.snapshot().state.heroes[0].ray.beam.is_none());
    assert!(sim.snapshot().presentations.is_empty());
}
#[test]
fn owner_beam_predicts_cost_and_graphics_then_authority_rejection_removes_both() {
    use crate::replication::{OwnerCombatAction, OwnerPredictionState, ReplicationStamp};
    let mut sim = arena();
    sim.step(DreamInput::default());
    let stamp = ReplicationStamp {
        match_epoch: 1,
        server_tick: 1,
        gameplay_tick: 1,
        scene_revision: 1,
        revision: 1,
    };
    let cp = sim
        .capture_replication(stamp)
        .unwrap()
        .owner_checkpoint(1, 1)
        .unwrap();
    let collision = sim.world.resource::<collision::CollisionWorld>().clone();
    let bases = sim
        .snapshot()
        .state
        .platforms
        .iter()
        .map(|p| p.presentation(1))
        .collect::<Vec<_>>();
    let mut predicted = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    let sample = beam_sample(1, 1);
    predicted
        .step_restricted_combat_with_bases(
            DreamInput::default(),
            &collision,
            &bases,
            Some(OwnerCombatAction::BeamBegin {
                key: sample.key,
                aim: sample.aim,
            }),
            None,
        )
        .unwrap();
    assert!(predicted.beam_graphics().is_some());
    assert!(
        predicted
            .checkpoint()
            .validate_for(cp.expectation())
            .is_ok()
    );
    assert_eq!(predicted.hero_view().dreamlance_ammo, 5);
    let mut missing = sample;
    missing.query_gameplay_tick = 0;
    assert_eq!(
        sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(missing)])
            .unwrap()[0]
            .reason,
        ActionReason::MissingHistory
    );
    let stamp = ReplicationStamp {
        server_tick: 2,
        gameplay_tick: 2,
        revision: 2,
        ..stamp
    };
    let cp = sim
        .capture_replication(stamp)
        .unwrap()
        .owner_checkpoint(1, 1)
        .unwrap();
    predicted.try_restore(&cp, cp.expectation()).unwrap();
    assert!(predicted.beam_graphics().is_none());
    assert_eq!(predicted.hero_view().dreamlance_ammo, 6);
    let sample = beam_sample(2, 2);
    predicted
        .step_restricted_combat_with_bases(
            DreamInput::default(),
            &collision,
            &bases,
            Some(OwnerCombatAction::BeamBegin {
                key: sample.key,
                aim: sample.aim,
            }),
            None,
        )
        .unwrap();
    sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(sample)])
        .unwrap();
    for sequence in 3..=35 {
        let sample = beam_sample(sequence, sim.snapshot().tick);
        predicted
            .step_restricted_combat_with_bases(
                DreamInput::default(),
                &collision,
                &bases,
                None,
                Some((sample.key, sample.aim)),
            )
            .unwrap();
        sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamAim(sample)])
            .unwrap();
        assert_eq!(
            predicted.hero_view().dreamlance_ammo,
            sim.snapshot().hero.dreamlance_ammo
        );
        assert_eq!(
            predicted.beam_graphics(),
            sim.snapshot().presentations.first().copied()
        );
    }
}
#[test]
fn beam_aim_change_uses_latest_accepted_sample_at_next_damage_tick() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(beam_sample(1, 1))])
        .unwrap();
    for sequence in 2..=7 {
        let mut sample = beam_sample(sequence, sim.snapshot().tick);
        sample.aim = [1.0, 0.0];
        sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamAim(sample)])
            .unwrap();
    }
    assert_eq!(sim.snapshot().enemies[0].hp, 196.0);
    assert!(matches!(
        sim.snapshot().presentations[0].kind,
        engine_core::GraphicsKind::Beam {
            direction: [1.0, 0.0, 0.0],
            ..
        }
    ));
}
#[test]
fn beam_positive_replication_preserves_primitive_and_conservative_bounds() {
    use crate::replication::*;
    let mut sim = arena();
    sim.step(DreamInput::default());
    sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(beam_sample(1, 1))])
        .unwrap();
    let stamp = ReplicationStamp {
        match_epoch: 1,
        server_tick: 2,
        gameplay_tick: 2,
        scene_revision: 1,
        revision: 2,
    };
    let capture = sim.capture_replication(stamp).unwrap();
    let id = sim.snapshot().presentations[0].id;
    assert!(
        capture
            .project_effect(id, &ApprovedSources::default())
            .is_none()
    );
    let effect = capture
        .project_effect(id, &ApprovedSources::new([1]).unwrap())
        .unwrap();
    let replica = PublicReplica::Effect(effect);
    assert_eq!(
        PublicReplica::decode(&replica.encode().unwrap()).unwrap(),
        replica
    );
    assert!(capture.bounds(ReplicationKey::Effect(id)).unwrap().1 >= 20.0);
    let cp = capture.owner_checkpoint(1, 1).unwrap();
    let owner = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    let view = DreamPresentation::from_replicas(capture.global(), &owner, &[replica]).unwrap();
    assert_eq!(
        view.presentations.len(),
        1,
        "owner component and public effect share one lifecycle"
    );
    assert_eq!(view.presentations[0].id, id);
}
#[test]
fn stale_stop_cannot_cancel_a_newer_beam_episode() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    let begin = beam_sample(5, 1);
    sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(begin)])
        .unwrap();
    let mut stop = begin.key;
    stop.command_sequence = 4;
    let result = sim
        .step_multiplayer_with_actions(
            &[],
            &[CombatAction::BeamStop {
                key: stop,
                execution_server_tick: 3,
            }],
        )
        .unwrap();
    assert_eq!(result[0].reason, ActionReason::InvalidAction);
    assert!(sim.snapshot().state.heroes[0].ray.beam.is_some());
    assert!(!sim.snapshot().presentations.is_empty());
}
#[test]
fn authority_trace_is_opt_in_bounded_and_records_geometry_defense_and_final_damage() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    sim.step_multiplayer_with_actions(&[], &[online(1, 1)])
        .unwrap();
    assert!(sim.take_combat_traces().is_empty());
    let mut sim = arena();
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.combat.grant_shield(
            20.0,
            2.0,
            engine_core::ShieldPolicy::Replace,
            engine_core::DurationPolicy::Reset,
        );
    }
    sim.step(DreamInput::default());
    sim.set_combat_trace_enabled(true);
    let verdict = sim
        .step_multiplayer_with_actions(&[], &[online(1, 1)])
        .unwrap();
    let traces = sim.take_combat_traces();
    assert_eq!(traces.len(), 1);
    let trace = &traces[0];
    assert_eq!(trace.verdict, verdict[0].combat.clone().unwrap());
    assert_eq!(trace.verdict.damage, 12.0);
    assert_eq!(trace.execution_gameplay_tick, 2);
    assert_eq!(trace.query_gameplay_tick, 1);
    assert_eq!(
        trace.muzzle.unwrap()[1],
        sim.snapshot().hero.elevation + 0.9
    );
    assert_eq!(trace.selected.as_ref().unwrap().shield, 20.0);
    assert_eq!(trace.shield_credits.len(), 1);
    assert_eq!(trace.shield_credits[0].spent_before, 0.0);
    assert_eq!(trace.shield_credits[0].spent_after, 20.0);
    assert!(serde_json::to_vec(trace).unwrap().len() <= trace.retained_bytes());
    let saved = serde_json::to_string(&sim.snapshot()).unwrap();
    assert!(!saved.contains("shield_credits"));
    assert!(!saved.contains("muzzle"));
    let ray = beam_sample(9, 1);
    let mut scratch = sim
        .world
        .resource_mut::<crate::combat_trace::CombatTraceState>();
    for _ in 0..140 {
        scratch.begin(ray, 2, 1, 0);
    }
    assert_eq!(
        scratch.pending.len(),
        crate::combat_trace::MAX_COMBAT_TRACE_RECORDS
    );
    drop(scratch);
    sim.set_combat_trace_enabled(false);
    assert!(sim.take_combat_traces().is_empty());
}
#[test]
fn trace_restore_clears_retained_hidden_geometry_without_changing_opt_in() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    sim.set_combat_trace_enabled(true);
    sim.step_multiplayer_with_actions(&[], &[online(1, 1)])
        .unwrap();
    assert!(
        !sim.world
            .resource::<crate::combat_trace::CombatTraceState>()
            .pending
            .is_empty()
    );
    let snapshot = sim.snapshot();
    sim.try_restore(&snapshot).unwrap();
    assert!(sim.take_combat_traces().is_empty());
    sim.step_multiplayer_with_actions(&[], &[online(2, 2)])
        .unwrap();
    assert_eq!(
        sim.take_combat_traces()[0].verdict.reason,
        CombatReason::Cooldown
    );
}
#[test]
fn disconnect_and_encounter_clear_remove_beam_without_a_future_combat_tick() {
    let mut sim = arena();
    sim.step(DreamInput::default());
    sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(beam_sample(1, 1))])
        .unwrap();
    assert!(sim.set_player_active(1, false));
    assert!(sim.snapshot().state.heroes[0].ray.beam.is_none());
    assert!(sim.snapshot().presentations.is_empty());
    assert!(sim.resume_player(1));
    assert!(sim.snapshot().state.heroes[0].ray.beam.is_none());
    let mut sim = arena();
    sim.step(DreamInput::default());
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.health.hp = 1.0;
    }
    sim.step_multiplayer_with_actions(&[], &[CombatAction::BeamBegin(beam_sample(1, 1))])
        .unwrap();
    assert_eq!(sim.snapshot().phase, RunPhase::Reward);
    assert!(sim.snapshot().state.heroes[0].ray.beam.is_none());
    assert!(
        sim.snapshot()
            .presentations
            .iter()
            .all(|effect| !matches!(effect.kind, engine_core::GraphicsKind::Beam { .. }))
    );
}

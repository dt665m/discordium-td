use super::*;
use crate::*;
use serde_json::Value;
fn stamp(sim: &DreamSimulation) -> ReplicationStamp {
    ReplicationStamp {
        match_epoch: 7,
        server_tick: 1000,
        gameplay_tick: sim.snapshot().tick,
        scene_revision: 1,
        revision: 12,
    }
}
fn party() -> DreamSimulation {
    let mut sim = DreamSimulation::new(42, false);
    sim.continue_run();
    sim.add_player(2);
    sim
}
#[test]
fn public_party_count_survives_spatial_omission_and_excludes_inactive_travelers() {
    let mut sim = party();
    sim.add_pending_player(3);
    let capture = sim.capture_replication(stamp(&sim)).unwrap();
    let checkpoint = capture.owner_checkpoint(1, 1).unwrap();
    let owner =
        OwnerPredictionState::from_checkpoint(&checkpoint, checkpoint.expectation()).unwrap();
    // No remote actors are disclosed to this view, but both active travelers
    // still belong to the party. Pending and disconnected travelers do not.
    let presentation = DreamPresentation::from_replicas(capture.global(), &owner, &[]).unwrap();
    assert_eq!(presentation.heroes.len(), 1);
    assert_eq!(presentation.active_travelers, 2);
    sim.set_player_active(2, false);
    assert_eq!(
        sim.capture_replication(stamp(&sim))
            .unwrap()
            .global()
            .active_travelers,
        1
    );
}
fn assert_no_keys(value: &Value, forbidden: &[&str]) {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                assert!(!forbidden.contains(&key.as_str()), "forbidden key {key}");
                assert_no_keys(value, forbidden);
            }
        }
        Value::Array(values) => {
            for value in values {
                assert_no_keys(value, forbidden);
            }
        }
        _ => {}
    }
}
#[test]
fn public_fields_are_positive_and_independent_of_nearby_secrets() {
    let mut sim = party();
    let capture = sim.capture_replication(stamp(&sim)).unwrap();
    let before = serde_json::to_value(capture.project_hero(2).unwrap()).unwrap();
    let owner_before = capture.owner_checkpoint(1, 1).unwrap().encode().unwrap();
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        if hero.view.id == 2 {
            hero.motion.position = [0.0, 0.0, 2.0];
            hero.view.shards = 999_999;
            hero.view.attack_power = 17.0;
            hero.progression.xp = 333.0;
            hero.loadout.0[0] = memory_slot(MemoryKind::Wisp);
            hero.rewards = vec![Reward {
                kind: RewardKind::Memory(MemoryKind::Wisp),
                rarity: Rarity::Rare,
                title: "PRIVATE_REWARD".into(),
                description: "PRIVATE_DESCRIPTION".into(),
            }];
        }
    }
    // Compare the same public pose, to isolate private field mutations.
    let mut run = sim.world.resource_mut::<Run>();
    run.rng ^= 999;
    run.next_id += 100;
    run.message = "PRIVATE_HIDDEN_BOSS_PHASE".into();
    drop(run);
    let capture = sim.capture_replication(stamp(&sim)).unwrap();
    let mut after = serde_json::to_value(capture.project_hero(2).unwrap()).unwrap();
    after["position"] = before["position"].clone();
    assert_eq!(before, after);
    assert_eq!(
        owner_before,
        capture.owner_checkpoint(1, 1).unwrap().encode().unwrap()
    );
    let global = serde_json::to_value(capture.global()).unwrap();
    for value in [&after, &global] {
        assert_no_keys(
            value,
            &[
                "seed",
                "state",
                "run",
                "rng",
                "critical_rng",
                "next_id",
                "reinforcements",
                "memories",
                "loadout",
                "xp",
                "xp_next",
                "shards",
                "rewards",
                "attack_power",
                "ability_power",
                "movement_speed",
                "critical_chance",
                "defense",
            ],
        );
        assert!(!value.to_string().contains("PRIVATE_"));
    }
}
#[test]
fn owner_checkpoint_restores_exact_selected_latent_state_and_rng() {
    let mut sim = party();
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        if hero.view.id == 2 {
            hero.combat.shield = 17.0;
            hero.combat.shield_remaining = 2.0;
            hero.motion.velocity = [1.0, 0.0, 2.0];
            hero.motion.movement_lock_ticks = 18;
            hero.action.recovery = 0.4;
            hero.progression.xp = 13.0;
            hero.loadout.0[1].cooldown = 2.4;
            hero.critical_rng.next_u64().unwrap();
        }
    }
    let capture = sim.capture_replication(stamp(&sim)).unwrap();
    let checkpoint = capture.owner_checkpoint(2, 9).unwrap();
    let bytes = checkpoint.encode().unwrap();
    assert!(bytes.len() <= 16 * 1024);
    let value = serde_json::to_value(&checkpoint).unwrap();
    assert_no_keys(
        &value,
        &[
            "run",
            "seed",
            "next_id",
            "heroes",
            "enemies",
            "projectiles",
            "wisps",
            "delayed",
            "reinforcements",
            "attack_index",
        ],
    );
    let decoded = OwnerCheckpoint::decode(&bytes, checkpoint.expectation()).unwrap();
    assert_eq!(decoded, checkpoint);
    let state = OwnerPredictionState::from_checkpoint(&decoded, checkpoint.expectation()).unwrap();
    assert_eq!(state.owner(), 2);
    assert_eq!(state.hero_view(), sim.snapshot_for(2).hero);
    let mut original_rng = checkpoint.hero.critical_rng.clone();
    let mut restored_rng = state.checkpoint.hero.critical_rng.clone();
    for _ in 0..12 {
        assert_eq!(original_rng.next_u64(), restored_rng.next_u64());
    }
    assert!(capture.owner_checkpoint(999, 1).is_err());
}

#[test]
fn owner_checkpoint_roundtrips_only_valid_same_tick_input_continuity() {
    use engine_net::{commands::InputContinuity, types::ServerTick};

    let sim = party();
    let checkpoint = sim
        .capture_replication(stamp(&sim))
        .unwrap()
        .owner_checkpoint(1, 2)
        .unwrap();
    let at = ServerTick(checkpoint.stamp().server_tick);
    let held = DreamInput {
        movement: [0.6, 0.8],
        aim: [0.0, -1.0],
        attack: true,
        ..Default::default()
    };
    for continuity in [
        InputContinuity::default(),
        InputContinuity {
            last_input: None,
            missing_streak: u32::MAX,
        },
        InputContinuity {
            last_input: Some(held),
            missing_streak: 2,
        },
    ] {
        let captured = checkpoint
            .clone()
            .with_input_continuity(at, continuity.clone())
            .unwrap();
        let bytes = captured.encode().unwrap();
        let decoded = OwnerCheckpoint::decode(&bytes, captured.expectation()).unwrap();
        assert_eq!(decoded.input_continuity(), &continuity);
        assert_eq!(decoded, captured);
    }
    assert!(
        checkpoint
            .clone()
            .with_input_continuity(ServerTick(at.0 + 1), InputContinuity::default())
            .is_err()
    );
    for invalid in [
        DreamInput { dash: true, ..held },
        DreamInput {
            casts: [true, false, false, false],
            ..held
        },
        DreamInput {
            action_sequences: [9, 0, 0, 0, 0],
            ..held
        },
        DreamInput {
            movement: [1.0, 1.0],
            ..held
        },
        DreamInput {
            aim: [f32::NAN, 0.0],
            ..held
        },
    ] {
        assert!(
            checkpoint
                .clone()
                .with_input_continuity(
                    at,
                    InputContinuity {
                        last_input: Some(invalid),
                        missing_streak: 0,
                    }
                )
                .is_err()
        );
    }
}
#[test]
fn owner_identity_revision_and_malformed_restore_fail_transactionally() {
    let sim = party();
    let checkpoint = sim
        .capture_replication(stamp(&sim))
        .unwrap()
        .owner_checkpoint(1, 2)
        .unwrap();
    let expected = checkpoint.expectation();
    let mut state = OwnerPredictionState::from_checkpoint(&checkpoint, expected).unwrap();
    let before = state.clone();
    for wrong in [
        OwnerExpectation {
            owner: 2,
            ..expected
        },
        OwnerExpectation {
            scene_revision: 99,
            ..expected
        },
        OwnerExpectation {
            match_epoch: 8,
            ..expected
        },
        OwnerExpectation {
            ownership_revision: 3,
            ..expected
        },
        OwnerExpectation {
            minimum_revision: 13,
            ..expected
        },
    ] {
        assert!(state.try_restore(&checkpoint, wrong).is_err());
        assert_eq!(state, before);
    }
    let mut invalid = checkpoint.clone();
    invalid.hero.motion.velocity[0] = f32::NAN;
    assert!(state.try_restore(&invalid, expected).is_err());
    assert_eq!(state, before);
    let mut value = serde_json::to_value(&checkpoint).unwrap();
    value["hero"]["actor"]["view"]["id"] = Value::from(2);
    assert!(OwnerCheckpoint::decode(&serde_json::to_vec(&value).unwrap(), expected).is_err());
    let mut value = serde_json::to_value(&checkpoint).unwrap();
    value["hero"]["actor"]["surprise"] = Value::Bool(true);
    assert!(OwnerCheckpoint::decode(&serde_json::to_vec(&value).unwrap(), expected).is_err());
    assert!(OwnerCheckpoint::decode(&vec![b' '; 16 * 1024 + 1], expected).is_err());
    let mut value = serde_json::to_value(&checkpoint).unwrap();
    value["hero"]["combat"]["statuses"] = Value::Array(vec![Value::Null; 65]);
    assert!(OwnerCheckpoint::decode(&serde_json::to_vec(&value).unwrap(), expected).is_err());
}
#[test]
fn hidden_sources_omit_effect_identity_and_strip_projectile_owner() {
    let mut sim = party();
    sim.step_multiplayer(&[(
        2,
        DreamInput {
            aim: [1.0, 0.0],
            dash: true,
            casts: [false, true, false, false],
            action_sequences: [44, 0, 45, 0, 0],
            ..Default::default()
        },
    )]);
    let capture = sim.capture_replication(stamp(&sim)).unwrap();
    let hidden = ApprovedSources::new([1]).unwrap();
    let disclosed = ApprovedSources::new([1, 2]).unwrap();
    let effects: Vec<_> = capture
        .keys()
        .into_iter()
        .filter_map(|key| match key {
            ReplicationKey::Effect(id) if id.owner == 2 => Some(id),
            _ => None,
        })
        .collect();
    assert!(!effects.is_empty());
    for id in effects {
        assert!(capture.project_effect(id, &hidden).is_none());
        let effect = capture.project_effect(id, &disclosed).unwrap();
        assert_eq!(effect.id.owner, 2);
        assert_eq!(effect.id.match_epoch, 7); // Never seed-folded epoch from old GraphicsId.
    }
    let projectiles: Vec<_> = capture
        .keys()
        .into_iter()
        .filter_map(|key| match key {
            ReplicationKey::Projectile(id) => Some(id),
            _ => None,
        })
        .collect();
    assert!(!projectiles.is_empty());
    for id in projectiles {
        assert_eq!(capture.project_projectile(id, &hidden).unwrap().owner, None);
        assert_eq!(
            capture.project_projectile(id, &disclosed).unwrap().owner,
            Some(2)
        );
    }
    let proxy = CoarseEffectProxy {
        proxy_id: 8,
        cell: [1, -2],
        cell_size: 8,
        age_ticks: 0,
        duration_ticks: 20,
    }
    .project(stamp(&sim))
    .unwrap();
    assert_eq!(proxy.id.owner, 0);
    assert_eq!(proxy.position, [12.0, -12.0]);
    assert!(!serde_json::to_string(&proxy).unwrap().contains("45"));
}
#[test]
fn presentation_has_owner_data_and_explicit_remote_placeholders() {
    let sim = party();
    let capture = sim.capture_replication(stamp(&sim)).unwrap();
    let checkpoint = capture.owner_checkpoint(1, 1).unwrap();
    let owner =
        OwnerPredictionState::from_checkpoint(&checkpoint, checkpoint.expectation()).unwrap();
    let replicas = vec![PublicReplica::Hero(capture.project_hero(2).unwrap())];
    let shown = DreamPresentation::from_replicas(capture.global(), &owner, &replicas).unwrap();
    assert_eq!(shown.hero, sim.snapshot_for(1).hero);
    let remote = shown.heroes.iter().find(|v| v.id == 2).unwrap();
    assert!(remote.memories.iter().all(|v| v.level == 0));
    assert_eq!(remote.attack_power, 0.0);
    assert_eq!(shown.enemies_remaining, 0); // Does not reveal hidden/future enemy count.
    assert_eq!(shown.stamp.server_tick, 1000);
    assert_eq!(shown.tick, sim.snapshot().tick);
}

#[test]
fn presentation_stages_unresolved_sources_without_bypassing_replica_validation() {
    let sim = party();
    let capture = sim.capture_replication(stamp(&sim)).unwrap();
    let checkpoint = capture.owner_checkpoint(1, 1).unwrap();
    let owner =
        OwnerPredictionState::from_checkpoint(&checkpoint, checkpoint.expectation()).unwrap();
    let enemy = sim.snapshot().enemies[0].id;
    for (source, source_id) in [
        (PublicReplica::Hero(capture.project_hero(2).unwrap()), 2),
        (
            PublicReplica::Enemy(capture.project_enemy(enemy).unwrap()),
            enemy,
        ),
    ] {
        let effect = PublicEffectView {
            id: engine_core::GraphicsId {
                scope: None,
                match_epoch: capture.global().stamp.match_epoch,
                owner: source_id,
                action_seq: 19,
                slot: 0xc200,
            },
            kind: engine_core::GraphicsKind::RadialPulse,
            position: [2.0, 3.0],
            radius: 1.9,
            age_ticks: 1,
            duration_ticks: 22,
        };
        let dependents = vec![
            PublicReplica::Projectile(PublicProjectileView {
                id: 900_001,
                owner: Some(source_id),
                position: [1.0, 2.0],
                direction: [1.0, 0.0],
                radius: 0.3,
                friendly: false,
                essence: None,
            }),
            PublicReplica::Wisp(PublicWispView {
                id: 900_002,
                owner: Some(source_id),
                position: [3.0, 4.0],
                remaining: 1.0,
                essence: None,
            }),
            PublicReplica::Effect(effect),
        ];
        for replica in &dependents {
            assert_eq!(
                PublicReplica::decode(&replica.encode().unwrap()).unwrap(),
                *replica
            );
        }
        // Source scopes may arrive after their dependents, leave before their
        // replacements, and re-enter. Presentation adds no extra staging store.
        for source_present in [false, true, false, true] {
            let mut replicas = dependents.clone();
            if source_present {
                replicas.push(source.clone());
            }
            let shown = DreamPresentation::from_replicas(capture.global(), &owner, &replicas)
                .expect("independent actor delivery must not block the owner view");
            assert_eq!(shown.hero, owner.hero_view());
            assert_eq!(shown.stamp, capture.global().stamp);
            for count in [
                shown.projectiles.len(),
                shown.wisps.len(),
                shown.presentations.len(),
            ] {
                assert_eq!(count, usize::from(source_present));
            }
            if source_present {
                assert_eq!(shown.projectiles[0].owner, source_id);
                assert_eq!(shown.wisps[0].owner, source_id);
                assert_eq!(shown.presentations[0].id, effect.id);
                assert_eq!(shown.presentations[0].pos, effect.position);
                assert_eq!(shown.presentations[0].radius, effect.radius);
            }
        }
        // Unresolved roots still undergo identity, epoch and payload checks.
        for replica in dependents {
            assert_eq!(
                DreamPresentation::from_replicas(
                    capture.global(),
                    &owner,
                    &[replica.clone(), replica]
                ),
                Err(ReplicationError::DuplicateReplica)
            );
        }
        let mut wrong_epoch = effect;
        wrong_epoch.id.match_epoch += 1;
        assert_eq!(
            DreamPresentation::from_replicas(
                capture.global(),
                &owner,
                &[PublicReplica::Effect(wrong_epoch)]
            ),
            Err(ReplicationError::WrongEpoch)
        );
        let mut malformed = effect;
        malformed.radius = -1.0;
        assert!(PublicReplica::Effect(malformed).encode().is_err());
        assert_eq!(
            DreamPresentation::from_replicas(
                capture.global(),
                &owner,
                &[PublicReplica::Effect(malformed)]
            ),
            Err(ReplicationError::InvalidState)
        );
    }
}
#[test]
fn public_payload_codecs_reject_malformed_fields_before_publication() {
    let sim = party();
    let capture = sim.capture_replication(stamp(&sim)).unwrap();
    let actor = PublicReplica::Hero(capture.project_hero(2).unwrap());
    assert_eq!(
        PublicReplica::decode(&actor.encode().unwrap()).unwrap(),
        actor
    );
    assert_eq!(
        DreamGlobalView::decode(&capture.global().encode().unwrap()).unwrap(),
        *capture.global()
    );
    let mut raw = serde_json::to_value(&actor).unwrap();
    raw["value"]["hp"] = Value::from(-1.0);
    assert!(PublicReplica::decode(&serde_json::to_vec(&raw).unwrap()).is_err());
    let mut raw = serde_json::to_value(&actor).unwrap();
    raw["value"]["critical_rng"] = Value::from(7);
    assert!(PublicReplica::decode(&serde_json::to_vec(&raw).unwrap()).is_err());
    let mut raw = serde_json::to_value(&actor).unwrap();
    raw["value"]["position"] = Value::Array(vec![Value::Null; 65]);
    assert!(PublicReplica::decode(&serde_json::to_vec(&raw).unwrap()).is_err());
    assert!(PublicReplica::decode(&vec![b' '; 4097]).is_err());
    let mut global = capture.global().clone();
    global.elapsed = f32::INFINITY;
    assert!(global.encode().is_err());
    let mut raw = serde_json::to_value(capture.global()).unwrap();
    raw["stamp"]["match_epoch"] = Value::from(0);
    assert!(DreamGlobalView::decode(&serde_json::to_vec(&raw).unwrap()).is_err());
}

#[test]
fn restricted_owner_replays_shared_motion_dash_attack_without_world_state() {
    let mut sim = party();
    // Distant stationary enemies keep combat active without owner impacts.
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [50.0, 50.0];
        enemy.motor.movement_lock = 1000.0;
        enemy.action.recovery = 1000.0;
    }
    let cp = sim
        .capture_replication(stamp(&sim))
        .unwrap()
        .owner_checkpoint(1, 1)
        .unwrap();
    let mut predicted = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    for tick in 0..90 {
        let input = DreamInput {
            movement: [0.3, -0.8],
            aim: [1.0, 0.0],
            dash: tick == 3 || tick == 80,
            attack: true,
            ..Default::default()
        };
        predicted
            .step_restricted_combat_with_bases(
                input,
                &collision(),
                &required_platforms(&sim, &predicted),
                None,
                None,
            )
            .unwrap();
        sim.step_multiplayer(&[(1, input)]);
        let authority = sim
            .capture_replication(stamp(&sim))
            .unwrap_or_else(|e| {
                panic!(
                    "tick {tick}: {e:?} effects {:?}",
                    sim.snapshot().state.effects
                )
            })
            .owner_checkpoint(1, 1)
            .unwrap();
        assert_eq!(
            predicted.checkpoint.hero.motion, authority.hero.motion,
            "motor tick {tick}"
        );
        assert_eq!(
            predicted.checkpoint.hero.action, authority.hero.action,
            "action tick {tick}"
        );
        assert_eq!(
            predicted.checkpoint.hero.actor.view, authority.hero.actor.view,
            "body tick {tick}"
        );
    }
    let before = predicted.clone();
    assert!(
        predicted
            .step_restricted(
                DreamInput {
                    movement: [f32::NAN, 0.0],
                    ..Default::default()
                },
                &collision()
            )
            .is_err()
    );
    assert_eq!(predicted, before);
}

#[test]
fn restricted_owner_restores_replays_and_ignores_unpredicted_casts() {
    let sim = party();
    let cp = sim
        .capture_replication(stamp(&sim))
        .unwrap()
        .owner_checkpoint(1, 1)
        .unwrap();
    let mut predicted = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    let input = DreamInput {
        movement: [1.0, 0.0],
        casts: [true; 4],
        ..Default::default()
    };
    predicted
        .step_restricted_combat_with_bases(
            input,
            &collision(),
            &required_platforms(&sim, &predicted),
            None,
            None,
        )
        .unwrap();
    let first = predicted.clone();
    predicted.try_restore(&cp, cp.expectation()).unwrap();
    predicted
        .step_restricted_combat_with_bases(
            input,
            &collision(),
            &required_platforms(&sim, &predicted),
            None,
            None,
        )
        .unwrap();
    assert_eq!(predicted, first);
    let mut no_casts = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    no_casts
        .step_restricted_combat_with_bases(
            DreamInput {
                casts: [false; 4],
                ..input
            },
            &collision(),
            &required_platforms(&sim, &predicted),
            None,
            None,
        )
        .unwrap();
    assert_eq!(predicted, no_casts);
    let mut context = cp.context().clone();
    context.paused = true;
    let paused =
        OwnerCheckpoint::new(cp.hero.clone(), context, 1, cp.collision_identity()).unwrap();
    predicted
        .try_restore(&paused, paused.expectation())
        .unwrap();
    let before = predicted.clone();
    predicted
        .step_restricted_combat_with_bases(
            input,
            &collision(),
            &required_platforms(&sim, &predicted),
            None,
            None,
        )
        .unwrap();
    assert_eq!(predicted, before);
}

#[test]
fn world_owned_reinforcement_graphics_allow_committed_capture() {
    let mut sim = party();
    let enemies: Vec<_> = sim
        .world
        .query_filtered::<Entity, With<Enemy>>()
        .iter(&sim.world)
        .collect();
    for entity in enemies {
        sim.world.despawn(entity);
    }
    sim.world.resource_mut::<Run>().reinforcements = 5;
    sim.step(Default::default());
    assert!(sim.snapshot().state.effects.iter().any(|v| v.id.owner == 0));
    sim.capture_replication(stamp(&sim)).unwrap();
}

fn collision() -> crate::collision::CollisionWorld {
    crate::collision::CollisionManifest::current(1)
        .build()
        .unwrap()
}

fn required_platforms(
    sim: &DreamSimulation,
    owner: &OwnerPredictionState,
) -> Vec<PublicPlatformView> {
    sim.snapshot()
        .platforms
        .into_iter()
        .filter(|p| owner.required_bases().contains(&p.collider))
        .collect()
}

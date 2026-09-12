use super::*;
use crate::{collision::*, *};
use engine_core::{ColliderKey, KinematicState, Stance};

fn stamp(sim: &DreamSimulation, tick: u64) -> ReplicationStamp {
    ReplicationStamp {
        match_epoch: 1,
        server_tick: tick,
        gameplay_tick: sim.snapshot().tick,
        scene_revision: sim.snapshot().collision_manifest().scene_revision(),
        revision: tick + 1,
    }
}
fn quiet_game() -> DreamSimulation {
    let mut sim = DreamSimulation::new(17, false);
    sim.continue_run();
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [50.0, 50.0];
        enemy.motor.movement_lock = 1000.0;
    }
    sim
}
fn checkpoint(sim: &DreamSimulation, tick: u64) -> OwnerCheckpoint {
    sim.capture_replication(stamp(sim, tick))
        .unwrap()
        .owner_checkpoint(1, 1)
        .unwrap()
}
fn environment(sim: &DreamSimulation) -> CollisionWorld {
    sim.world.resource::<CollisionWorld>().clone()
}
fn custom_ceiling() -> CollisionManifest {
    let current = CollisionManifest::current(1);
    let mut colliders = current.colliders()[..1].to_vec();
    colliders.push(engine_core::StaticCollider {
        key: ColliderKey {
            index: 2,
            generation: 1,
        },
        position: [0.0, 1.9, 2.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        shape: engine_core::CollisionShape::Box {
            half_extents: [2.0, 0.25, 3.0],
        },
    });
    CollisionManifest::from_parts(1, current.config(), colliders).unwrap()
}

#[test]
fn shared_capsule_sweeps_dash_at_perimeter_and_restores_replay_exactly() {
    let mut sim = quiet_game();
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.motion.position = [ARENA_RADIUS - 1.5, 0.0, 0.0];
    }
    let collision = environment(&sim);
    let cp = checkpoint(&sim, 1);
    let mut predicted = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    for tick in 0..90 {
        let input = DreamInput {
            movement: if tick < 25 { [1.0, 0.0] } else { [0.0, -1.0] },
            aim: [0.0, 1.0],
            dash: tick == 0 || tick == 75,
            attack: tick % 11 == 0,
            ..Default::default()
        };
        predicted
            .step_restricted_combat_with_bases(
                input,
                &collision,
                &required_platforms(&sim, &predicted),
                None,
                None,
            )
            .unwrap();
        sim.step(input);
        let authority = checkpoint(&sim, tick + 2);
        assert_eq!(
            predicted.checkpoint.hero.motion, authority.hero.motion,
            "motion tick {tick}"
        );
        assert_eq!(
            predicted.checkpoint.hero.action, authority.hero.action,
            "attack tick {tick}"
        );
        assert_eq!(predicted.checkpoint.hero.motion.facing, [0.0, 1.0]);
        if tick < 25 {
            assert!(authority.hero.motion.position[0] <= ARENA_RADIUS - 0.79);
        }
        if tick == 44 {
            let decoded =
                OwnerCheckpoint::decode(&authority.encode().unwrap(), authority.expectation())
                    .unwrap();
            predicted
                .try_restore(&decoded, decoded.expectation())
                .unwrap();
        }
    }
}

#[test]
fn saved_jump_buffer_vertical_velocity_and_crouch_resume_in_both_paths() {
    let mut sim = quiet_game();
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.motion.jump_buffer_ticks = 3;
    }
    let collision = environment(&sim);
    let cp = checkpoint(&sim, 1);
    let mut predicted = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    for tick in 0..35 {
        predicted
            .step_restricted_combat_with_bases(
                DreamInput::default(),
                &collision,
                &required_platforms(&sim, &predicted),
                None,
                None,
            )
            .unwrap();
        sim.step(DreamInput::default());
        assert_eq!(
            predicted.checkpoint.hero.motion,
            checkpoint(&sim, tick + 2).hero.motion
        );
        if tick == 0 {
            assert!(predicted.hero_view().elevation > 0.0);
        }
    }
    let manifest = custom_ceiling();
    sim.world.insert_resource(manifest.build().unwrap());
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        *hero.motion = KinematicState::new([0.0, 0.0, 2.0], 1);
        hero.motion.stance = Stance::Crouched;
    }
    let collision = environment(&sim);
    let cp = checkpoint(&sim, 40);
    let bases = sim.snapshot().platforms;
    let mut predicted = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    for tick in 0..60 {
        let input = DreamInput {
            movement: [1.0, 0.0],
            ..Default::default()
        };
        predicted
            .step_restricted_combat_with_bases(input, &collision, &bases, None, None)
            .unwrap();
        sim.step(input);
        assert_eq!(
            predicted.checkpoint.hero.motion,
            checkpoint(&sim, tick + 41).hero.motion
        );
        if tick == 0 {
            assert!(predicted.hero_view().crouched);
        }
    }
    assert!(!predicted.hero_view().crouched);
}

#[test]
fn owner_dependency_mismatch_is_transactional_and_historical_scene_remains_usable() {
    let sim = quiet_game();
    let cp = checkpoint(&sim, 1);
    let old = environment(&sim);
    let mut predicted = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    let before = predicted.clone();
    let other_revision = CollisionManifest::current(2).build().unwrap();
    assert!(
        predicted
            .step_restricted(DreamInput::default(), &other_revision)
            .is_err()
    );
    assert_eq!(predicted, before);
    let other_geometry = custom_ceiling().build().unwrap();
    assert!(
        predicted
            .step_restricted(DreamInput::default(), &other_geometry)
            .is_err()
    );
    assert_eq!(predicted, before);
    predicted
        .step_restricted_combat_with_bases(
            DreamInput {
                movement: [1.0, 0.0],
                ..Default::default()
            },
            &old,
            &required_platforms(&sim, &predicted),
            None,
            None,
        )
        .unwrap();
    assert_ne!(predicted.hero_view().position, before.hero_view().position);
}

#[test]
fn full_restore_owns_exact_collision_manifest_and_complete_base_state() {
    let mut sim = quiet_game();
    let manifest = custom_ceiling();
    sim.world.insert_resource(manifest.build().unwrap());
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.motion.position = [1.0, 0.25, -10.0];
        hero.motion.stance = Stance::Crouched;
    }
    sim.step(DreamInput::default());
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        assert!(hero.motion.base.attachment.is_some());
        hero.motion.base.revision = 9;
    }
    let saved = sim.snapshot();
    let wire = DreamCheckpoint::new(saved.clone(), 1)
        .unwrap()
        .encode()
        .unwrap();
    let decoded = DreamCheckpoint::decode(&wire, 1).unwrap();
    sim.world
        .insert_resource(CollisionManifest::current(2).build().unwrap());
    sim.try_restore(&decoded.snapshot).unwrap();
    assert_eq!(sim.snapshot(), saved);
    assert_eq!(environment(&sim).identity(), manifest.identity());
    let owner = checkpoint(&sim, 100);
    let restored = OwnerCheckpoint::decode(&owner.encode().unwrap(), owner.expectation()).unwrap();
    assert_eq!(restored.hero.motion, saved.state.heroes[0].motion);
    let mut prediction =
        OwnerPredictionState::from_checkpoint(&restored, restored.expectation()).unwrap();
    let before = prediction.clone();
    // Static profile cannot invent the required moving-base history.
    assert!(
        prediction
            .step_restricted(DreamInput::default(), &environment(&sim))
            .is_err()
    );
    assert_eq!(prediction, before);
    let mut malformed = saved.clone();
    malformed.state.heroes[0].motion.scene_revision = 2;
    assert!(sim.try_restore(&malformed).is_err());
    assert_eq!(sim.snapshot(), saved);
}

#[test]
fn blink_uses_capsule_sweep_and_resets_support_without_tunneling() {
    let collision = CollisionManifest::current(1).build().unwrap();
    let mut state = KinematicState::new([ARENA_RADIUS - 2.0, 0.0, 0.0], 1);
    engine_core::advance_kinematic(
        &mut state,
        &collision.manifest().config(),
        Default::default(),
        collision.scene(),
    )
    .unwrap();
    assert!(state.grounded);
    collision.blink(&mut state, [1.0, 0.0], 9.3).unwrap();
    assert!(state.position[0] > ARENA_RADIUS - 2.0 && state.position[0] <= ARENA_RADIUS - 0.79);
    assert!(!state.grounded);
    assert!(state.ground.is_none());
    assert!(state.base.attachment.is_none());
    assert_eq!(state.suppress_snap_ticks, 1);
}

#[test]
fn collision_positive_codec_bounds_hashes_and_checkpoint_schema_are_fenced() {
    let manifest = CollisionManifest::current(1);
    assert_eq!(
        CollisionManifest::decode(&manifest.encode().unwrap()).unwrap(),
        manifest
    );
    assert_ne!(
        manifest.identity(),
        CollisionManifest::current(2).identity()
    );
    assert_ne!(manifest.identity(), custom_ceiling().identity());
    for field in ["schema", "scene_revision"] {
        let mut value = serde_json::to_value(&manifest).unwrap();
        value[field] = serde_json::json!(0);
        assert!(CollisionManifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut value = serde_json::to_value(&manifest).unwrap();
    value["config"]["speed"] = serde_json::json!(999.0);
    assert!(CollisionManifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(&manifest).unwrap();
    value["colliders"][0]["private_seed"] = serde_json::json!(123);
    assert!(CollisionManifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(CollisionManifest::decode(&vec![b' '; 16 * 1024 + 1]).is_err());
    let sim = quiet_game();
    let owner = checkpoint(&sim, 1);
    let mut value = serde_json::to_value(&owner).unwrap();
    value["schema"] = serde_json::json!(3);
    assert!(
        OwnerCheckpoint::decode(&serde_json::to_vec(&value).unwrap(), owner.expectation()).is_err()
    );
    assert!(DreamCheckpoint::new(sim.snapshot(), 2).is_err());
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

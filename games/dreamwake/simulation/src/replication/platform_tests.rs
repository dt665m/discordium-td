use super::*;
use crate::*;
fn game() -> DreamSimulation {
    let mut sim = DreamSimulation::new(17, false);
    sim.continue_run();
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [50.0, 50.0];
        enemy.motor.movement_lock = 1000.0;
    }
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.motion.position = [1.0, 0.25, -10.0];
        hero.motion.velocity = [0.0; 3];
    }
    sim.step(DreamInput::default());
    sim
}
fn capture(sim: &DreamSimulation) -> CommittedReplication {
    let snapshot = sim.snapshot();
    sim.capture_replication(ReplicationStamp {
        match_epoch: 1,
        server_tick: u64::from(snapshot.tick) + 1,
        gameplay_tick: snapshot.tick,
        scene_revision: 1,
        revision: u64::from(snapshot.tick) + 1,
    })
    .unwrap()
}
#[test]
fn rider_replays_exact_frozen_base_and_restores_complete_snapshot() {
    let mut sim = game();
    let committed = capture(&sim);
    let checkpoint = committed.owner_checkpoint(1, 1).unwrap();
    let platform = sim.snapshot().platforms[0];
    assert_eq!(checkpoint.required_bases(), &[platform.collider]);
    assert!(checkpoint.hero.motion.base.attachment.is_some());
    let mut owner =
        OwnerPredictionState::from_checkpoint(&checkpoint, checkpoint.expectation()).unwrap();
    let collision = committed.collision().build().unwrap();
    for tick in 0..90 {
        owner
            .step_restricted_combat_with_bases(
                DreamInput::default(),
                &collision,
                &[platform],
                None,
                None,
            )
            .unwrap();
        sim.step(DreamInput::default());
        let authority = capture(&sim).owner_checkpoint(1, 1).unwrap();
        assert_eq!(
            owner.checkpoint.hero.motion, authority.hero.motion,
            "tick {tick}"
        );
        assert_eq!(owner.gameplay_tick(), sim.snapshot().tick);
        if tick == 44 {
            sim = DreamSimulation::try_from_snapshot(&sim.snapshot()).unwrap();
            owner
                .try_restore(&authority, authority.expectation())
                .unwrap();
        }
    }
    assert_ne!(
        owner.checkpoint.hero.motion.position,
        checkpoint.hero.motion.position
    );
}
#[test]
fn unavailable_or_wrong_generation_base_is_transactionally_rejected() {
    let sim = game();
    let committed = capture(&sim);
    let checkpoint = committed.owner_checkpoint(1, 1).unwrap();
    let mut owner =
        OwnerPredictionState::from_checkpoint(&checkpoint, checkpoint.expectation()).unwrap();
    let before = owner.clone();
    let collision = committed.collision().build().unwrap();
    assert!(
        owner
            .step_restricted(DreamInput::default(), &collision)
            .is_err()
    );
    assert_eq!(owner, before);
    let mut platform = sim.snapshot().platforms[0];
    platform.collider.generation += 1;
    assert!(
        owner
            .step_restricted_combat_with_bases(
                DreamInput::default(),
                &collision,
                &[platform],
                None,
                None
            )
            .is_err()
    );
    assert_eq!(owner, before);
    platform = sim.snapshot().platforms[0];
    let mut forged = checkpoint.clone();
    forged
        .hero
        .motion
        .base
        .attachment
        .as_mut()
        .unwrap()
        .local_anchor[0] += 1.0;
    let forged_owner =
        OwnerPredictionState::from_checkpoint(&forged, forged.expectation()).unwrap();
    assert!(
        forged_owner
            .validate_collision_with_bases(&collision, &[platform])
            .is_err()
    );
    platform.scene_revision += 1;
    assert!(
        owner
            .validate_collision_with_bases(&collision, &[platform])
            .is_err()
    );
}
#[test]
fn dash_and_blink_detach_a_rider() {
    let mut sim = game();
    let committed = capture(&sim);
    let checkpoint = committed.owner_checkpoint(1, 1).unwrap();
    let collision = sim
        .world
        .resource::<crate::platform::MotionEnvironment>()
        .clone();
    let mut motion = checkpoint.hero.motion;
    collision
        .collision()
        .blink(&mut motion, [1.0, 0.0], 3.0)
        .unwrap();
    assert!(motion.base.attachment.is_none());
    assert!(motion.base.revision > checkpoint.hero.motion.base.revision);
    sim.step(DreamInput {
        dash: true,
        movement: [1.0, 0.0],
        ..Default::default()
    });
    let motion = capture(&sim).owner_checkpoint(1, 1).unwrap().hero.motion;
    assert!(motion.base.attachment.is_none());
}

#[test]
fn walkoff_detaches_and_forged_attachment_restore_fails() {
    let mut sim = game();
    let mut bad = sim.snapshot();
    bad.state.heroes[0]
        .motion
        .base
        .attachment
        .as_mut()
        .unwrap()
        .pose_tick += 1;
    assert!(DreamSimulation::try_from_snapshot(&bad).is_err());
    bad = sim.snapshot();
    bad.state.heroes[0]
        .motion
        .base
        .attachment
        .as_mut()
        .unwrap()
        .local_anchor[0] += 1.0;
    assert!(DreamSimulation::try_from_snapshot(&bad).is_err());
    let initial_revision = capture(&sim)
        .owner_checkpoint(1, 1)
        .unwrap()
        .hero
        .motion
        .base
        .revision;
    for _ in 0..45 {
        sim.step(DreamInput {
            movement: [1.0, 0.0],
            ..Default::default()
        });
    }
    let checkpoint = capture(&sim).owner_checkpoint(1, 1).unwrap();
    assert!(checkpoint.hero.motion.base.attachment.is_none());
    assert!(checkpoint.hero.motion.base.revision > initial_revision);
}
#[test]
fn dead_rider_keeps_current_attachment_while_party_continues() {
    let mut sim = game();
    sim.add_player(2);
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        if hero.view.id == 1 {
            hero.health.hp = 0.0;
        }
    }
    for _ in 0..3 {
        sim.step(DreamInput::default());
    }
    let checkpoint = capture(&sim).owner_checkpoint(1, 1).unwrap();
    assert_eq!(
        checkpoint.hero.motion.base.attachment.unwrap().pose_tick,
        u64::from(sim.snapshot().tick)
    );
    assert!(DreamSimulation::try_from_snapshot(&sim.snapshot()).is_ok());
}

#[test]
fn upgraded_speed_admits_reachable_platform_and_missing_history_stops_before_contact() {
    let mut sim = game();
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        *hero.motion = engine_core::KinematicState::new([0.0, 0.0, 2.1], 1);
        hero.view.movement_speed = 120.0;
    }
    let committed = capture(&sim);
    let checkpoint = committed.owner_checkpoint(1, 1).unwrap();
    let platform = sim.snapshot().platforms[0];
    assert_eq!(checkpoint.required_bases(), &[platform.collider]);
    let collision = committed.collision().build().unwrap();
    let mut owner =
        OwnerPredictionState::from_checkpoint(&checkpoint, checkpoint.expectation()).unwrap();
    for _ in 0..crate::platform::DEPENDENCY_LOOKAHEAD_TICKS {
        let input = DreamInput {
            movement: [0.0, -1.0],
            ..Default::default()
        };
        owner
            .step_restricted_combat_with_bases(input, &collision, &[platform], None, None)
            .unwrap();
        sim.step(input);
        assert_eq!(
            owner.checkpoint.hero.motion,
            capture(&sim).owner_checkpoint(1, 1).unwrap().hero.motion
        );
    }
    // A checkpoint just outside its expanded admission envelope can be retained
    // longer than one replay burst, but it cannot silently reach a missing base.
    let speed = 15.0;
    let motion = engine_core::KinematicState::new([0.0; 3], 1);
    let radius = crate::platform::dependency_radius(&motion, speed, &collision.manifest().config());
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        *hero.motion = engine_core::KinematicState::new([0.0, 0.0, -10.0 + radius + 0.1], 1);
        hero.view.movement_speed = speed;
    }
    let checkpoint = capture(&sim).owner_checkpoint(1, 1).unwrap();
    assert!(checkpoint.required_bases().is_empty());
    let mut owner =
        OwnerPredictionState::from_checkpoint(&checkpoint, checkpoint.expectation()).unwrap();
    let mut rejected = false;
    for _ in 0..256 {
        let before = owner.clone();
        if owner
            .step_restricted(
                DreamInput {
                    movement: [0.0, -1.0],
                    ..Default::default()
                },
                &collision,
            )
            .is_err()
        {
            assert_eq!(owner, before);
            assert!(owner.checkpoint.hero.motion.position[2] > -6.0);
            rejected = true;
            break;
        }
    }
    assert!(
        rejected,
        "unadmitted support must stop prediction before interaction"
    );
}

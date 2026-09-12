use super::*;
use crate::{DreamInput, DreamSimulation};
fn sample(tick: u32) -> PublicPlatformView {
    let mut platform = crate::state::Platform::new(1 << 63);
    platform.advance(tick).unwrap();
    platform.presentation(1)
}
#[test]
fn trajectory_is_bounded_exact_and_periodic() {
    for tick in 0..1024 {
        let view = sample(tick);
        assert!(view.valid());
        assert_eq!(view.pose_at(tick).unwrap(), pose(view.phase));
        assert_eq!(view.pose_at(tick + 1).unwrap(), sample(tick + 1).pose);
        assert!(view.pose_at(tick + 257).is_err());
    }
    assert_eq!(sample(0).pose, sample(1024).pose);
}
#[test]
fn platform_clock_snapshot_and_pause_are_authoritative() {
    let mut sim = DreamSimulation::new(17, false);
    assert_eq!(sim.snapshot().platforms[0].gameplay_tick, 0);
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().platforms[0].gameplay_tick, 0);
    sim.continue_run();
    sim.step(DreamInput::default());
    let saved = sim.snapshot();
    assert_eq!(saved.platforms[0].gameplay_tick, saved.tick);
    sim.world.resource_mut::<crate::state::Run>().paused = true;
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().platforms, saved.platforms);
}
#[test]
fn composed_environment_rejects_wrong_scene_and_invalid_pose() {
    let collision = crate::collision::CollisionManifest::current(1)
        .build()
        .unwrap();
    let mut view = sample(0);
    assert!(MotionEnvironment::from_dependencies(&collision, &[view], 0).is_ok());
    view.scene_revision = 2;
    assert!(MotionEnvironment::from_dependencies(&collision, &[view], 0).is_err());
    view = sample(0);
    view.pose.position[0] += 1.0;
    assert!(MotionEnvironment::from_dependencies(&collision, &[view], 0).is_err());
}

#[test]
fn support_carry_crosses_quaternion_period_boundary_without_teleport() {
    let collision = crate::collision::CollisionManifest::current(1)
        .build()
        .unwrap();
    let view = sample(1022);
    let mut motion = engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
    let mut combat = engine_core::CombatState::default();
    for tick in 1022..1027 {
        let previous = Vec3::from_array(motion.position);
        let environment = MotionEnvironment::from_dependencies(&collision, &[view], tick).unwrap();
        crate::systems::advance_owner_motion(
            &mut motion,
            &mut combat,
            7.8,
            DreamInput::default(),
            &environment,
        )
        .unwrap();
        assert!(motion.base.attachment.is_some());
        assert!(previous.distance(Vec3::from_array(motion.position)) < 0.05);
    }
}

#[test]
fn charge_carries_while_held_then_detaches_for_authored_execution() {
    let collision = crate::collision::CollisionManifest::current(1)
        .build()
        .unwrap();
    let view = sample(0);
    let mut motion = engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
    let mut combat = engine_core::CombatState::default();
    let mut charge = crate::ChargedMovement::default();
    for tick in 0..10 {
        let environment = MotionEnvironment::from_dependencies(&collision, &[view], tick).unwrap();
        let command = match tick {
            1 => engine_core::ChargeCommand::Begin { episode: 1 },
            9 => engine_core::ChargeCommand::Release { episode: 1 },
            _ => engine_core::ChargeCommand::None,
        };
        crate::systems::advance_owner_charged_motion(
            &mut charge,
            None,
            &mut motion,
            &mut combat,
            7.8,
            DreamInput {
                charge: command,
                aim: [1.0, 0.0],
                ..Default::default()
            },
            &environment,
        )
        .unwrap();
        if tick < 9 {
            assert!(motion.base.attachment.is_some());
        } else {
            assert!(motion.base.attachment.is_none());
            assert!(matches!(
                charge.state.phase,
                engine_core::ChargePhase::Executing { cursor: 1, .. }
            ));
        }
    }
    assert_eq!(charge.stamina.current(), 70.0);
}

#[test]
fn missing_support_guard_uses_the_next_authored_sample() {
    let motion = engine_core::KinematicState::new([5.0, 0.01, -10.0], 1);
    let config = crate::collision::movement_profile();
    let mut charge = crate::ChargedMovement::default();
    assert!(!missing_base_may_interact(
        &motion,
        7.8,
        DreamInput::default(),
        charge.maximum_displacement(engine_core::ChargeCommand::None),
        &config
    ));
    charge.state.phase = engine_core::ChargePhase::Executing {
        curve: crate::CHARGE_CURVE,
        cursor: 4,
        direction: [-1.0, 0.0],
        distance: 7.5,
    };
    assert!(missing_base_may_interact(
        &motion,
        7.8,
        DreamInput::default(),
        charge.maximum_displacement(engine_core::ChargeCommand::None),
        &config
    ));
}

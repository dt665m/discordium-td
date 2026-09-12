use super::*;
use bevy::prelude::*;

fn key(index: u64) -> ColliderKey {
    ColliderKey {
        index,
        generation: 1,
    }
}
fn pose(x: f32) -> BasePose {
    BasePose::stationary([x, -0.5, 0.0], Quat::IDENTITY.to_array())
}
fn sample(previous: BasePose, current: BasePose) -> BaseSample {
    BaseSample {
        collider: key(1),
        previous,
        current,
    }
}
fn scene(samples: &[BaseSample], extras: Vec<StaticCollider>) -> CollisionScene {
    let mut colliders: Vec<_> = samples
        .iter()
        .map(|sample| StaticCollider {
            key: sample.collider,
            position: sample.current.position,
            rotation: sample.current.rotation,
            shape: CollisionShape::Box {
                half_extents: [2.0, 0.5, 2.0],
            },
        })
        .collect();
    colliders.extend(extras);
    CollisionScene::new(1, colliders).unwrap()
}
fn step(
    state: &mut KinematicState,
    config: &KinematicConfig,
    input: KinematicInput,
    frame: &BaseFrame,
    scene: &CollisionScene,
    detach: bool,
) -> Result<KinematicReport, KinematicError> {
    advance_kinematic_with_bases(
        state,
        config,
        input,
        scene,
        BaseStep {
            frame,
            config: BaseConfig::default(),
            detach,
        },
    )
}
fn attached(x: f32) -> KinematicState {
    let frame = BaseFrame::new(1, 0, 1, vec![sample(pose(0.0), pose(0.0))]).unwrap();
    let mut state = KinematicState::new([x, 0.005, 0.0], 1);
    let report = step(
        &mut state,
        &KinematicConfig::default(),
        KinematicInput::default(),
        &frame,
        &scene(frame.samples(), vec![]),
        false,
    )
    .unwrap();
    assert!(report.base_changed);
    assert_eq!(state.base.revision, 1);
    assert_eq!(state.base.attachment.unwrap().pose_tick, 1);
    state
}

#[test]
fn translating_support_carries_before_intent_and_keeps_local_anchor() {
    let mut state = attached(0.0);
    let anchor = state.base.attachment.unwrap().local_anchor;
    let mut current = pose(0.1);
    current.linear_velocity = [6.0, 0.0, 0.0];
    let frame = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), current)]).unwrap();
    let report = step(
        &mut state,
        &KinematicConfig::default(),
        KinematicInput::default(),
        &frame,
        &scene(frame.samples(), vec![]),
        false,
    )
    .unwrap();
    assert!(!report.base_changed);
    assert!((state.position[0] - 0.1).abs() < 0.0001, "{state:?}");
    assert_eq!(state.velocity, [0.0; 3]);
    assert_eq!(state.base.attachment.unwrap().local_anchor, anchor);
    assert_eq!(state.base.attachment.unwrap().pose_tick, 2);
}

#[test]
fn rotating_support_moves_anchor_and_rotates_facing_from_historical_pose() {
    let mut state = attached(1.0);
    let mut current = pose(0.0);
    current.rotation = Quat::from_rotation_y(0.2).to_array();
    current.angular_velocity = [0.0, 12.0, 0.0];
    let frame = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), current)]).unwrap();
    step(
        &mut state,
        &KinematicConfig::default(),
        KinematicInput::default(),
        &frame,
        &scene(frame.samples(), vec![]),
        false,
    )
    .unwrap();
    let expected = Quat::from_rotation_y(0.2) * Vec3::X;
    assert!((state.position[0] - expected.x).abs() < 0.0001, "{state:?}");
    assert!((state.position[2] - expected.z).abs() < 0.0001);
    let facing = Quat::from_rotation_y(0.2) * -Vec3::Z;
    assert!((state.facing[0] - facing.x).abs() < 0.0001);
    assert!((state.facing[1] - facing.z).abs() < 0.0001);
}

#[test]
fn jump_inherits_linear_and_angular_velocity_once_before_motion() {
    let mut state = attached(1.0);
    let mut current = pose(0.1);
    current.linear_velocity = [6.0, 0.0, 0.0];
    current.angular_velocity = [0.0, 2.0, 0.0];
    let frame = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), current)]).unwrap();
    let config = KinematicConfig {
        gravity: 0.0,
        air_acceleration: 0.0,
        ..Default::default()
    };
    let report = step(
        &mut state,
        &config,
        KinematicInput {
            jump: true,
            ..Default::default()
        },
        &frame,
        &scene(frame.samples(), vec![]),
        false,
    )
    .unwrap();
    assert!(report.jumped && report.base_changed);
    assert_eq!(state.base.attachment, None);
    assert_eq!(state.base.revision, 2);
    assert!((state.velocity[0] - 6.0).abs() < 0.0001, "{state:?}");
    assert!((state.velocity[2] + 2.0).abs() < 0.0001);
    assert!((state.position[0] - 1.2).abs() < 0.0001);
    let after_jump = state;
    let next = BaseFrame::new(1, 2, 3, vec![sample(current, current)]).unwrap();
    step(
        &mut state,
        &config,
        KinematicInput::default(),
        &next,
        &scene(next.samples(), vec![]),
        false,
    )
    .unwrap();
    assert_eq!(state.velocity, after_jump.velocity);
    assert_eq!(state.base.revision, 2);
}

#[test]
fn explicit_detach_preserves_world_position_before_own_motion_and_is_once_only() {
    let mut state = attached(0.0);
    let mut current = pose(0.1);
    current.linear_velocity = [6.0, 0.0, 0.0];
    let frame = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), current)]).unwrap();
    let config = KinematicConfig {
        gravity: 0.0,
        air_acceleration: 0.0,
        ..Default::default()
    };
    step(
        &mut state,
        &config,
        KinematicInput::default(),
        &frame,
        &scene(frame.samples(), vec![]),
        true,
    )
    .unwrap();
    assert!((state.position[0] - 0.1).abs() < 0.0001);
    assert!((state.velocity[0] - 6.0).abs() < 0.001);
    assert_eq!(state.base.attachment, None);
    let next = BaseFrame::new(1, 2, 3, vec![sample(current, current)]).unwrap();
    step(
        &mut state,
        &config,
        KinematicInput::default(),
        &next,
        &scene(next.samples(), vec![]),
        true,
    )
    .unwrap();
    assert!((state.velocity[0] - 6.0).abs() < 0.001);
    assert_eq!(state.base.revision, 2);
}

#[test]
fn historical_replay_is_independent_of_later_current_poses() {
    let initial = attached(0.7);
    let mut authority = initial;
    let mut history = Vec::new();
    let mut correction = None;
    let mut previous = pose(0.0);
    for tick in 1..=15 {
        let mut current = pose(tick as f32 * 0.02);
        current.rotation = Quat::from_rotation_y(tick as f32 * 0.01).to_array();
        current.linear_velocity = [1.2, 0.0, 0.0];
        current.angular_velocity = [0.0, 0.6, 0.0];
        let frame = BaseFrame::new(1, tick, tick + 1, vec![sample(previous, current)]).unwrap();
        let collision = scene(frame.samples(), vec![]);
        let input = KinematicInput {
            movement: [0.2, 0.1],
            ..Default::default()
        };
        step(
            &mut authority,
            &KinematicConfig::default(),
            input,
            &frame,
            &collision,
            false,
        )
        .unwrap();
        history.push((frame, collision, input));
        if tick == 7 {
            correction = Some(serde_json::to_vec(&authority).unwrap());
        }
        previous = current;
    }
    let later_render_pose = pose(999.0);
    assert_ne!(later_render_pose.position, previous.position);
    let mut replay = initial;
    for (frame, collision, input) in &history {
        step(
            &mut replay,
            &KinematicConfig::default(),
            *input,
            frame,
            collision,
            false,
        )
        .unwrap();
    }
    assert_eq!(replay, authority);
    let mut corrected: KinematicState = serde_json::from_slice(&correction.unwrap()).unwrap();
    for (frame, collision, input) in history.iter().skip(7) {
        step(
            &mut corrected,
            &KinematicConfig::default(),
            *input,
            frame,
            collision,
            false,
        )
        .unwrap();
    }
    assert_eq!(
        corrected, authority,
        "restored attachment replays the exact retained base intervals"
    );
    let bytes = serde_json::to_vec(&authority).unwrap();
    assert_eq!(
        serde_json::from_slice::<KinematicState>(&bytes).unwrap(),
        authority
    );
}

#[test]
fn missing_stale_wrong_generation_and_wrong_scene_history_are_transactional() {
    let initial = attached(0.0);
    let good = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), pose(0.0))]).unwrap();
    let collision = scene(good.samples(), vec![]);
    let empty = BaseFrame::new(1, 1, 2, vec![]).unwrap();
    let stale = BaseFrame::new(1, 0, 1, good.samples().to_vec()).unwrap();
    let wrong_scene = BaseFrame::new(2, 1, 2, good.samples().to_vec()).unwrap();
    let mut wrong_key = good.samples().to_vec();
    wrong_key[0].collider.generation = 2;
    let generation = BaseFrame::new(1, 1, 2, wrong_key).unwrap();
    let mut moved = good.samples().to_vec();
    moved[0].current.position[0] = 0.1;
    let mismatched_pose = BaseFrame::new(1, 1, 2, moved).unwrap();
    for frame in [empty, stale, wrong_scene, generation, mismatched_pose] {
        let mut state = initial;
        assert!(
            step(
                &mut state,
                &KinematicConfig::default(),
                KinematicInput::default(),
                &frame,
                &collision,
                false
            )
            .is_err()
        );
        assert_eq!(state, initial);
    }
    let mut state = initial;
    assert_eq!(
        advance_kinematic(
            &mut state,
            &KinematicConfig::default(),
            KinematicInput::default(),
            &collision
        ),
        Err(KinematicError::MissingBaseHistory)
    );
    assert_eq!(state, initial);
}

#[test]
fn carry_into_wall_is_swept_and_queries_share_the_tick_budget() {
    let initial = attached(0.0);
    let frame = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), pose(0.8))]).unwrap();
    let collision = scene(
        frame.samples(),
        vec![StaticCollider::cuboid(
            key(2),
            [0.6, 1.0, 0.0],
            [0.01, 1.0, 5.0],
        )],
    );
    let config = KinematicConfig {
        max_step_height: 0.0,
        ..Default::default()
    };
    let mut state = initial;
    let report = step(
        &mut state,
        &config,
        KinematicInput::default(),
        &frame,
        &collision,
        false,
    )
    .unwrap();
    assert!(report.base_carry_blocked);
    assert!(state.position[0] < 0.3, "{state:?}");
    assert!(report.queries > 1);
    let mut state = initial;
    let tiny = KinematicConfig {
        max_queries: 1,
        ..config
    };
    assert_eq!(
        step(
            &mut state,
            &tiny,
            KinematicInput::default(),
            &frame,
            &collision,
            false
        ),
        Err(KinematicError::QueryBudgetExceeded)
    );
    assert_eq!(state, initial);
}

#[test]
fn base_frames_bound_identity_pose_motion_and_allocation() {
    assert!(BaseFrame::new(1, 1, 3, vec![]).is_err());
    assert!(BaseFrame::new(1, u64::MAX, 0, vec![]).is_err());
    assert!(
        BaseFrame::new(
            1,
            0,
            1,
            vec![sample(pose(0.0), pose(0.0)); MAX_MOVING_BASES + 1]
        )
        .is_err()
    );
    let mut invalid = sample(pose(0.0), pose(0.0));
    invalid.current.angular_velocity[0] = f32::NAN;
    assert!(BaseFrame::new(1, 0, 1, vec![invalid]).is_err());
    let mut state = attached(0.0);
    let initial = state;
    let frame = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), pose(3.0))]).unwrap();
    assert_eq!(
        step(
            &mut state,
            &KinematicConfig::default(),
            KinematicInput::default(),
            &frame,
            &scene(frame.samples(), vec![]),
            false
        ),
        Err(KinematicError::BaseMotionLimit)
    );
    assert_eq!(state, initial);
    let mut state = attached(0.0);
    state.base.revision = u64::MAX;
    let initial = state;
    let frame = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), pose(0.0))]).unwrap();
    assert_eq!(
        step(
            &mut state,
            &KinematicConfig::default(),
            KinematicInput::default(),
            &frame,
            &scene(frame.samples(), vec![]),
            true
        ),
        Err(KinematicError::BaseRevisionExhausted)
    );
    assert_eq!(state, initial);
}

#[test]
fn walking_off_inherits_once_after_the_ordinary_ground_transition() {
    let config = KinematicConfig {
        speed: 12.0,
        acceleration: 0.0,
        air_acceleration: 0.0,
        braking: 0.0,
        gravity: 0.0,
        max_step_height: 0.0,
        ..Default::default()
    };
    let mut state = attached(1.8);
    state.velocity = [12.0, 0.0, 0.0];
    let mut base_pose = pose(0.0);
    base_pose.linear_velocity = [2.0, 0.0, 0.0];
    let mut departed = false;
    for tick in 1..=10 {
        let frame = BaseFrame::new(1, tick, tick + 1, vec![sample(base_pose, base_pose)]).unwrap();
        let collision = scene(frame.samples(), vec![]);
        let input = KinematicInput {
            movement: [1.0, 0.0],
            ..Default::default()
        };
        let mut unbased = state;
        unbased.base.attachment = None;
        advance_kinematic(&mut unbased, &config, input, &collision).unwrap();
        let report = step(&mut state, &config, input, &frame, &collision, false).unwrap();
        if report.base_changed {
            assert!(!departed);
            departed = true;
            assert_eq!(state.base.attachment, None);
            assert_eq!(state.base.revision, 2);
            assert!((state.velocity[0] - unbased.velocity[0] - 2.0).abs() < 0.001);
        } else if departed {
            assert_eq!(
                state.velocity, unbased.velocity,
                "departure contribution applied twice"
            );
            break;
        }
    }
    assert!(departed);
}

#[test]
fn parent_change_preserves_world_velocity_even_when_dismount_inheritance_is_disabled() {
    let config = KinematicConfig {
        braking: 0.0,
        ..Default::default()
    };
    let mut state = attached(0.0);
    // The old support is key 2. Introducing an equal-pose support with key 1
    // makes the deterministic contact ordering choose the new parent.
    state.base.attachment.as_mut().unwrap().collider = key(2);
    state.ground.as_mut().unwrap().collider = key(2);
    let mut old_pose = pose(0.0);
    old_pose.linear_velocity = [2.0, 0.0, 0.0];
    let mut new_pose = pose(0.0);
    new_pose.linear_velocity = [5.0, 0.0, 0.0];
    let frame = BaseFrame::new(
        1,
        1,
        2,
        vec![
            BaseSample {
                collider: key(2),
                previous: old_pose,
                current: old_pose,
            },
            sample(new_pose, new_pose),
        ],
    )
    .unwrap();
    let before = state.position;
    let report = advance_kinematic_with_bases(
        &mut state,
        &config,
        KinematicInput::default(),
        &scene(frame.samples(), vec![]),
        BaseStep {
            frame: &frame,
            detach: false,
            config: BaseConfig {
                inherit_linear: 0.0,
                inherit_angular: 0.0,
                ..Default::default()
            },
        },
    )
    .unwrap();
    assert!(report.base_changed);
    assert_eq!(state.base.attachment.unwrap().collider, key(1));
    assert_eq!(state.base.revision, 2);
    assert!(Vec3::from_array(state.position).distance(Vec3::from_array(before)) < 0.000001);
    assert!(
        (state.velocity[0] + 3.0).abs() < 0.0001,
        "old world 2 becomes new relative -3: {state:?}"
    );
}

#[test]
fn committed_attachment_validation_rejects_forged_tick_and_anchor_without_step() {
    let state = attached(0.25);
    let frame = BaseFrame::new(1, 1, 2, vec![sample(pose(0.0), pose(0.0))]).unwrap();
    frame.validate_attachment(&state).unwrap();
    let mut wrong = state;
    wrong.base.attachment.as_mut().unwrap().pose_tick = 0;
    assert_eq!(
        frame.validate_attachment(&wrong),
        Err(KinematicError::InvalidBaseFrame)
    );
    wrong = state;
    wrong.base.attachment.as_mut().unwrap().local_anchor[0] += 1.0;
    assert_eq!(
        frame.validate_attachment(&wrong),
        Err(KinematicError::InvalidBaseFrame)
    );
}

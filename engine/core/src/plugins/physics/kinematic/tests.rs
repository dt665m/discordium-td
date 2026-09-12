use super::*;
fn key(index: u64) -> ColliderKey {
    ColliderKey {
        index,
        generation: 1,
    }
}
fn floor() -> StaticCollider {
    StaticCollider::cuboid(key(1), [0.0, -0.5, 0.0], [20.0, 0.5, 20.0])
}
#[test]
fn flat_motion_is_grounded_and_normalized() {
    let scene = CollisionScene::new(1, vec![floor()]).unwrap();
    let config = KinematicConfig::default();
    let mut state = KinematicState::new([0.0, 0.02, 0.0], 1);
    for _ in 0..60 {
        advance_kinematic(
            &mut state,
            &config,
            KinematicInput {
                movement: [1.0, 1.0],
                ..Default::default()
            },
            &scene,
        )
        .unwrap();
    }
    assert!(state.grounded, "{state:?}");
    assert!((state.position[1] - config.skin).abs() < 0.001, "{state:?}");
    assert!(bevy::prelude::Vec3::from_array(state.velocity).length() <= config.speed + 0.001);
}

#[test]
fn continuous_dash_cannot_tunnel_through_thin_wall() {
    let scene = CollisionScene::new(
        1,
        vec![
            floor(),
            StaticCollider::cuboid(key(2), [2.0, 2.0, 0.0], [0.005, 2.0, 10.0]),
        ],
    )
    .unwrap();
    let config = KinematicConfig {
        dash_speed: 300.0,
        ..Default::default()
    };
    let mut state = KinematicState::new([0.0, 0.01, 0.0], 1);
    let report = advance_kinematic(
        &mut state,
        &config,
        KinematicInput {
            movement: [1.0, 0.0],
            dash: true,
            ..Default::default()
        },
        &scene,
    )
    .unwrap();
    assert!(report.slide_contacts > 0);
    assert!(
        state.position[0] <= 2.0 - 0.005 - config.radius + 0.001,
        "{state:?}"
    );
    assert!(state.position[0] > 1.5, "{state:?}");
}

#[test]
fn corner_contact_ties_ignore_insertion_order() {
    let colliders = vec![
        floor(),
        StaticCollider::cuboid(key(3), [2.0, 2.0, 0.0], [0.1, 2.0, 10.0]),
        StaticCollider::cuboid(key(2), [0.0, 2.0, 2.0], [10.0, 2.0, 0.1]),
    ];
    let a = CollisionScene::new(1, colliders.clone()).unwrap();
    let b = CollisionScene::new(1, colliders.into_iter().rev().collect()).unwrap();
    let mut qa = super::scene::Queries::new(&a, 100);
    let mut qb = super::scene::Queries::new(&b, 100);
    let movement = bevy::prelude::Vec3::new(3.0, 0.0, 3.0);
    let foot = bevy::prelude::Vec3::new(0.0, 0.005, 0.0);
    let ha = qa.cast(foot, 1.8, 0.3, movement, 0.005).unwrap().unwrap();
    let hb = qb.cast(foot, 1.8, 0.3, movement, 0.005).unwrap().unwrap();
    assert_eq!(ha.collider, key(2));
    assert_eq!(ha.collider, hb.collider);
    assert_eq!(ha.fraction, hb.fraction);
    let config = KinematicConfig::default();
    let mut sa = KinematicState::new(foot.to_array(), 1);
    let mut sb = sa;
    for _ in 0..60 {
        let input = KinematicInput {
            movement: [1.0, 1.0],
            ..Default::default()
        };
        advance_kinematic(&mut sa, &config, input, &a).unwrap();
        advance_kinematic(&mut sb, &config, input, &b).unwrap();
        assert_eq!(sa, sb);
    }
    assert!(sa.position[0] < 1.61 && sa.position[2] < 1.61, "{sa:?}");
}

#[test]
fn jump_suppresses_snap_and_buffer_is_consumed_once() {
    let scene = CollisionScene::new(1, vec![floor()]).unwrap();
    let config = KinematicConfig::default();
    let mut state = KinematicState::new([0.0, 0.005, 0.0], 1);
    let report = advance_kinematic(
        &mut state,
        &config,
        KinematicInput {
            jump: true,
            ..Default::default()
        },
        &scene,
    )
    .unwrap();
    assert!(report.jumped && !state.grounded);
    assert!(state.position[1] > 0.1 && state.velocity[1] > 0.0);
    assert_eq!(state.jump_buffer_ticks, 0);
    for _ in 0..100 {
        assert!(
            !advance_kinematic(&mut state, &config, Default::default(), &scene)
                .unwrap()
                .jumped
        );
    }
    assert!(state.grounded);
}

#[test]
fn crouch_release_under_ceiling_remains_crouched_until_clear() {
    let scene = CollisionScene::new(
        1,
        vec![
            floor(),
            StaticCollider::cuboid(key(2), [0.0, 1.5, 0.0], [0.5, 0.3, 3.0]),
        ],
    )
    .unwrap();
    let config = KinematicConfig::default();
    let mut state = KinematicState::new([0.0, 0.005, 0.0], 1);
    state.stance = Stance::Crouched;
    let report = advance_kinematic(&mut state, &config, Default::default(), &scene).unwrap();
    assert!(report.stand_blocked);
    assert_eq!(state.stance, Stance::Crouched);
    for _ in 0..45 {
        advance_kinematic(
            &mut state,
            &config,
            KinematicInput {
                movement: [1.0, 0.0],
                ..Default::default()
            },
            &scene,
        )
        .unwrap();
    }
    assert_eq!(state.stance, Stance::Standing, "{state:?}");
    assert!((state.position[1] - config.skin).abs() < 0.001, "{state:?}");
}

fn traverse_step(height: f32, ceiling: bool) -> (KinematicState, bool) {
    let mut colliders = vec![
        floor(),
        StaticCollider::cuboid(key(2), [1.5, height * 0.5, 0.0], [1.5, height * 0.5, 3.0]),
    ];
    if ceiling {
        colliders.push(StaticCollider::cuboid(
            key(3),
            [0.0, 2.0, 0.0],
            [4.0, 0.1, 3.0],
        ));
    }
    let scene = CollisionScene::new(1, colliders).unwrap();
    let config = KinematicConfig {
        fixed_dt: 0.05,
        acceleration: 1000.0,
        ..Default::default()
    };
    let mut state = KinematicState::new([-1.0, 0.005, 0.0], 1);
    let mut stepped = false;
    for _ in 0..12 {
        stepped |= advance_kinematic(
            &mut state,
            &config,
            KinematicInput {
                movement: [1.0, 0.0],
                ..Default::default()
            },
            &scene,
        )
        .unwrap()
        .stepped;
    }
    (state, stepped)
}
#[test]
fn step_height_boundary_and_upward_clearance() {
    let (accepted, stepped) = traverse_step(0.35, false);
    assert!(
        accepted.position[0] > 0.5 && stepped,
        "{accepted:?} stepped={stepped}"
    );
    assert!((accepted.position[1] - 0.355).abs() < 0.01, "{accepted:?}");
    let (blocked, _) = traverse_step(0.36, false);
    assert!(blocked.position[0] < 0.0, "{blocked:?}");
    let (ceiling, _) = traverse_step(0.35, true);
    assert!(ceiling.position[0] < 0.0, "{ceiling:?}");
}

fn ramp(angle: f32) -> CollisionScene {
    let rotation = bevy::prelude::Quat::from_rotation_z(angle);
    let position = rotation * bevy::prelude::Vec3::new(0.0, -0.25, 0.0);
    let mut collider = StaticCollider::cuboid(key(1), position.to_array(), [8.0, 0.25, 4.0]);
    collider.rotation = rotation.to_array();
    CollisionScene::new(1, vec![collider]).unwrap()
}
#[test]
fn slope_classification_prevents_climbing_unwalkable_ramps() {
    let config = KinematicConfig {
        max_step_height: 0.0,
        ..Default::default()
    };
    for (degrees, walkable) in [(30.0_f32, true), (60.0, false)] {
        let angle = degrees.to_radians();
        let scene = ramp(angle);
        let x = -2.0;
        let y = x * angle.tan() + config.radius * (1.0 / angle.cos() - 1.0) + 0.01;
        let mut state = KinematicState::new([x, y, 0.0], 1);
        advance_kinematic(&mut state, &config, Default::default(), &scene).unwrap();
        assert_eq!(state.grounded, walkable, "degrees={degrees}, {state:?}");
        for _ in 0..20 {
            advance_kinematic(
                &mut state,
                &config,
                KinematicInput {
                    movement: [1.0, 0.0],
                    ..Default::default()
                },
                &scene,
            )
            .unwrap();
        }
        if walkable {
            assert!(
                state.position[0] > x + 0.4 && state.position[1] > y,
                "{state:?}"
            );
        } else {
            assert!(state.position[0] <= x + 0.01, "{state:?}");
        }
    }
}

#[test]
fn overlap_resolution_is_bounded_and_transactional() {
    let config = KinematicConfig::default();
    let scene = CollisionScene::new(1, vec![floor()]).unwrap();
    let mut state = KinematicState::new([0.0, -0.1, 0.0], 1);
    let report = advance_kinematic(&mut state, &config, Default::default(), &scene).unwrap();
    assert!(report.depenetrations > 0 && state.position[1] >= 0.0);
    let trapped = CollisionScene::new(
        1,
        vec![
            floor(),
            StaticCollider::cuboid(key(2), [0.0, 2.0, 0.0], [10.0, 1.0, 10.0]),
        ],
    )
    .unwrap();
    let mut invalid = KinematicState::new([0.0, 0.005, 0.0], 1);
    let before = invalid;
    assert_eq!(
        advance_kinematic(&mut invalid, &config, Default::default(), &trapped),
        Err(KinematicError::DepenetrationLimit)
    );
    assert_eq!(invalid, before);
    let tiny_budget = KinematicConfig {
        max_queries: 1,
        ..config
    };
    assert_eq!(
        advance_kinematic(&mut invalid, &tiny_budget, Default::default(), &scene),
        Err(KinematicError::QueryBudgetExceeded)
    );
    assert_eq!(invalid, before);
}

#[test]
fn restoring_serialized_state_replays_identical_contacts() {
    let scene = CollisionScene::new(
        1,
        vec![
            floor(),
            StaticCollider::cuboid(key(2), [2.0, 2.0, 0.0], [0.1, 2.0, 10.0]),
        ],
    )
    .unwrap();
    let config = KinematicConfig::default();
    let inputs: Vec<_> = (0..80)
        .map(|i| KinematicInput {
            movement: [1.0, 0.2],
            jump: i == 8,
            dash: i == 30,
            crouch: (40..50).contains(&i),
        })
        .collect();
    let mut state = KinematicState::new([0.0, 0.02, 0.0], 1);
    let mut checkpoints = vec![state];
    for input in &inputs {
        advance_kinematic(&mut state, &config, *input, &scene).unwrap();
        checkpoints.push(state);
    }
    for (i, checkpoint) in checkpoints.iter().enumerate() {
        let mut restored: KinematicState =
            serde_json::from_slice(&serde_json::to_vec(checkpoint).unwrap()).unwrap();
        for input in &inputs[i..] {
            advance_kinematic(&mut restored, &config, *input, &scene).unwrap();
        }
        assert_eq!(restored, state, "boundary {i}");
    }
}

#[test]
fn invalid_geometry_and_input_reject_without_mutation() {
    let bad = StaticCollider::cuboid(key(1), [0.0; 3], [f32::NAN, 1.0, 1.0]);
    assert!(CollisionScene::new(1, vec![bad]).is_err());
    assert!(CollisionScene::new(1, vec![floor(), floor()]).is_err());
    assert!(CollisionScene::new(1, vec![floor(); MAX_COLLIDERS + 1]).is_err());
    let scene = CollisionScene::new(1, vec![floor()]).unwrap();
    let mut state = KinematicState::new([0.0, 0.005, 0.0], 1);
    let before = state;
    assert_eq!(
        advance_kinematic(
            &mut state,
            &Default::default(),
            KinematicInput {
                movement: [f32::INFINITY, 0.0],
                ..Default::default()
            },
            &scene
        ),
        Err(KinematicError::InvalidInput)
    );
    assert_eq!(state, before);
    state.scene_revision = 2;
    let before = state;
    assert_eq!(
        advance_kinematic(&mut state, &Default::default(), Default::default(), &scene),
        Err(KinematicError::SceneRevisionMismatch)
    );
    assert_eq!(state, before);
}

#[test]
fn overlap_ties_reordered_shapes_restore_the_same_position() {
    let colliders = vec![
        floor(),
        StaticCollider::cuboid(key(2), [-0.1, 2.0, 0.0], [0.1, 2.0, 10.0]),
    ];
    let a = CollisionScene::new(1, colliders.clone()).unwrap();
    let b = CollisionScene::new(1, colliders.into_iter().rev().collect()).unwrap();
    let mut sa = KinematicState::new([0.2, -0.1, 0.0], 1);
    let mut sb = sa;
    advance_kinematic(&mut sa, &Default::default(), Default::default(), &a).unwrap();
    advance_kinematic(&mut sb, &Default::default(), Default::default(), &b).unwrap();
    assert_eq!(sa, sb);
    assert!(sa.position[0] >= 0.3 && sa.position[1] >= 0.0, "{sa:?}");
}

#[test]
fn teleported_character_suppresses_ground_snap_for_declared_tick() {
    let scene = CollisionScene::new(1, vec![floor()]).unwrap();
    let mut state = KinematicState::new([0.0, 0.08, 0.0], 1);
    state.suppress_snap_ticks = 1;
    advance_kinematic(&mut state, &Default::default(), Default::default(), &scene).unwrap();
    assert!(!state.grounded && state.position[1] > 0.06, "{state:?}");
    advance_kinematic(&mut state, &Default::default(), Default::default(), &scene).unwrap();
    assert!(state.grounded && state.position[1] < 0.01, "{state:?}");
}

#[test]
fn braking_stops_ground_motion_and_air_control_remains_bounded() {
    let scene = CollisionScene::new(1, vec![floor()]).unwrap();
    let config = KinematicConfig::default();
    let mut state = KinematicState::new([0.0, 0.005, 0.0], 1);
    for _ in 0..30 {
        advance_kinematic(
            &mut state,
            &config,
            KinematicInput {
                movement: [1.0, 0.0],
                ..Default::default()
            },
            &scene,
        )
        .unwrap();
    }
    for _ in 0..20 {
        advance_kinematic(&mut state, &config, Default::default(), &scene).unwrap();
    }
    assert!(
        bevy::prelude::Vec3::from_array(state.velocity).length() < 0.001,
        "{state:?}"
    );
    advance_kinematic(
        &mut state,
        &config,
        KinematicInput {
            jump: true,
            ..Default::default()
        },
        &scene,
    )
    .unwrap();
    advance_kinematic(
        &mut state,
        &config,
        KinematicInput {
            movement: [1.0, 0.0],
            ..Default::default()
        },
        &scene,
    )
    .unwrap();
    assert!(state.velocity[0] <= config.air_acceleration * config.fixed_dt + 0.001);
}

#[test]
fn slope_threshold_has_explicit_walkable_sides() {
    let config = KinematicConfig {
        max_step_height: 0.0,
        ..Default::default()
    };
    for (degrees, grounded) in [(44.0_f32, true), (46.0, false)] {
        let angle = degrees.to_radians();
        let scene = ramp(angle);
        let y = config.radius * (1.0 / angle.cos() - 1.0) + 0.01;
        let mut state = KinematicState::new([0.0, y, 0.0], 1);
        advance_kinematic(&mut state, &config, Default::default(), &scene).unwrap();
        assert_eq!(state.grounded, grounded, "slope {degrees}: {state:?}");
    }
}

#[test]
fn ecs_plugin_advances_only_its_ordered_schedule_and_reports_faults() {
    use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
    #[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
    struct Step;
    let mut app = App::new();
    app.init_schedule(Step)
        .insert_resource(CollisionScene::new(1, vec![floor()]).unwrap())
        .add_plugins(KinematicPlugin(Step));
    let actor = app
        .world_mut()
        .spawn((
            KinematicState::new([0.0, 0.005, 0.0], 1),
            KinematicConfig::default(),
            KinematicInput {
                movement: [1.0, 0.0],
                ..Default::default()
            },
        ))
        .id();
    let before = *app.world().get::<KinematicState>(actor).unwrap();
    app.update();
    assert_eq!(*app.world().get::<KinematicState>(actor).unwrap(), before);
    app.world_mut().run_schedule(Step);
    assert!(app.world().get::<KinematicState>(actor).unwrap().position[0] > 0.0);
    assert_eq!(
        app.world()
            .get::<KinematicStatus>(actor)
            .unwrap()
            .last_error,
        None
    );
    app.world_mut()
        .get_mut::<KinematicInput>(actor)
        .unwrap()
        .movement[0] = f32::NAN;
    let before = *app.world().get::<KinematicState>(actor).unwrap();
    app.world_mut().run_schedule(Step);
    assert_eq!(*app.world().get::<KinematicState>(actor).unwrap(), before);
    assert_eq!(
        app.world()
            .get::<KinematicStatus>(actor)
            .unwrap()
            .last_error,
        Some(KinematicError::InvalidInput)
    );
}

#[test]
fn authored_displacement_uses_capsule_sweep_and_rolls_back_on_error() {
    let scene = CollisionScene::new(
        1,
        vec![
            floor(),
            StaticCollider::cuboid(key(2), [2.0, 2.0, 0.0], [0.005, 2.0, 10.0]),
        ],
    )
    .unwrap();
    let config = KinematicConfig::default();
    let bases = BaseFrame::new(1, 0, 1, vec![]).unwrap();
    let mut state = KinematicState::new([0.0, 0.01, 0.0], 1);
    let report = advance_kinematic_with_authored_motion(
        &mut state,
        &config,
        KinematicInput::default(),
        &scene,
        BaseStep {
            frame: &bases,
            config: BaseConfig::default(),
            detach: true,
        },
        [7.5, 0.0],
    )
    .unwrap();
    assert!(report.slide_contacts > 0);
    assert!(state.position[0] <= 2.0 - 0.005 - config.radius + 0.001);
    let before = state;
    assert!(
        advance_kinematic_with_authored_motion(
            &mut state,
            &config,
            KinematicInput::default(),
            &scene,
            BaseStep {
                frame: &bases,
                config: BaseConfig::default(),
                detach: true
            },
            [f32::NAN, 0.0]
        )
        .is_err()
    );
    assert_eq!(state, before);
}

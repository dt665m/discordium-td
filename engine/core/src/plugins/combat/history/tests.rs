use super::*;
fn limits() -> HistoryLimits {
    HistoryLimits {
        frames: 3,
        poses_per_frame: 4,
        total_poses: 8,
    }
}
fn pose(id: u64, position: Vec3) -> CombatPose {
    CombatPose {
        entity: ColliderKey {
            index: id,
            generation: 1,
        },
        segment: 1,
        pose_revision: 1,
        position,
        rotation: Quat::IDENTITY,
        shape: CollisionShape::Capsule {
            half_segment: 0.5,
            radius: 0.5,
        },
        metadata: HitMetadata([1, 7, 23, 0]),
    }
}
fn budget() -> QueryBudget {
    QueryBudget::new(64, 16).unwrap()
}
fn ray() -> HitQuery {
    HitQuery::Ray {
        ray: Ray3d::new(Vec3::ZERO, Dir3::NEG_Z),
        distance: 10.0,
    }
}
#[test]
fn ray_capsule_and_convex_queries_are_canonical_3d_and_stable() {
    let mut h = HitHistory::new(limits()).unwrap();
    let a = pose(2, Vec3::new(0.0, 0.0, -3.0));
    let mut b = pose(1, Vec3::new(0.0, 0.0, -3.0));
    b.shape = CollisionShape::Box {
        half_extents: [0.5; 3],
    };
    h.capture(1, 1, &[a, b]).unwrap();
    let hits = h
        .frame(1, 1)
        .unwrap()
        .query(ray(), &mut budget(), |_| true)
        .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.pose.entity.index).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!((hits[0].distance - 2.5).abs() < 0.0001);
    let mut reversed = HitHistory::new(limits()).unwrap();
    reversed.capture(1, 1, &[b, a]).unwrap();
    assert_eq!(
        hits,
        reversed
            .frame(1, 1)
            .unwrap()
            .query(ray(), &mut budget(), |_| true)
            .unwrap()
    );
    let tilted = HitQuery::Ray {
        ray: Ray3d::new(Vec3::new(0.0, 4.0, -3.0), Dir3::NEG_Y),
        distance: 10.0,
    };
    assert_eq!(
        h.frame(1, 1)
            .unwrap()
            .query(tilted, &mut budget(), |_| true)
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn volumes_use_real_geometry_and_game_metadata_filters() {
    let mut h = HitHistory::new(limits()).unwrap();
    let inside = pose(1, Vec3::new(0.0, 0.0, -3.0));
    let outside = pose(2, Vec3::new(4.0, 0.0, -3.0));
    h.capture(1, 1, &[outside, inside]).unwrap();
    let frame = h.frame(1, 1).unwrap();
    let cone = HitQuery::Cone {
        origin: Vec3::ZERO,
        direction: Dir3::NEG_Z,
        length: 5.0,
        end_radius: 1.5,
    };
    assert_eq!(
        frame
            .query(cone, &mut budget(), |p| p.metadata.0[1] == 7)
            .unwrap()
            .len(),
        1
    );
    assert!(
        frame
            .query(cone, &mut budget(), |p| p.metadata.0[1] == 8)
            .unwrap()
            .is_empty()
    );
    let sphere = HitQuery::Sphere {
        center: Vec3::new(0.0, 0.0, -3.0),
        radius: 1.0,
    };
    assert_eq!(
        frame.query(sphere, &mut budget(), |_| true).unwrap().len(),
        1
    );
    let circle = HitQuery::Circle {
        center: Vec3::new(0.0, 0.0, -3.0),
        normal: Dir3::Y,
        radius: 1.0,
        half_thickness: 0.1,
    };
    assert_eq!(
        frame.query(circle, &mut budget(), |_| true).unwrap().len(),
        1
    );
}
#[test]
fn missing_history_lifecycle_and_teleport_are_not_interpolated() {
    let mut h = HitHistory::new(limits()).unwrap();
    let original = pose(1, Vec3::ZERO);
    h.capture(1, 1, &[original]).unwrap();
    let mut teleported = original;
    teleported.segment = 2;
    teleported.position = Vec3::splat(10.0);
    h.capture(3, 1, &[teleported]).unwrap();
    assert_eq!(h.frame(2, 1).unwrap_err(), HistoryError::MissingHistory);
    assert_eq!(h.frame(3, 2).unwrap_err(), HistoryError::WrongScene);
    assert_eq!(
        h.frame(3, 1).unwrap().pose(original.entity, 1),
        Err(HistoryError::Discontinuity)
    );
    let mut respawn = teleported;
    respawn.entity.generation = 2;
    h.capture(4, 1, &[respawn]).unwrap();
    assert_eq!(
        h.frame(4, 1).unwrap().pose(original.entity, 2),
        Err(HistoryError::WrongLifecycle)
    );
    assert_eq!(
        h.frame(1, 1)
            .unwrap()
            .pose(original.entity, 1)
            .unwrap()
            .position,
        Vec3::ZERO
    );
}
#[test]
fn retention_caps_and_invalid_capture_are_transactional() {
    let mut h = HitHistory::new(HistoryLimits {
        frames: 2,
        poses_per_frame: 2,
        total_poses: 2,
    })
    .unwrap();
    let a = pose(1, Vec3::ZERO);
    let b = pose(2, Vec3::X);
    h.capture(1, 1, &[a]).unwrap();
    assert_eq!(h.capture(2, 1, &[a, a]), Err(HistoryError::InvalidPose));
    assert_eq!(h.len(), 1);
    assert_eq!(h.capture(2, 1, &[a, b, a]), Err(HistoryError::Capacity));
    let mut invalid = a;
    invalid.position.x = f32::NAN;
    assert_eq!(h.capture(2, 1, &[invalid]), Err(HistoryError::InvalidPose));
    h.capture(2, 1, &[a, b]).unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h.retained_poses(), 2);
    assert_eq!(h.frame(1, 1).unwrap_err(), HistoryError::MissingHistory);
    assert_eq!(h.capture(2, 1, &[]), Err(HistoryError::InvalidFrame));
    assert_eq!(h.frame(2, 1).unwrap().poses().len(), 2);
}
#[test]
fn aggregate_query_and_result_budgets_never_return_partial_hits() {
    let mut h = HitHistory::new(limits()).unwrap();
    h.capture(
        1,
        1,
        &[
            pose(1, Vec3::new(0.0, 0.0, -3.0)),
            pose(2, Vec3::new(0.0, 0.0, -4.0)),
        ],
    )
    .unwrap();
    let frame = h.frame(1, 1).unwrap();
    let mut work = QueryBudget::new(1, 2).unwrap();
    assert_eq!(
        frame.query(ray(), &mut work, |_| true),
        Err(HistoryError::QueryBudget)
    );
    assert_eq!(work.used_tests(), 1);
    let mut result = QueryBudget::new(8, 1).unwrap();
    assert_eq!(
        frame.query(ray(), &mut result, |_| true),
        Err(HistoryError::ResultBudget)
    );
    let mut aggregate = QueryBudget::new(8, 2).unwrap();
    assert_eq!(
        frame.query(ray(), &mut aggregate, |_| true).unwrap().len(),
        2
    );
    assert_eq!(
        frame.query(ray(), &mut aggregate, |_| true),
        Err(HistoryError::ResultBudget)
    );
}
#[test]
fn history_owns_poses_and_concurrent_queries_cannot_mutate_live_ecs() {
    #[derive(Component)]
    struct Actor(CombatPose);
    let mut world = World::new();
    let original = pose(1, Vec3::new(0.0, 0.0, -3.0));
    let entity = world.spawn(Actor(original)).id();
    let mut h = HitHistory::new(limits()).unwrap();
    h.capture(1, 1, &[world.get::<Actor>(entity).unwrap().0])
        .unwrap();
    world.get_mut::<Actor>(entity).unwrap().0.position = Vec3::splat(100.0);
    let frame = h.frame(1, 1).unwrap();
    std::thread::scope(|scope| {
        let one = scope.spawn(|| frame.query(ray(), &mut budget(), |_| true).unwrap());
        let two = scope.spawn(|| frame.query(ray(), &mut budget(), |_| true).unwrap());
        assert_eq!(one.join().unwrap(), two.join().unwrap());
    });
    assert_eq!(frame.poses()[0], original);
    assert_eq!(
        world.get::<Actor>(entity).unwrap().0.position,
        Vec3::splat(100.0)
    );
}

#[test]
fn fractional_samples_use_exact_endpoints_and_do_not_mutate_history() {
    let mut history = HitHistory::new(limits()).unwrap();
    let a = pose(1, Vec3::new(-2.0, 0.0, -3.0));
    let mut b = pose(1, Vec3::new(2.0, 0.0, -3.0));
    b.pose_revision = 2;
    b.metadata = HitMetadata([9, 8, 7, 6]);
    history.capture(10, 1, &[a]).unwrap();
    assert_eq!(
        history
            .sample_pose(10, 0, 1, a.entity, 1, &mut budget())
            .unwrap(),
        a
    );
    assert_eq!(
        history.sample_pose(10, 1, 1, a.entity, 1, &mut budget()),
        Err(HistoryError::MissingHistory)
    );
    history.capture(11, 1, &[b]).unwrap();
    let sample = history
        .sample_frame(10, 32768, 1, &mut budget(), |_| true)
        .unwrap();
    assert_eq!(sample.poses()[0].position, Vec3::new(0.0, 0.0, -3.0));
    assert_eq!(sample.poses()[0].metadata, a.metadata);
    assert_eq!(
        sample.query(ray(), &mut budget(), |_| true).unwrap().len(),
        1
    );
    assert_eq!(*history.frame(10, 1).unwrap().pose(a.entity, 1).unwrap(), a);
    assert_eq!(*history.frame(11, 1).unwrap().pose(b.entity, 1).unwrap(), b);
    let almost = history
        .sample_pose(10, u16::MAX, 1, a.entity, 1, &mut budget())
        .unwrap();
    assert!(almost.position.x < b.position.x && almost.position.x > 1.999);
}

#[test]
fn fractional_samples_reject_lifecycle_shape_scene_and_cohort_discontinuities() {
    let a = pose(1, Vec3::new(0.0, 0.0, -3.0));
    for change in 0..5 {
        let mut history = HitHistory::new(limits()).unwrap();
        history.capture(10, 1, &[a]).unwrap();
        let mut b = a;
        match change {
            0 => b.entity.generation += 1,
            1 => b.segment += 1,
            2 => b.shape = CollisionShape::Ball { radius: 0.5 },
            _ => {}
        }
        history
            .capture(
                11,
                if change == 3 { 2 } else { 1 },
                if change == 4 {
                    &[]
                } else {
                    std::slice::from_ref(&b)
                },
            )
            .unwrap();
        assert!(
            history
                .sample_frame(10, 32768, 1, &mut budget(), |_| true)
                .is_err()
        );
    }
    let mut history = HitHistory::new(limits()).unwrap();
    history.capture(10, 1, &[]).unwrap();
    history.capture(11, 1, &[a]).unwrap();
    assert_eq!(
        history
            .sample_frame(10, 1, 1, &mut budget(), |_| true)
            .unwrap_err(),
        HistoryError::MissingEntity
    );
    let mut tiny = QueryBudget::new(1, 1).unwrap();
    assert!(history.sample_frame(10, 1, 1, &mut tiny, |_| true).is_err());
    assert_eq!(tiny.used_tests(), 1);
}

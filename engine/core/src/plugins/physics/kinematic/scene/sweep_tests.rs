use super::*;
fn key(index: u64) -> ColliderKey {
    ColliderKey {
        index,
        generation: 1,
    }
}
fn budget() -> SceneQueryBudget {
    SceneQueryBudget::new(128, 8).unwrap()
}
fn wall(index: u64, x: f32, half: f32) -> StaticCollider {
    StaticCollider::cuboid(key(index), [x, 1.0, 0.0], [half, 2.0, 4.0])
}
#[test]
fn sweep_hits_fast_thin_blocker_and_returns_world_geometry() {
    let scene = CollisionScene::new(1, vec![wall(1, 0.0, 0.001)]).unwrap();
    let hit = scene
        .sphere_cast(
            Vec3::new(-50.0, 1.0, 0.0),
            Vec3::X * 100.0,
            0.1,
            &mut budget(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(hit.collider, key(1));
    assert!((hit.toi - 0.49899).abs() < 0.00001);
    assert!((hit.point.x + 0.001).abs() < 0.00001);
    assert!(hit.normal.dot(-Vec3::X) > 0.9999);
    assert!(
        scene
            .sphere_cast(
                Vec3::new(-50.0, 6.0, 0.0),
                Vec3::X * 100.0,
                0.1,
                &mut budget()
            )
            .unwrap()
            .is_none()
    );
}
#[test]
fn nearest_hit_precedes_identity_and_exact_ties_use_stable_key() {
    let a = CollisionScene::new(1, vec![wall(2, 0.0, 0.1), wall(1, 4.0, 0.1)]).unwrap();
    assert_eq!(
        a.sphere_cast(
            Vec3::new(-5.0, 1.0, 0.0),
            Vec3::X * 10.0,
            0.1,
            &mut budget()
        )
        .unwrap()
        .unwrap()
        .collider,
        key(2)
    );
    let a = CollisionScene::new(1, vec![wall(8, 0.0, 0.1), wall(3, 0.0, 0.1)]).unwrap();
    let b = CollisionScene::new(1, vec![wall(3, 0.0, 0.1), wall(8, 0.0, 0.1)]).unwrap();
    let cast = |scene: &CollisionScene| {
        scene
            .sphere_cast(
                Vec3::new(-5.0, 1.0, 0.0),
                Vec3::X * 10.0,
                0.1,
                &mut budget(),
            )
            .unwrap()
            .unwrap()
    };
    assert_eq!(cast(&a), cast(&b));
    assert_eq!(cast(&a).collider, key(3));
}
#[test]
fn initial_overlap_stationary_and_separating_motion_hit_at_zero() {
    let box_scene = CollisionScene::new(1, vec![wall(1, 0.0, 1.0)]).unwrap();
    let ball_scene = CollisionScene::new(
        1,
        vec![StaticCollider {
            key: key(2),
            position: [0.0, 1.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            shape: CollisionShape::Ball { radius: 1.0 },
        }],
    )
    .unwrap();
    for scene in [&box_scene, &ball_scene] {
        for displacement in [Vec3::ZERO, Vec3::X * 4.0, -Vec3::X * 4.0] {
            let hit = scene
                .sphere_cast(Vec3::Y, displacement, 0.25, &mut budget())
                .unwrap()
                .unwrap();
            assert_eq!(hit.toi, 0.0);
            assert!(hit.point.is_finite());
            assert!((hit.normal.length() - 1.0).abs() < 0.00001);
        }
    }
    assert!(
        box_scene
            .sphere_cast(Vec3::new(-5.0, 1.0, 0.0), Vec3::ZERO, 0.25, &mut budget())
            .unwrap()
            .is_none()
    );
}
#[test]
fn touching_start_and_end_are_inclusive_and_rotation_is_world_space() {
    let scene = CollisionScene::new(1, vec![wall(1, 0.0, 1.0)]).unwrap();
    assert_eq!(
        scene
            .sphere_cast(Vec3::new(-1.25, 1.0, 0.0), -Vec3::X, 0.25, &mut budget())
            .unwrap()
            .unwrap()
            .toi,
        0.0
    );
    let hit = scene
        .sphere_cast(Vec3::new(-2.25, 1.0, 0.0), Vec3::X, 0.25, &mut budget())
        .unwrap()
        .unwrap();
    assert!((hit.toi - 1.0).abs() < 0.00001);
    let mut rotated = wall(2, 0.0, 0.1);
    rotated.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2).to_array();
    let scene = CollisionScene::new(1, vec![rotated]).unwrap();
    let hit = scene
        .sphere_cast(
            Vec3::new(0.0, 1.0, -5.0),
            Vec3::Z * 10.0,
            0.1,
            &mut budget(),
        )
        .unwrap()
        .unwrap();
    assert!(hit.normal.dot(-Vec3::Z) > 0.999);
    assert!((hit.point.z + 0.1).abs() < 0.0001);
}
#[test]
fn invalid_inputs_and_exhausted_aggregate_budgets_fail_without_partial_hit() {
    let scene = CollisionScene::new(1, vec![wall(1, 0.0, 0.1), wall(2, 2.0, 0.1)]).unwrap();
    let mut insufficient = SceneQueryBudget::new(3, 1).unwrap();
    assert_eq!(
        scene.sphere_cast(-Vec3::X * 5.0, Vec3::X * 10.0, 0.1, &mut insufficient),
        Err(KinematicError::QueryBudgetExceeded)
    );
    assert_eq!(insufficient.pair_tests_used(), 0);
    let mut aggregate = SceneQueryBudget::new(10, 1).unwrap();
    scene
        .sphere_cast(-Vec3::X * 5.0, Vec3::X * 10.0, 0.1, &mut aggregate)
        .unwrap()
        .unwrap();
    assert_eq!(aggregate.pair_tests_used(), 4);
    assert_eq!(aggregate.results_used(), 1);
    assert_eq!(
        scene.sphere_cast(-Vec3::X * 5.0, Vec3::X * 10.0, 0.1, &mut aggregate),
        Err(KinematicError::QueryBudgetExceeded)
    );
    assert_eq!(aggregate.pair_tests_used(), 8);
    assert_eq!(aggregate.results_used(), 1);
    for (start, movement, radius) in [
        (Vec3::NAN, Vec3::X, 0.1),
        (Vec3::ZERO, Vec3::INFINITY, 0.1),
        (Vec3::ZERO, Vec3::X, f32::NAN),
        (Vec3::ZERO, Vec3::X, 0.0),
        (Vec3::ZERO, Vec3::X, 10_001.0),
        (Vec3::X * 99_999.0, Vec3::X * 2.0, 0.1),
    ] {
        let mut work = budget();
        assert_eq!(
            scene.sphere_cast(start, movement, radius, &mut work),
            Err(KinematicError::InvalidInput)
        );
        assert_eq!(work.pair_tests_used(), 0);
    }
    assert!(SceneQueryBudget::new(MAX_SCENE_QUERY_TESTS + 1, 1).is_err());
    assert!(SceneQueryBudget::new(1, MAX_SCENE_QUERY_RESULTS + 1).is_err());
}
#[test]
fn empty_scene_misses_without_using_query_or_result_work() {
    let scene = CollisionScene::new(1, vec![]).unwrap();
    let mut work = budget();
    assert_eq!(
        scene.sphere_cast(Vec3::ZERO, Vec3::X, 0.1, &mut work),
        Ok(None)
    );
    assert_eq!(work.pair_tests_used(), 0);
    assert_eq!(work.results_used(), 0);
}

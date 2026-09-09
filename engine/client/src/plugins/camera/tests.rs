use super::*;
use std::time::Duration;

#[test]
fn screen_movement_tracks_camera_yaw_without_diagonal_speed_boost() {
    for offset in [Vec3::new(0.0, 30.0, 23.0), Vec3::new(18.0, 32.0, 26.0)] {
        let camera = GlobalTransform::from(
            Transform::from_translation(offset).looking_at(Vec3::ZERO, Vec3::Y),
        );
        let down = Vec2::new(offset.x, offset.z).normalize();
        let right = Vec2::new(down.y, -down.x);
        assert!(screen_axes_to_world(&camera, Vec2::X).abs_diff_eq(right, 1e-6));
        assert!(screen_axes_to_world(&camera, -Vec2::Y).abs_diff_eq(-down, 1e-6));
        let diagonal = screen_axes_to_world(&camera, Vec2::ONE.normalize());
        assert!((diagonal.length() - 1.0).abs() < 1e-6);
        assert!(diagonal.abs_diff_eq((right + down).normalize(), 1e-6));
    }
}

fn camera_app() -> (App, Entity) {
    let mut app = App::new();
    app.insert_resource(Time::<()>::default())
        .init_resource::<CameraSettings>()
        .add_systems(Update, (systems::follow, systems::zoom).chain());
    let entity = app
        .world_mut()
        .spawn((
            CameraRig::default(),
            Transform::default(),
            Projection::Orthographic(OrthographicProjection::default_3d()),
        ))
        .id();
    (app, entity)
}

fn step(app: &mut App, hz: u32) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f64(1.0 / hz as f64));
    app.update();
}

#[test]
fn focus_and_zoom_are_frame_rate_independent_and_zoom_is_bounded() {
    let mut results = Vec::new();
    for hz in [30, 120] {
        let (mut app, entity) = camera_app();
        {
            let mut settings = app.world_mut().resource_mut::<CameraSettings>();
            settings.target = Vec3::new(4.0, 0.0, -3.0);
            settings.zoom = 5.0;
        }
        step(&mut app, hz);
        let Projection::Orthographic(projection) = app.world().get::<Projection>(entity).unwrap()
        else {
            panic!()
        };
        assert!(projection.scale > 1.0 && projection.scale < 1.4);
        for _ in 1..hz {
            step(&mut app, hz);
        }
        let focus = app.world().get::<CameraRig>(entity).unwrap().focus;
        let Projection::Orthographic(projection) = app.world().get::<Projection>(entity).unwrap()
        else {
            panic!()
        };
        assert!((projection.scale - 1.4).abs() < 1e-5);
        results.push((focus, projection.scale));
    }
    assert!(results[0].0.abs_diff_eq(results[1].0, 1e-5));
    assert!((results[0].1 - results[1].1).abs() < 1e-5);
}

#[test]
fn disabled_smoothing_follows_immediately() {
    let (mut app, entity) = camera_app();
    {
        let mut settings = app.world_mut().resource_mut::<CameraSettings>();
        settings.target = Vec3::X;
        settings.smoothing = 0.0;
        settings.zoom_smoothing = 0.0;
        settings.zoom = 0.1;
    }
    step(&mut app, 60);
    assert_eq!(app.world().get::<CameraRig>(entity).unwrap().focus, Vec3::X);
    let Projection::Orthographic(projection) = app.world().get::<Projection>(entity).unwrap()
    else {
        panic!()
    };
    assert_eq!(projection.scale, 0.75);
    app.world_mut().clear_trackers();
    step(&mut app, 60);
    let mut changed = app
        .world_mut()
        .query_filtered::<Entity, Or<(Changed<Transform>, Changed<Projection>)>>();
    assert_eq!(changed.iter(app.world()).count(), 0);
}

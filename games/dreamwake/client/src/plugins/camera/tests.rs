use super::*;
use crate::DreamView;
use engine_client::camera::CameraSettings;
#[test]
fn stationary_aim_does_not_move_the_camera() {
    let mut snapshot =
        crate::offline_presentation(dreamwake_sim::DreamSimulation::new(7, false).snapshot());
    snapshot.hero.position = [96.0, -64.0];
    let mut app = App::new();
    app.insert_resource(DreamView(snapshot))
        .init_resource::<ButtonInput<KeyCode>>()
        .add_plugins(DreamCameraPlugin);
    app.update();
    let target = app.world().resource::<CameraSettings>().target;
    assert_eq!(target, Vec3::new(96.0, 0.0, -64.0));
    app.world_mut().resource_mut::<DreamView>().0.hero.facing = [-1.0, 0.0];
    app.update();
    assert_eq!(app.world().resource::<CameraSettings>().target, target);
}

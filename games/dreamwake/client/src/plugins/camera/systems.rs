use crate::DreamView;
use bevy::prelude::*;
use engine_client::camera::CameraSettings;
pub(super) fn target(view: Res<DreamView>, mut camera: ResMut<CameraSettings>) {
    let p = view.0.hero.position;
    let target = (Vec3::new(p[0], 0.0, p[1]) * 0.30)
        .clamp(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 0.0, 5.0));
    if camera.target != target {
        camera.target = target;
    }
}
pub(super) fn zoom(keys: Res<ButtonInput<KeyCode>>, mut camera: ResMut<CameraSettings>) {
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) {
        camera.zoom = (camera.zoom - 0.1).max(camera.zoom_bounds[0]);
    }
    if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        camera.zoom = (camera.zoom + 0.1).min(camera.zoom_bounds[1]);
    }
}

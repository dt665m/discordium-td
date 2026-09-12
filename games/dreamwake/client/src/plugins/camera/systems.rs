use crate::DreamView;
use bevy::prelude::*;
use engine_client::camera::CameraSettings;
pub(super) fn target(
    view: Res<DreamView>,
    mut camera: ResMut<CameraSettings>,
    time: Option<Res<Time>>,
    mut kick: Local<f32>,
    runtime: Option<NonSendMut<crate::plugins::network::Runtime>>,
) {
    *kick *= (-time.as_ref().map_or(0.0, |time| time.delta_secs()) * 24.0).exp();
    if let Some(mut runtime) = runtime {
        if let Some(session) = runtime.prediction.as_mut() {
            let cues = std::mem::take(&mut session.events.camera_cues);
            *kick = (*kick + f32::from(cues) * 0.06).min(0.12);
        }
    } else {
        *kick = 0.0;
    }
    let p = view.0.hero.position;
    let target = Vec3::new(p[0], view.0.hero.elevation + *kick, p[1]);
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

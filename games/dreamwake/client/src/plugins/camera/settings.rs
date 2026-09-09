use bevy::prelude::*;
use engine_client::camera::CameraSettings;
/// Dreamwake framing defaults, independent of the selected art renderer.
pub fn defaults() -> CameraSettings {
    CameraSettings {
        offset: Vec3::new(18.0, 32.0, 26.0),
        viewport_height: 40.0,
        smoothing: 5.0,
        ..default()
    }
}

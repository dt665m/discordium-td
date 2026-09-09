use bevy::prelude::*;

#[derive(Resource)]
pub struct CameraSettings {
    pub zoom: f32,
    pub target: Vec3,
    pub offset: Vec3,
    pub up: Vec3,
    pub viewport_height: f32,
    /// Exponential response rate in inverse seconds; zero follows immediately.
    pub smoothing: f32,
    /// Exponential zoom response in inverse seconds; zero changes immediately.
    pub zoom_smoothing: f32,
    pub zoom_bounds: [f32; 2],
}
impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            target: Vec3::ZERO,
            offset: Vec3::new(0.0, 30.0, 23.0),
            up: Vec3::Y,
            viewport_height: 30.0,
            smoothing: 4.5,
            zoom_smoothing: 12.0,
            zoom_bounds: [0.75, 1.4],
        }
    }
}

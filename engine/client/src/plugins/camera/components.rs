use bevy::prelude::*;

#[derive(Component, Default)]
pub struct CameraRig {
    pub(super) focus: Vec3,
}

pub mod settings;
mod systems;
#[cfg(test)]
mod tests;
use crate::plugins::input::DreamInputSystems;
use bevy::prelude::*;
use engine_client::camera::CameraSystems;
pub struct DreamCameraPlugin;
impl Plugin for DreamCameraPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(settings::defaults())
            .add_systems(Update, systems::target.in_set(CameraSystems::Target))
            .add_systems(PreUpdate, systems::zoom.in_set(DreamInputSystems::Keyboard));
    }
}

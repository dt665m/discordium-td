//! Reusable camera rig and input projection, independent of game art.
mod components;
mod input_projection;
mod settings;
mod systems;
#[cfg(test)]
mod tests;

use crate::graphics::GraphicsSet;
use bevy::prelude::*;
pub use components::CameraRig;
pub use input_projection::{cursor_on_ground, screen_axes_to_world};
pub use settings::CameraSettings;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraSystems {
    /// Games update their framing and follow target here.
    Target,
    Follow,
    Zoom,
}

pub struct CameraPlugin;
impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraSettings>()
            .configure_sets(
                Update,
                (
                    CameraSystems::Target,
                    CameraSystems::Follow,
                    CameraSystems::Zoom,
                )
                    .chain()
                    .after(GraphicsSet::Adapt)
                    .before(GraphicsSet::Render),
            )
            .add_systems(Startup, systems::setup)
            .add_systems(
                Update,
                (
                    systems::follow.in_set(CameraSystems::Follow),
                    systems::zoom.in_set(CameraSystems::Zoom),
                ),
            );
    }
}

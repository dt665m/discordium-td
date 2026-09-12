//! Serializable graphics effect state and lifecycle in the shared simulation.
//! Client plugins supply the rendering for these effects.
mod components;
mod identity;
pub mod systems;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
pub use components::{GraphicsInstance, GraphicsKind};
pub use identity::{GraphicsId, GraphicsScope};
pub use systems::age_graphics;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GraphicsStep;
pub struct GraphicsPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for GraphicsPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), age_graphics.in_set(GraphicsStep));
    }
}

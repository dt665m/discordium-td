//! Canonical movement, displacement, bounds and shared geometry.
mod bounds;
pub mod kinematic;
mod movement;
pub mod spatial;
pub mod systems;
#[cfg(test)]
mod tests;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
pub use bounds::CircleBounds;
pub use movement::MotorState;
pub use systems::tick_motors;

pub struct PhysicsPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for PhysicsPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_plugins(MotorPlugin(self.0.clone()));
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MotorStep;
pub struct MotorPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for MotorPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), tick_motors.in_set(MotorStep));
    }
}

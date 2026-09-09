mod components;
pub mod systems;
pub use components::*;
pub use systems::*;
#[cfg(test)]
mod tests;

use crate::TargetIndex;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CompanionStep;
pub struct CompanionPlugin<S: ScheduleLabel + Clone> {
    schedule: S,
}
impl<S: ScheduleLabel + Clone> CompanionPlugin<S> {
    pub fn new(schedule: S) -> Self {
        Self { schedule }
    }
}
impl<S: ScheduleLabel + Clone> Plugin for CompanionPlugin<S> {
    fn build(&self, app: &mut App) {
        app.init_resource::<TargetIndex>();
        app.add_systems(self.schedule.clone(), tick_companions.in_set(CompanionStep));
    }
}

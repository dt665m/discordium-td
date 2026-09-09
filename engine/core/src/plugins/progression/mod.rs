//! Reusable actor progression; games supply XP awards, curves and rewards.
mod components;
mod experience;
mod systems;
#[cfg(test)]
mod tests;
mod upgrades;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
pub use components::{ExperienceResult, PendingExperience, Progression, ProgressionError};
use systems::grant_experience;
pub use upgrades::{add_capped, multiply_capped, multiply_floored, purchase_rank, spend_currency};

#[derive(Resource)]
struct ExperienceCurve(fn(u32) -> f32);
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProgressionStep;
/// The game supplies its pure level-to-next-threshold rule; no default curve exists.
pub struct ProgressionPlugin<S: ScheduleLabel + Clone> {
    schedule: S,
    curve: fn(u32) -> f32,
}
impl<S: ScheduleLabel + Clone> ProgressionPlugin<S> {
    pub fn new(schedule: S, curve: fn(u32) -> f32) -> Self {
        Self { schedule, curve }
    }
}
impl<S: ScheduleLabel + Clone> Plugin for ProgressionPlugin<S> {
    fn build(&self, app: &mut App) {
        app.insert_resource(ExperienceCurve(self.curve))
            .add_systems(
                self.schedule.clone(),
                grant_experience.in_set(ProgressionStep),
            );
    }
}

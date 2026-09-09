mod components;
pub mod systems;
pub use components::*;
pub use systems::*;
#[cfg(test)]
mod tests;

use crate::TargetIndex;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectileStep;

pub struct ProjectilePlugin<S: ScheduleLabel + Clone> {
    schedule: S,
    collect_collision_candidates: bool,
}
impl<S: ScheduleLabel + Clone> ProjectilePlugin<S> {
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            collect_collision_candidates: true,
        }
    }

    /// Disable the cache when game effects must resolve collisions sequentially
    /// against actor positions changed by earlier impacts in the same tick.
    pub fn with_collision_candidates(mut self, enabled: bool) -> Self {
        self.collect_collision_candidates = enabled;
        self
    }
}
impl<S: ScheduleLabel + Clone> Plugin for ProjectilePlugin<S> {
    fn build(&self, app: &mut App) {
        app.init_resource::<TargetIndex>();
        if self.collect_collision_candidates {
            app.add_systems(
                self.schedule.clone(),
                tick_projectiles.in_set(ProjectileStep),
            );
        } else {
            app.add_systems(
                self.schedule.clone(),
                advance_projectiles.in_set(ProjectileStep),
            );
        }
    }
}

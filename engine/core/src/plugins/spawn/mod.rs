//! Optional simulated lifetimes and ownership using Bevy's entity lifecycle.
mod lifecycle;
mod ownership;
pub mod systems;
#[cfg(test)]
mod tests;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
pub use lifecycle::Lifetime;
pub use ownership::{DespawnWithOwner, OwnedSpawns};
pub use systems::expire_lifetimes;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpawnStep;
pub struct SpawnPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for SpawnPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), expire_lifetimes.in_set(SpawnStep));
    }
}

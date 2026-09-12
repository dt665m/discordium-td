//! Generic action execution and game-typed ability loadouts.
mod actions;
mod charge;
pub use charge::{ChargeCommand, ChargePhase, ChargeState, MotionCurveKey, authored_motion_delta};
mod delayed;
mod loadouts;
pub mod systems;
#[cfg(test)]
mod tests;
pub use actions::ActionState;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
pub use delayed::DelayedAction;
pub use loadouts::{AbilitySlot, Loadout};
pub use systems::{tick_actions, tick_delayed_actions, tick_loadouts};

pub struct AbilitiesPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for AbilitiesPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_plugins(ActionPlugin(self.0.clone()));
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActionStep;
pub struct ActionPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for ActionPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), tick_actions.in_set(ActionStep));
    }
}

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DelayedActionStep;
pub struct DelayedActionPlugin<T: Send + Sync + 'static, S: ScheduleLabel + Clone> {
    schedule: S,
    marker: std::marker::PhantomData<T>,
}
impl<T: Send + Sync + 'static, S: ScheduleLabel + Clone> DelayedActionPlugin<T, S> {
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            marker: std::marker::PhantomData,
        }
    }
}
impl<T: Send + Sync + 'static, S: ScheduleLabel + Clone> Plugin for DelayedActionPlugin<T, S> {
    fn build(&self, app: &mut App) {
        app.add_systems(
            self.schedule.clone(),
            tick_delayed_actions::<T>.in_set(DelayedActionStep),
        );
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoadoutStep;
pub struct LoadoutPlugin<
    K: Send + Sync + 'static,
    M: Send + Sync + 'static,
    S: ScheduleLabel + Clone,
> {
    schedule: S,
    marker: std::marker::PhantomData<(K, M)>,
}
impl<K: Send + Sync + 'static, M: Send + Sync + 'static, S: ScheduleLabel + Clone>
    LoadoutPlugin<K, M, S>
{
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            marker: std::marker::PhantomData,
        }
    }
}
impl<K: Send + Sync + 'static, M: Send + Sync + 'static, S: ScheduleLabel + Clone> Plugin
    for LoadoutPlugin<K, M, S>
{
    fn build(&self, app: &mut App) {
        app.add_systems(
            self.schedule.clone(),
            tick_loadouts::<K, M>.in_set(LoadoutStep),
        );
    }
}

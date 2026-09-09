//! Reusable combat state and lifecycle; games choose damage and ability policy.
pub mod companions;
mod damage;
mod effects;
mod health;
mod meters;
pub mod projectiles;
mod shields;
mod status;
pub mod systems;
pub mod targeting;
#[cfg(test)]
mod tests;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
pub use companions::{CompanionPlugin, CompanionState, CompanionStep, tick_companions};
pub use damage::{DamageOutcome, resolve_damage};
pub use effects::CombatState;
pub use health::{Depleted, Health};
pub use meters::{Meter, Regeneration};
pub use projectiles::{
    ProjectileImpact, ProjectilePlugin, ProjectileSourcePolicy, ProjectileState, ProjectileStep,
    advance_projectiles, tick_projectiles,
};
pub use shields::ShieldPolicy;
pub use status::{DurationPolicy, StatusId, TimedStatus};
use std::marker::PhantomData;
pub use systems::{regenerate_meters, tick_combat, update_health};
pub use targeting::*;

pub struct CombatPlugin<S: ScheduleLabel + Clone> {
    schedule: S,
    collect_collision_candidates: bool,
}
impl<S: ScheduleLabel + Clone> CombatPlugin<S> {
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            collect_collision_candidates: true,
        }
    }
    pub fn with_collision_candidates(mut self, enabled: bool) -> Self {
        self.collect_collision_candidates = enabled;
        self
    }
}
impl<S: ScheduleLabel + Clone> Plugin for CombatPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            CombatEffectsPlugin(self.schedule.clone()),
            HealthPlugin(self.schedule.clone()),
            ProjectilePlugin::new(self.schedule.clone())
                .with_collision_candidates(self.collect_collision_candidates),
            CompanionPlugin::new(self.schedule.clone()),
        ));
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HealthStep;
pub struct HealthPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for HealthPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), update_health.in_set(HealthStep));
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CombatStep;
pub struct CombatEffectsPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for CombatEffectsPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), tick_combat.in_set(CombatStep));
    }
}

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MeterStep;
pub struct MeterPlugin<K: Send + Sync + 'static, S: ScheduleLabel + Clone> {
    schedule: S,
    marker: PhantomData<K>,
}
impl<K: Send + Sync + 'static, S: ScheduleLabel + Clone> MeterPlugin<K, S> {
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            marker: PhantomData,
        }
    }
}
impl<K: Send + Sync + 'static, S: ScheduleLabel + Clone> Plugin for MeterPlugin<K, S> {
    fn build(&self, app: &mut App) {
        app.add_systems(
            self.schedule.clone(),
            regenerate_meters::<K>.in_set(MeterStep),
        );
    }
}

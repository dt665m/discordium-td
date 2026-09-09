use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
/// Explicit fixed duration makes headless simulation independent of wall time.
#[derive(Resource, Clone, Copy)]
pub struct SimulationStep(pub f32);
pub fn advance_cooldown(remaining: &mut f32, dt: f32) {
    *remaining = (*remaining - dt).max(0.0);
}

/// An adapter exposes game-owned cooldowns to the shared ticking system.
pub trait CooldownOwner: Component<Mutability = bevy::ecs::component::Mutable> {
    fn tick_cooldowns(&mut self, dt: f32);
}
pub fn tick_cooldowns<T: CooldownOwner>(step: Res<SimulationStep>, mut actors: Query<&mut T>) {
    for mut actor in &mut actors {
        actor.tick_cooldowns(step.0);
    }
}
pub struct CooldownPlugin<T: CooldownOwner, S: ScheduleLabel + Clone> {
    schedule: S,
    actor: std::marker::PhantomData<T>,
}
impl<T: CooldownOwner, S: ScheduleLabel + Clone> CooldownPlugin<T, S> {
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            actor: std::marker::PhantomData,
        }
    }
}
impl<T: CooldownOwner, S: ScheduleLabel + Clone> Plugin for CooldownPlugin<T, S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.schedule.clone(), tick_cooldowns::<T>);
    }
}

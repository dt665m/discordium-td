use crate::SimulationStep;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
/// Canonical X/Z coordinates, independent of camera and graphics backend.
pub mod spatial {
    pub fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
        [a[0] + b[0], a[1] + b[1]]
    }
    pub fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
        [a[0] - b[0], a[1] - b[1]]
    }
    pub fn scale(a: [f32; 2], s: f32) -> [f32; 2] {
        [a[0] * s, a[1] * s]
    }
    pub fn length(a: [f32; 2]) -> f32 {
        a[0].hypot(a[1])
    }
    pub fn normalize(a: [f32; 2]) -> [f32; 2] {
        let len = length(a);
        if len > 0.0001 && len.is_finite() {
            scale(a, 1.0 / len)
        } else {
            [0.0; 2]
        }
    }
    pub fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
        length(sub(a, b))
    }
    pub fn integrate(position: [f32; 2], velocity: [f32; 2], dt: f32) -> [f32; 2] {
        add(position, scale(velocity, dt))
    }
}

/// Adapter for games whose authoritative spatial data is part of a richer actor.
pub trait MovementOwner: Component<Mutability = bevy::ecs::component::Mutable> {
    fn integrate_motion(&mut self, dt: f32);
}
pub fn integrate_motion<T: MovementOwner>(step: Res<SimulationStep>, mut actors: Query<&mut T>) {
    for mut actor in &mut actors {
        actor.integrate_motion(step.0);
    }
}

pub struct MovementPlugin<T: MovementOwner, S: ScheduleLabel + Clone> {
    schedule: S,
    actor: std::marker::PhantomData<T>,
}
impl<T: MovementOwner, S: ScheduleLabel + Clone> MovementPlugin<T, S> {
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            actor: std::marker::PhantomData,
        }
    }
}
impl<T: MovementOwner, S: ScheduleLabel + Clone> Plugin for MovementPlugin<T, S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.schedule.clone(), integrate_motion::<T>);
    }
}

use super::bounds::CircleBounds;
use crate::plugins::physics::spatial;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MotorState {
    pub position: [f32; 2],
    pub facing: [f32; 2],
    pub velocity: [f32; 2],
    pub movement_lock: f32,
    pub dash_direction: [f32; 2],
    pub dash_remaining: f32,
    pub dash_cooldown: f32,
    pub dash_speed: f32,
}
impl MotorState {
    /// Apply instantaneous displacement without changing velocity or facing.
    /// Games supply response: zero is immunity, one is full displacement, and
    /// intermediate values resist a portion of it. Values above one amplify it.
    pub fn displace(
        &mut self,
        displacement: [f32; 2],
        response: f32,
        bounds: Option<CircleBounds>,
    ) {
        if !response.is_finite() || response <= 0.0 {
            return;
        }
        self.position = spatial::add(self.position, spatial::scale(displacement, response));
        if let Some(bounds) = bounds {
            self.position = bounds.clamp(self.position);
        }
    }
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        for timer in [
            &mut self.movement_lock,
            &mut self.dash_remaining,
            &mut self.dash_cooldown,
        ] {
            crate::advance_cooldown(timer, dt);
        }
    }
    pub fn start_dash(&mut self, direction: [f32; 2], speed: f32, duration: f32, cooldown: f32) {
        self.dash_direction = spatial::normalize(direction);
        self.dash_speed = speed;
        self.dash_remaining = duration.max(0.0);
        self.dash_cooldown = cooldown.max(0.0);
    }
    /// Integrate once after timers/input acceptance; this does not tick timers.
    pub fn advance(
        &mut self,
        movement: [f32; 2],
        aim: [f32; 2],
        speed: f32,
        bounds: Option<CircleBounds>,
        dt: f32,
    ) {
        let aim = spatial::normalize(aim);
        if spatial::length(aim) > 0.0 {
            self.facing = aim;
        }
        self.velocity = if self.dash_remaining > 0.0 {
            spatial::scale(self.dash_direction, self.dash_speed)
        } else if self.movement_lock > 0.0 {
            [0.0; 2]
        } else {
            spatial::scale(spatial::normalize(movement), speed)
        };
        self.position = spatial::integrate(self.position, self.velocity, dt);
        if let Some(bounds) = bounds {
            self.position = bounds.clamp(self.position);
        }
    }
}

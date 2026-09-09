use bevy::prelude::*;
use serde::{Deserialize, Serialize};
/// Serializable payloads carry game-selected identity and execution parameters.
/// Readiness is sticky until game logic executes and removes the entity.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelayedAction<T: Send + Sync + 'static> {
    pub remaining: f32,
    pub payload: T,
    pub ready: bool,
}
impl<T: Send + Sync + 'static> DelayedAction<T> {
    pub fn new(delay: f32, payload: T) -> Self {
        let remaining = if delay.is_finite() {
            delay.max(0.0)
        } else {
            0.0
        };
        Self {
            remaining,
            payload,
            ready: remaining == 0.0,
        }
    }
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        crate::advance_cooldown(&mut self.remaining, dt);
        self.ready |= self.remaining == 0.0;
    }
}

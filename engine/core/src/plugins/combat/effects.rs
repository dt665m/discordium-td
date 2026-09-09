//! Serializable defensive state and its shared countdown behavior.
use super::TimedStatus;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CombatState {
    pub shield: f32,
    pub shield_remaining: f32,
    pub invulnerability_remaining: f32,
    pub statuses: Vec<TimedStatus>,
}

pub(super) fn nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

impl CombatState {
    pub fn tick(&mut self, dt: f32) {
        let dt = nonnegative(dt);
        self.shield_remaining = (self.shield_remaining - dt).max(0.0);
        if self.shield_remaining == 0.0 {
            self.shield = 0.0;
        }
        self.invulnerability_remaining = (self.invulnerability_remaining - dt).max(0.0);
        for status in &mut self.statuses {
            status.remaining = (status.remaining - dt).max(0.0);
        }
        self.statuses.retain(|status| status.remaining > 0.0);
    }
}

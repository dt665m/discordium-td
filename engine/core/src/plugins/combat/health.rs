use bevy::prelude::*;
use serde::{Deserialize, Serialize};
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub hp: f32,
    pub max_hp: f32,
}
impl Health {
    /// Creates finite health capacity. Zero capacity is valid and starts depleted.
    ///
    /// # Panics
    /// Panics if capacity is negative or non-finite. Callers that change the public
    /// fields directly must preserve finite values and nonnegative capacity.
    pub fn new(max_hp: f32) -> Self {
        assert!(
            max_hp.is_finite() && max_hp >= 0.0,
            "health capacity must be finite and nonnegative"
        );
        Self { hp: max_hp, max_hp }
    }
    /// Applies nonnegative damage, capped at remaining HP. NaN and negative
    /// amounts do nothing; positive infinity depletes the actor.
    pub fn damage(&mut self, amount: f32) -> f32 {
        let dealt = amount.max(0.0).min(self.hp.max(0.0));
        self.hp -= dealt;
        dealt
    }
    pub fn heal(&mut self, amount: f32) {
        if self.hp > 0.0 {
            self.hp = (self.hp + amount.max(0.0)).min(self.max_hp);
        }
    }
}

/// Games decide how death affects their encounters; this reusable marker records it.
#[derive(Component, Debug)]
pub struct Depleted;

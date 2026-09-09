use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A committed action preserves its target until game logic consumes it.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionState {
    pub target: [f32; 2],
    pub windup: f32,
    pub recovery: f32,
    pub sequence: u32,
    pub due: bool,
}
impl ActionState {
    pub fn idle(&self) -> bool {
        self.windup <= 0.0 && self.recovery <= 0.0 && !self.due
    }
    pub fn commit(&mut self, target: [f32; 2], windup: f32) -> bool {
        if !self.idle() || !windup.is_finite() || windup < 0.0 {
            return false;
        }
        self.target = target;
        self.windup = windup;
        self.sequence = self.sequence.wrapping_add(1);
        self.due = windup == 0.0;
        true
    }
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        crate::advance_cooldown(&mut self.recovery, dt);
        if self.windup > 0.0 {
            crate::advance_cooldown(&mut self.windup, dt);
            self.due |= self.windup == 0.0;
        }
    }
    pub fn take_due(&mut self) -> bool {
        std::mem::take(&mut self.due)
    }
    pub fn begin_recovery(&mut self, seconds: f32) {
        self.windup = 0.0;
        self.due = false;
        self.recovery = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
    }
    pub fn cancel(&mut self) {
        self.windup = 0.0;
        self.recovery = 0.0;
        self.due = false;
    }
}

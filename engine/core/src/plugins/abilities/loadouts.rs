use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AbilitySlot<K, M> {
    pub kind: K,
    pub level: u8,
    pub modifier: Option<M>,
    pub cooldown: f32,
    pub max_cooldown: f32,
}
impl<K, M> AbilitySlot<K, M> {
    pub fn new(kind: K, max_cooldown: f32) -> Self {
        Self {
            kind,
            level: 1,
            modifier: None,
            cooldown: 0.0,
            max_cooldown,
        }
    }
    pub fn ready(&self) -> bool {
        self.cooldown <= 0.0
    }
    pub fn activate(&mut self) -> bool {
        if !self.ready() {
            return false;
        }
        self.cooldown = self.max_cooldown;
        true
    }
    pub fn refresh(&mut self) {
        self.cooldown = 0.0;
    }
    pub fn reduce_cooldown(&mut self, amount: f32) {
        self.cooldown = (self.cooldown - amount.max(0.0)).max(0.0);
    }
    pub fn scale_remaining(&mut self, factor: f32) {
        self.cooldown *= factor.max(0.0);
    }
    pub fn upgrade(&mut self, cap: u8) -> bool {
        if self.level >= cap {
            return false;
        }
        self.level += 1;
        true
    }
    /// Replacement policy explicitly chooses whether the socket survives.
    pub fn replace(&mut self, kind: K, level: u8, preserve_modifier: bool) {
        self.kind = kind;
        self.level = level;
        if !preserve_modifier {
            self.modifier = None;
        }
        self.refresh();
    }
}

/// Slot count and ability/modifier identifiers are supplied by each game.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Loadout<K: Send + Sync + 'static, M: Send + Sync + 'static>(pub Vec<AbilitySlot<K, M>>);
impl<K: Send + Sync + 'static, M: Send + Sync + 'static> Loadout<K, M> {
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        for slot in &mut self.0 {
            crate::advance_cooldown(&mut slot.cooldown, dt);
        }
    }
    pub fn swap(&mut self, a: usize, b: usize) -> bool {
        if a >= self.0.len() || b >= self.0.len() {
            return false;
        }
        self.0.swap(a, b);
        true
    }
}

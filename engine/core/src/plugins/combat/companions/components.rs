//! Owner-following companions with deterministic orbit and firing intent.
use crate::{
    CollisionTarget, nearest_target,
    spatial::{add, normalize, sub},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompanionState {
    pub id: u64,
    pub owner: u64,
    pub faction: u8,
    pub position: [f32; 2],
    pub remaining: f32,
    pub orbit_angle: f32,
    pub orbit_radius: f32,
    pub orbit_speed: f32,
    pub fire_remaining: f32,
    pub fire_interval: f32,
    pub range: f32,
    pub expired: bool,
    /// A game consumes this direction to create its configured projectile/effect.
    #[serde(skip)]
    pub pending_shot: Option<[f32; 2]>,
}

impl CompanionState {
    /// Missing owners expire the summon; inactive owners pause motion and firing.
    /// Lifetime always advances, including while an owner is inactive.
    pub fn tick(&mut self, dt: f32, targets: &[CollisionTarget]) {
        self.pending_shot = None;
        if self.expired {
            return;
        }
        self.remaining -= dt;
        let Some(owner) = targets.iter().find(|t| t.id == self.owner) else {
            self.expired = true;
            return;
        };
        if self.remaining <= 0.0 {
            self.expired = true;
            return;
        }
        if !owner.active {
            return;
        }
        self.orbit_angle += dt * self.orbit_speed;
        self.position = add(
            owner.position,
            [
                self.orbit_angle.cos() * self.orbit_radius,
                self.orbit_angle.sin() * self.orbit_radius,
            ],
        );
        self.fire_remaining -= dt;
        if self.fire_remaining > 0.0 {
            return;
        }
        if let Some(target) = nearest_target(
            self.position,
            self.range,
            self.faction,
            targets.iter().copied(),
        ) {
            self.pending_shot = Some(normalize(sub(target.position, self.position)));
            self.fire_remaining = self.fire_interval;
        }
    }
}

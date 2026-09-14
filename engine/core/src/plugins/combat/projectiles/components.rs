//! Generic projectile motion and swept hit candidates; games resolve effects.
use crate::{
    CollisionTarget, segment_distance,
    spatial::{distance, integrate, length, scale},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectileSourcePolicy {
    #[default]
    Independent,
    RequirePresent,
    RequireActive,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectileImpact {
    pub target_id: u64,
    pub distance: f32,
}

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectileState {
    pub id: u64,
    pub owner: u64,
    pub faction: u8,
    pub position: [f32; 2],
    pub previous_position: [f32; 2],
    pub direction: [f32; 2],
    pub speed: f32,
    pub remaining: f32,
    pub radius: f32,
    pub hits_remaining: u32,
    pub hit_ids: Vec<u64>,
    /// Optional world-origin distance limit.
    pub max_distance: Option<f32>,
    pub source_policy: ProjectileSourcePolicy,
    pub expired: bool,
    /// Rebuilt each step when automatic candidate collection is enabled.
    /// Effects must recheck target liveness before recording hits.
    #[serde(skip)]
    pub pending_impacts: Vec<ProjectileImpact>,
}

/// Advance the deterministic, publicly projectable part of a projectile.
/// Collision contacts and source-presence policy remain authority concerns.
pub fn advance_projectile_motion(
    position: &mut [f32; 2],
    direction: [f32; 2],
    speed: f32,
    remaining: &mut f32,
    max_distance: Option<f32>,
    dt: f32,
) -> bool {
    if !dt.is_finite() || dt < 0.0 {
        return false;
    }
    *position = integrate(*position, scale(direction, speed), dt);
    *remaining -= dt;
    *remaining > 0.0 && max_distance.is_none_or(|limit| length(*position) <= limit)
}

impl ProjectileState {
    pub fn record_hit(&mut self, target_id: u64) -> bool {
        if self.expired || self.hits_remaining == 0 || self.hit_ids.contains(&target_id) {
            return false;
        }
        self.hit_ids.push(target_id);
        self.hits_remaining -= 1;
        true
    }

    pub fn finished(&self) -> bool {
        self.expired || self.hits_remaining == 0
    }

    pub fn tick(&mut self, dt: f32, targets: &[CollisionTarget]) {
        self.advance(dt, targets);
        self.pending_impacts = self.impacts(targets.iter().copied());
    }

    /// Integrate motion and expiry only, for games resolving against live targets.
    pub fn advance(&mut self, dt: f32, targets: &[CollisionTarget]) {
        self.pending_impacts.clear();
        if self.finished() {
            return;
        }
        self.previous_position = self.position;
        let motion_active = advance_projectile_motion(
            &mut self.position,
            self.direction,
            self.speed,
            &mut self.remaining,
            self.max_distance,
            dt,
        );
        let source_available = match self.source_policy {
            ProjectileSourcePolicy::Independent => true,
            ProjectileSourcePolicy::RequirePresent => targets.iter().any(|t| t.id == self.owner),
            ProjectileSourcePolicy::RequireActive => {
                targets.iter().any(|t| t.id == self.owner && t.active)
            }
        };
        if !motion_active || !source_available {
            self.expired = true;
            return;
        }
    }

    /// Resolve each projectile in stable id order against current actor positions.
    /// Earlier projectile effects can invalidate an earlier phase's index.
    pub fn impacts(
        &self,
        targets: impl IntoIterator<Item = CollisionTarget>,
    ) -> Vec<ProjectileImpact> {
        if self.finished() {
            return Vec::new();
        }
        let mut impacts: Vec<_> = targets
            .into_iter()
            .filter(|t| {
                t.active
                    && t.faction != self.faction
                    && t.id != self.owner
                    && !self.hit_ids.contains(&t.id)
                    && segment_distance(t.position, self.previous_position, self.position)
                        < self.radius + t.radius
            })
            .map(|t| ProjectileImpact {
                target_id: t.id,
                distance: distance(t.position, self.previous_position),
            })
            .collect();
        impacts.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.target_id.cmp(&b.target_id))
        });
        impacts
    }
}

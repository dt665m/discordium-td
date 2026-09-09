//! Generic projectile motion and swept hit candidates; games resolve effects.
use crate::{
    CollisionTarget, SimulationStep, TargetIndex, segment_distance,
    spatial::{distance, integrate, length, scale},
};
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
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
        self.position = integrate(self.position, scale(self.direction, self.speed), dt);
        self.remaining -= dt;
        let source_available = match self.source_policy {
            ProjectileSourcePolicy::Independent => true,
            ProjectileSourcePolicy::RequirePresent => targets.iter().any(|t| t.id == self.owner),
            ProjectileSourcePolicy::RequireActive => {
                targets.iter().any(|t| t.id == self.owner && t.active)
            }
        };
        if self.remaining <= 0.0
            || self
                .max_distance
                .is_some_and(|limit| length(self.position) > limit)
            || !source_available
        {
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

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectileStep;

pub fn tick_projectiles(
    step: Res<SimulationStep>,
    targets: Res<TargetIndex>,
    mut projectiles: Query<&mut ProjectileState>,
) {
    for mut projectile in &mut projectiles {
        projectile.tick(step.0, &targets.0);
    }
}

pub fn advance_projectiles(
    step: Res<SimulationStep>,
    targets: Res<TargetIndex>,
    mut projectiles: Query<&mut ProjectileState>,
) {
    for mut projectile in &mut projectiles {
        projectile.advance(step.0, &targets.0);
    }
}

pub struct ProjectilePlugin<S: ScheduleLabel + Clone> {
    schedule: S,
    collect_collision_candidates: bool,
}
impl<S: ScheduleLabel + Clone> ProjectilePlugin<S> {
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            collect_collision_candidates: true,
        }
    }

    /// Disable the cache when game effects must resolve collisions sequentially
    /// against actor positions changed by earlier impacts in the same tick.
    pub fn with_collision_candidates(mut self, enabled: bool) -> Self {
        self.collect_collision_candidates = enabled;
        self
    }
}
impl<S: ScheduleLabel + Clone> Plugin for ProjectilePlugin<S> {
    fn build(&self, app: &mut App) {
        app.init_resource::<TargetIndex>();
        if self.collect_collision_candidates {
            app.add_systems(
                self.schedule.clone(),
                tick_projectiles.in_set(ProjectileStep),
            );
        } else {
            app.add_systems(
                self.schedule.clone(),
                advance_projectiles.in_set(ProjectileStep),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bolt() -> ProjectileState {
        ProjectileState {
            id: 10,
            owner: 1,
            faction: 1,
            position: [0.0; 2],
            previous_position: [0.0; 2],
            direction: [1.0, 0.0],
            speed: 10.0,
            remaining: 3.0,
            radius: 0.1,
            hits_remaining: 2,
            hit_ids: vec![],
            max_distance: None,
            source_policy: ProjectileSourcePolicy::Independent,
            expired: false,
            pending_impacts: vec![],
        }
    }
    #[test]
    fn swept_hits_are_ordered_filtered_and_consumed_once() {
        let mut bolt = bolt();
        let targets = [
            CollisionTarget {
                id: 3,
                faction: 2,
                position: [5.0, 0.0],
                radius: 0.2,
                active: true,
            },
            CollisionTarget {
                id: 2,
                faction: 2,
                position: [5.0, 0.0],
                radius: 0.2,
                active: true,
            },
            CollisionTarget {
                id: 4,
                faction: 1,
                position: [2.0, 0.0],
                radius: 0.2,
                active: true,
            },
        ];
        bolt.tick(1.0, &targets);
        assert_eq!(bolt.position, [10.0, 0.0]);
        assert_eq!(
            bolt.pending_impacts
                .iter()
                .map(|v| v.target_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(bolt.record_hit(2));
        assert!(!bolt.record_hit(2));
        assert!(bolt.record_hit(3));
        assert!(bolt.finished());
    }
    #[test]
    fn expiration_precedes_collision_and_present_dead_owner_is_allowed() {
        let mut bolt = bolt();
        bolt.source_policy = ProjectileSourcePolicy::RequirePresent;
        let owner = CollisionTarget {
            id: 1,
            faction: 1,
            position: [0.0; 2],
            radius: 0.5,
            active: false,
        };
        bolt.tick(0.1, &[owner]);
        assert!(!bolt.expired);
        bolt.tick(3.0, &[owner]);
        assert!(bolt.expired && bolt.pending_impacts.is_empty());
        let mut missing = super::tests::bolt();
        missing.source_policy = ProjectileSourcePolicy::RequirePresent;
        missing.tick(0.1, &[]);
        assert!(missing.expired);
    }

    #[test]
    fn plugin_ticks_serializable_state_without_despawning_payload() {
        let mut app = App::new();
        app.insert_resource(SimulationStep(1.0));
        app.add_plugins(ProjectilePlugin::new(Update));
        let entity = app.world_mut().spawn(bolt()).id();
        app.update();
        assert_eq!(
            app.world().get::<ProjectileState>(entity).unwrap().position,
            [10.0, 0.0]
        );
        app.update();
        app.update();
        assert!(app.world().get::<ProjectileState>(entity).unwrap().expired);
    }

    #[test]
    fn plugin_can_defer_collision_collection_until_game_resolution() {
        let mut app = App::new();
        app.insert_resource(SimulationStep(1.0));
        app.add_plugins(ProjectilePlugin::new(Update).with_collision_candidates(false));
        let target = CollisionTarget {
            id: 2,
            faction: 2,
            position: [5.0, 0.0],
            radius: 0.5,
            active: true,
        };
        app.world_mut().resource_mut::<TargetIndex>().0.push(target);
        let entity = app.world_mut().spawn(bolt()).id();
        app.update();
        let state = app.world().get::<ProjectileState>(entity).unwrap();
        assert_eq!(state.position, [10.0, 0.0]);
        assert_eq!(state.remaining, 2.0);
        assert!(state.pending_impacts.is_empty());
        assert_eq!(state.impacts([target])[0].target_id, 2);
    }

    #[test]
    fn current_targets_can_invalidate_cached_impacts() {
        let mut bolt = bolt();
        let mut target = CollisionTarget {
            id: 2,
            faction: 2,
            position: [5.0, 0.0],
            radius: 0.5,
            active: true,
        };
        bolt.tick(1.0, &[target]);
        assert_eq!(bolt.pending_impacts.len(), 1);
        target.position = [5.0, 2.0];
        assert!(bolt.impacts([target]).is_empty());
        target.position = [5.0, 0.0];
        target.active = false;
        assert!(bolt.impacts([target]).is_empty());
    }

    #[test]
    fn restored_projectile_preserves_hit_history_and_continuation() {
        let mut original = bolt();
        original.tick(0.1, &[]);
        original.record_hit(2);
        let bytes = serde_json::to_vec(&original).unwrap();
        let mut restored: ProjectileState = serde_json::from_slice(&bytes).unwrap();
        let targets = [CollisionTarget {
            id: 2,
            faction: 2,
            position: [2.0, 0.0],
            radius: 0.5,
            active: true,
        }];
        original.tick(0.1, &targets);
        restored.tick(0.1, &targets);
        assert_eq!(original, restored);
        assert!(restored.pending_impacts.is_empty());
    }
}

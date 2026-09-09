//! Owner-following companions with deterministic orbit and firing intent.
use crate::{
    CollisionTarget, SimulationStep, TargetIndex, nearest_target,
    spatial::{add, normalize, sub},
};
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use serde::{Deserialize, Serialize};

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummonState {
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

impl SummonState {
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

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SummonStep;
pub fn tick_summons(
    step: Res<SimulationStep>,
    targets: Res<TargetIndex>,
    mut summons: Query<&mut SummonState>,
) {
    for mut summon in &mut summons {
        summon.tick(step.0, &targets.0);
    }
}
pub struct SummonPlugin<S: ScheduleLabel + Clone> {
    schedule: S,
}
impl<S: ScheduleLabel + Clone> SummonPlugin<S> {
    pub fn new(schedule: S) -> Self {
        Self { schedule }
    }
}
impl<S: ScheduleLabel + Clone> Plugin for SummonPlugin<S> {
    fn build(&self, app: &mut App) {
        app.init_resource::<TargetIndex>();
        app.add_systems(self.schedule.clone(), tick_summons.in_set(SummonStep));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn summon() -> SummonState {
        SummonState {
            id: 4,
            owner: 1,
            faction: 1,
            position: [0.0; 2],
            remaining: 5.0,
            orbit_angle: 0.0,
            orbit_radius: 1.0,
            orbit_speed: 0.0,
            fire_remaining: 0.0,
            fire_interval: 0.7,
            range: 12.0,
            expired: false,
            pending_shot: None,
        }
    }
    fn owner(active: bool) -> CollisionTarget {
        CollisionTarget {
            id: 1,
            faction: 1,
            position: [1.0, 0.0],
            radius: 0.5,
            active,
        }
    }
    #[test]
    fn follows_fires_and_pauses_for_inactive_owner() {
        let mut summon = summon();
        let enemy = CollisionTarget {
            id: 2,
            faction: 2,
            position: [5.0, 0.0],
            radius: 0.5,
            active: true,
        };
        summon.tick(0.1, &[owner(true), enemy]);
        assert_eq!(summon.position, [2.0, 0.0]);
        assert_eq!(summon.pending_shot, Some([1.0, 0.0]));
        summon.tick(0.1, &[owner(false), enemy]);
        assert_eq!(summon.pending_shot, None);
        assert_eq!(summon.fire_remaining, 0.7);
        assert!(summon.remaining < 4.9);
    }
    #[test]
    fn missing_owner_and_expiration_never_fire() {
        let mut missing = summon();
        missing.tick(0.1, &[]);
        assert!(missing.expired);
        let mut expired = summon();
        expired.tick(5.0, &[owner(true)]);
        assert!(expired.expired && expired.pending_shot.is_none());
    }

    #[test]
    fn plugin_uses_refreshed_index_and_leaves_cleanup_to_game() {
        let mut app = App::new();
        app.insert_resource(SimulationStep(0.1));
        app.add_plugins(SummonPlugin::new(Update));
        app.world_mut()
            .resource_mut::<TargetIndex>()
            .0
            .push(owner(true));
        let entity = app.world_mut().spawn(summon()).id();
        app.update();
        assert_eq!(
            app.world().get::<SummonState>(entity).unwrap().position,
            [2.0, 0.0]
        );
        app.world_mut().resource_mut::<TargetIndex>().0.clear();
        app.update();
        assert!(app.world().get::<SummonState>(entity).unwrap().expired);
    }

    #[test]
    fn restored_summon_preserves_orbit_and_firing_schedule() {
        let mut original = summon();
        original.orbit_speed = 1.8;
        original.tick(0.1, &[owner(true)]);
        let bytes = serde_json::to_vec(&original).unwrap();
        let mut restored: SummonState = serde_json::from_slice(&bytes).unwrap();
        let targets = [
            owner(true),
            CollisionTarget {
                id: 2,
                faction: 2,
                position: [4.0, 0.0],
                radius: 0.5,
                active: true,
            },
        ];
        for _ in 0..12 {
            original.tick(0.1, &targets);
            restored.tick(0.1, &targets);
            assert_eq!(original, restored);
        }
    }
}

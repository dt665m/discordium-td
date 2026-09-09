use crate::{CollisionTarget, SimulationStep, TargetIndex};
use bevy::prelude::*;

use super::*;
fn summon() -> CompanionState {
    CompanionState {
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
    app.add_plugins(CompanionPlugin::new(Update));
    app.world_mut()
        .resource_mut::<TargetIndex>()
        .0
        .push(owner(true));
    let entity = app.world_mut().spawn(summon()).id();
    app.update();
    assert_eq!(
        app.world().get::<CompanionState>(entity).unwrap().position,
        [2.0, 0.0]
    );
    app.world_mut().resource_mut::<TargetIndex>().0.clear();
    app.update();
    assert!(app.world().get::<CompanionState>(entity).unwrap().expired);
}

#[test]
fn restored_summon_preserves_orbit_and_firing_schedule() {
    let mut original = summon();
    original.orbit_speed = 1.8;
    original.tick(0.1, &[owner(true)]);
    let bytes = serde_json::to_vec(&original).unwrap();
    let mut restored: CompanionState = serde_json::from_slice(&bytes).unwrap();
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

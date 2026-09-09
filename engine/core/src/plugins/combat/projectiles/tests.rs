use crate::{CollisionTarget, SimulationStep, TargetIndex};
use bevy::prelude::*;

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

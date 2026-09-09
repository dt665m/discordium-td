use super::*;
use bevy::prelude::*;
#[test]
fn lifetime_expiry_cleans_owned_entities_in_the_same_step() {
    let mut app = App::new();
    app.add_plugins((crate::SystemPlugin::new(0.5), SpawnPlugin(Update)));
    let owner = app.world_mut().spawn(Lifetime(1.0)).id();
    let child = app
        .world_mut()
        .spawn((DespawnWithOwner(owner), Lifetime(1.0)))
        .id();
    let unrelated = app.world_mut().spawn_empty().id();
    app.update();
    assert!(app.world().get_entity(owner).is_ok());
    app.update();
    assert!(app.world().get_entity(owner).is_err());
    assert!(app.world().get_entity(child).is_err());
    assert!(app.world().get_entity(unrelated).is_ok());
}

#[test]
fn ownership_reassignment_and_nested_cleanup_follow_bevy_lifecycle() {
    let mut app = App::new();
    app.add_plugins((crate::SystemPlugin::new(0.5), SpawnPlugin(Update)));
    let former_owner = app.world_mut().spawn_empty().id();
    let owner = app.world_mut().spawn(Lifetime(0.5)).id();
    let child = app.world_mut().spawn(DespawnWithOwner(former_owner)).id();
    let grandchild = app.world_mut().spawn(DespawnWithOwner(child)).id();
    app.world_mut()
        .entity_mut(child)
        .insert(DespawnWithOwner(owner));
    app.world_mut().despawn(former_owner);
    assert!(app.world().get_entity(child).is_ok());
    app.update();
    for entity in [owner, child, grandchild] {
        assert!(app.world().get_entity(entity).is_err());
    }
}

#[test]
fn restored_lifetimes_expire_on_the_same_step_and_zero_delta_stays_idle() {
    let mut app = App::new();
    app.add_plugins((crate::SystemPlugin::new(0.25), SpawnPlugin(Update)));
    let first = app.world_mut().spawn(Lifetime(0.75)).id();
    app.update();
    let saved = serde_json::to_string(app.world().get::<Lifetime>(first).unwrap()).unwrap();
    let restored: Lifetime = serde_json::from_str(&saved).unwrap();
    let second = app.world_mut().spawn(restored).id();
    app.world_mut().resource_mut::<crate::SimulationStep>().0 = 0.0;
    app.world_mut().clear_trackers();
    app.update();
    assert!(
        !app.world()
            .entity(first)
            .get_ref::<Lifetime>()
            .unwrap()
            .is_changed()
    );
    app.world_mut().resource_mut::<crate::SimulationStep>().0 = 0.25;
    app.update();
    assert_eq!(
        app.world().get::<Lifetime>(first),
        app.world().get::<Lifetime>(second)
    );
    app.update();
    assert!(app.world().get_entity(first).is_err());
    assert!(app.world().get_entity(second).is_err());
}

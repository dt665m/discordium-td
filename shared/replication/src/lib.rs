//! Stable network identities mapped to world-local Bevy entities.
//!
//! Install [`NetIdentityPlugin`] before inserting identities through commands.
//! The exclusive-world helpers also work in bare worlds. Gameplay state stays in
//! components: the index contains only identities and local entity handles.
mod identity;
mod index;

pub use identity::NetId;
pub use index::{
    DuplicateNetId, NetEntityIndex, NetIdentityPlugin, clear, clear_epoch, despawn, entity, spawn,
    upsert,
};

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    #[derive(Component, Debug, PartialEq)]
    struct State(u32);
    #[derive(Resource)]
    struct KeepResource;

    #[test]
    fn same_wire_id_resolves_different_local_entities_after_restore() {
        let id = NetId::new(9, 0, 4);
        let mut server = World::new();
        let source = spawn(&mut server, id, State(42)).unwrap();
        let mut restored = World::new();
        for _ in 0..5 {
            restored.spawn_empty();
        }
        let destination = upsert(&mut restored, id, State(42));
        assert_ne!(source, destination);
        assert_eq!(entity(&server, id), Some(source));
        assert_eq!(entity(&restored, id), Some(destination));
        assert_eq!(restored.get::<State>(destination), Some(&State(42)));
        assert_eq!(upsert(&mut restored, id, State(43)), destination);
    }

    #[test]
    fn namespaces_and_epochs_are_distinct_and_duplicate_spawn_is_atomic() {
        let mut world = World::new();
        let first_id = NetId::new(1, 0, 7);
        let first = spawn(&mut world, first_id, State(1)).unwrap();
        let enemy = spawn(&mut world, NetId::new(1, 1, 7), State(2)).unwrap();
        let next = spawn(&mut world, NetId::new(2, 0, 7), State(3)).unwrap();
        let count = world.entities().len();
        assert_eq!(
            spawn(&mut world, first_id, State(99)),
            Err(DuplicateNetId {
                id: first_id,
                existing: first
            })
        );
        assert_eq!(world.entities().len(), count);
        assert_eq!(world.get::<State>(first), Some(&State(1)));
        assert_ne!(first, enemy);
        assert_ne!(first, next);
    }

    #[test]
    fn raw_replacement_removal_and_despawn_keep_index_current() {
        let mut app = App::new();
        app.add_plugins(NetIdentityPlugin);
        let world = app.world_mut();
        let old = NetId::new(1, 0, 1);
        let new = NetId::new(1, 0, 2);
        let local = world.spawn(old).id();
        assert_eq!(entity(world, old), Some(local));
        world.entity_mut(local).insert(new);
        assert_eq!(entity(world, old), None);
        assert_eq!(entity(world, new), Some(local));
        world.entity_mut(local).remove::<NetId>();
        assert!(world.resource::<NetEntityIndex>().is_empty());
        world.entity_mut(local).insert(old);
        world.despawn(local);
        let replacement = world.spawn_empty().id();
        assert_ne!(local, replacement);
        assert_eq!(entity(world, old), None);
        assert!(world.resource::<NetEntityIndex>().is_empty());
    }

    #[test]
    fn epoch_cleanup_preserves_other_matches_resources_and_unrelated_entities() {
        let mut world = World::new();
        world.insert_resource(KeepResource);
        let unrelated = world.spawn(State(99)).id();
        let old = NetId::new(1, 0, 1);
        let new = NetId::new(2, 0, 1);
        spawn(&mut world, old, State(1)).unwrap();
        let current = spawn(&mut world, new, State(2)).unwrap();
        assert_eq!(clear_epoch(&mut world, 1), 1);
        assert_eq!(entity(&world, old), None);
        assert_eq!(entity(&world, new), Some(current));
        assert_eq!(clear(&mut world), 1);
        assert!(world.contains_resource::<KeepResource>());
        assert!(world.get_entity(unrelated).is_ok());
        assert!(world.resource::<NetEntityIndex>().is_empty());
        assert!(!despawn(&mut world, old));
    }

    #[test]
    #[should_panic(expected = "duplicate live NetId")]
    fn raw_duplicate_insertion_cannot_silently_steal_a_binding() {
        let mut world = World::new();
        let id = NetId::new(1, 0, 1);
        spawn(&mut world, id, ()).unwrap();
        world.spawn(id);
    }
}

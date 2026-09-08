use crate::NetEntityIndex;
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};

/// Wire identity, independent of local entity allocation and generation.
///
/// Namespaces separate actor kinds sharing a numeric ID; epochs separate matches.
/// This component is immutable so every identity change runs the index hooks.
/// Direct insertion requires the plugin and a unique ID; use [`crate::spawn`]
/// when duplicate input is possible. A duplicate raw insertion is a programmer
/// error and panics rather than silently stealing an existing binding.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[component(immutable, on_insert = insert, on_discard = discard)]
pub struct NetId {
    pub epoch: u32,
    pub namespace: u8,
    pub value: u64,
}

impl NetId {
    pub const fn new(epoch: u32, namespace: u8, value: u64) -> Self {
        Self {
            epoch,
            namespace,
            value,
        }
    }
}

fn insert(mut world: DeferredWorld, context: HookContext) {
    let id = *world
        .get::<NetId>(context.entity)
        .expect("inserted identity");
    let mut index = world
        .get_resource_mut::<NetEntityIndex>()
        .expect("install NetIdentityPlugin before inserting NetId");
    if let Some(previous) = index.bindings.get(&id) {
        assert_eq!(*previous, context.entity, "duplicate live NetId {id:?}");
    }
    index.bindings.insert(id, context.entity);
}

fn discard(mut world: DeferredWorld, context: HookContext) {
    let id = *world
        .get::<NetId>(context.entity)
        .expect("replaced identity");
    if let Some(mut index) = world.get_resource_mut::<NetEntityIndex>() {
        if index.bindings.get(&id) == Some(&context.entity) {
            index.bindings.remove(&id);
        }
    }
}

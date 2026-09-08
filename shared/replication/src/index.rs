use crate::NetId;
use bevy::prelude::*;
use std::{collections::HashMap, fmt};

/// Identity-only index maintained by component lifecycle hooks.
///
/// Do not remove or replace this resource while identified entities exist.
#[derive(Resource, Default)]
pub struct NetEntityIndex {
    pub(crate) bindings: HashMap<NetId, Entity>,
}

impl NetEntityIndex {
    /// Iterate identity bindings in unspecified order. Sort IDs for deterministic simulation.
    pub fn iter(&self) -> impl Iterator<Item = (NetId, Entity)> + '_ {
        self.bindings.iter().map(|(&id, &entity)| (id, entity))
    }

    /// Resolve a binding maintained by NetId lifecycle hooks.
    pub fn get(&self, id: &NetId) -> Option<&Entity> {
        self.bindings.get(id)
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

pub struct NetIdentityPlugin;
impl Plugin for NetIdentityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetEntityIndex>();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuplicateNetId {
    pub id: NetId,
    pub existing: Entity,
}
impl fmt::Display for DuplicateNetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "network identity {:?} already belongs to {:?}",
            self.id, self.existing
        )
    }
}
impl std::error::Error for DuplicateNetId {}

/// Resolve an identity in this world, validating the full local generation and ID.
pub fn entity(world: &World, id: NetId) -> Option<Entity> {
    let local = *world.get_resource::<NetEntityIndex>()?.bindings.get(&id)?;
    (world.get::<NetId>(local) == Some(&id)).then_some(local)
}

/// Spawn once. `bundle` must not contain `NetId`; duplicate IDs leave the world unchanged.
pub fn spawn(world: &mut World, id: NetId, bundle: impl Bundle) -> Result<Entity, DuplicateNetId> {
    world.init_resource::<NetEntityIndex>();
    if let Some(existing) = entity(world, id) {
        return Err(DuplicateNetId { id, existing });
    }
    Ok(world.spawn((id, bundle)).id())
}

/// Insert state on an existing local entity or spawn it, preserving local handles.
/// `bundle` must not contain `NetId`.
pub fn upsert(world: &mut World, id: NetId, bundle: impl Bundle) -> Entity {
    if let Some(local) = entity(world, id) {
        world.entity_mut(local).insert(bundle);
        local
    } else {
        spawn(world, id, bundle).expect("exclusive world duplicate check")
    }
}

/// Despawn the identified entity. Unrelated local entities are untouched.
pub fn despawn(world: &mut World, id: NetId) -> bool {
    entity(world, id).is_some_and(|local| world.despawn(local))
}

/// Despawn all identified entities while preserving resources and unrelated entities.
pub fn clear(world: &mut World) -> usize {
    clear_matching(world, |_| true)
}

/// Despawn identities from one match epoch, preserving all other epochs.
pub fn clear_epoch(world: &mut World, epoch: u32) -> usize {
    clear_matching(world, |id| id.epoch == epoch)
}

fn clear_matching(world: &mut World, matches: impl Fn(NetId) -> bool) -> usize {
    let ids: Vec<_> = world
        .get_resource::<NetEntityIndex>()
        .map(|index| {
            index
                .bindings
                .keys()
                .copied()
                .filter(|id| matches(*id))
                .collect()
        })
        .unwrap_or_default();
    ids.into_iter().filter(|id| despawn(world, *id)).count()
}

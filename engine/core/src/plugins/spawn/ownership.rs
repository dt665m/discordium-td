use bevy::prelude::*;

/// Runtime ownership with automatic recursive cleanup through Bevy relationships.
/// Rebuild this link from game-supplied stable IDs when restoring a snapshot.
#[derive(Component, Debug, Clone, Copy)]
#[relationship(relationship_target = OwnedSpawns)]
pub struct DespawnWithOwner(pub Entity);

#[derive(Component, Debug, Default)]
#[relationship_target(relationship = DespawnWithOwner, linked_spawn)]
pub struct OwnedSpawns(Vec<Entity>);

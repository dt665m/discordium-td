//! Identity lookup for commands/lifecycle; tick hot paths use native queries.
use super::*;
use game_replication::{NetEntityIndex, NetId};
#[derive(Component)]
pub struct SimulationOwned;
#[derive(Resource, Default)]
pub(crate) struct Epoch(pub u32);
pub(crate) trait Actor: Component {
    const NAMESPACE: u8;
}
impl Actor for HeroState {
    const NAMESPACE: u8 = game_shared::actor_namespace::HERO;
}
impl Actor for EnemyState {
    const NAMESPACE: u8 = game_shared::actor_namespace::ENEMY;
}
impl Actor for TowerState {
    const NAMESPACE: u8 = game_shared::actor_namespace::TOWER;
}
pub(crate) fn local<T: Actor>(world: &World, id: u64) -> Option<Entity> {
    game_replication::entity(
        world,
        NetId::new(world.resource::<Epoch>().0, T::NAMESPACE, id),
    )
}
pub(crate) fn actor_ids<T: Actor>(world: &World) -> Vec<u64> {
    let epoch = world.resource::<Epoch>().0;
    let mut ids: Vec<_> = world
        .resource::<NetEntityIndex>()
        .iter()
        .filter_map(|(id, e)| {
            (id.epoch == epoch && id.namespace == T::NAMESPACE && world.get::<T>(e).is_some())
                .then_some(id.value)
        })
        .collect();
    ids.sort_unstable();
    ids
}
pub(crate) fn clear_actors<T: Actor>(world: &mut World) {
    for id in actor_ids::<T>(world) {
        if let Some(e) = local::<T>(world, id) {
            world.despawn(e);
        }
    }
}
pub(crate) fn hero<'a>(world: &'a World, id: &u64) -> Option<HeroRef<'a>> {
    let e = local::<HeroState>(world, *id)?;
    Some(HeroRef {
        role: world.get::<HeroState>(e)?,
        position: world.get::<Position>(e)?,
        #[cfg(test)]
        health: world.get::<Health>(e)?,
        facing: world.get::<Facing>(e)?,
        attack: world.get::<Attack>(e)?,
    })
}
pub(crate) fn hero_mut<'a>(world: &'a mut World, id: &u64) -> Option<HeroMut<'a>> {
    let e = local::<HeroState>(world, *id)?;
    let (role, position, health, facing, attack) = world
        .entity_mut(e)
        .into_mutable()
        .into_components_mut::<(
            &mut HeroState,
            &mut Position,
            &mut Health,
            &mut Facing,
            &mut Attack,
        )>()
        .ok()?;
    Some(HeroMut {
        role: role.into_inner(),
        position: position.into_inner(),
        health: health.into_inner(),
        facing: facing.into_inner(),
        attack: attack.into_inner(),
    })
}
pub(crate) fn hero_entries(world: &World) -> impl Iterator<Item = (u64, HeroRef<'_>)> {
    actor_ids::<HeroState>(world)
        .into_iter()
        .filter_map(|id| Some((id, hero(world, &id)?)))
}
pub(crate) fn insert_hero(world: &mut World, id: u64, bundle: HeroBundle) {
    game_replication::upsert(
        world,
        NetId::new(world.resource::<Epoch>().0, HeroState::NAMESPACE, id),
        (SimulationOwned, bundle, HeroStep::default()),
    );
}
pub(crate) fn remove_hero(world: &mut World, id: &u64) -> Option<HeroState> {
    let e = local::<HeroState>(world, *id)?;
    let role = world.get::<HeroState>(e)?.clone();
    world.despawn(e);
    Some(role)
}
pub(crate) fn enemy<'a>(world: &'a World, id: &u64) -> Option<EnemyRef<'a>> {
    let e = local::<EnemyState>(world, *id)?;
    Some(EnemyRef {
        role: world.get::<EnemyState>(e)?,
        position: world.get::<Position>(e)?,
        #[cfg(test)]
        health: world.get::<Health>(e)?,
        #[cfg(test)]
        facing: world.get::<Facing>(e)?,
    })
}
pub(crate) fn enemy_mut<'a>(world: &'a mut World, id: &u64) -> Option<EnemyMut<'a>> {
    let e = local::<EnemyState>(world, *id)?;
    let (role, position, health, facing, attack) = world
        .entity_mut(e)
        .into_mutable()
        .into_components_mut::<(
            &mut EnemyState,
            &mut Position,
            &mut Health,
            &mut Facing,
            &mut Attack,
        )>()
        .ok()?;
    Some(EnemyMut {
        role: role.into_inner(),
        position: position.into_inner(),
        health: health.into_inner(),
        facing: facing.into_inner(),
        attack: attack.into_inner(),
    })
}
pub(crate) fn enemy_entries(world: &World) -> impl Iterator<Item = (u64, EnemyRef<'_>)> {
    actor_ids::<EnemyState>(world)
        .into_iter()
        .filter_map(|id| Some((id, enemy(world, &id)?)))
}
pub(crate) fn enemies(world: &World) -> impl Iterator<Item = EnemyRef<'_>> {
    enemy_entries(world).map(|(_, v)| v)
}
pub(crate) fn insert_enemy(world: &mut World, id: u64, bundle: EnemyBundle) {
    game_replication::upsert(
        world,
        NetId::new(world.resource::<Epoch>().0, EnemyState::NAMESPACE, id),
        (SimulationOwned, bundle, EnemyStep::default()),
    );
}
pub(crate) fn remove_enemy(world: &mut World, id: &u64) -> Option<EnemyState> {
    let e = local::<EnemyState>(world, *id)?;
    let role = world.get::<EnemyState>(e)?.clone();
    world.despawn(e);
    Some(role)
}
pub(crate) fn tower<'a>(world: &'a World, id: &u64) -> Option<TowerRef<'a>> {
    let e = local::<TowerState>(world, *id)?;
    Some(TowerRef {
        role: world.get::<TowerState>(e)?,
        position: world.get::<Position>(e)?,
    })
}
pub(crate) fn tower_entries(world: &World) -> impl Iterator<Item = (u64, TowerRef<'_>)> {
    actor_ids::<TowerState>(world)
        .into_iter()
        .filter_map(|id| Some((id, tower(world, &id)?)))
}
pub(crate) fn towers(world: &World) -> impl Iterator<Item = TowerRef<'_>> {
    tower_entries(world).map(|(_, v)| v)
}
pub(crate) fn insert_tower(world: &mut World, id: u64, bundle: TowerBundle) {
    game_replication::upsert(
        world,
        NetId::new(world.resource::<Epoch>().0, TowerState::NAMESPACE, id),
        (SimulationOwned, bundle, TowerStep::default()),
    );
}
pub(crate) fn remove_tower(world: &mut World, id: &u64) -> Option<TowerState> {
    let e = local::<TowerState>(world, *id)?;
    let role = world.get::<TowerState>(e)?.clone();
    world.despawn(e);
    Some(role)
}

/// Retain matching local entities; never touch resources or foreign entities.
pub(crate) fn prune_snapshot_actors(world: &mut World, delta: &WorldDelta, meta: &SimMeta) {
    let desired: std::collections::BTreeSet<_> = delta
        .heroes
        .iter()
        .map(|v| NetId::new(meta.match_epoch, HeroState::NAMESPACE, v.client_id))
        .chain(
            delta
                .enemies
                .iter()
                .map(|v| NetId::new(meta.match_epoch, EnemyState::NAMESPACE, v.id)),
        )
        .chain(
            delta
                .towers
                .iter()
                .map(|v| NetId::new(meta.match_epoch, TowerState::NAMESPACE, v.id)),
        )
        .collect();
    let mut query = world.query_filtered::<(Entity, &NetId), With<SimulationOwned>>();
    let removed: Vec<_> = query
        .iter(world)
        .filter_map(|(entity, id)| (!desired.contains(id)).then_some(entity))
        .collect();
    for entity in removed {
        world.despawn(entity);
    }
}

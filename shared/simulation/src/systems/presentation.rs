//! Simulation-owned transient lifetimes and full replay-stable identities.
use super::*;
use game_shared::{PresentationId, PresentationInstance};

type Key = (u32, u64, u32, u16);
fn key(id: PresentationId) -> Key {
    (id.match_epoch, id.owner, id.action_seq, id.slot)
}
#[derive(Resource, Default)]
pub(crate) struct PresentationEntities {
    entries: BTreeMap<Key, Entity>,
    next_order: u64,
}

pub(crate) fn insert(world: &mut World, instance: PresentationInstance) {
    let identity = key(instance.id);
    if let Some(entity) = world
        .resource::<PresentationEntities>()
        .entries
        .get(&identity)
        .copied()
    {
        world.get_mut::<Presentation>(entity).unwrap().instance = instance;
        return;
    }
    let order = {
        let mut index = world.resource_mut::<PresentationEntities>();
        let order = index.next_order;
        index.next_order += 1;
        order
    };
    let entity = world
        .spawn((SimulationOwned, Presentation { instance, order }))
        .id();
    world
        .resource_mut::<PresentationEntities>()
        .entries
        .insert(identity, entity);
}
pub(crate) fn instances(world: &World) -> Vec<&PresentationInstance> {
    let mut ordered: Vec<_> = world
        .resource::<PresentationEntities>()
        .entries
        .values()
        .filter_map(|&entity| world.get::<Presentation>(entity))
        .collect();
    ordered.sort_unstable_by_key(|p| p.order);
    ordered.into_iter().map(|p| &p.instance).collect()
}
pub(crate) fn restore(world: &mut World, instances: &[PresentationInstance]) {
    let wanted: std::collections::BTreeSet<_> =
        instances.iter().map(|instance| key(instance.id)).collect();
    let absent: Vec<_> = world
        .resource::<PresentationEntities>()
        .entries
        .iter()
        .filter_map(|(id, &entity)| (!wanted.contains(id)).then_some((*id, entity)))
        .collect();
    for (id, entity) in absent {
        world.despawn(entity);
        world
            .resource_mut::<PresentationEntities>()
            .entries
            .remove(&id);
    }
    for (order, &instance) in instances.iter().enumerate() {
        insert(world, instance);
        let entity = world.resource::<PresentationEntities>().entries[&key(instance.id)];
        world.get_mut::<Presentation>(entity).unwrap().order = order as u64;
    }
    world.resource_mut::<PresentationEntities>().next_order = instances.len() as u64;
}
pub(crate) fn clear(world: &mut World) {
    restore(world, &[]);
}
pub(crate) fn age(
    mut instances: Query<(Entity, &mut Presentation)>,
    mut index: ResMut<PresentationEntities>,
    mut commands: Commands,
) {
    for (entity, mut presentation) in &mut instances {
        presentation.instance.age_ticks += 1;
        if presentation.instance.age_ticks >= presentation.instance.duration_ticks {
            index.entries.remove(&key(presentation.instance.id));
            commands.entity(entity).despawn();
        }
    }
}

//! Shared entry points operating on the host's simulation entities.
use super::*;
use game_shared::{ClientAction, HeroSnapshot};

pub fn add_player(world: &mut World, id: u64) {
    world.resource_scope(|world, mut g: Mut<Globals>| g.add_player(world, id));
}
pub fn remove_player(world: &mut World, id: u64) {
    world.resource_scope(|world, mut g: Mut<Globals>| g.remove_player(world, id));
}
pub fn has_players(world: &World) -> bool {
    world.resource::<Globals>().has_players(world)
}
pub fn tick(world: &World) -> u32 {
    world.resource::<Globals>().tick
}
pub fn world_delta(world: &World) -> WorldDelta {
    snapshot::world_delta(world)
}
/// Capture one current-epoch hero through the existing network identity index.
pub fn hero_snapshot(world: &World, client_id: u64) -> Option<HeroSnapshot> {
    snapshot::hero_snapshot(world, client_id)
}
pub fn sim_meta(world: &World) -> SimMeta {
    world.resource::<Globals>().sim_meta(world)
}
pub fn join_snapshot(world: &World, id: u64) -> JoinSnapshot {
    world.resource::<Globals>().join_snapshot(world, id)
}
pub fn queue_command(world: &mut World, id: u64, command: ClientCommand) {
    world.resource_scope(|world, mut g: Mut<Globals>| g.queue_command(world, id, command));
}
pub fn queue_action(world: &mut World, id: u64, action: ClientAction) {
    world.resource_scope(|world, mut g: Mut<Globals>| g.queue_action(world, id, action));
}
pub fn ready_actions(world: &World) -> Vec<(u64, ClientAction)> {
    world.resource::<Globals>().ready_actions(world)
}
pub fn set_historical_targets(world: &mut World, id: u64, seq: u32, targets: Vec<(u64, [f32; 2])>) {
    world.resource_scope(|world, mut g: Mut<Globals>| {
        g.set_historical_targets(world, id, seq, targets)
    });
}
pub fn step(world: &mut World) -> TickOutput {
    world.run_schedule(SimulationTick);
    TickOutput {
        reliable_events: std::mem::take(&mut world.resource_mut::<TickEvents>().0),
    }
}
pub fn apply_snapshot(world: &mut World, delta: &WorldDelta, meta: &SimMeta) {
    world.resource_scope(|world, mut g: Mut<Globals>| g.apply_snapshot(world, delta, meta));
}

pub fn presentations(world: &World) -> Vec<&game_shared::PresentationInstance> {
    systems::presentation::instances(world)
}

impl Globals {
    pub(crate) fn join_snapshot(&self, world: &World, client_id: u64) -> JoinSnapshot {
        JoinSnapshot {
            you: client_id,
            build_nodes: BUILD_NODES.to_vec(),
            world: self.world_delta(world),
        }
    }

    pub(crate) fn world_delta(&self, world: &World) -> WorldDelta {
        snapshot::capture(world, self)
    }

    pub(crate) fn sim_meta(&self, world: &World) -> SimMeta {
        SimMeta {
            match_epoch: world.resource::<Epoch>().0,
            next_entity_id: self.next_entity_id,
            wave_remaining: self.wave_remaining,
            next_spawn_tick: self.next_spawn_tick,
            next_spawn_point_index: self.next_spawn_point_index as u8,
            intermission_until: self.intermission_until,
        }
    }
}

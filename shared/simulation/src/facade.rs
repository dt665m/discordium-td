use super::*;

/// Isolated ECS world used by prediction, replay and headless callers.
#[derive(Debug)]
pub struct Simulation {
    pub world: World,
}
impl Default for Simulation {
    fn default() -> Self {
        Self::new()
    }
}
impl Simulation {
    pub fn new() -> Self {
        let mut app = App::new();
        app.add_plugins(SimulationPlugins);
        Self {
            world: std::mem::take(app.world_mut()),
        }
    }
    pub fn add_player(&mut self, id: u64) {
        world::add_player(&mut self.world, id)
    }
    pub fn remove_player(&mut self, id: u64) {
        world::remove_player(&mut self.world, id)
    }
    pub fn has_players(&self) -> bool {
        world::has_players(&self.world)
    }
    pub fn tick(&self) -> u32 {
        world::tick(&self.world)
    }
    pub fn world_delta(&self) -> WorldDelta {
        world::world_delta(&self.world)
    }
    pub fn sim_meta(&self) -> SimMeta {
        world::sim_meta(&self.world)
    }
    pub fn join_snapshot(&self, id: u64) -> JoinSnapshot {
        world::join_snapshot(&self.world, id)
    }
    pub fn queue_command(&mut self, id: u64, command: ClientCommand) {
        world::queue_command(&mut self.world, id, command)
    }
    pub fn queue_action(&mut self, id: u64, action: game_shared::ClientAction) {
        world::queue_action(&mut self.world, id, action)
    }
    pub fn ready_actions(&self) -> Vec<(u64, game_shared::ClientAction)> {
        world::ready_actions(&self.world)
    }
    pub fn set_historical_targets(&mut self, id: u64, seq: u32, targets: Vec<(u64, [f32; 2])>) {
        world::set_historical_targets(&mut self.world, id, seq, targets)
    }
    pub fn step(&mut self) -> TickOutput {
        world::step(&mut self.world)
    }
    pub fn presentations(&self) -> Vec<&game_shared::PresentationInstance> {
        world::presentations(&self.world)
    }
    pub fn set_tick(&mut self, tick: u32) {
        self.world.resource_mut::<Globals>().tick = tick;
    }
    pub fn from_snapshot(delta: &WorldDelta, meta: &SimMeta) -> Self {
        let mut sim = Self::new();
        sim.apply_snapshot(delta, meta);
        sim
    }
    pub fn apply_snapshot(&mut self, delta: &WorldDelta, meta: &SimMeta) {
        world::apply_snapshot(&mut self.world, delta, meta)
    }
}

//! Match services are separate resources, rather than a second actor database.
use super::*;
#[derive(Resource, Default)]
pub(crate) struct CommandInbox(pub Vec<(u64, ClientCommand)>);
#[derive(Resource, Default)]
pub(crate) struct HistoricalTargets(pub BTreeMap<(u64, u32), Vec<(u64, [f32; 2])>>);
#[derive(Resource, Debug)]
pub(crate) struct NavigationCache {
    pub mesh: Option<NavMesh>,
    pub dirty: bool,
}
impl Default for NavigationCache {
    fn default() -> Self {
        Self {
            mesh: None,
            dirty: true,
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct TickEvents(pub Vec<ReliableGameEvent>);

#[derive(Resource, Debug)]
pub(crate) struct Globals {
    pub(crate) tick: u32,
    pub(crate) phase: MatchPhase,
    pub(crate) team_life: i32,
    pub(crate) wave: u32,
    pub(crate) objective_hp: f32,
    pub(crate) node_occupancy: BTreeMap<u32, u64>,
    pub(crate) next_entity_id: u64,
    pub(crate) wave_remaining: u32,
    pub(crate) next_spawn_tick: u32,
    pub(crate) next_spawn_point_index: usize,
    pub(crate) intermission_until: u32,
    pub(crate) reset_at_tick: Option<u32>,
}

impl Default for Globals {
    fn default() -> Self {
        Self::new()
    }
}

impl Globals {
    pub(crate) fn new() -> Self {
        Self {
            tick: 0,
            phase: MatchPhase::InProgress,
            team_life: INITIAL_TEAM_LIFE,
            wave: 0,
            objective_hp: OBJECTIVE_MAX_HP,
            node_occupancy: BTreeMap::new(),
            next_entity_id: 1_000_000,
            wave_remaining: 0,
            next_spawn_tick: 0,
            next_spawn_point_index: 0,
            intermission_until: WAVE_PREP_TICKS,
            reset_at_tick: None,
        }
    }
}

use serde::{Deserialize, Serialize};
mod packed_patch;
pub mod replication;
mod wire;
pub use replication::{apply_world_patch, build_world_patch};
pub use wire::{decode, decode_with_limit, encode};

pub const PROTOCOL_ID: u64 = 13;

/// Stable actor namespaces used by world-local replication identity indexes.
/// These distinguish player IDs from server-allocated world IDs.
pub mod actor_namespace {
    pub const HERO: u8 = 0;
    pub const ENEMY: u8 = 1;
    pub const TOWER: u8 = 2;
}
/// At most two waiting movement ticks, plus the tick being executed.
pub const MAX_MOVEMENT_BACKLOG: usize = 3;
/// Bound speculative whole-world work during missing authority (500 ms).
pub const MAX_PREDICTION_TICKS: usize = 15;
pub const SERVER_TICK_HZ: u32 = 30;
pub const FIXED_DT_SECONDS: f32 = 1.0 / SERVER_TICK_HZ as f32;

pub const WORLD_HALF_WIDTH: f32 = 24.0;
pub const WORLD_HALF_HEIGHT: f32 = 14.0;

pub const HERO_SPEED: f32 = 9.5;
pub const HERO_MAX_HP: f32 = 100.0;
pub const HERO_MAX_MANA: f32 = 100.0;
pub const HERO_MANA_REGEN_PER_TICK: f32 = 0.22;
pub const HERO_COLLIDER_RADIUS: f32 = 0.48;
pub const HERO_REGULAR_ATTACK_RANGE: f32 = 2.2;
pub const HERO_REGULAR_ATTACK_ARC_DOT_THRESHOLD: f32 = 0.45;
pub const HERO_REGULAR_ATTACK_DAMAGE: f32 = 30.0;
pub const HERO_REGULAR_ATTACK_WINDUP_TICKS: u32 = 5;
pub const HERO_REGULAR_ATTACK_RECOVERY_TICKS: u32 = 9;
pub const HERO_ABILITY_COOLDOWN_TICKS: u32 = 36;
pub const HERO_ABILITY_RADIUS: f32 = 3.5;
pub const HERO_ABILITY_DAMAGE: f32 = 24.0;
pub const HERO_ABILITY_MANA_COST: f32 = 35.0;
pub const HERO_CHARGE_STARTUP_TICKS: u32 = 6;
pub const HERO_CHARGE_RELEASE_LAG_TICKS: u32 = 3;
pub const HERO_CHARGE_MANA_PER_TICK: f32 = 2.6;
pub const HERO_CHARGE_HEALTH_PER_TICK: f32 = 0.6;
pub const HERO_CHARGE_POWER_GAIN_PER_TICK: f32 = 2.2;
pub const HERO_CHARGE_POWER_METER_MAX: f32 = 100.0;
pub const HERO_POWER_DECAY_INTERVAL_TICKS: u32 = 3;
pub const HERO_POWER_DECAY_PER_INTERVAL: f32 = 1.25;
pub const HERO_POWERED_ATTACK_DAMAGE_MULTIPLIER: f32 = 2.0;
pub const HERO_POWERED_ABILITY_MANA_COST_MULTIPLIER: f32 = 0.5;
pub const HERO_POWERED_ABILITY_COOLDOWN_MULTIPLIER: f32 = 0.5;
pub const HERO_POWERED_ABILITY_RADIUS_MULTIPLIER: f32 = 3.0;
pub const HERO_POWERED_REGULAR_ATTACK_RANGE_MULTIPLIER: f32 = 10.0;

pub const TOWER_BUILD_COST: u32 = 110;
pub const BUILD_COMMAND_MAX_DISTANCE: f32 = 6.5;
pub const TOWER_RANGE: f32 = 5.5;
pub const TOWER_DAMAGE: f32 = 13.0;
pub const TOWER_RELOAD_TICKS: u32 = SERVER_TICK_HZ;
pub const TOWER_COLLIDER_RADIUS: f32 = 0.68;

pub const OBJECTIVE_MAX_HP: f32 = 100.0;
pub const OBJECTIVE_COLLIDER_RADIUS: f32 = 1.05;
pub const ENEMY_OBJECTIVE_DAMAGE: f32 = 17.0;
pub const INITIAL_TEAM_LIFE: i32 = 10;
pub const INITIAL_GOLD: u32 = 180;
pub const BASE_POSITION: [f32; 2] = [21.0, 0.0];
pub const ENEMY_GRUNT_COLLIDER_RADIUS: f32 = 0.44;
pub const ENEMY_TANK_COLLIDER_RADIUS: f32 = 0.62;
pub const ENEMY_REGULAR_ATTACK_RANGE: f32 = 1.75;
pub const ENEMY_REGULAR_ATTACK_ARC_DOT_THRESHOLD: f32 = 0.3;
pub const ENEMY_REGULAR_ATTACK_DAMAGE: f32 = 8.0;
pub const ENEMY_REGULAR_ATTACK_WINDUP_TICKS: u32 = 7;
pub const ENEMY_REGULAR_ATTACK_RECOVERY_TICKS: u32 = 11;

pub const WAVE_PREP_TICKS: u32 = SERVER_TICK_HZ * 4;
pub const WAVE_SPAWN_INTERVAL_TICKS: u32 = SERVER_TICK_HZ / 2;
pub const MATCH_RESET_TICKS: u32 = SERVER_TICK_HZ * 5;
pub const MAX_WAVES: u32 = 20;
pub const BASE_ENEMIES_PER_WAVE: u32 = 8;
pub const EXTRA_ENEMIES_PER_WAVE: u32 = 4;

pub const ENEMY_SPAWN_POINTS: &[[f32; 2]] = &[[-22.0, -8.0], [-22.0, 0.0], [-22.0, 8.0]];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BuildNodeDef {
    pub node_id: u32,
    pub lane: u8,
    pub pos: [f32; 2],
}

pub const BUILD_NODES: [BuildNodeDef; 8] = [
    BuildNodeDef {
        node_id: 1,
        lane: 0,
        pos: [-12.0, -4.5],
    },
    BuildNodeDef {
        node_id: 2,
        lane: 0,
        pos: [-1.0, -3.7],
    },
    BuildNodeDef {
        node_id: 3,
        lane: 0,
        pos: [10.0, -4.4],
    },
    BuildNodeDef {
        node_id: 4,
        lane: 0,
        pos: [18.0, -3.4],
    },
    BuildNodeDef {
        node_id: 5,
        lane: 0,
        pos: [-12.0, 4.5],
    },
    BuildNodeDef {
        node_id: 6,
        lane: 0,
        pos: [-1.0, 3.7],
    },
    BuildNodeDef {
        node_id: 7,
        lane: 0,
        pos: [10.0, 4.4],
    },
    BuildNodeDef {
        node_id: 8,
        lane: 0,
        pos: [18.0, 3.4],
    },
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MatchPhase {
    InProgress,
    Victory,
    Defeat,
}

impl Default for MatchPhase {
    fn default() -> Self {
        Self::InProgress
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AbilityId {
    ArcBurst,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TowerType {
    Arrow,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EnemyType {
    Grunt,
    Tank,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct FacingComponent {
    pub dir: [f32; 2],
}

impl Default for FacingComponent {
    fn default() -> Self {
        Self { dir: [1.0, 0.0] }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct DirectionalAttackComponent {
    pub range: f32,
    pub arc_dot_threshold: f32,
    pub damage: f32,
    pub windup_ticks: u32,
    pub recovery_ticks: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AttackPhase {
    Ready,
    Windup,
    Recovery,
}

impl Default for AttackPhase {
    fn default() -> Self {
        Self::Ready
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChargePhase {
    Idle,
    Startup,
    Charging,
    Recovery,
}

impl Default for ChargePhase {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DirectionalAttackStateComponent {
    pub phase: AttackPhase,
    pub ticks_remaining: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ChargeProfileComponent {
    pub startup_ticks: u32,
    pub release_lag_ticks: u32,
    pub mana_per_tick: f32,
    pub health_per_tick: f32,
    pub power_gain_per_tick: f32,
    pub power_meter_max: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PowerDecayProfileComponent {
    pub interval_ticks: u32,
    pub amount_per_interval: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PoweredUpModifiersComponent {
    pub attack_damage_multiplier: f32,
    pub ability_mana_cost_multiplier: f32,
    pub ability_cooldown_multiplier: f32,
    pub ability_radius_multiplier: f32,
    pub regular_attack_range_multiplier: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct ChargeStateComponent {
    pub phase: ChargePhase,
    pub phase_ticks_remaining: u32,
    pub input_held: bool,
    pub power_meter: f32,
    pub power_active: bool,
    pub power_decay_ticks_remaining: u32,
}

pub const HERO_REGULAR_ATTACK: DirectionalAttackComponent = DirectionalAttackComponent {
    range: HERO_REGULAR_ATTACK_RANGE,
    arc_dot_threshold: HERO_REGULAR_ATTACK_ARC_DOT_THRESHOLD,
    damage: HERO_REGULAR_ATTACK_DAMAGE,
    windup_ticks: HERO_REGULAR_ATTACK_WINDUP_TICKS,
    recovery_ticks: HERO_REGULAR_ATTACK_RECOVERY_TICKS,
};

pub const ENEMY_REGULAR_ATTACK: DirectionalAttackComponent = DirectionalAttackComponent {
    range: ENEMY_REGULAR_ATTACK_RANGE,
    arc_dot_threshold: ENEMY_REGULAR_ATTACK_ARC_DOT_THRESHOLD,
    damage: ENEMY_REGULAR_ATTACK_DAMAGE,
    windup_ticks: ENEMY_REGULAR_ATTACK_WINDUP_TICKS,
    recovery_ticks: ENEMY_REGULAR_ATTACK_RECOVERY_TICKS,
};

pub const HERO_CHARGE_PROFILE: ChargeProfileComponent = ChargeProfileComponent {
    startup_ticks: HERO_CHARGE_STARTUP_TICKS,
    release_lag_ticks: HERO_CHARGE_RELEASE_LAG_TICKS,
    mana_per_tick: HERO_CHARGE_MANA_PER_TICK,
    health_per_tick: HERO_CHARGE_HEALTH_PER_TICK,
    power_gain_per_tick: HERO_CHARGE_POWER_GAIN_PER_TICK,
    power_meter_max: HERO_CHARGE_POWER_METER_MAX,
};

pub const HERO_POWER_DECAY_PROFILE: PowerDecayProfileComponent = PowerDecayProfileComponent {
    interval_ticks: HERO_POWER_DECAY_INTERVAL_TICKS,
    amount_per_interval: HERO_POWER_DECAY_PER_INTERVAL,
};

pub const HERO_POWERED_MODIFIERS: PoweredUpModifiersComponent = PoweredUpModifiersComponent {
    attack_damage_multiplier: HERO_POWERED_ATTACK_DAMAGE_MULTIPLIER,
    ability_mana_cost_multiplier: HERO_POWERED_ABILITY_MANA_COST_MULTIPLIER,
    ability_cooldown_multiplier: HERO_POWERED_ABILITY_COOLDOWN_MULTIPLIER,
    ability_radius_multiplier: HERO_POWERED_ABILITY_RADIUS_MULTIPLIER,
    regular_attack_range_multiplier: HERO_POWERED_REGULAR_ATTACK_RANGE_MULTIPLIER,
};

pub fn enemy_collider_radius(enemy_type: EnemyType) -> f32 {
    match enemy_type {
        EnemyType::Grunt => ENEMY_GRUNT_COLLIDER_RADIUS,
        EnemyType::Tank => ENEMY_TANK_COLLIDER_RADIUS,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ClientCommand {
    Move {
        seq: u32,
        dir: [f32; 2],
    },
    SetLockTarget {
        seq: u32,
        target_id: Option<u64>,
    },
    SetCharging {
        seq: u32,
        active: bool,
    },
    BasicAttack {
        seq: u32,
    },
    CastAbility {
        seq: u32,
        ability: AbilityId,
    },
    BuildTower {
        seq: u32,
        node_id: u32,
        tower_type: TowerType,
    },
}

impl ClientCommand {
    pub fn seq(&self) -> u32 {
        match self {
            Self::Move { seq, .. }
            | Self::SetLockTarget { seq, .. }
            | Self::SetCharging { seq, .. }
            | Self::BasicAttack { seq }
            | Self::CastAbility { seq, .. }
            | Self::BuildTower { seq, .. } => *seq,
        }
    }
}

/// Reliable action anchored after the last movement generated before the button press.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ClientAction {
    /// Predicted target-view tick; untrusted and bounded by server policy.
    pub view_tick: Option<u32>,
    pub match_epoch: u32,
    pub command: ClientCommand,
    pub after_move_seq: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BuildRejectReason {
    InvalidNode,
    Occupied,
    InsufficientGold,
    TooFar,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReliableGameEvent {
    WaveStarted {
        wave: u32,
    },
    TowerBuilt {
        owner: u64,
        node_id: u32,
        tower_id: u64,
    },
    BuildRejected {
        owner: u64,
        node_id: u32,
        reason: BuildRejectReason,
    },
    AbilityCast {
        owner: u64,
        pos: [f32; 2],
        radius: f32,
    },
    EnemyKilled {
        enemy_id: u64,
        killer: u64,
        reward: u32,
    },
    ObjectiveDamaged {
        lane: u8,
        hp: f32,
        team_life: i32,
    },
    Victory,
    Defeat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReliableServerMessage {
    JoinSnapshot(JoinSnapshot),
    Event(ReliableGameEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeroSnapshot {
    pub respawn_generation: u32,
    pub client_id: u64,
    pub pos: [f32; 2],
    pub facing: FacingComponent,
    pub lock_mode_active: bool,
    pub lock_target_id: Option<u64>,
    pub regular_attack: DirectionalAttackStateComponent,
    pub charge_profile: ChargeProfileComponent,
    pub power_decay_profile: PowerDecayProfileComponent,
    pub powered_modifiers: PoweredUpModifiersComponent,
    pub charge_state: ChargeStateComponent,
    pub hp: f32,
    pub mana: f32,
    pub gold: u32,
    pub ability_cooldown_ticks: u32,
    /// Independent channel watermarks: movement never acknowledges an action.
    pub move_dir: [f32; 2],
    pub pending_moves: Vec<(u32, [f32; 2])>,
    pub pending_actions: Vec<ClientAction>,
    pub last_move_seq: Option<u32>,
    pub last_action_seq: Option<u32>,
    pub last_attack_seq: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EnemyLockTarget {
    Hero(u64),
    Base,
}

/// Logical wave spawn, independent of speculative tower/entity allocation order.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EnemySpawnIdentity {
    pub wave: u32,
    pub ordinal: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EnemySnapshot {
    pub spawn: EnemySpawnIdentity,
    pub id: u64,
    pub lane: u8,
    pub pos: [f32; 2],
    pub vel: [f32; 2],
    pub facing: FacingComponent,
    pub regular_attack: DirectionalAttackStateComponent,
    pub hp: f32,
    pub max_hp: f32,
    pub enemy_type: EnemyType,
    pub speed: f32,
    pub reward: u32,
    pub lock_target: EnemyLockTarget,
    pub target_pos: [f32; 2],
    pub waypoint: [f32; 2],
    pub repath_cooldown: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TowerSnapshot {
    pub id: u64,
    pub owner: u64,
    pub lane: u8,
    pub node_id: u32,
    pub pos: [f32; 2],
    pub reload_ticks_remaining: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ObjectiveSnapshot {
    pub lane: u8,
    pub hp: f32,
    pub max_hp: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SimMeta {
    pub match_epoch: u32,
    pub next_entity_id: u64,
    pub wave_remaining: u32,
    pub next_spawn_tick: u32,
    pub next_spawn_point_index: u8,
    pub intermission_until: u32,
}

/// Stable across prediction, snapshot restore, and replay. Slots distinguish
/// multiple presentation instances produced by one mechanic action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PresentationId {
    pub match_epoch: u32,
    pub owner: u64,
    pub action_seq: u32,
    pub slot: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum PresentationKind {
    ArcBurst,
}

/// Simulation-owned lifetime. Renderers consume this state, never network events.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PresentationInstance {
    pub id: PresentationId,
    pub kind: PresentationKind,
    pub pos: [f32; 2],
    pub radius: f32,
    pub age_ticks: u32,
    pub duration_ticks: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldDelta {
    pub presentations: Vec<PresentationInstance>,
    pub tick: u32,
    pub phase: MatchPhase,
    pub match_restart_ticks_remaining: Option<u32>,
    pub wave: u32,
    pub team_life: i32,
    pub objectives: Vec<ObjectiveSnapshot>,
    pub heroes: Vec<HeroSnapshot>,
    pub enemies: Vec<EnemySnapshot>,
    pub towers: Vec<TowerSnapshot>,
    pub sim_meta: Option<SimMeta>,
}

impl WorldDelta {
    pub fn last_move_seq_for(&self, client_id: u64) -> Option<u32> {
        self.heroes
            .iter()
            .find(|hero| hero.client_id == client_id)
            .and_then(|hero| hero.last_move_seq)
    }
}

/// Client-to-server: bundled recent movement inputs for redundancy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientMoveBundle {
    pub match_epoch: u32,
    pub actions: Vec<ClientAction>,
    pub moves: Vec<(u32, [f32; 2])>, // (seq, dir) pairs, oldest first
}

/// Client-to-server: acknowledges the latest received server tick.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientAck {
    pub tick: u32,
}

/// Client unreliable message: either a move bundle or a tick ack (or both).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClientUnreliableMessage {
    MoveBundle(ClientMoveBundle),
    MoveBundleWithAck {
        moves: Vec<(u32, [f32; 2])>,
        ack_tick: u32,
    },
}

/// Server-to-client: either a full snapshot or a delta patch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ServerWorldMessage {
    Full(WorldDelta),
    Patch(WorldPatch),
}

impl ServerWorldMessage {
    /// Same Full variant on the wire, without cloning a world just to serialize it.
    pub fn encode_full(world: &WorldDelta) -> Vec<u8> {
        #[derive(Serialize)]
        enum Borrowed<'a> {
            Full(&'a WorldDelta),
        }
        encode(&Borrowed::Full(world))
    }
}

/// Current values of changed records over an acknowledged frame range.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldPatch {
    pub tick: u32,
    pub baseline_tick: u32,
    pub records: Vec<RecordPatch>,
}

/// Stable replication namespaces; one actor's layout never shifts another's.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum StateKey {
    World,
    Hero(u64),
    Enemy(u64),
    Tower(u64),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordPatch {
    pub key: StateKey,
    /// None removes the record; Some resizes it before applying current values.
    pub state_len: Option<u32>,
    /// One bit per byte of the reconstructed record, least-significant bit first.
    pub mask: Vec<u8>,
    /// Current values for set bits, in ascending byte-offset order.
    pub values: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JoinSnapshot {
    pub you: u64,
    pub build_nodes: Vec<BuildNodeDef>,
    pub world: WorldDelta,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DebugWorldMessageKind {
    Full,
    Patch,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DebugRenderActorKind {
    LocalHero,
    RemoteHero,
    Enemy,
    Tower,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebugRenderActor {
    pub id: u64,
    pub kind: DebugRenderActorKind,
    pub pos: [f32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientInterpolationDebug {
    pub snapshot_ticks: Vec<u32>,
    pub render_time: f32,
    pub interpolation_delay: f32,
    pub enemy_render_lead: f32,
    pub older_tick: Option<u32>,
    pub newer_tick: Option<u32>,
    pub factor: Option<f32>,
}

// Debug JSON is versioned independently of the gameplay wire protocol.
pub const DEBUG_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DebugNetworkHealth {
    pub browser_drops: Option<DebugBrowserDrops>,
    pub transport: String,
    pub rtt_ms: f64,
    pub packet_loss: f64,
    pub sent_bytes_per_second: f64,
    pub received_bytes_per_second: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DebugBrowserDrops {
    pub send_backpressure: u64,
    pub receive_overflow: u64,
    pub invalid_size: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DebugReplicationHealth {
    pub full_snapshots: u64,
    pub patches: u64,
    pub baseline_misses: u64,
    pub decode_errors: u64,
    pub transport_errors: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DebugCaptureHealth {
    pub dropped_frames: u64,
    pub dropped_batches: u64,
    pub upload_failures: u64,
    pub uploaded_batches: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DebugServerIdentity {
    pub realm: String,
    pub instance: String,
    pub process_session: String,
    pub build: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebugConditionerDirection {
    pub queued_packets: usize,
    pub queued_bytes: usize,
    pub simulated_loss_drops: u64,
    pub outage_drops: u64,
    pub overflow_drops: u64,
    pub transition_drops: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebugConditioner {
    pub enabled: bool,
    pub delay_each_way_ms: f64,
    pub jitter_ms: f64,
    pub packet_loss: f32,
    pub baseline_rtt_ms: Option<f64>,
    pub outage_active: bool,
    pub incoming: DebugConditionerDirection,
    pub outgoing: DebugConditionerDirection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientDebugFrame {
    #[serde(default)]
    pub conditioner: Option<DebugConditioner>,
    pub marker: bool,
    pub replication: DebugReplicationHealth,
    pub schema_version: u32,
    pub session_id: String,
    pub capture_elapsed_ms: f64,
    pub frame_duration_ms: f64,
    pub snapshot_age_ms: Option<f64>,
    pub input_ack_age_ms: Option<f64>,
    pub disconnect_reason: Option<String>,
    pub network: Option<DebugNetworkHealth>,
    pub recorder: DebugCaptureHealth,
    pub frame_index: u64,
    pub client_id: Option<u64>,
    pub connected: bool,
    pub menu_visible: bool,
    pub menu_status: String,
    pub phase: MatchPhase,
    pub wave: u32,
    pub team_life: i32,
    pub objectives: Vec<ObjectiveSnapshot>,
    pub applied_world_tick: u32,
    pub latest_server_tick: Option<u32>,
    pub latest_server_message: Option<DebugWorldMessageKind>,
    pub latest_acked_input_seq: Option<u32>,
    pub latest_sim_meta: Option<SimMeta>,
    pub predicted_tick: Option<u32>,
    pub input_dir: [f32; 2],
    pub pending_action_count: usize,
    pub pending_move_count: usize,
    pub rtt_ema: f32,
    pub jitter_ema: f32,
    pub reconciliation_offset: [f32; 2],
    pub authoritative_heroes: Vec<HeroSnapshot>,
    pub authoritative_enemies: Vec<EnemySnapshot>,
    pub authoritative_towers: Vec<TowerSnapshot>,
    pub predicted_heroes: Vec<HeroSnapshot>,
    pub predicted_enemies: Vec<EnemySnapshot>,
    pub predicted_towers: Vec<TowerSnapshot>,
    pub rendered_actors: Vec<DebugRenderActor>,
    pub interpolation: ClientInterpolationDebug,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientDebugBridgeExport {
    pub enabled: bool,
    pub latest: Option<ClientDebugFrame>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DebugClientPlatform {
    Native,
    Wasm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientDebugUploadBatch {
    pub schema_version: u32,
    pub session_id: String,
    pub build: String,
    pub instance_id: String,
    pub source: DebugClientPlatform,
    pub upload_seq: u64,
    pub frames: Vec<ClientDebugFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerDebugClientFrame {
    pub network: Option<DebugNetworkHealth>,
    pub client_id: u64,
    pub last_acked_tick: Option<u32>,
    pub confirmed_baseline_tick: Option<u32>,
    pub sent_history_len: usize,
    pub snapshot_payload_bytes: Option<usize>,
    pub snapshot_baseline_tick: Option<u32>,
    pub world: WorldDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebugServerConditioner {
    pub packets: DebugConditioner,
    pub peers: usize,
    pub peer_limit_drops: u64,
    pub expired_peers: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerDebugFrame {
    #[serde(default)]
    pub conditioner: Option<DebugServerConditioner>,
    pub schema_version: u32,
    pub identity: DebugServerIdentity,
    pub capture_unix_ms: u64,
    pub capture_elapsed_ms: f64,
    pub step_duration_ms: f64,
    pub tick: u32,
    pub clients: Vec<ServerDebugClientFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerDebugBridgeExport {
    pub enabled: bool,
    pub latest: Option<ServerDebugFrame>,
    pub frames: Vec<ServerDebugFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebugRecorderInfo {
    pub limit_reached: bool,
    pub max_run_bytes: u64,
    pub schema_version: u32,
    pub identity: DebugServerIdentity,
    pub dropped_records: u64,
    pub write_failures: u64,
    pub written_records: u64,
    pub enabled: bool,
    pub run_id: Option<String>,
    pub run_dir: Option<String>,
    pub server_frames_path: Option<String>,
    pub client_frames_path: Option<String>,
}

pub fn clamp_to_world(pos: [f32; 2]) -> [f32; 2] {
    [
        pos[0].clamp(-WORLD_HALF_WIDTH, WORLD_HALF_WIDTH),
        pos[1].clamp(-WORLD_HALF_HEIGHT, WORLD_HALF_HEIGHT),
    ]
}

pub fn normalize_or_zero(dir: [f32; 2]) -> [f32; 2] {
    if !dir[0].is_finite() || !dir[1].is_finite() {
        return [0.0, 0.0];
    }
    let length_sq = dir[0] * dir[0] + dir[1] * dir[1];
    if length_sq <= f32::EPSILON {
        return [0.0, 0.0];
    }

    let inv_len = length_sq.sqrt().recip();
    [dir[0] * inv_len, dir[1] * inv_len]
}

pub fn cardinalize_dir_or(dir: [f32; 2], fallback: [f32; 2]) -> [f32; 2] {
    let normalized = normalize_or_zero(dir);
    if normalized == [0.0, 0.0] {
        let fallback = normalize_or_zero(fallback);
        if fallback == [0.0, 0.0] {
            return [1.0, 0.0];
        }
        if fallback[0].abs() >= fallback[1].abs() {
            [fallback[0].signum(), 0.0]
        } else {
            [0.0, fallback[1].signum()]
        }
    } else if normalized[0].abs() >= normalized[1].abs() {
        [normalized[0].signum(), 0.0]
    } else {
        [0.0, normalized[1].signum()]
    }
}

pub fn distance_sq(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

pub fn directional_attack_can_hit(
    attacker_pos: [f32; 2],
    facing: [f32; 2],
    target_pos: [f32; 2],
    profile: DirectionalAttackComponent,
) -> bool {
    let to_target = [
        target_pos[0] - attacker_pos[0],
        target_pos[1] - attacker_pos[1],
    ];
    let dist_sq = to_target[0] * to_target[0] + to_target[1] * to_target[1];
    if dist_sq > profile.range * profile.range {
        return false;
    }

    let facing = normalize_or_zero(facing);
    if facing == [0.0, 0.0] {
        return false;
    }

    let inv_len = dist_sq.max(f32::EPSILON).sqrt().recip();
    let to_target_norm = [to_target[0] * inv_len, to_target[1] * inv_len];
    let dot = facing[0] * to_target_norm[0] + facing[1] * to_target_norm[1];
    dot >= profile.arc_dot_threshold
}

pub fn is_newer_input_seq(new_seq: u32, old_seq: u32) -> bool {
    if new_seq == old_seq {
        return false;
    }

    let diff = new_seq.wrapping_sub(old_seq);
    diff != 0 && diff < (1_u32 << 31)
}

#[cfg(test)]
mod tests {
    use super::{
        ENEMY_REGULAR_ATTACK, EnemyType, cardinalize_dir_or, directional_attack_can_hit,
        enemy_collider_radius, is_newer_input_seq, normalize_or_zero,
    };

    #[test]
    fn input_sequence_wraparound_is_supported() {
        assert!(is_newer_input_seq(1, u32::MAX - 2));
        assert!(!is_newer_input_seq(u32::MAX - 2, 1));
    }

    #[test]
    fn normalize_zero_returns_zero() {
        assert_eq!(normalize_or_zero([0.0, 0.0]), [0.0, 0.0]);
    }

    #[test]
    fn enemy_radius_matches_type() {
        assert!(enemy_collider_radius(EnemyType::Grunt) < enemy_collider_radius(EnemyType::Tank));
    }

    #[test]
    fn cardinalization_prefers_dominant_axis() {
        assert_eq!(cardinalize_dir_or([0.2, 0.9], [1.0, 0.0]), [0.0, 1.0]);
        assert_eq!(cardinalize_dir_or([-0.8, 0.1], [1.0, 0.0]), [-1.0, 0.0]);
    }

    #[test]
    fn directional_attack_requires_facing_cone() {
        let attacker = [0.0, 0.0];
        assert!(directional_attack_can_hit(
            attacker,
            [1.0, 0.0],
            [1.0, 0.1],
            ENEMY_REGULAR_ATTACK
        ));
        assert!(!directional_attack_can_hit(
            attacker,
            [1.0, 0.0],
            [-1.0, 0.0],
            ENEMY_REGULAR_ATTACK
        ));
    }
}

#[cfg(test)]
mod protocol_edge_tests {
    use super::*;

    #[test]
    fn decode_rejects_trailing_and_oversized_declared_collections() {
        let mut payload = encode(&ClientAck { tick: 5 });
        payload.push(0);
        assert!(decode::<ClientAck>(&payload).is_err());
        assert!(decode::<ClientMoveBundle>(&u64::MAX.to_le_bytes()).is_err());
        assert_eq!(
            decode::<ClientAck>(&encode(&ClientAck { tick: 5 }))
                .unwrap()
                .tick,
            5
        );
    }

    #[test]
    fn half_range_sequence_boundary_and_nonfinite_vectors() {
        assert!(is_newer_input_seq(0x7fff_ffff, 0));
        assert!(!is_newer_input_seq(0x8000_0000, 0));
        assert_eq!(normalize_or_zero([f32::INFINITY, 0.0]), [0.0, 0.0]);
        assert_eq!(normalize_or_zero([f32::NAN, 1.0]), [0.0, 0.0]);
    }
}

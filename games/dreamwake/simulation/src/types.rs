//! Public, renderer-independent Dreamwake state. Positions are canonical world X/Z.
use serde::{Deserialize, Serialize};

pub const TICK_HZ: u32 = 60;
pub const DT: f32 = 1.0 / TICK_HZ as f32;
/// Spacious demo arena for movement, visibility and multiplayer exercises.
pub const ARENA_RADIUS: f32 = 128.0;
pub const ARENA_FLOOR_HALF_EXTENT: f32 = ARENA_RADIUS + 2.0;
/// Demo survivability; ordinary damage, healing and death still apply.
pub const STARTING_HEALTH: f32 = 10_000.0;
/// Authored shutters populate spatial graph cells; the first is the central combat cover.
pub const DEMO_COVER_POSITIONS: [[f32; 2]; 45] = [
    [0.0, -6.0],
    [-96.0, -64.0],
    [-96.0, -32.0],
    [-96.0, 0.0],
    [-96.0, 32.0],
    [-96.0, 64.0],
    [-64.0, -96.0],
    [-64.0, -64.0],
    [-64.0, -32.0],
    [-64.0, 0.0],
    [-64.0, 32.0],
    [-64.0, 64.0],
    [-64.0, 96.0],
    [-32.0, -96.0],
    [-32.0, -64.0],
    [-32.0, -32.0],
    [-32.0, 0.0],
    [-32.0, 32.0],
    [-32.0, 64.0],
    [-32.0, 96.0],
    [0.0, -96.0],
    [0.0, -64.0],
    [0.0, -32.0],
    [0.0, 32.0],
    [0.0, 64.0],
    [0.0, 96.0],
    [32.0, -96.0],
    [32.0, -64.0],
    [32.0, -32.0],
    [32.0, 0.0],
    [32.0, 32.0],
    [32.0, 64.0],
    [32.0, 96.0],
    [64.0, -96.0],
    [64.0, -64.0],
    [64.0, -32.0],
    [64.0, 0.0],
    [64.0, 32.0],
    [64.0, 64.0],
    [64.0, 96.0],
    [96.0, -64.0],
    [96.0, -32.0],
    [96.0, 0.0],
    [96.0, 32.0],
    [96.0, 64.0],
];
/// Inclusive activation radius measured between authoritative ground positions.
pub const ENEMY_AGGRO_RANGE: f32 = 24.0;
/// Includes inactive/pending heroes because all retain authoritative combat poses.
pub const MAX_HEROES: usize = 128;
pub const MAX_ENCOUNTER_SPAWNS: usize = 32;
/// Training enemies use the same public enemy roots as encounter enemies.
pub const AMBIENT_ENEMY_POSITIONS: [[f32; 2]; 45] = {
    let mut positions = DEMO_COVER_POSITIONS;
    let mut i = 0;
    while i < positions.len() {
        positions[i][0] += 5.0;
        positions[i][1] += 5.0;
        i += 1;
    }
    positions[0] = [16.0, 16.0];
    positions
};
pub const TOTAL_ROOMS: usize = 10;
/// Dreamwake balance: stamina spent once when a valid charge releases.
pub const CHARGE_STAMINA_COST: f32 = 30.0;
/// Missing transport input holds movement/aim/attack briefly; edges never repeat.
pub const HELD_INPUT_GRACE_TICKS: u8 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct DreamInput {
    pub movement: [f32; 2],
    pub aim: [f32; 2],
    pub attack: bool,
    pub dash: bool,
    pub charge: engine_core::ChargeCommand,
    pub casts: [bool; 4],
    /// Reliable action sequence: dash first, then the four Memory slots.
    /// Zero uses the simulation tick for deterministic offline callers.
    pub action_sequences: [u32; 5],
}
impl DreamInput {
    pub fn held_only(&self) -> Self {
        Self {
            movement: self.movement,
            aim: self.aim,
            attack: self.attack,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunPhase {
    Intro,
    Combat,
    Reward,
    Rest,
    Transition,
    Victory,
    Defeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryKind {
    Crescent,
    Starfall,
    Nova,
    Blink,
    Aegis,
    Wisp,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EssenceKind {
    Twin,
    Echo,
    Vast,
    Frost,
    Leech,
    Haste,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnemyKind {
    Melee,
    Ranged,
    Ambusher,
    Support,
    Elite,
    Boss,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpgradeKind {
    Attack,
    Ability,
    Movement,
    Critical,
    Recovery,
    Health,
    Defense,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rarity {
    Common,
    Rare,
    Epic,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemorySlot {
    pub kind: MemoryKind,
    pub level: u8,
    pub essence: Option<EssenceKind>,
    pub cooldown: f32,
    pub max_cooldown: f32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RewardKind {
    Memory(MemoryKind),
    Essence(EssenceKind),
    Upgrade(UpgradeKind),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reward {
    pub kind: RewardKind,
    pub rarity: Rarity,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeroView {
    pub id: u64,
    pub position: [f32; 2],
    pub elevation: f32,
    pub crouched: bool,
    pub facing: [f32; 2],
    pub velocity: [f32; 2],
    pub hp: f32,
    pub max_hp: f32,
    pub shield: f32,
    pub level: u32,
    pub xp: f32,
    pub xp_next: f32,
    pub shards: u32,
    pub memories: [MemorySlot; 4],
    pub attack_cooldown: f32,
    pub dash_cooldown: f32,
    pub stamina: f32,
    pub max_stamina: f32,
    pub charge_ticks: u16,
    pub charge_executing: bool,
    pub charge_cooldown_ticks: u16,
    pub dreamlance_ammo: u8,
    pub dreamlance_cooldown: f32,
    pub invulnerable: bool,
    pub dashing: bool,
    pub combo: u32,
    pub hit_flash: f32,
    pub attack_flash: f32,
    pub attack_power: f32,
    pub ability_power: f32,
    pub movement_speed: f32,
    pub critical_chance: f32,
    pub recovery: f32,
    pub defense: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnemyView {
    pub id: u64,
    pub position: [f32; 2],
    pub facing: [f32; 2],
    pub hp: f32,
    pub max_hp: f32,
    pub kind: EnemyKind,
    /// Seconds remaining before the committed attack. Zero means no warning.
    pub windup: f32,
    /// The committed target stays still while the player evades the warning.
    pub target: [f32; 2],
    pub warn_radius: f32,
    pub phase: u8,
    pub slowed: bool,
    pub hit_flash: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectileView {
    pub owner: u64,
    pub id: u64,
    pub position: [f32; 2],
    pub direction: [f32; 2],
    pub radius: f32,
    pub friendly: bool,
    pub essence: Option<EssenceKind>,
}

/// Persistent summons have gameplay-owned identity and lifetime too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WispView {
    pub owner: u64,
    pub id: u64,
    pub position: [f32; 2],
    pub remaining: f32,
    pub essence: Option<EssenceKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DamageNumber {
    pub id: u64,
    pub position: [f32; 2],
    pub amount: f32,
    pub critical: bool,
    pub friendly: bool,
    pub age: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DreamSnapshot {
    pub tick: u32,
    pub seed: u64,
    pub phase: RunPhase,
    /// Zero based; room 9 is the final boss.
    pub room: usize,
    pub realm: usize,
    pub encounter_name: String,
    pub hero: HeroView,
    pub heroes: Vec<HeroView>,
    pub ready: bool,
    pub awaiting_party: bool,
    pub enemies: Vec<EnemyView>,
    pub covers: Vec<crate::replication::PublicCoverView>,
    pub platforms: Vec<crate::replication::PublicPlatformView>,
    pub projectiles: Vec<ProjectileView>,
    pub wisps: Vec<WispView>,
    /// Historical wire field name retained while graphics types are reorganized.
    pub presentations: Vec<engine_core::GraphicsInstance>,
    pub damage_numbers: Vec<DamageNumber>,
    pub rewards: Vec<Reward>,
    pub kills: u32,
    pub elapsed: f32,
    pub lucid: bool,
    pub paused: bool,
    pub cleared: u32,
    pub enemies_remaining: usize,
    pub message: String,
    pub(super) state: super::SavedState,
}

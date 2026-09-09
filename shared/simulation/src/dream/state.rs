use super::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component)]
pub(super) struct DreamOwned;
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Hero {
    pub view: HeroView,
    pub dash_left: f32,
    pub dodge_left: f32,
    pub dash_direction: [f32; 2],
    pub movement_lock: f32,
    pub shield_left: f32,
    pub ready: bool,
    pub rewards: Vec<Reward>,
    pub pending_levels: u32,
    pub attack_sequence: u32,
}
impl Default for Hero {
    fn default() -> Self {
        Self {
            view: HeroView {
                id: 1,
                position: [0.0, 2.0],
                facing: [0.0, -1.0],
                velocity: [0.0; 2],
                hp: 220.0,
                max_hp: 220.0,
                shield: 0.0,
                level: 1,
                xp: 0.0,
                xp_next: 50.0,
                shards: 0,
                memories: [
                    MemoryKind::Crescent,
                    MemoryKind::Starfall,
                    MemoryKind::Nova,
                    MemoryKind::Aegis,
                ]
                .map(MemorySlot::new),
                attack_cooldown: 0.0,
                dash_cooldown: 0.0,
                invulnerable: false,
                dashing: false,
                combo: 0,
                hit_flash: 0.0,
                attack_flash: 0.0,
                attack_power: 1.0,
                ability_power: 1.0,
                movement_speed: 7.8,
                critical_chance: 0.08,
                recovery: 1.0,
                defense: 0.0,
            },
            dash_left: 0.0,
            dodge_left: 0.0,
            dash_direction: [0.0, -1.0],
            movement_lock: 0.0,
            shield_left: 0.0,
            ready: false,
            rewards: Vec::new(),
            pending_levels: 0,
            attack_sequence: 0,
        }
    }
}
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Enemy {
    pub view: EnemyView,
    pub recovery: f32,
    pub slow_left: f32,
    pub attack_index: u32,
}
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Projectile {
    pub view: ProjectileView,
    pub speed: f32,
    pub damage: f32,
    pub lifetime: f32,
    pub pierce: u8,
    pub hit_ids: Vec<u64>,
}
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Wisp {
    pub view: WispView,
    pub damage: f32,
    pub fire_left: f32,
    pub orbit: f32,
}
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Effect(pub game_shared::PresentationInstance);
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Number(pub DamageNumber);
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct DelayedCast {
    pub action_sequence: u32,
    pub presentation_slot: u16,
    pub owner: u64,
    pub id: u64,
    pub remaining: f32,
    pub kind: MemoryKind,
    pub essence: Option<EssenceKind>,
    pub power: f32,
    pub origin: [f32; 2],
    pub direction: [f32; 2],
}
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Run {
    pub tick: u32,
    pub seed: u64,
    pub rng: u64,
    pub phase: RunPhase,
    pub room: usize,
    pub encounter_name: String,
    pub rewards: Vec<Reward>,
    pub kills: u32,
    pub elapsed: f32,
    pub lucid: bool,
    pub paused: bool,
    pub cleared: u32,
    pub next_id: u64,
    pub message: String,
    pub party_size: usize,
    pub reinforcements: u32,
}
impl Run {
    pub fn id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        ((self.rng >> 40) as f32) / (1_u32 << 24) as f32
    }
    pub fn index(&mut self, count: usize) -> usize {
        ((self.random() * count as f32) as usize).min(count.saturating_sub(1))
    }
}
#[derive(Resource, Default)]
pub(super) struct Input(pub Vec<(u64, DreamInput)>);

/// Complete authoritative continuation, including random generator and delayed casts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedState {
    pub run: Run,
    pub heroes: Vec<Hero>,
    pub enemies: Vec<Enemy>,
    pub projectiles: Vec<Projectile>,
    pub wisps: Vec<Wisp>,
    pub effects: Vec<Effect>,
    pub numbers: Vec<Number>,
    pub delayed: Vec<DelayedCast>,
}

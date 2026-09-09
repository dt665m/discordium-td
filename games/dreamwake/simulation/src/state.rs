use super::*;
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use engine_core::{CooldownOwner, Health, advance_cooldown};
use serde::{Deserialize, Serialize};

#[derive(QueryData)]
#[query_data(mutable)]
pub(super) struct HeroActor {
    pub actor: &'static mut Hero,
    pub health: &'static mut Health,
}
#[derive(QueryData)]
#[query_data(mutable)]
pub(super) struct EnemyActor {
    pub actor: &'static mut Enemy,
    pub health: &'static mut Health,
}

macro_rules! actor_access {
    ($item:ident, $readonly:ident, $actor:ty) => {
        impl std::ops::Deref for $item<'_, '_> {
            type Target = $actor;
            fn deref(&self) -> &Self::Target {
                &self.actor
            }
        }
        impl std::ops::DerefMut for $item<'_, '_> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.actor
            }
        }
        impl std::ops::Deref for $readonly<'_, '_> {
            type Target = $actor;
            fn deref(&self) -> &Self::Target {
                self.actor
            }
        }
    };
}
actor_access!(HeroActorItem, HeroActorReadOnlyItem, Hero);
actor_access!(EnemyActorItem, EnemyActorReadOnlyItem, Enemy);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedActor<T> {
    pub actor: T,
    pub health: Health,
}
impl<T> std::ops::Deref for SavedActor<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.actor
    }
}
impl<T> std::ops::DerefMut for SavedActor<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.actor
    }
}

impl CooldownOwner for Hero {
    fn tick_cooldowns(&mut self, dt: f32) {
        for timer in [
            &mut self.view.attack_cooldown,
            &mut self.view.dash_cooldown,
            &mut self.dash_left,
            &mut self.dodge_left,
            &mut self.movement_lock,
            &mut self.shield_left,
        ] {
            advance_cooldown(timer, dt);
        }
        for slot in &mut self.view.memories {
            advance_cooldown(&mut slot.cooldown, dt);
        }
    }
}
impl CooldownOwner for Enemy {
    fn tick_cooldowns(&mut self, dt: f32) {
        advance_cooldown(&mut self.recovery, dt);
        advance_cooldown(&mut self.slow_left, dt);
    }
}

#[derive(Component)]
pub(super) struct DreamOwned;
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Hero {
    pub view: HeroBody,
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
            view: HeroBody {
                id: 1,
                position: [0.0, 2.0],
                facing: [0.0, -1.0],
                velocity: [0.0; 2],
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
    pub view: EnemyBody,
    pub recovery: f32,
    pub slow_left: f32,
    pub attack_index: u32,
}
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Projectile {
    pub view: ProjectileView,
    pub previous_position: [f32; 2],
    pub speed: f32,
    pub damage: f32,
    pub lifetime: f32,
    pub pierce: u8,
    pub hit_ids: Vec<u64>,
}
impl engine_core::MovementOwner for Projectile {
    fn integrate_motion(&mut self, dt: f32) {
        self.previous_position = self.view.position;
        self.view.position = engine_core::spatial::integrate(
            self.view.position,
            engine_core::spatial::scale(self.view.direction, self.speed),
            dt,
        );
    }
}
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Wisp {
    pub view: WispView,
    pub damage: f32,
    pub fire_left: f32,
    pub orbit: f32,
}
pub(super) type Effect = engine_core::PresentationInstance;
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
    pub heroes: Vec<SavedActor<Hero>>,
    pub enemies: Vec<SavedActor<Enemy>>,
    pub projectiles: Vec<Projectile>,
    pub wisps: Vec<Wisp>,
    pub effects: Vec<Effect>,
    pub numbers: Vec<Number>,
    pub delayed: Vec<DelayedCast>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct HeroBody {
    pub id: u64,
    pub position: [f32; 2],
    pub facing: [f32; 2],
    pub velocity: [f32; 2],
    pub shield: f32,
    pub level: u32,
    pub xp: f32,
    pub xp_next: f32,
    pub shards: u32,
    pub memories: [MemorySlot; 4],
    pub attack_cooldown: f32,
    pub dash_cooldown: f32,
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
impl HeroBody {
    pub fn snapshot(&self, health: &Health) -> HeroView {
        HeroView {
            hp: health.hp,
            max_hp: health.max_hp,
            id: self.id,
            position: self.position,
            facing: self.facing,
            velocity: self.velocity,
            shield: self.shield,
            level: self.level,
            xp: self.xp,
            xp_next: self.xp_next,
            shards: self.shards,
            memories: self.memories,
            attack_cooldown: self.attack_cooldown,
            dash_cooldown: self.dash_cooldown,
            invulnerable: self.invulnerable,
            dashing: self.dashing,
            combo: self.combo,
            hit_flash: self.hit_flash,
            attack_flash: self.attack_flash,
            attack_power: self.attack_power,
            ability_power: self.ability_power,
            movement_speed: self.movement_speed,
            critical_chance: self.critical_chance,
            recovery: self.recovery,
            defense: self.defense,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct EnemyBody {
    pub id: u64,
    pub position: [f32; 2],
    pub facing: [f32; 2],
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
impl EnemyBody {
    pub fn snapshot(&self, health: &Health) -> EnemyView {
        EnemyView {
            hp: health.hp,
            max_hp: health.max_hp,
            id: self.id,
            position: self.position,
            facing: self.facing,
            kind: self.kind,
            windup: self.windup,
            target: self.target,
            warn_radius: self.warn_radius,
            phase: self.phase,
            slowed: self.slowed,
            hit_flash: self.hit_flash,
        }
    }
}

//! Game identity and policy state, composed with authoritative engine components.
use super::*;
use bevy::ecs::query::QueryData;
use serde::{Deserialize, Serialize};
mod enemies;
pub(crate) use enemies::*;
mod platform;
pub(crate) use platform::*;
mod cover;
pub(crate) use cover::*;
mod charge;
pub(crate) use charge::*;
mod ray;
pub(crate) use ray::*;
mod combat;
pub(crate) use combat::*;
mod projectiles;
pub(super) use projectiles::*;

pub(super) const CRITICAL_RANDOM_DOMAIN: u64 = 0x4352_4954;
pub(super) const FROST_STATUS: engine_core::StatusId = engine_core::StatusId(1);
pub(super) type MemoryLoadout = engine_core::Loadout<MemoryKind, EssenceKind>;
pub(super) type EquippedMemory = engine_core::AbilitySlot<MemoryKind, EssenceKind>;
pub(super) fn memory_slot(kind: MemoryKind) -> EquippedMemory {
    engine_core::AbilitySlot::new(kind, kind.cooldown())
}
fn initial_loadout() -> MemoryLoadout {
    engine_core::Loadout(
        [
            MemoryKind::Crescent,
            MemoryKind::Starfall,
            MemoryKind::Nova,
            MemoryKind::Aegis,
        ]
        .into_iter()
        .map(memory_slot)
        .collect(),
    )
}
fn memory_view(slot: &EquippedMemory) -> MemorySlot {
    MemorySlot {
        kind: slot.kind,
        level: slot.level,
        essence: slot.modifier,
        cooldown: slot.cooldown,
        max_cooldown: slot.max_cooldown,
    }
}
#[derive(QueryData)]
#[query_data(mutable)]
pub(super) struct HeroActor {
    pub charge: &'static mut ChargedMovement,
    pub ray: &'static mut RayState,
    pub actor: &'static mut Hero,
    pub health: &'static mut Health,
    pub combat: &'static mut engine_core::CombatState,
    pub combat_identity: &'static mut CombatIdentity,
    pub defense_episodes: &'static mut DefenseEpisodes,
    pub motion: &'static mut engine_core::KinematicState,
    pub motion_status: &'static mut engine_core::KinematicStatus,
    pub action: &'static mut engine_core::ActionState,
    pub loadout: &'static mut MemoryLoadout,
    pub progression: &'static mut engine_core::Progression,
}
#[derive(QueryData)]
#[query_data(mutable)]
pub(super) struct EnemyActor {
    pub ai: &'static mut EnemyAi,
    pub actor: &'static mut Enemy,
    pub health: &'static mut Health,
    pub combat: &'static mut engine_core::CombatState,
    pub combat_identity: &'static mut CombatIdentity,
    pub defense_episodes: &'static mut DefenseEpisodes,
    pub motor: &'static mut engine_core::MotorState,
    pub action: &'static mut engine_core::ActionState,
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
                &self.actor
            }
        }
    };
}
actor_access!(HeroActorItem, HeroActorReadOnlyItem, Hero);
actor_access!(EnemyActorItem, EnemyActorReadOnlyItem, Enemy);

#[derive(Component)]
pub(super) struct DreamOwned;
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(CombatIdentity, DefenseEpisodes, RayState, ChargedMovement)]
pub(super) struct Hero {
    pub view: HeroBody,
    /// Network admission is authoritative game state. Pending heroes can supply
    /// an owner checkpoint but cannot participate before initial scope decode.
    pub active: bool,
    /// Complete saved owner prediction state; never a public HeroView field.
    pub critical_rng: engine_core::system::RandomStream,
    pub ready: bool,
    pub rewards: Vec<Reward>,
}
impl Default for Hero {
    fn default() -> Self {
        Self {
            active: true,
            view: HeroBody {
                id: 1,
                shards: 0,
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
            critical_rng: engine_core::system::RandomStream::new(0, 1, 1, CRITICAL_RANDOM_DOMAIN)
                .expect("fixed nonzero hero generation"),
            ready: false,
            rewards: vec![],
        }
    }
}
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(CombatIdentity, DefenseEpisodes)]
pub(super) struct Enemy {
    pub view: EnemyBody,
    /// Pattern indexing is a game choice, distinct from engine action identity.
    pub attack_index: u32,
}
/// Snapshots save the engine components themselves, never a mutable view copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedHero {
    pub charge: ChargedMovement,
    pub ray: RayState,
    pub actor: Hero,
    pub health: Health,
    pub combat: engine_core::CombatState,
    pub combat_identity: CombatIdentity,
    pub defense_episodes: DefenseEpisodes,
    pub motion: engine_core::KinematicState,
    pub action: engine_core::ActionState,
    pub loadout: MemoryLoadout,
    pub progression: engine_core::Progression,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedEnemy {
    pub ai: EnemyAi,
    pub actor: Enemy,
    pub health: Health,
    pub combat: engine_core::CombatState,
    pub combat_identity: CombatIdentity,
    pub defense_episodes: DefenseEpisodes,
    pub motor: engine_core::MotorState,
    pub action: engine_core::ActionState,
}
macro_rules! saved_access {
    ($saved:ty, $actor:ty) => {
        impl std::ops::Deref for $saved {
            type Target = $actor;
            fn deref(&self) -> &Self::Target {
                &self.actor
            }
        }
        impl std::ops::DerefMut for $saved {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.actor
            }
        }
    };
}
saved_access!(SavedHero, Hero);
saved_access!(SavedEnemy, Enemy);
impl SavedHero {
    pub fn new(id: u64, seed: u64) -> Self {
        let mut actor = Hero::default();
        actor.view.id = id;
        actor.critical_rng =
            engine_core::system::RandomStream::new(seed, id, 1, CRITICAL_RANDOM_DOMAIN)
                .expect("fixed nonzero hero generation");
        Self {
            actor,
            charge: Default::default(),
            ray: Default::default(),
            health: Health::new(STARTING_HEALTH),
            combat: Default::default(),
            combat_identity: Default::default(),
            defense_episodes: Default::default(),
            motion: engine_core::KinematicState::new([0.0, 0.0, 2.0], 1),
            action: Default::default(),
            loadout: initial_loadout(),
            progression: engine_core::Progression {
                level: 1,
                xp: 0.0,
                xp_next: 50.0,
                pending_levels: 0,
            },
        }
    }
    pub fn spawn(self, world: &mut World) {
        world.spawn((
            DreamOwned,
            self.actor,
            self.ray,
            self.charge,
            self.health,
            self.combat,
            self.combat_identity,
            self.defense_episodes,
            self.motion,
            self.action,
            self.loadout,
            self.progression,
        ));
    }
    pub fn snapshot(&self) -> HeroView {
        HeroView {
            id: self.view.id,
            position: crate::collision::planar_position(&self.motion),
            elevation: self.motion.position[1],
            crouched: self.motion.stance == engine_core::Stance::Crouched,
            facing: self.motion.facing,
            velocity: crate::collision::planar_velocity(&self.motion),
            hp: self.health.hp,
            max_hp: self.health.max_hp,
            shield: self.combat.shield,
            level: self.progression.level,
            xp: self.progression.xp,
            xp_next: self.progression.xp_next,
            shards: self.view.shards,
            memories: std::array::from_fn(|i| memory_view(&self.loadout.0[i])),
            attack_cooldown: self.action.recovery,
            stamina: self.charge.stamina.current(),
            max_stamina: self.charge.stamina.capacity(),
            charge_ticks: match self.charge.state.phase {
                engine_core::ChargePhase::Charging { ticks } => ticks,
                _ => 0,
            },
            charge_executing: matches!(
                self.charge.state.phase,
                engine_core::ChargePhase::Executing { .. }
            ),
            charge_cooldown_ticks: self.charge.state.cooldown_ticks,
            dreamlance_ammo: self.ray.ammo.current() as u8,
            dreamlance_cooldown: self.ray.cooldown,
            dash_cooldown: f32::from(self.motion.dash_cooldown_ticks) * DT,
            invulnerable: self.combat.is_invulnerable() || self.motion.dash_ticks > 0,
            dashing: self.motion.dash_ticks > 0 && self.health.hp > 0.0,
            combo: self.view.combo,
            hit_flash: self.view.hit_flash,
            attack_flash: self.view.attack_flash,
            attack_power: self.view.attack_power,
            ability_power: self.view.ability_power,
            movement_speed: self.view.movement_speed,
            critical_chance: self.view.critical_chance,
            recovery: self.view.recovery,
            defense: self.view.defense,
        }
    }
}
impl SavedEnemy {
    pub fn spawn(self, world: &mut World) {
        world.spawn((
            DreamOwned,
            self.ai,
            self.actor,
            self.health,
            self.combat,
            self.combat_identity,
            self.defense_episodes,
            self.motor,
            self.action,
        ));
    }
    pub fn snapshot(&self) -> EnemyView {
        EnemyView {
            id: self.view.id,
            position: self.motor.position,
            facing: self.motor.facing,
            hp: self.health.hp,
            max_hp: self.health.max_hp,
            kind: self.view.kind,
            windup: self.action.windup,
            target: self.action.target,
            warn_radius: self.view.warn_radius,
            phase: self.view.phase,
            slowed: self.combat.status(FROST_STATUS).is_some(),
            hit_flash: self.view.hit_flash,
        }
    }
}
pub(super) type Effect = engine_core::GraphicsInstance;
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Number(pub DamageNumber);
pub(super) type DelayedCast = engine_core::DelayedAction<CastPayload>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct CastPayload {
    pub spawn: Option<(crate::starfall::SpawnKey, u32)>,
    pub action_sequence: u32,
    pub presentation_slot: u16,
    pub owner: u64,
    pub id: u64,
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
    pub encounter_spawns: usize,
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
pub(super) type Input = DreamInputs;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedState {
    pub combat_history: crate::combat_history::CombatArchive,
    pub collision: crate::collision::CollisionManifest,
    pub run: Run,
    pub heroes: Vec<SavedHero>,
    pub enemies: Vec<SavedEnemy>,
    pub covers: Vec<SavedCover>,
    pub platforms: Vec<SavedPlatform>,
    pub projectiles: Vec<SavedProjectile>,
    pub wisps: Vec<SavedWisp>,
    pub effects: Vec<Effect>,
    pub numbers: Vec<Number>,
    pub delayed: Vec<DelayedCast>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct HeroBody {
    pub id: u64,
    pub shards: u32,
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
pub(super) struct EnemyBody {
    pub id: u64,
    pub kind: EnemyKind,
    pub warn_radius: f32,
    pub phase: u8,
    pub hit_flash: f32,
}

//! Shared Bevy ECS simulation for server authority and isolated client prediction.
pub mod dream;
pub mod plugins;
mod systems;
pub use plugins::{SimulationPlugins, SimulationTick};
mod facade;
mod resources;
mod restore;
mod rules;
mod snapshot;
mod storage;
mod tuning;
pub use facade::Simulation;
use resources::*;
use rules::*;
use tuning::*;
pub mod components;
use components::*;
pub mod world;
use bevy::prelude::*;
use std::{
    collections::{BTreeMap, VecDeque},
    f32::consts::TAU,
};
pub use storage::SimulationOwned;
use storage::*;

use game_shared::{
    AbilityId, AttackPhase, BASE_ENEMIES_PER_WAVE, BASE_POSITION, BUILD_COMMAND_MAX_DISTANCE,
    BUILD_NODES, BuildRejectReason, ChargePhase, ChargeProfileComponent, ChargeStateComponent,
    ClientCommand, DirectionalAttackComponent, DirectionalAttackStateComponent,
    ENEMY_OBJECTIVE_DAMAGE, ENEMY_REGULAR_ATTACK, ENEMY_SPAWN_POINTS, EXTRA_ENEMIES_PER_WAVE,
    EnemyLockTarget, EnemyType, FIXED_DT_SECONDS, FacingComponent, HERO_ABILITY_COOLDOWN_TICKS,
    HERO_ABILITY_DAMAGE, HERO_ABILITY_MANA_COST, HERO_ABILITY_RADIUS, HERO_CHARGE_PROFILE,
    HERO_COLLIDER_RADIUS, HERO_MANA_REGEN_PER_TICK, HERO_MAX_HP, HERO_MAX_MANA,
    HERO_POWER_DECAY_PROFILE, HERO_POWERED_MODIFIERS, HERO_REGULAR_ATTACK, HERO_SPEED,
    INITIAL_GOLD, INITIAL_TEAM_LIFE, JoinSnapshot, MATCH_RESET_TICKS, MAX_WAVES, MatchPhase,
    OBJECTIVE_COLLIDER_RADIUS, OBJECTIVE_MAX_HP, PowerDecayProfileComponent,
    PoweredUpModifiersComponent, ReliableGameEvent, SimMeta, TOWER_BUILD_COST,
    TOWER_COLLIDER_RADIUS, TOWER_DAMAGE, TOWER_RANGE, TOWER_RELOAD_TICKS, TowerType,
    WAVE_PREP_TICKS, WAVE_SPAWN_INTERVAL_TICKS, WORLD_HALF_HEIGHT, WORLD_HALF_WIDTH, WorldDelta,
    cardinalize_dir_or, clamp_to_world, directional_attack_can_hit, distance_sq,
    enemy_collider_radius, is_newer_input_seq, normalize_or_zero,
};
use vleue_navigator::NavMesh;

#[derive(Debug, Clone)]
pub struct TickOutput {
    pub reliable_events: Vec<ReliableGameEvent>,
}

#[cfg(test)]
use game_shared::TowerSnapshot;
#[cfg(test)]
mod tests;

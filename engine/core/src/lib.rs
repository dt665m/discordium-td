//! Renderer-independent deterministic simulation plugins and components.
pub mod plugins;
pub use abilities::{
    AbilitiesPlugin, AbilitySlot, ActionPlugin, ActionState, ActionStep, DelayedAction,
    DelayedActionPlugin, DelayedActionStep, Loadout, LoadoutPlugin, LoadoutStep, tick_actions,
    tick_delayed_actions, tick_loadouts,
};
pub use combat::companions::{CompanionPlugin, CompanionState, CompanionStep, tick_companions};
pub use combat::projectiles::{
    ProjectileImpact, ProjectilePlugin, ProjectileSourcePolicy, ProjectileState, ProjectileStep,
    advance_projectiles, tick_projectiles,
};
pub use combat::targeting::*;
pub use combat::{
    CombatEffectsPlugin, CombatPlugin, CombatState, CombatStep, DamageOutcome, Depleted,
    DurationPolicy, Health, HealthPlugin, HealthStep, Meter, MeterPlugin, MeterStep, Regeneration,
    ShieldPolicy, StatusId, TimedStatus, regenerate_meters, resolve_damage, tick_combat,
    update_health,
};
pub use graphics::{
    GraphicsId, GraphicsInstance, GraphicsKind, GraphicsPlugin, GraphicsStep, age_graphics,
};
pub use physics::{
    CircleBounds, MotorPlugin, MotorState, MotorStep, PhysicsPlugin, spatial, tick_motors,
};
pub use plugins::{abilities, combat, graphics, physics, progression, spawn, system};
pub use progression::{
    ExperienceResult, PendingExperience, Progression, ProgressionError, ProgressionPlugin,
    ProgressionStep, add_capped, multiply_capped, multiply_floored, purchase_rank, spend_currency,
};
pub use spawn::{
    DespawnWithOwner, Lifetime, OwnedSpawns, SpawnPlugin, SpawnStep, expire_lifetimes,
};
pub use system::{SimulationStep, SystemPlugin, advance_cooldown};

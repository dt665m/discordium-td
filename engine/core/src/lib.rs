//! Renderer-independent deterministic simulation plugins and components.
include!(concat!(env!("OUT_DIR"), "/source_identity.rs"));
pub mod plugins;
pub use abilities::{
    AbilitiesPlugin, AbilitySlot, ActionPlugin, ActionState, ActionStep, ChargeCommand,
    ChargePhase, ChargeState, DelayedAction, DelayedActionPlugin, DelayedActionStep, Loadout,
    LoadoutPlugin, LoadoutStep, MotionCurveKey, authored_motion_delta, tick_actions,
    tick_delayed_actions, tick_loadouts,
};
pub use combat::companions::{CompanionPlugin, CompanionState, CompanionStep, tick_companions};
pub use combat::history::{
    CombatPose, HistoryError, HistoryLimits, Hit, HitFrame, HitHistory, HitMetadata, HitQuery,
    QueryBudget,
};
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
    GraphicsId, GraphicsInstance, GraphicsKind, GraphicsPlugin, GraphicsScope, GraphicsStep,
    age_graphics,
};
pub use physics::kinematic::{
    BaseAttachment, BaseConfig, BaseFrame, BasePose, BaseSample, BaseState, BaseStep, ColliderKey,
    CollisionScene, CollisionShape, GroundContact, KinematicConfig, KinematicError, KinematicInput,
    KinematicPlugin, KinematicReport, KinematicState, KinematicStatus, KinematicStep,
    MAX_MOVING_BASES, MAX_SCENE_QUERY_RESULTS, MAX_SCENE_QUERY_TESTS, SceneCastHit,
    SceneQueryBudget, Stance, StaticCollider, advance_kinematic,
    advance_kinematic_with_authored_motion, advance_kinematic_with_bases, validate_kinematic_state,
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

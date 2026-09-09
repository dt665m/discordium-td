//! The same headless plugin is composed by authority and client prediction.
use super::*;
use bevy::ecs::schedule::ScheduleLabel;

/// Supply one authenticated input per player, then run `DreamStep` once.
#[derive(Resource, Default, Clone)]
pub struct DreamInputs(pub Vec<(u64, DreamInput)>);
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DreamStep;

/// Public integration points in one deterministic, explicitly stepped schedule.
/// `Active` and `Combat` are conditional parents, not sequential stages.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DreamSystems {
    Active,
    Combat,
    Graphics,
    Inputs,
    ActorTimers,
    Begin,
    Ambient,
    Players,
    Delays,
    DelayedCasts,
    CompanionTargets,
    Companions,
    CompanionActions,
    Enemies,
    ProjectileTargets,
    Projectiles,
    ProjectileHits,
    Health,
    Deaths,
    Finish,
    FinalHealth,
}

/// Game rules and engine gameplay plugins, without transport, graphics or audio.
/// The explicit step supports replay without consulting wall-clock time.
#[derive(Debug, Clone, Copy)]
pub struct DreamwakePlugin {
    pub seed: u64,
    pub lucid: bool,
}
impl DreamwakePlugin {
    pub fn new(seed: u64, lucid: bool) -> Self {
        Self { seed, lucid }
    }
}
pub(super) fn initialize(app: &mut App, seed: u64, lucid: bool) {
    app.add_plugins(engine_core::SystemPlugin::new(DT))
        .insert_resource(DreamInputs::default())
        .insert_resource(Run {
            tick: 0,
            seed,
            rng: seed.max(1),
            phase: RunPhase::Intro,
            room: 0,
            encounter_name: "The dream is waiting".into(),
            rewards: vec![],
            kills: 0,
            elapsed: 0.0,
            lucid,
            paused: false,
            cleared: 0,
            next_id: 1 << 63,
            message: "Vesper · Moonbound".into(),
            party_size: 1,
            reinforcements: 0,
        });
    SavedHero::new(1).spawn(app.world_mut());
}
impl Plugin for DreamwakePlugin {
    fn build(&self, app: &mut App) {
        use DreamSystems as D;
        initialize(app, self.seed, self.lucid);
        app.init_schedule(DreamStep)
            .add_plugins((
                engine_core::GraphicsPlugin(DreamStep),
                engine_core::CombatPlugin::new(DreamStep).with_collision_candidates(false),
                engine_core::AbilitiesPlugin(DreamStep),
                engine_core::PhysicsPlugin(DreamStep),
                engine_core::LoadoutPlugin::<MemoryKind, EssenceKind, _>::new(DreamStep),
                engine_core::DelayedActionPlugin::<CastPayload, _>::new(DreamStep),
                engine_core::ProgressionPlugin::new(DreamStep, catalog::experience_threshold),
                engine_core::SpawnPlugin(DreamStep),
            ))
            // Set conditions are cached once per schedule run: a clear/death may
            // change Run.phase, but the final health sync must still execute.
            .configure_sets(
                DreamStep,
                (
                    D::Active.run_if(is_active),
                    D::Combat.in_set(D::Active).run_if(is_combat),
                    D::Graphics.in_set(D::Active),
                    D::Ambient.in_set(D::Active),
                ),
            )
            .configure_sets(
                DreamStep,
                (
                    D::Inputs,
                    D::ActorTimers,
                    D::Begin,
                    D::Players,
                    D::Delays,
                    D::DelayedCasts,
                    D::CompanionTargets,
                    D::Companions,
                    D::CompanionActions,
                    D::Enemies,
                    D::ProjectileTargets,
                    D::Projectiles,
                    D::ProjectileHits,
                    D::Health,
                    D::Deaths,
                    D::Finish,
                    D::FinalHealth,
                )
                    .in_set(D::Combat),
            )
            // Chain edges retain Bevy's automatic ApplyDeferred barriers: casts,
            // summons, bolts and depletion markers become visible to consumers.
            .configure_sets(
                DreamStep,
                (
                    D::Graphics,
                    D::Inputs,
                    D::ActorTimers,
                    D::Begin,
                    D::Ambient,
                    D::Players,
                    D::Delays,
                    D::DelayedCasts,
                    D::CompanionTargets,
                    D::Companions,
                    D::CompanionActions,
                    D::Enemies,
                    D::ProjectileTargets,
                    D::Projectiles,
                    D::ProjectileHits,
                    D::Health,
                    D::Deaths,
                    D::Finish,
                    D::FinalHealth,
                )
                    .chain(),
            )
            .configure_sets(
                DreamStep,
                (
                    engine_core::GraphicsStep.in_set(D::Graphics),
                    engine_core::HealthStep.in_set(D::Health),
                    engine_core::CombatStep.in_set(D::ActorTimers),
                    engine_core::ActionStep.in_set(D::ActorTimers),
                    engine_core::MotorStep.in_set(D::ActorTimers),
                    engine_core::LoadoutStep.in_set(D::ActorTimers),
                    engine_core::DelayedActionStep.in_set(D::Delays),
                    engine_core::CompanionStep.in_set(D::Companions),
                    engine_core::ProjectileStep.in_set(D::Projectiles),
                    engine_core::SpawnStep.in_set(D::ActorTimers),
                    engine_core::ProgressionStep
                        .in_set(D::Deaths)
                        .after(systems::resolve_deaths),
                ),
            )
            .add_systems(
                DreamStep,
                (
                    canonicalize_inputs.in_set(D::Inputs),
                    systems::begin_tick.in_set(D::Begin),
                    systems::age_transients.in_set(D::Ambient),
                    systems::player_actions.in_set(D::Players),
                    systems::delayed_casts.in_set(D::DelayedCasts),
                    systems::refresh_target_index.in_set(D::CompanionTargets),
                    systems::wisp_actions.in_set(D::CompanionActions),
                    systems::enemy_actions.in_set(D::Enemies),
                    systems::refresh_target_index.in_set(D::ProjectileTargets),
                    systems::projectile_actions.in_set(D::ProjectileHits),
                    systems::resolve_deaths.in_set(D::Deaths),
                    systems::finish_encounter.in_set(D::Finish),
                    engine_core::update_health.in_set(D::FinalHealth),
                ),
            );
    }
}
fn is_active(run: Res<Run>) -> bool {
    !run.paused && run.phase != RunPhase::Intro && run.party_size > 0
}
fn is_combat(run: Res<Run>) -> bool {
    run.phase == RunPhase::Combat
}
fn canonicalize_inputs(mut inputs: ResMut<DreamInputs>) {
    inputs.0.sort_by_key(|(id, _)| *id);
    inputs.0.dedup_by_key(|(id, _)| *id);
}

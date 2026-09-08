//! The same simulation plugins run in the server host and isolated prediction worlds.
use super::*;
use bevy::app::PluginGroupBuilder;

#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SimulationTick;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SimulationSet {
    Begin,
    Reset,
    Active,
    Input,
    PrepareMovement,
    EnemyTargets,
    EnemyMovement,
    EnemyResolution,
    HeroTargets,
    HeroMovement,
    HeroResolution,
    TowerTargets,
    TowerAttacks,
    TowerResolution,
    Spawn,
    Finish,
}

pub struct SimulationPlugins;
impl PluginGroup for SimulationPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(game_replication::NetIdentityPlugin)
            .add(SimulationCorePlugin)
            .add(InputPlugin)
            .add(MovementPlugin)
            .add(CombatPlugin)
            .add(LifecyclePlugin)
            .add(PresentationPlugin)
    }
}

pub struct SimulationCorePlugin;
impl Plugin for SimulationCorePlugin {
    fn build(&self, app: &mut App) {
        use SimulationSet::*;
        if bevy::tasks::ComputeTaskPool::try_get().is_none() {
            bevy::app::TaskPoolOptions::default().create_default_pools();
        }
        app.init_resource::<Globals>()
            .init_resource::<Epoch>()
            .init_resource::<TickEvents>()
            .init_resource::<CommandInbox>()
            .init_resource::<HistoricalTargets>()
            .init_resource::<NavigationCache>()
            .init_resource::<snapshot::SnapshotQueries>()
            .init_resource::<systems::lifecycle::TickStatus>()
            .init_schedule(SimulationTick);
        // Inter-phase dependencies are ordered. Within heavy phases, query
        // iteration uses the compute pool without dispatching every tiny phase.
        app.edit_schedule(SimulationTick, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::default());
        });
        app.configure_sets(SimulationTick, (Begin, Reset, Active).chain());
        app.configure_sets(SimulationTick, Active.run_if(systems::lifecycle::running));
        app.configure_sets(
            SimulationTick,
            (
                Input,
                PrepareMovement,
                EnemyTargets,
                EnemyMovement,
                EnemyResolution,
                HeroTargets,
                HeroMovement,
                HeroResolution,
                TowerTargets,
                TowerAttacks,
                TowerResolution,
                Spawn,
                Finish,
            )
                .chain()
                .in_set(Active),
        );
    }
}

pub struct InputPlugin;
impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            SimulationTick,
            systems::input::ingest.in_set(SimulationSet::Input),
        );
        app.add_systems(
            SimulationTick,
            systems::input::consume_moves.in_set(SimulationSet::PrepareMovement),
        );
    }
}

pub struct MovementPlugin;
impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<systems::movement::EnemyFrame>()
            .init_resource::<systems::movement::HeroFrame>();
        app.add_systems(
            SimulationTick,
            systems::movement::update_navigation.in_set(SimulationSet::PrepareMovement),
        );
        app.add_systems(
            SimulationTick,
            systems::movement::prepare_enemies.in_set(SimulationSet::EnemyTargets),
        );
        app.add_systems(
            SimulationTick,
            systems::movement::advance_enemies.in_set(SimulationSet::EnemyMovement),
        );
        app.add_systems(
            SimulationTick,
            systems::movement::prepare_heroes.in_set(SimulationSet::HeroTargets),
        );
        app.add_systems(
            SimulationTick,
            systems::movement::advance_heroes.in_set(SimulationSet::HeroMovement),
        );
    }
}

pub struct CombatPlugin;
impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<systems::combat::TowerTargets>();
        app.add_systems(
            SimulationTick,
            systems::combat::resolve_enemies.in_set(SimulationSet::EnemyResolution),
        );
        app.add_systems(
            SimulationTick,
            systems::combat::resolve_heroes.in_set(SimulationSet::HeroResolution),
        );
        app.add_systems(
            SimulationTick,
            systems::combat::prepare_towers.in_set(SimulationSet::TowerTargets),
        );
        app.add_systems(
            SimulationTick,
            systems::combat::advance_towers.in_set(SimulationSet::TowerAttacks),
        );
        app.add_systems(
            SimulationTick,
            systems::combat::resolve_towers.in_set(SimulationSet::TowerResolution),
        );
    }
}

pub struct LifecyclePlugin;
impl Plugin for LifecyclePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            SimulationTick,
            systems::lifecycle::begin.in_set(SimulationSet::Begin),
        );
        app.add_systems(
            SimulationTick,
            systems::lifecycle::reset_if_finished.in_set(SimulationSet::Reset),
        );
        app.add_systems(
            SimulationTick,
            systems::lifecycle::spawn.in_set(SimulationSet::Spawn),
        );
        app.add_systems(
            SimulationTick,
            systems::lifecycle::finish.in_set(SimulationSet::Finish),
        );
    }
}

pub struct PresentationPlugin;
impl Plugin for PresentationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<systems::presentation::PresentationEntities>();
        app.add_systems(
            SimulationTick,
            systems::presentation::age.in_set(SimulationSet::Begin),
        );
    }
}

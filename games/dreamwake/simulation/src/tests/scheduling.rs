use crate::plugin::initialize;
use crate::*;
use bevy::ecs::schedule::ScheduleLabel;

/// The previous production composition, retained only as a differential oracle.
/// It deliberately uses independent schedules and exclusive step wrappers.
mod nested_reference {
    use super::*;
    #[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
    enum Old {
        Game,
        Ambient,
        Graphics,
        Actors,
        Health,
        Delay,
        Target,
        Summon,
        Projectile,
    }
    pub fn simulation(seed: u64, lucid: bool) -> DreamSimulation {
        let mut app = App::new();
        initialize(&mut app, seed, lucid);
        app.init_schedule(DreamStep);
        for schedule in [
            Old::Game,
            Old::Ambient,
            Old::Graphics,
            Old::Actors,
            Old::Health,
            Old::Delay,
            Old::Target,
            Old::Summon,
            Old::Projectile,
        ] {
            app.init_schedule(schedule);
        }
        app.add_plugins((
            engine_core::GraphicsPlugin(Old::Graphics),
            engine_core::HealthPlugin(Old::Health),
            engine_core::CombatEffectsPlugin(Old::Actors),
            engine_core::ActionPlugin(Old::Actors),
            engine_core::MotorPlugin(Old::Actors),
            engine_core::LoadoutPlugin::<MemoryKind, EssenceKind, _>::new(Old::Actors),
            engine_core::DelayedActionPlugin::<CastPayload, _>::new(Old::Delay),
            engine_core::CompanionPlugin::new(Old::Summon),
            engine_core::ProjectilePlugin::new(Old::Projectile).with_collision_candidates(false),
        ))
        .add_systems(DreamStep, advance)
        .add_systems(Old::Ambient, systems::age_transients)
        .add_systems(Old::Actors, systems::tick_dreamlance)
        .add_systems(Old::Target, systems::refresh_target_index)
        .add_systems(
            Old::Game,
            (
                systems::begin_tick,
                crate::platform::advance_platforms,
                systems::age_transients,
                systems::player_actions,
                delays,
                systems::delayed_casts,
                summons,
                systems::wisp_actions,
                systems::enemy_actions,
                projectiles,
                systems::advance_covers,
                systems::capture_combat_poses,
                systems::projectile_actions,
                systems::ray_actions,
                systems::resolve_combat_batch,
                crate::combat_trace::finalize_combat_traces,
                health,
                systems::resolve_deaths,
                systems::finish_encounter,
                systems::beam_graphics,
            )
                .chain(),
        );
        DreamSimulation::from_app(app)
    }
    fn delays(world: &mut World) {
        world.run_schedule(Old::Delay);
    }
    fn summons(world: &mut World) {
        world.run_schedule(Old::Target);
        world.run_schedule(Old::Summon);
    }
    fn projectiles(world: &mut World) {
        world.run_schedule(Old::Target);
        world.run_schedule(Old::Projectile);
    }
    fn health(world: &mut World) {
        world.run_schedule(Old::Health);
    }
    fn advance(world: &mut World) {
        let run = world.resource::<Run>();
        if run.paused || run.phase == RunPhase::Intro || run.party_size == 0 {
            return;
        }
        let phase = run.phase;
        world.run_schedule(Old::Graphics);
        if phase != RunPhase::Combat {
            world.run_schedule(Old::Ambient);
            return;
        }
        let mut inputs = world.resource_mut::<DreamInputs>();
        inputs.0.sort_by_key(|(id, _)| *id);
        inputs.0.dedup_by_key(|(id, _)| *id);
        world.run_schedule(Old::Actors);
        world.run_schedule(Old::Game);
        world.run_schedule(Old::Health);
    }
}

fn compare_step(
    flat: &mut DreamSimulation,
    old: &mut DreamSimulation,
    inputs: &[(u64, DreamInput)],
) {
    flat.step_multiplayer(inputs);
    old.step_multiplayer(inputs);
    assert_eq!(flat.snapshot(), old.snapshot());
}

#[test]
fn schedule_build_rejects_any_unordered_conflicting_systems() {
    use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
    let mut app = App::new();
    app.add_plugins(DreamwakePlugin::new(43, false));
    app.edit_schedule(DreamStep, |schedule| {
        schedule.set_build_settings(ScheduleBuildSettings {
            ambiguity_detection: LogLevel::Error,
            ..Default::default()
        });
    });
    let mut simulation = DreamSimulation::from_app(app);
    simulation.continue_run();
    simulation.step(DreamInput::default());
}

#[test]
fn flattened_schedule_matches_previous_nested_schedule_through_replay_and_clear() {
    let mut flat = DreamSimulation::new(43, true);
    let mut old = nested_reference::simulation(43, true);
    compare_step(&mut flat, &mut old, &[]); // Intro freezes all schedules.
    for simulation in [&mut flat, &mut old] {
        simulation.add_player(22);
        simulation.continue_run_for(1);
        simulation.continue_run_for(22);
        let world = simulation.world_mut();
        for mut hero in world.query::<HeroActor>().iter_mut(world) {
            hero.loadout.0[0] = memory_slot(MemoryKind::Blink);
            hero.loadout.0[0].modifier = Some(EssenceKind::Echo);
            hero.loadout.0[1] = memory_slot(MemoryKind::Wisp);
            hero.loadout.0[1].modifier = Some(EssenceKind::Twin);
            hero.loadout.0[2].modifier = Some(EssenceKind::Vast);
            hero.health.hp = 5000.0;
            hero.health.max_hp = 5000.0;
        }
    }
    for tick in 0..180_u32 {
        let input = DreamInput {
            charge: Default::default(),
            movement: [0.6, -0.2],
            aim: [0.0, -1.0],
            attack: true,
            dash: tick.is_multiple_of(80),
            casts: [
                tick.is_multiple_of(50),
                tick.is_multiple_of(30),
                tick.is_multiple_of(60),
                false,
            ],
            action_sequences: [tick + 1; 5],
        };
        compare_step(
            &mut flat,
            &mut old,
            &[(22, input), (1, input), (22, DreamInput::default())],
        );
        if tick == 90 {
            let bytes = serde_json::to_vec(&flat.snapshot()).unwrap();
            let saved = serde_json::from_slice(&bytes).unwrap();
            flat.restore(&saved);
            old.restore(&saved);
        }
    }
    // The first health pass observes a fallen teammate. Clear revives them, so
    // the second pass must remove Depleted despite Combat changing to Reward.
    for simulation in [&mut flat, &mut old] {
        let world = simulation.world_mut();
        let enemies: Vec<_> = world
            .query_filtered::<Entity, With<Enemy>>()
            .iter(world)
            .collect();
        for entity in enemies {
            world.despawn(entity);
        }
        world.resource_mut::<Run>().reinforcements = 0;
        for mut hero in world.query::<HeroActor>().iter_mut(world) {
            if hero.view.id == 22 {
                hero.health.hp = 0.0;
            }
        }
    }
    compare_step(&mut flat, &mut old, &[]);
    assert_eq!(flat.snapshot().phase, RunPhase::Reward);
    for simulation in [&mut flat, &mut old] {
        let world = simulation.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, (With<Hero>, With<engine_core::Depleted>)>()
                .iter(world)
                .count(),
            0
        );
        world.resource_mut::<Run>().phase = RunPhase::Combat;
        for mut hero in world.query::<HeroActor>().iter_mut(world) {
            hero.health.hp = 0.0;
        }
    }
    compare_step(&mut flat, &mut old, &[]);
    assert_eq!(flat.snapshot().phase, RunPhase::Defeat);
}

#[test]
fn ambient_and_frozen_phases_match_previous_schedule() {
    for (phase, paused, party_size) in [
        (RunPhase::Intro, false, 1),
        (RunPhase::Rest, false, 1),
        (RunPhase::Reward, false, 1),
        (RunPhase::Transition, false, 1),
        (RunPhase::Victory, false, 1),
        (RunPhase::Defeat, false, 1),
        (RunPhase::Combat, true, 1),
        (RunPhase::Combat, false, 0),
    ] {
        let mut flat = DreamSimulation::new(9, false);
        let mut old = nested_reference::simulation(9, false);
        for simulation in [&mut flat, &mut old] {
            let world = simulation.world_mut();
            let mut run = world.resource_mut::<Run>();
            run.phase = phase;
            run.paused = paused;
            run.party_size = party_size;
            world.spawn((
                DreamOwned,
                engine_core::GraphicsInstance {
                    id: engine_core::GraphicsId {
                        scope: None,
                        match_epoch: 9,
                        owner: 1,
                        action_seq: 7,
                        slot: 1,
                    },
                    kind: engine_core::GraphicsKind::RadialPulse,
                    pos: [0.0; 2],
                    radius: 1.0,
                    age_ticks: 0,
                    duration_ticks: 2,
                },
            ));
            for mut hero in world.query::<HeroActor>().iter_mut(world) {
                hero.action.recovery = 2.0;
                hero.view.hit_flash = 1.0;
            }
        }
        for _ in 0..3 {
            compare_step(&mut flat, &mut old, &[]);
        }
        let snapshot = flat.snapshot();
        assert_eq!(snapshot.tick, 0);
        assert_eq!(snapshot.hero.attack_cooldown, 2.0);
        let frozen = paused || party_size == 0 || phase == RunPhase::Intro;
        assert_eq!(snapshot.presentations.len(), usize::from(frozen));
    }
}

#[test]
fn consuming_an_app_and_restoring_preserves_unrelated_ecs_state() {
    let mut app = App::new();
    app.add_plugins(DreamwakePlugin::new(9, false));
    let unrelated = app.world_mut().spawn(Health::new(12.0)).id();
    let mut simulation = DreamSimulation::from_app(app);
    simulation.continue_run();
    let saved = simulation.snapshot();
    simulation.step(DreamInput::default());
    simulation.restore(&saved);
    assert_eq!(
        simulation.world().get::<Health>(unrelated).unwrap().hp,
        12.0
    );
    assert_eq!(simulation.snapshot(), saved);
}

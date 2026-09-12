use crate::*;
use bevy::ecs::system::RunSystemOnce;

fn enqueue_strike(
    In(owner): In<u64>,
    run: Res<Run>,
    mut batch: ResMut<systems::CombatBatch>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<EnemyActor, Without<Hero>>,
) {
    let hero = heroes
        .iter_mut()
        .find(|hero| hero.view.id == owner)
        .unwrap();
    let enemy = enemies.single_mut().unwrap();
    systems::hit_enemy(
        &mut batch,
        systems::CombatActionKey::new(
            run.tick,
            owner,
            hero.combat_identity.generation,
            0,
            u64::from(run.tick),
            0,
        ),
        &hero,
        &enemy,
        1.0,
        None,
        [0.0; 2],
        false,
    );
}
fn strike(In(owner): In<u64>, world: &mut World) -> f32 {
    let before = world
        .query_filtered::<&Health, With<Enemy>>()
        .single(world)
        .unwrap()
        .hp;
    world.run_system_once(systems::begin_tick).unwrap();
    world
        .run_system_once(crate::platform::advance_platforms)
        .unwrap();
    world.run_system_once_with(enqueue_strike, owner).unwrap();
    world
        .run_system_once(systems::capture_combat_poses)
        .unwrap();
    world
        .run_system_once(systems::resolve_combat_batch)
        .unwrap();
    before
        - world
            .query_filtered::<&Health, With<Enemy>>()
            .single(world)
            .unwrap()
            .hp
}
fn arena() -> DreamSimulation {
    let mut sim = DreamSimulation::new(42, false);
    sim.add_player(2);
    sim.world.resource_scope(|world, mut run: Mut<Run>| {
        let mut queue = bevy::ecs::world::CommandQueue::default();
        encounters::spawn_enemy(
            &mut Commands::new(&mut queue, world),
            &mut run,
            EnemyKind::Boss,
            [0.0; 2],
        );
        queue.apply(world);
    });
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.view.critical_chance = 0.5;
    }
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        *enemy.health = Health::new(10_000.0);
    }
    sim
}

#[test]
fn critical_hits_do_not_depend_on_encounter_or_other_hero_draws() {
    let mut baseline = arena();
    let mut perturbed = DreamSimulation::from_snapshot(&baseline.snapshot());
    for _ in 0..40 {
        for _ in 0..7 {
            perturbed.world.resource_mut::<Run>().random();
        }
        perturbed.world.run_system_once_with(strike, 2).unwrap();
        let a = baseline.world.run_system_once_with(strike, 1).unwrap();
        let b = perturbed.world.run_system_once_with(strike, 1).unwrap();
        assert!((a - b).abs() < 0.001);
    }
    let a = baseline.snapshot();
    let b = perturbed.snapshot();
    assert_eq!(
        a.state.heroes[0].critical_rng,
        b.state.heroes[0].critical_rng
    );
    assert_eq!(a.state.heroes[0].critical_rng.counter(), 40);
    assert_eq!(a.state.heroes[1].critical_rng.counter(), 0);
    assert_eq!(b.state.heroes[1].critical_rng.counter(), 40);
}

#[test]
fn restore_retains_owner_random_counter_and_rejects_foreign_stream() {
    let mut sim = arena();
    for _ in 0..5 {
        sim.world.run_system_once_with(strike, 1).unwrap();
    }
    let before = sim.snapshot();
    let bytes = DreamCheckpoint::new(before.clone(), 1)
        .unwrap()
        .encode()
        .unwrap();
    let decoded = DreamCheckpoint::decode(&bytes, 1).unwrap();
    let mut replay = DreamSimulation::try_from_checkpoint(&decoded, 1).unwrap();
    for _ in 0..20 {
        assert_eq!(
            sim.world.run_system_once_with(strike, 1).unwrap(),
            replay.world.run_system_once_with(strike, 1).unwrap()
        );
    }
    assert_eq!(sim.snapshot(), replay.snapshot());
    let mut invalid = before.clone();
    invalid.state.heroes[0].critical_rng = invalid.state.heroes[1].critical_rng.clone();
    let expected = sim.snapshot();
    assert!(sim.try_restore(&invalid).is_err());
    assert_eq!(sim.snapshot(), expected);
}

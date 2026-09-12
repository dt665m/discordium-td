use crate::*;
use bevy::ecs::system::RunSystemOnce;

pub(super) fn arena(enemy_count: usize) -> DreamSimulation {
    let mut sim = DreamSimulation::new(73, false);
    sim.continue_run();
    // Local combat fixtures isolate the original shutter; full-world coverage
    // exercises all authored spatial props separately.
    let extra_covers = sim
        .world
        .query::<(Entity, &Cover)>()
        .iter(&sim.world)
        .filter(|(_, cover)| cover.ground_position != DEMO_COVER_POSITIONS[0])
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    for entity in extra_covers {
        sim.world.despawn(entity);
    }
    let enemies = sim
        .world
        .query_filtered::<Entity, With<Enemy>>()
        .iter(&sim.world)
        .collect::<Vec<_>>();
    for entity in enemies {
        sim.world.despawn(entity);
    }
    sim.world.resource_scope(|world, mut run: Mut<Run>| {
        run.reinforcements = 0;
        let mut queue = bevy::ecs::world::CommandQueue::default();
        for _ in 0..enemy_count {
            encounters::spawn_enemy(
                &mut Commands::new(&mut queue, world),
                &mut run,
                EnemyKind::Melee,
                [0.0, -1.0],
            );
        }
        queue.apply(world);
    });
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        *hero.combat = Default::default();
        hero.motion.position = [0.0; 3];
        hero.motion.facing = [0.0, -1.0];
        hero.view.critical_chance = 0.0;
        *hero.health = Health::new(100.0);
    }
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        *enemy.health = Health::new(200.0);
        enemy.action.due = true;
        enemy.action.target = [0.0; 2];
        enemy.view.warn_radius = 3.0;
    }
    sim
}
#[test]
fn equal_time_lethal_actions_trade_before_death_cleanup() {
    let mut sim = arena(1);
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.health.hp = 10.0;
    }
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.health.hp = 10.0;
    }
    sim.step(DreamInput {
        attack: true,
        aim: [0.0, -1.0],
        ..Default::default()
    });
    let snapshot = sim.snapshot();
    assert_eq!(
        snapshot.hero.hp, 0.0,
        "an eligible enemy attack trades with its killer"
    );
    assert_eq!(snapshot.kills, 1);
    assert!(snapshot.enemies.is_empty());
}
#[test]
fn equal_time_damage_does_not_gain_first_hit_immunity_or_entity_order_bias() {
    let mut sim = arena(2);
    let mut reversed = sim.snapshot();
    reversed.state.enemies.reverse();
    let mut other = DreamSimulation::from_snapshot(&reversed);
    sim.step(DreamInput::default());
    other.step(DreamInput::default());
    assert_eq!(
        sim.snapshot().hero.hp,
        66.0,
        "both pre-eligible seventeen-damage attacks commit"
    );
    assert_eq!(sim.snapshot(), other.snapshot());
}
#[test]
fn same_tick_shield_activation_absorbs_the_complete_eligible_batch() {
    let mut sim = arena(2);
    sim.step(DreamInput {
        casts: [false, false, false, true],
        ..Default::default()
    });
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.hero.hp, 100.0);
    assert_eq!(snapshot.hero.shield, 21.0);
    let defense = &snapshot.state.heroes[0].defense_episodes;
    assert_eq!(defense.episodes.iter().map(|e| e.spent).sum::<f32>(), 34.0);
    assert!(defense.valid(snapshot.tick));
}
#[test]
fn historical_shield_credit_cannot_spend_refresh_credit_or_absorb_twice() {
    let mut ledger = DefenseEpisodes::default();
    let mut combat = engine_core::CombatState::default();
    combat.grant_shield(
        55.0,
        7.0,
        engine_core::ShieldPolicy::Replace,
        engine_core::DurationPolicy::Reset,
    );
    ledger.reconcile(&combat, 1).unwrap();
    let old = ledger.cutoff();
    combat.grant_shield(
        20.0,
        7.0,
        engine_core::ShieldPolicy::AddCapped(100.0),
        engine_core::DurationPolicy::Reset,
    );
    ledger.reconcile(&combat, 2).unwrap();
    assert_eq!(ledger.absorb(1, old, 55.0, 80.0, 2), (55.0, 55.0));
    assert_eq!(ledger.remaining_at(2), 20.0);
    assert_eq!(ledger.absorb(1, old, 55.0, 80.0, 2), (0.0, 0.0));
    assert_eq!(ledger.remaining_at(2), 20.0);
    assert!(ledger.valid(2));
}
fn late_hit(
    In((target, generation, query)): In<(u64, u32, u32)>,
    run: Res<Run>,
    mut batch: ResMut<systems::CombatBatch>,
    heroes: Query<HeroActorReadOnly>,
) {
    let hero = heroes.single().unwrap();
    systems::stage_historical_damage(
        &mut batch,
        systems::CombatActionKey::new(
            run.tick,
            hero.view.id,
            hero.combat_identity.generation,
            4,
            u64::from(run.tick),
            0,
        ),
        target,
        generation,
        query,
        40.0,
    );
}
fn apply_late(sim: &mut DreamSimulation, target: u64, generation: u32, query: u32) {
    sim.world.run_system_once(systems::begin_tick).unwrap();
    sim.world
        .run_system_once(systems::capture_combat_poses)
        .unwrap();
    sim.world
        .run_system_once_with(late_hit, (target, generation, query))
        .unwrap();
    sim.world
        .run_system_once(systems::resolve_combat_batch)
        .unwrap();
}
#[test]
fn complete_checkpoint_restores_history_for_the_next_late_damage_transaction() {
    let mut sim = arena(1);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.action.due = false;
        enemy.action.recovery = 100.0;
        enemy.combat.grant_shield(
            25.0,
            0.05,
            engine_core::ShieldPolicy::Replace,
            engine_core::DurationPolicy::Reset,
        );
    }
    for _ in 0..6 {
        sim.step(DreamInput::default());
    }
    let saved = sim.snapshot();
    let target = saved.state.enemies[0].view.id;
    let generation = saved.state.enemies[0].combat_identity.generation;
    let checkpoint = DreamCheckpoint::new(saved, 1).unwrap();
    let decoded = DreamCheckpoint::decode(&checkpoint.encode().unwrap(), 1).unwrap();
    let mut restored = DreamSimulation::try_from_checkpoint(&decoded, 1).unwrap();
    apply_late(&mut sim, target, generation, 1);
    apply_late(&mut restored, target, generation, 1);
    assert_eq!(sim.snapshot(), restored.snapshot());
    assert_eq!(
        sim.snapshot().enemies[0].hp,
        185.0,
        "expired historical shield absorbs twenty-five once"
    );
    apply_late(&mut sim, target, generation, 1);
    assert_eq!(
        sim.snapshot().enemies[0].hp,
        145.0,
        "old shield credit was already consumed"
    );
}
#[test]
fn missing_history_and_old_lifecycle_never_guess_current_geometry() {
    let mut sim = arena(1);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.action.due = false;
    }
    sim.step(DreamInput::default());
    let snapshot = sim.snapshot();
    let target = snapshot.enemies[0].id;
    apply_late(&mut sim, target, 1, 0);
    assert_eq!(sim.snapshot().enemies[0].hp, 200.0);
    assert_eq!(
        sim.world.resource::<systems::CombatBatch>().error,
        Some("missing historical combat poses")
    );
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.combat_identity.discontinuity(true).unwrap();
    }
    apply_late(&mut sim, target, 1, 1);
    assert_eq!(sim.snapshot().enemies[0].hp, 200.0);
}
#[test]
fn headless_capsules_and_teleport_segments_survive_complete_restore() {
    let mut sim = arena(1);
    sim.step(DreamInput::default());
    let first = sim
        .world
        .resource::<crate::combat_history::CombatHistory>()
        .archive
        .frames[0]
        .poses
        .iter()
        .find(|p| p.id == 1)
        .unwrap()
        .clone();
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.motion.position = [4.0, 2.0, 1.0];
        hero.combat_identity.discontinuity(false).unwrap();
    }
    sim.step(DreamInput::default());
    let snapshot = sim.snapshot();
    let current = snapshot
        .state
        .combat_history
        .frames
        .last()
        .unwrap()
        .poses
        .iter()
        .find(|p| p.id == 1)
        .unwrap();
    assert!(current.position[1] > 2.0);
    assert_eq!(current.segment, first.segment + 1);
    let restored = DreamSimulation::from_snapshot(&snapshot);
    let history = restored
        .world
        .resource::<crate::combat_history::CombatHistory>();
    assert!(
        history
            .prepared
            .frame(1, 1)
            .unwrap()
            .pose(
                engine_core::ColliderKey {
                    index: 1,
                    generation: 1
                },
                current.segment
            )
            .is_err()
    );
    assert_eq!(restored.snapshot(), snapshot);
}

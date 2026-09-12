use crate::*;

fn isolated(distance: f32) -> DreamSimulation {
    let mut sim = super::combat::arena(1);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [distance, 0.0];
        enemy.ai.home = [distance, 0.0];
        enemy.ai.awake = false;
        enemy.ai.target = None;
        enemy.ai.wake_delay = 0.0;
        *enemy.action = Default::default();
        enemy.view.warn_radius = 0.0;
    }
    sim
}
fn place_hero(sim: &mut DreamSimulation, id: u64, position: [f32; 2], alive: bool) {
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        if hero.view.id == id {
            hero.motion.position = [position[0], 0.0, position[1]];
            hero.health.hp = if alive { hero.health.max_hp } else { 0.0 };
        }
    }
}

#[test]
fn inclusive_aggro_boundary_and_sleeping_ai_state_are_exact() {
    for (distance, awake) in [
        (ENEMY_AGGRO_RANGE + 0.01, false),
        (ENEMY_AGGRO_RANGE, true),
        (ENEMY_AGGRO_RANGE - 0.01, true),
    ] {
        let mut sim = isolated(distance);
        let before = sim.snapshot().state.enemies[0].clone();
        sim.step(DreamInput::default());
        let after = sim.snapshot().state.enemies[0].clone();
        assert_eq!(after.ai.awake, awake);
        if awake {
            assert!(after.motor.position[0] < before.motor.position[0]);
        } else {
            for _ in 0..12 {
                sim.step(DreamInput::default());
            }
            let idle = sim.snapshot().state.enemies[0].clone();
            assert_eq!(idle.ai, before.ai);
            assert_eq!(idle.motor, before.motor);
            assert_eq!(idle.action, before.action);
            assert_eq!(idle.actor, before.actor);
            assert_eq!(idle.health, before.health);
            // Collision timestamps stay fresh independently of private AI sleep.
            assert_eq!(idle.combat_identity.tick, sim.gameplay_tick());
        }
    }
}

#[test]
fn pending_and_dead_heroes_cannot_wake_enemies() {
    let mut sim = isolated(0.0);
    assert!(sim.add_player(2));
    assert!(sim.add_pending_player(3));
    place_hero(&mut sim, 1, [80.0, 0.0], true);
    place_hero(&mut sim, 2, [0.0, 0.0], false);
    place_hero(&mut sim, 3, [0.0, 0.0], true);
    sim.step(DreamInput::default());
    let enemy = sim.snapshot().state.enemies[0].clone();
    assert!(!enemy.ai.awake);
    assert!(enemy.action.idle());
    assert_eq!(enemy.motor.position, [0.0, 0.0]);
    assert!(sim.set_player_active(3, true));
    sim.step(DreamInput::default());
    assert!(sim.snapshot().state.enemies[0].ai.awake);
    assert!(sim.snapshot().state.enemies[0].action.windup > 0.0);
}

#[test]
fn latched_target_chases_beyond_detection_and_restores_identically() {
    let mut sim = isolated(12.0);
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().state.enemies[0].ai.target, Some(1));
    place_hero(&mut sim, 1, [80.0, 0.0], true);
    let before = sim.snapshot().state.enemies[0].motor.position[0];
    sim.step(DreamInput::default());
    let snapshot = sim.snapshot();
    assert!(snapshot.state.enemies[0].ai.awake);
    assert!(snapshot.state.enemies[0].motor.position[0] > before);
    let checkpoint = DreamCheckpoint::decode(
        &DreamCheckpoint::new(snapshot, 1).unwrap().encode().unwrap(),
        1,
    )
    .unwrap();
    let mut restored = DreamSimulation::try_from_checkpoint(&checkpoint, 1).unwrap();
    for _ in 0..5 {
        sim.step(DreamInput::default());
        restored.step(DreamInput::default());
        assert_eq!(sim.snapshot(), restored.snapshot());
        assert_eq!(sim.snapshot().state.enemies[0].ai.target, Some(1));
    }
}

#[test]
fn detection_ties_are_stable_and_nearer_actors_do_not_steal_target() {
    let mut sim = isolated(10.0);
    assert!(sim.add_player(2));
    place_hero(&mut sim, 1, [0.0, 0.0], true);
    place_hero(&mut sim, 2, [20.0, 0.0], true);
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().state.enemies[0].ai.target, Some(1));
    place_hero(&mut sim, 1, [-80.0, 0.0], true);
    place_hero(&mut sim, 2, [12.0, 0.0], true);
    let before = sim.snapshot().state.enemies[0].motor.position[0];
    sim.step(DreamInput::default());
    let enemy = sim.snapshot().state.enemies[0].clone();
    assert_eq!(enemy.ai.target, Some(1));
    assert!(enemy.motor.position[0] < before);
}

#[test]
fn unavailable_target_reacquires_nearby_or_idles_then_can_wake_again() {
    for unavailable in 0..3 {
        let mut sim = isolated(10.0);
        assert!(sim.add_player(2));
        place_hero(&mut sim, 2, [80.0, 0.0], true);
        sim.step(DreamInput::default());
        assert_eq!(sim.snapshot().state.enemies[0].ai.target, Some(1));
        match unavailable {
            0 => place_hero(&mut sim, 1, [0.0, 0.0], false),
            1 => {
                assert!(sim.set_player_active(1, false));
            }
            _ => {
                assert!(sim.remove_player(1));
            }
        }
        // Removal/defeat can be captured before the next AI phase resolves
        // the stable target. That complete snapshot must still restore exactly.
        let checkpoint = DreamCheckpoint::decode(
            &DreamCheckpoint::new(sim.snapshot(), 1)
                .unwrap()
                .encode()
                .unwrap(),
            1,
        )
        .unwrap();
        let mut restored = DreamSimulation::try_from_checkpoint(&checkpoint, 1).unwrap();
        sim.step(DreamInput::default());
        restored.step(DreamInput::default());
        assert_eq!(sim.snapshot(), restored.snapshot());
        let sleeping = sim.snapshot().state.enemies[0].clone();
        assert_eq!(sleeping.ai.target, None);
        assert!(!sleeping.ai.awake);
        assert!(sleeping.action.idle());
        place_hero(&mut sim, 2, [15.0, 0.0], true);
        sim.step(DreamInput::default());
        assert_eq!(sim.snapshot().state.enemies[0].ai.target, Some(2));
    }
    let mut sim = isolated(10.0);
    assert!(sim.add_player(2));
    place_hero(&mut sim, 2, [22.0, 0.0], true);
    sim.step(DreamInput::default());
    place_hero(&mut sim, 1, [0.0, 0.0], false);
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().state.enemies[0].ai.target, Some(2));
}

#[test]
fn leaving_detection_keeps_committed_attack_and_recovery_movement_lock() {
    let mut sim = isolated(1.0);
    sim.step(DreamInput::default());
    let committed = sim.snapshot().state.enemies[0].clone();
    assert!(committed.action.windup > 0.0);
    place_hero(&mut sim, 1, [80.0, 0.0], true);
    sim.step(DreamInput::default());
    let winding = sim.snapshot().state.enemies[0].clone();
    assert_eq!(winding.ai.target, Some(1));
    assert_eq!(winding.action.target, committed.action.target);
    assert_eq!(winding.motor.position, committed.motor.position);
    assert!(winding.action.windup > 0.0);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.action.begin_recovery(DT * 2.5);
    }
    sim.step(DreamInput::default());
    assert_eq!(
        sim.snapshot().state.enemies[0].motor.position,
        committed.motor.position
    );
    sim.step(DreamInput::default());
    sim.step(DreamInput::default());
    assert!(sim.snapshot().state.enemies[0].motor.position[0] > committed.motor.position[0]);
}

#[test]
fn already_spawned_enemy_projectile_expires_while_source_sleeps() {
    let mut sim = isolated(4.0);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.view.kind = EnemyKind::Ranged;
        enemy.action.due = true;
        enemy.action.target = [0.0, 0.0];
    }
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().projectiles.len(), 1);
    for mut projectile in sim
        .world
        .query::<&mut engine_core::ProjectileState>()
        .iter_mut(&mut sim.world)
    {
        projectile.remaining = DT * 1.5;
    }
    assert!(sim.add_player(2));
    place_hero(&mut sim, 2, [80.0, 0.0], true);
    place_hero(&mut sim, 1, [80.0, 0.0], false);
    sim.step(DreamInput::default());
    assert!(!sim.snapshot().state.enemies[0].ai.awake);
    assert_eq!(sim.snapshot().projectiles.len(), 1);
    sim.step(DreamInput::default());
    assert!(sim.snapshot().projectiles.is_empty());
}

#[test]
fn ambient_population_is_private_bounded_and_does_not_block_encounter_clear() {
    use crate::replication::*;
    let mut sim = DreamSimulation::new(42, false);
    sim.continue_run();
    let snapshot = sim.snapshot();
    assert_eq!(
        snapshot
            .state
            .enemies
            .iter()
            .filter(|enemy| enemy.ai.role == EnemyRole::Ambient)
            .count(),
        AMBIENT_ENEMY_POSITIONS.len()
    );
    let capture = sim
        .capture_replication(ReplicationStamp {
            match_epoch: 1,
            server_tick: 1,
            gameplay_tick: snapshot.tick,
            scene_revision: 1,
            revision: 1,
        })
        .unwrap();
    for enemy in snapshot
        .state
        .enemies
        .iter()
        .filter(|enemy| enemy.ai.role == EnemyRole::Ambient)
    {
        let public = capture.project_enemy(enemy.view.id).unwrap();
        let value = serde_json::to_value(public).unwrap();
        for private in ["ai", "role", "home", "awake", "wake_delay"] {
            assert!(value.get(private).is_none());
        }
        assert_eq!(public.position, enemy.ai.home);
        // Existing public target is the telegraphed attack point, never an actor ID.
        assert_eq!(public.target, enemy.action.target);
    }
    let encounter = sim
        .world
        .query::<(Entity, &EnemyAi)>()
        .iter(&sim.world)
        .filter(|(_, ai)| ai.role == EnemyRole::Encounter)
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    for entity in encounter {
        sim.world.despawn(entity);
    }
    sim.world.resource_mut::<Run>().reinforcements = 0;
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().phase, RunPhase::Reward);
    assert_eq!(sim.snapshot().enemies.len(), AMBIENT_ENEMY_POSITIONS.len());
    assert_eq!(sim.snapshot().kills, 0);
}

#[test]
fn full_population_history_fits_declared_snapshot_and_retained_player_limits() {
    let mut sim = DreamSimulation::new(42, false);
    sim.continue_run();
    for id in 2..=MAX_HEROES as u64 {
        assert!(sim.add_pending_player(id));
    }
    assert!(!sim.add_pending_player(MAX_HEROES as u64 + 1));
    assert!(!sim.add_player(MAX_HEROES as u64 + 1));
    assert!(sim.add_pending_player(1));
    sim.world.resource_scope(|world, mut run: Mut<Run>| {
        let mut queue = bevy::ecs::world::CommandQueue::default();
        for _ in 0..MAX_ENCOUNTER_SPAWNS + 4 {
            encounters::spawn_enemy(
                &mut Commands::new(&mut queue, world),
                &mut run,
                EnemyKind::Melee,
                [110.0, 0.0],
            );
        }
        queue.apply(world);
    });
    for _ in 0..COMBAT_HISTORY_TICKS {
        sim.step(DreamInput::default());
    }
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.state.heroes.len(), MAX_HEROES);
    assert_eq!(
        snapshot.state.enemies.len(),
        MAX_ENCOUNTER_SPAWNS + AMBIENT_ENEMY_POSITIONS.len()
    );
    assert_eq!(snapshot.state.run.encounter_spawns, MAX_ENCOUNTER_SPAWNS);
    let expected = MAX_HEROES
        + MAX_ENCOUNTER_SPAWNS
        + AMBIENT_ENEMY_POSITIONS.len()
        + DEMO_COVER_POSITIONS.len();
    assert_eq!(expected, 250);
    assert!(
        snapshot
            .state
            .combat_history
            .frames
            .iter()
            .all(|frame| frame.poses.len() == expected)
    );
    fn costs(value: &serde_json::Value) -> (usize, usize) {
        match value {
            serde_json::Value::Object(map) => map.iter().fold((1, 0), |(n, t), (k, v)| {
                let (a, b) = costs(v);
                (n + a + 1, t + b + k.len())
            }),
            serde_json::Value::Array(list) => list.iter().fold((1, 0), |(n, t), v| {
                let (a, b) = costs(v);
                (n + a, t + b)
            }),
            serde_json::Value::String(text) => (1, text.len()),
            _ => (1, 0),
        }
    }
    let raw = serde_json::to_value(&snapshot).unwrap();
    eprintln!(
        "full population snapshot structure: {:?}, bytes {}",
        costs(&raw),
        serde_json::to_vec(&raw).unwrap().len()
    );
    let bytes = DreamCheckpoint::new(snapshot.clone(), 1)
        .unwrap()
        .encode()
        .unwrap();
    assert!(bytes.len() < 8 * 1024 * 1024);
    eprintln!("full population checkpoint bytes: {}", bytes.len());
    assert_eq!(
        DreamCheckpoint::decode(&bytes, 1).unwrap().snapshot,
        snapshot
    );
    let mut malformed = snapshot.clone();
    malformed.state.enemies[0].ai.target = Some(0);
    assert!(DreamCheckpoint::new(malformed, 1).is_err());
    let mut malformed = snapshot.clone();
    malformed.state.enemies[0].ai.awake = !malformed.state.enemies[0].ai.awake;
    assert!(DreamCheckpoint::new(malformed, 1).is_err());
    let mut malformed = snapshot;
    malformed.state.enemies[0].ai.wake_delay = 2.0;
    assert!(DreamCheckpoint::new(malformed, 1).is_err());
}

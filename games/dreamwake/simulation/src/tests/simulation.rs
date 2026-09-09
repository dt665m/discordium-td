use crate::*;

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn normalized(v: [f32; 2]) -> [f32; 2] {
    let l = v[0].hypot(v[1]).max(0.001);
    [v[0] / l, v[1] / l]
}
fn test_hero(sim: &mut DreamSimulation, action: impl FnOnce(&mut HeroActorItem<'_, '_>)) {
    let mut query = sim.world.query::<HeroActor>();
    action(&mut query.single_mut(&mut sim.world).unwrap());
}
fn start(seed: u64) -> DreamSimulation {
    let mut sim = DreamSimulation::new(seed, false);
    sim.continue_run();
    sim
}
fn clear_enemies(sim: &mut DreamSimulation) {
    sim.world.resource_mut::<Run>().reinforcements = 0;
    let entities: Vec<_> = sim
        .world
        .query_filtered::<Entity, With<Enemy>>()
        .iter(&sim.world)
        .collect();
    for entity in entities {
        sim.world.despawn(entity);
    }
}
fn enemy(sim: &mut DreamSimulation, kind: EnemyKind, pos: [f32; 2]) {
    sim.world.resource_scope(|world, mut run: Mut<Run>| {
        let mut queue = bevy::ecs::world::CommandQueue::default();
        encounters::spawn_enemy(&mut Commands::new(&mut queue, world), &mut run, kind, pos);
        queue.apply(world);
    });
}
fn memory(sim: &mut DreamSimulation, kind: MemoryKind, essence: Option<EssenceKind>) {
    test_hero(sim, |hero| {
        hero.motor.position = [0.0, 0.0];
        hero.motor.facing = [0.0, -1.0];
        hero.loadout.0[0] = memory_slot(kind);
        hero.loadout.0[0].modifier = essence;
        hero.view.critical_chance = 0.0;
        encounters::recalculate_cooldowns(hero);
    });
}
fn cast(sim: &mut DreamSimulation) {
    sim.step(DreamInput {
        action_sequences: [0; 5],
        casts: [true, false, false, false],
        aim: [0.0, -1.0],
        ..Default::default()
    });
}

#[test]
fn opening_pause_bounds_and_restart() {
    let mut sim = DreamSimulation::new(64, true);
    assert_eq!(sim.snapshot().phase, RunPhase::Intro);
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().tick, 0);
    sim.continue_run();
    assert_eq!(sim.snapshot().phase, RunPhase::Combat);
    let snapshot = sim.snapshot();
    sim.set_paused(true);
    sim.step(DreamInput {
        action_sequences: [0; 5],
        movement: [1.0; 2],
        ..Default::default()
    });
    assert_eq!(sim.snapshot().hero, snapshot.hero);
    sim.set_paused(false);
    for _ in 0..180 {
        sim.step(DreamInput {
            action_sequences: [0; 5],
            movement: [1.0; 2],
            ..Default::default()
        });
    }
    assert!(distance(sim.snapshot().hero.position, [0.0; 2]) <= ARENA_RADIUS);
    sim.restart(987, false);
    assert_eq!(sim.snapshot().seed, 987);
    assert_eq!(sim.snapshot().phase, RunPhase::Intro);
    assert!(sim.snapshot().enemies.is_empty());
}

#[test]
fn missed_swings_advance_combo_and_idle_time_does_not_reset_it() {
    let mut sim = start(63);
    clear_enemies(&mut sim);
    enemy(&mut sim, EnemyKind::Boss, [20.0, 20.0]);
    test_hero(&mut sim, |h| {
        h.motor.position = [0.0; 2];
        h.combat.invulnerability_remaining = 100.0;
    });
    let attack = DreamInput {
        attack: true,
        aim: [0.0, -1.0],
        ..Default::default()
    };
    sim.step(attack);
    assert_eq!(sim.snapshot().hero.combo, 1);
    for _ in 0..300 {
        sim.step(DreamInput::default());
    }
    assert_eq!(sim.snapshot().hero.combo, 1);
    for mut e in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        e.motor.position = [20.0, 20.0];
    }
    sim.step(attack);
    assert_eq!(sim.snapshot().hero.combo, 2);
}

#[test]
fn casting_is_allowed_during_dash_while_basic_attack_is_blocked() {
    let mut sim = start(64);
    memory(&mut sim, MemoryKind::Aegis, None);
    sim.step(DreamInput {
        dash: true,
        attack: true,
        casts: [true, false, false, false],
        aim: [0.0, -1.0],
        ..Default::default()
    });
    let snapshot = sim.snapshot();
    assert!(snapshot.hero.dashing);
    assert_eq!(snapshot.hero.shield, 55.0);
    assert!(snapshot.hero.memories[0].cooldown > 0.0);
    assert_eq!(snapshot.hero.combo, 0);
}

#[test]
fn enemies_can_follow_during_recovery_but_cannot_commit_again() {
    let mut sim = start(65);
    clear_enemies(&mut sim);
    enemy(&mut sim, EnemyKind::Melee, [0.0, -10.0]);
    test_hero(&mut sim, |h| h.motor.position = [0.0; 2]);
    for mut e in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        e.action.begin_recovery(5.0);
    }
    let before = sim.snapshot().enemies[0].position;
    sim.step(DreamInput::default());
    let after = sim.snapshot();
    assert!(distance(after.enemies[0].position, [0.0; 2]) < distance(before, [0.0; 2]));
    assert_eq!(after.enemies[0].windup, 0.0);
    assert!(after.state.enemies[0].action.recovery > 4.0);
}

#[test]
fn generic_components_are_authoritative_and_survive_json_restore() {
    let mut sim = start(66);
    memory(&mut sim, MemoryKind::Wisp, Some(EssenceKind::Echo));
    cast(&mut sim);
    test_hero(&mut sim, |h| {
        h.combat.shield = 17.0;
        h.combat.shield_remaining = 2.0;
        h.motor.movement_lock = 0.3;
        h.action.recovery = 0.4;
        h.progression.xp = 13.0;
    });
    let count = sim
        .world
        .query_filtered::<(
            &Health,
            &engine_core::CombatState,
            &engine_core::MotorState,
            &engine_core::ActionState,
            &MemoryLoadout,
            &engine_core::Progression,
        ), With<Hero>>()
        .iter(&sim.world)
        .count();
    assert_eq!(count, 1);
    assert_eq!(
        sim.world
            .query::<(&Wisp, &engine_core::CompanionState)>()
            .iter(&sim.world)
            .count(),
        1
    );
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.hero.shield, 17.0);
    assert_eq!(snapshot.hero.attack_cooldown, 0.4);
    assert_eq!(snapshot.hero.xp, 13.0);
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    let decoded: DreamSnapshot = serde_json::from_slice(&encoded).unwrap();
    let mut restored = DreamSimulation::from_snapshot(&decoded);
    assert_eq!(snapshot, restored.snapshot());
    for _ in 0..40 {
        sim.step(DreamInput::default());
        restored.step(DreamInput::default());
        assert_eq!(sim.snapshot(), restored.snapshot());
    }
}

#[test]
fn basic_combo_has_direction_healing_and_memory_refresh() {
    let mut sim = start(3);
    clear_enemies(&mut sim);
    enemy(&mut sim, EnemyKind::Elite, [0.0, -2.0]);
    test_hero(&mut sim, |h| {
        h.motor.position = [0.0; 2];
        h.health.hp = 100.0;
        h.view.combo = 2;
        h.loadout.0[0].cooldown = 3.0;
    });
    sim.step(DreamInput {
        action_sequences: [0; 5],
        attack: true,
        aim: [0.0, -1.0],
        ..Default::default()
    });
    let snapshot = sim.snapshot();
    assert!(snapshot.enemies[0].hp < snapshot.enemies[0].max_hp);
    assert_eq!(snapshot.hero.hp, 105.0);
    assert!(snapshot.hero.memories[0].cooldown < 2.5);
    assert!(snapshot.hero.attack_flash > 0.0);
}

#[test]
fn all_six_memories_execute_and_reject_cooldown_duplicates() {
    for kind in MemoryKind::ALL {
        let mut sim = start(8);
        clear_enemies(&mut sim);
        enemy(&mut sim, EnemyKind::Boss, [0.0, -3.0]);
        memory(&mut sim, kind, None);
        cast(&mut sim);
        let one = sim.snapshot();
        let seq = one.tick;
        assert!(one.hero.memories[0].cooldown > 0.0, "{kind:?}");
        cast(&mut sim);
        let two = sim.snapshot();
        assert!(
            two.presentations.iter().all(|p| p.id.action_seq <= seq),
            "rejected {kind:?} spawned an effect"
        );
        match kind {
            MemoryKind::Starfall => {
                assert!(one.projectiles.len() == 1 || one.enemies[0].hp < one.enemies[0].max_hp)
            }
            MemoryKind::Wisp => assert_eq!(one.wisps.len(), 1),
            MemoryKind::Blink => assert!(one.hero.position[1] < -5.0 && one.hero.invulnerable),
            MemoryKind::Aegis => assert_eq!(one.hero.shield, 55.0),
            _ => assert!(one.enemies[0].hp < one.enemies[0].max_hp),
        }
    }
}

#[test]
fn twin_echo_vast_and_haste_change_cast_behavior() {
    let mut sim = start(5);
    memory(&mut sim, MemoryKind::Starfall, Some(EssenceKind::Twin));
    cast(&mut sim);
    assert_eq!(sim.snapshot().projectiles.len(), 3);
    let mut echo = start(5);
    memory(&mut echo, MemoryKind::Starfall, Some(EssenceKind::Echo));
    cast(&mut echo);
    assert_eq!(echo.snapshot().state.delayed.len(), 1);
    for _ in 0..34 {
        echo.step(DreamInput::default());
    }
    assert!(echo.snapshot().state.delayed.is_empty());
    assert_eq!(echo.snapshot().projectiles.len(), 2);
    let mut vast = start(5);
    memory(&mut vast, MemoryKind::Starfall, Some(EssenceKind::Vast));
    cast(&mut vast);
    assert_eq!(vast.snapshot().state.projectiles[0].state.hits_remaining, 4);
    assert!(vast.snapshot().projectiles[0].radius > 0.5);
    let mut haste = start(5);
    memory(&mut haste, MemoryKind::Nova, Some(EssenceKind::Haste));
    assert!((haste.snapshot().hero.memories[0].max_cooldown - 5.5 * 0.62).abs() < 0.001);
}

#[test]
fn frost_status_damage_synergy_and_leech_heal() {
    let mut sim = start(9);
    clear_enemies(&mut sim);
    enemy(&mut sim, EnemyKind::Boss, [0.0, -2.0]);
    memory(&mut sim, MemoryKind::Nova, Some(EssenceKind::Frost));
    cast(&mut sim);
    assert!(sim.snapshot().enemies[0].slowed);
    let first_hp = sim.snapshot().enemies[0].hp;
    test_hero(&mut sim, |h| {
        h.loadout.0[0].cooldown = 0.0;
    });
    cast(&mut sim);
    assert!((first_hp - sim.snapshot().enemies[0].hp - 52.0 * 1.2).abs() < 0.01);
    memory(&mut sim, MemoryKind::Nova, Some(EssenceKind::Leech));
    test_hero(&mut sim, |h| {
        h.health.hp = 100.0;
    });
    cast(&mut sim);
    assert!(sim.snapshot().hero.hp > 106.0);
}

#[test]
fn dash_protects_from_a_committed_warning() {
    let mut sim = start(4);
    clear_enemies(&mut sim);
    enemy(&mut sim, EnemyKind::Melee, [0.0, 0.5]);
    test_hero(&mut sim, |h| {
        h.motor.position = [0.0; 2];
        h.combat.invulnerability_remaining = 0.0;
    });
    for mut target in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        target.action.windup = DT * 0.5;
        target.action.target = [0.0; 2];
        target.view.warn_radius = 3.0;
    }
    let before = sim.snapshot().hero.hp;
    sim.step(DreamInput {
        action_sequences: [0; 5],
        dash: true,
        aim: [0.0, -1.0],
        ..Default::default()
    });
    assert_eq!(sim.snapshot().hero.hp, before);
    assert!(sim.snapshot().hero.invulnerable);
}

#[test]
fn warning_target_stays_committed_and_can_be_evaded() {
    let mut sim = start(23);
    clear_enemies(&mut sim);
    enemy(&mut sim, EnemyKind::Melee, [0.0, 0.5]);
    test_hero(&mut sim, |h| {
        h.motor.position = [0.0; 2];
        h.combat.invulnerability_remaining = 0.0;
    });
    for mut target in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        target.action.recovery = 0.0;
    }
    sim.step(DreamInput::default());
    let target = sim.snapshot().enemies[0].target;
    for _ in 0..25 {
        sim.step(DreamInput {
            action_sequences: [0; 5],
            movement: [1.0, 0.0],
            ..Default::default()
        });
        assert_eq!(sim.snapshot().enemies[0].target, target);
    }
    for _ in 0..12 {
        sim.step(DreamInput {
            action_sequences: [0; 5],
            movement: [1.0, 0.0],
            ..Default::default()
        });
    }
    assert_eq!(sim.snapshot().hero.hp, sim.snapshot().hero.max_hp);
}

#[test]
fn rewards_equip_upgrade_replace_essences_and_swap() {
    let mut sim = start(1);
    clear_enemies(&mut sim);
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().phase, RunPhase::Reward);
    test_hero(&mut sim, |hero| {
        hero.rewards = vec![Reward {
            kind: RewardKind::Memory(MemoryKind::Wisp),
            rarity: Rarity::Rare,
            title: "Wisp".into(),
            description: "summon".into(),
        }]
    });
    assert!(!sim.choose_reward(7, 0));
    assert!(!sim.choose_reward(0, 4));
    assert!(sim.choose_reward(0, 2));
    assert_eq!(sim.snapshot().hero.memories[2].kind, MemoryKind::Wisp);
    assert!(!sim.choose_reward(0, 2));
    assert!(sim.swap_memories(1, 2));
    assert_eq!(sim.snapshot().hero.memories[1].kind, MemoryKind::Wisp);
    sim.continue_run();
    assert!(!sim.swap_memories(0, 1));
    sim.set_paused(true);
    assert!(sim.swap_memories(0, 1));
    let old = sim.snapshot().hero.attack_power;
    test_hero(&mut sim, |h| encounters::upgrade(h, UpgradeKind::Attack));
    assert!((sim.snapshot().hero.attack_power - old - 0.3).abs() < 0.001);
}

#[test]
fn rest_recovers_health_and_spends_shards() {
    let mut sim = start(12);
    sim.world.resource_mut::<Run>().room = 2;
    sim.world.resource_mut::<Run>().phase = RunPhase::Transition;
    test_hero(&mut sim, |h| {
        h.health.hp = 40.0;
        h.view.shards = 90;
    });
    sim.continue_run();
    assert_eq!(sim.snapshot().phase, RunPhase::Rest);
    assert_eq!(sim.snapshot().hero.hp, 150.0);
    assert!(sim.buy_memory_upgrade(0));
    assert_eq!(sim.snapshot().hero.shards, 45);
    assert_eq!(sim.snapshot().hero.memories[0].level, 2);
    assert!(sim.buy_memory_upgrade(0));
    assert!(!sim.buy_memory_upgrade(0));
    sim.choose_reward(0, 0);
    sim.continue_run();
    assert_eq!(sim.snapshot().room, 4);
}

#[test]
fn deterministic_snapshots_restore_echo_projectiles_statuses_and_rng() {
    let mut sim = start(19);
    memory(&mut sim, MemoryKind::Nova, Some(EssenceKind::Echo));
    cast(&mut sim);
    let snapshot = sim.snapshot();
    let encoded = serde_json::to_string(&snapshot).unwrap();
    let decoded: DreamSnapshot = serde_json::from_str(&encoded).unwrap();
    assert_eq!(snapshot, decoded);
    let mut replay = DreamSimulation::from_snapshot(&decoded);
    assert_eq!(snapshot, replay.snapshot());
    for i in 0..360 {
        let input = DreamInput {
            action_sequences: [0; 5],
            movement: [((i as f32) * 0.01).sin(), -0.2],
            aim: [0.2, -1.0],
            attack: true,
            casts: [i % 30 == 0, i % 40 == 0, i % 100 == 0, i % 200 == 0],
            dash: i % 113 == 0,
        };
        sim.step(input);
        replay.step(input);
        assert_eq!(
            sim.snapshot(),
            replay.snapshot(),
            "diverged at replay tick {i}"
        );
    }
}

#[test]
fn presentation_lifetime_expires_and_reset_clears_effects() {
    let mut sim = start(42);
    memory(&mut sim, MemoryKind::Nova, None);
    cast(&mut sim);
    let ids: Vec<_> = sim.snapshot().presentations.iter().map(|v| v.id).collect();
    assert!(!ids.is_empty());
    for _ in 0..61 {
        sim.step(DreamInput::default());
    }
    assert!(
        sim.snapshot()
            .presentations
            .iter()
            .all(|p| !ids.contains(&p.id))
    );
    sim.restart(42, false);
    assert!(sim.snapshot().presentations.is_empty());
}

#[test]
fn idle_player_can_die_and_restart_quickly() {
    let mut sim = start(8);
    for _ in 0..18_000 {
        sim.step(DreamInput::default());
        if sim.snapshot().phase == RunPhase::Defeat {
            break;
        }
    }
    assert_eq!(sim.snapshot().phase, RunPhase::Defeat);
    let dead = sim.snapshot();
    sim.step(DreamInput {
        action_sequences: [0; 5],
        attack: true,
        ..Default::default()
    });
    assert_eq!(dead.hero.hp, sim.snapshot().hero.hp);
    assert_eq!(dead.hero.position, sim.snapshot().hero.position);
    assert_eq!(dead.tick, sim.snapshot().tick);
    sim.restart(9, false);
    sim.continue_run();
    assert_eq!(sim.snapshot().phase, RunPhase::Combat);
    assert_eq!(sim.snapshot().hero.hp, 220.0);
}

/// A deterministic player drives public input and reward APIs through a full run.
fn play_run(seed: u64, lucid: bool) -> (DreamSnapshot, Vec<u8>) {
    let mut sim = DreamSimulation::new(seed, lucid);
    sim.continue_run();
    let mut phases = Vec::new();
    for _ in 0..100_000 {
        let snapshot = sim.snapshot();
        match snapshot.phase {
            RunPhase::Victory | RunPhase::Defeat => return (snapshot, phases),
            RunPhase::Intro | RunPhase::Transition => sim.continue_run(),
            RunPhase::Rest => {
                for slot in 0..4 {
                    sim.buy_memory_upgrade(slot);
                }
                if !sim.choose_reward(2, 0) {
                    sim.continue_run();
                }
            }
            RunPhase::Reward => {
                let choice = snapshot
                    .rewards
                    .iter()
                    .position(|r| {
                        matches!(
                            r.kind,
                            RewardKind::Upgrade(
                                UpgradeKind::Ability
                                    | UpgradeKind::Health
                                    | UpgradeKind::Attack
                                    | UpgradeKind::Recovery
                            )
                        )
                    })
                    .unwrap_or(2);
                sim.choose_reward(choice, 0);
            }
            RunPhase::Combat => {
                for boss in snapshot
                    .enemies
                    .iter()
                    .filter(|e| e.kind == EnemyKind::Boss)
                {
                    if !phases.contains(&boss.phase) {
                        phases.push(boss.phase);
                        println!(
                            "seed={seed} boss phase {} at {:.1}s",
                            boss.phase, snapshot.elapsed
                        );
                    }
                }
                let Some(target) = snapshot.enemies.iter().min_by(|a, b| {
                    distance(a.position, snapshot.hero.position)
                        .total_cmp(&distance(b.position, snapshot.hero.position))
                }) else {
                    sim.step(DreamInput::default());
                    continue;
                };
                let dir = normalized([
                    target.position[0] - snapshot.hero.position[0],
                    target.position[1] - snapshot.hero.position[1],
                ]);
                let mut movement = if distance(target.position, snapshot.hero.position) > 2.6 {
                    dir
                } else {
                    [0.0; 2]
                };
                let danger = snapshot
                    .enemies
                    .iter()
                    .filter(|e| e.windup > 0.0 && e.kind != EnemyKind::Ranged)
                    .find(|e| distance(e.target, snapshot.hero.position) < e.warn_radius + 1.3);
                if let Some(danger) = danger {
                    let away = [
                        snapshot.hero.position[0] - danger.target[0],
                        snapshot.hero.position[1] - danger.target[1],
                    ];
                    movement = if away[0].hypot(away[1]) > 0.2 {
                        normalized(away)
                    } else {
                        [-dir[1], dir[0]]
                    };
                }
                let dash = danger.is_some_and(|e| e.windup < 0.35);
                sim.step(DreamInput {
                    action_sequences: [0; 5],
                    movement,
                    aim: dir,
                    attack: true,
                    dash,
                    casts: [
                        distance(target.position, snapshot.hero.position) < 4.8,
                        true,
                        distance(target.position, snapshot.hero.position) < 5.0,
                        snapshot.hero.shield < 10.0,
                    ],
                });
            }
        }
    }
    (sim.snapshot(), phases)
}

#[test]
fn complete_seeded_runs_reach_victory_and_all_boss_phases() {
    for seed in [1, 29, 881] {
        let (snapshot, phases) = play_run(seed, false);
        println!(
            "seed={seed} phase={:?} room={} elapsed={:.1}s hp={:.0} kills={} boss_phases={phases:?}",
            snapshot.phase, snapshot.room, snapshot.elapsed, snapshot.hero.hp, snapshot.kills
        );
        assert_eq!(
            snapshot.phase,
            RunPhase::Victory,
            "seed {seed}, room {}, health {}, elapsed {}",
            snapshot.room,
            snapshot.hero.hp,
            snapshot.elapsed
        );
        assert_eq!(phases, [1, 2, 3]);
        assert!(snapshot.hero.level > 3);
        assert!(snapshot.kills > 40);
    }
}

#[test]
fn seeds_and_lucid_pact_change_encounters_and_strength() {
    let a = start(7).snapshot();
    let b = start(18).snapshot();
    assert_ne!(a.enemies, b.enemies);
    let mut lucid = DreamSimulation::new(7, true);
    lucid.continue_run();
    assert!(lucid.snapshot().enemies.len() > a.enemies.len());
    assert!(lucid.snapshot().enemies[0].max_hp > a.enemies[0].max_hp);
}

#[test]
fn all_memory_essence_combinations_are_finite_and_replayable() {
    for kind in MemoryKind::ALL {
        for essence in EssenceKind::ALL {
            let mut sim = start(91);
            clear_enemies(&mut sim);
            enemy(&mut sim, EnemyKind::Boss, [0.0, -3.0]);
            memory(&mut sim, kind, Some(essence));
            cast(&mut sim);
            let mut restored = DreamSimulation::from_snapshot(&sim.snapshot());
            for _ in 0..90 {
                sim.step(DreamInput::default());
                restored.step(DreamInput::default());
            }
            let snapshot = sim.snapshot();
            assert_eq!(
                snapshot,
                restored.snapshot(),
                "{kind:?}/{essence:?} replay diverged"
            );
            assert!(snapshot.hero.hp.is_finite() && snapshot.hero.shield.is_finite());
            assert!(snapshot.enemies.iter().all(|e| e.hp.is_finite()));
            assert!(
                snapshot
                    .presentations
                    .iter()
                    .all(|p| p.age_ticks < p.duration_ticks)
            );
        }
    }
}

#[test]
fn lucid_full_run_is_winnable_with_public_controls() {
    let (snapshot, phases) = play_run(87, true);
    println!(
        "lucid seed87 phase={:?} {:.1}s kills={} hp={:.0}",
        snapshot.phase, snapshot.elapsed, snapshot.kills, snapshot.hero.hp
    );
    assert_eq!(snapshot.phase, RunPhase::Victory);
    assert_eq!(phases, [1, 2, 3]);
}

#[test]
fn reward_effects_expire_without_advancing_gameplay() {
    let mut sim = start(70);
    clear_enemies(&mut sim);
    sim.step(DreamInput {
        action_sequences: [0; 5],
        attack: true,
        ..Default::default()
    });
    let reward = sim.snapshot();
    assert_eq!(reward.phase, RunPhase::Reward);
    assert!(!reward.presentations.is_empty());
    for _ in 0..130 {
        sim.step(DreamInput {
            action_sequences: [0; 5],
            attack: true,
            dash: true,
            casts: [true; 4],
            movement: [1.0; 2],
            aim: [0.0, 1.0],
        });
    }
    let settled = sim.snapshot();
    assert!(settled.presentations.is_empty());
    assert!(settled.damage_numbers.is_empty());
    assert_eq!(settled.tick, reward.tick);
    assert_eq!(settled.elapsed, reward.elapsed);
    assert_eq!(settled.hero.position, reward.hero.position);
    assert_eq!(settled.hero.hp, reward.hero.hp);
    assert_eq!(settled.hero.memories, reward.hero.memories);
}

fn action_ids(snapshot: &DreamSnapshot, sequence: u32) -> Vec<engine_core::GraphicsId> {
    let mut ids: Vec<_> = snapshot
        .presentations
        .iter()
        .filter(|p| p.id.owner == 1 && p.id.action_seq == sequence && p.id.slot < 0x8000)
        .map(|p| p.id)
        .collect();
    ids.sort_by_key(|id| id.slot);
    ids
}

#[test]
fn reliable_cast_and_dash_ids_match_when_authority_accepts_three_ticks_later() {
    for kind in MemoryKind::ALL {
        for essence in [None, Some(EssenceKind::Echo), Some(EssenceKind::Twin)] {
            let mut initial = start(518);
            clear_enemies(&mut initial);
            enemy(&mut initial, EnemyKind::Boss, [14.0, -8.0]);
            memory(&mut initial, kind, essence);
            let mut predicted = DreamSimulation::from_snapshot(&initial.snapshot());
            let mut authority = DreamSimulation::from_snapshot(&initial.snapshot());
            let input = DreamInput {
                aim: [0.0, -1.0],
                casts: [true, false, false, false],
                action_sequences: [0, 771, 0, 0, 0],
                ..Default::default()
            };
            predicted.step(input);
            for _ in 0..3 {
                predicted.step(DreamInput::default());
                authority.step(DreamInput::default());
            }
            authority.step(input);
            let predicted_ids = action_ids(&predicted.snapshot(), 771);
            let authority_ids = action_ids(&authority.snapshot(), 771);
            assert!(
                !predicted_ids.is_empty(),
                "{kind:?}/{essence:?} missingacceptedvisual"
            );
            assert_eq!(
                predicted_ids, authority_ids,
                "{kind:?}/{essence:?} identitychangedwithlatency"
            );
            let predicted_main = predicted
                .snapshot()
                .presentations
                .into_iter()
                .find(|p| p.id == predicted_ids[0])
                .unwrap();
            let authority_main = authority
                .snapshot()
                .presentations
                .into_iter()
                .find(|p| p.id == authority_ids[0])
                .unwrap();
            assert_eq!(predicted_main.age_ticks, authority_main.age_ticks + 3);
            // Compare delayed repeat identities at the same age after their own cast.
            let delay = if essence == Some(EssenceKind::Echo) {
                36
            } else {
                14
            };
            for _ in 0..delay {
                predicted.step(DreamInput::default());
                authority.step(DreamInput::default());
            }
            if essence == Some(EssenceKind::Echo)
                || (essence == Some(EssenceKind::Twin)
                    && matches!(
                        kind,
                        MemoryKind::Crescent | MemoryKind::Nova | MemoryKind::Blink
                    ))
            {
                let predicted_repeat: Vec<_> = action_ids(&predicted.snapshot(), 771)
                    .into_iter()
                    .filter(|id| id.slot & 0x0800 != 0)
                    .collect();
                let authoritative_repeat: Vec<_> = action_ids(&authority.snapshot(), 771)
                    .into_iter()
                    .filter(|id| id.slot & 0x0800 != 0)
                    .collect();
                assert!(
                    !predicted_repeat.is_empty(),
                    "missing {kind:?}/{essence:?} repeat"
                );
                assert_eq!(predicted_repeat, authoritative_repeat);
            }
        }
    }
    let mut predicted = start(51);
    let mut authority = DreamSimulation::from_snapshot(&predicted.snapshot());
    let dash = DreamInput {
        dash: true,
        aim: [0.0, -1.0],
        action_sequences: [88, 0, 0, 0, 0],
        ..Default::default()
    };
    predicted.step(dash);
    for _ in 0..3 {
        authority.step(DreamInput::default());
    }
    authority.step(dash);
    assert_eq!(
        action_ids(&predicted.snapshot(), 88),
        action_ids(&authority.snapshot(), 88)
    );
}

#[test]
fn basic_swing_identity_uses_saved_counter_and_separate_slot_namespace() {
    let mut predicted = start(98);
    let mut authority = DreamSimulation::from_snapshot(&predicted.snapshot());
    let attack = DreamInput {
        attack: true,
        aim: [0.0, -1.0],
        ..Default::default()
    };
    predicted.step(attack);
    for _ in 0..3 {
        authority.step(DreamInput::default());
    }
    authority.step(attack);
    let find = |snapshot: DreamSnapshot| {
        snapshot
            .presentations
            .into_iter()
            .find(|p| p.id.owner == 1 && p.id.slot == 0x8000)
            .unwrap()
            .id
    };
    assert_eq!(find(predicted.snapshot()), find(authority.snapshot()));
    let saved = predicted.snapshot();
    let mut restored = DreamSimulation::from_snapshot(&saved);
    for _ in 0..22 {
        predicted.step(attack);
        restored.step(attack);
    }
    assert_eq!(predicted.snapshot(), restored.snapshot());
    assert!(
        predicted
            .snapshot()
            .presentations
            .iter()
            .any(|p| p.id.slot == 0x8000 && p.id.action_seq == 2)
    );
}

#[test]
fn world_ring_ids_use_actor_and_encounter_state_instead_of_wall_clock_tick() {
    let mut initial = start(503);
    clear_enemies(&mut initial);
    enemy(&mut initial, EnemyKind::Melee, [0.0, 0.5]);
    enemy(&mut initial, EnemyKind::Melee, [4.0, 0.0]);
    test_hero(&mut initial, |h| {
        h.motor.position = [0.0; 2];
        h.progression.xp = h.progression.xp_next;
    });
    let mut targets: Vec<_> = initial
        .world
        .query::<(Entity, EnemyActorReadOnly)>()
        .iter(&initial.world)
        .map(|(e, v)| (v.view.id, e))
        .collect();
    targets.sort_unstable();
    if let Some(mut action) = initial
        .world
        .get_mut::<engine_core::ActionState>(targets[0].1)
    {
        action.windup = DT * 0.5;
        action.target = [0.0; 2];
    }
    initial
        .world
        .get_mut::<Enemy>(targets[0].1)
        .unwrap()
        .view
        .warn_radius = 2.0;
    if let Some(mut second) = initial.world.get_mut::<Health>(targets[1].1) {
        second.hp = 0.0;
    }
    initial.world.resource_mut::<Run>().reinforcements = 8;
    let mut predicted = DreamSimulation::from_snapshot(&initial.snapshot());
    let mut authority = DreamSimulation::from_snapshot(&initial.snapshot());
    authority.world.resource_mut::<Run>().tick += 3;
    predicted.step(DreamInput::default());
    authority.step(DreamInput::default());
    let ids = |snapshot: DreamSnapshot| {
        snapshot
            .presentations
            .into_iter()
            .map(|p| p.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(predicted.snapshot()), ids(authority.snapshot()));
    let slots: Vec<_> = predicted
        .snapshot()
        .presentations
        .iter()
        .map(|p| p.id.slot)
        .collect();
    assert!(slots.contains(&0xc100));
    assert!(slots.contains(&0xc200));
    assert!(slots.contains(&0xc400));
    assert!(slots.iter().any(|slot| *slot >= 0xc500));
}

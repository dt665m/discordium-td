use crate::*;

fn edit_hero(sim: &mut DreamSimulation, id: u64, edit: impl FnOnce(&mut HeroActorItem<'_, '_>)) {
    let mut query = sim.world.query::<HeroActor>();
    edit(
        &mut query
            .iter_mut(&mut sim.world)
            .find(|h| h.view.id == id)
            .unwrap(),
    );
}
fn party(seed: u64) -> DreamSimulation {
    let mut sim = DreamSimulation::new(seed, false);
    assert!(sim.add_player(22));
    assert!(sim.continue_run_for(1));
    assert_eq!(sim.snapshot().phase, RunPhase::Intro);
    assert!(sim.snapshot().awaiting_party);
    assert!(sim.continue_run_for(22));
    assert_eq!(sim.snapshot().phase, RunPhase::Combat);
    sim
}
fn empty_arena(sim: &mut DreamSimulation) {
    let ids: Vec<_> = sim
        .world
        .query_filtered::<Entity, With<Enemy>>()
        .iter(&sim.world)
        .collect();
    for id in ids {
        sim.world.despawn(id);
    }
    sim.world.resource_mut::<Run>().reinforcements = 0;
}
fn add_enemy(sim: &mut DreamSimulation, kind: EnemyKind, position: [f32; 2]) {
    sim.world.resource_scope(|world, mut run: Mut<Run>| {
        let mut queue = bevy::ecs::world::CommandQueue::default();
        encounters::spawn_enemy(
            &mut Commands::new(&mut queue, world),
            &mut run,
            kind,
            position,
        );
        queue.apply(world);
    });
}
fn reward(kind: RewardKind) -> Reward {
    Reward {
        kind,
        rarity: Rarity::Rare,
        title: "Test blessing".into(),
        description: "Fixture".into(),
    }
}
fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn norm(v: [f32; 2]) -> [f32; 2] {
    let len = v[0].hypot(v[1]).max(0.001);
    [v[0] / len, v[1] / len]
}

#[test]
fn admission_empty_roster_and_party_restart() {
    let mut sim = DreamSimulation::new(33, false);
    assert!(sim.remove_player(1));
    sim.step_multiplayer(&[]);
    assert!(sim.snapshot().heroes.is_empty());
    assert_eq!(sim.snapshot().tick, 0);
    assert!(sim.add_player(65));
    assert!(sim.add_player(65));
    for id in [77, 88, 99] {
        assert!(sim.add_player(id));
    }
    assert!(sim.add_player(111));
    assert!(!sim.add_player(0));
    assert!(!sim.add_player(1 << 63));
    assert_eq!(sim.snapshot_for(65).hero.id, 65);
    assert_eq!(sim.snapshot().heroes.len(), 5);
    sim.restart_party(44, true);
    assert_eq!(
        sim.snapshot()
            .heroes
            .iter()
            .map(|h| h.id)
            .collect::<Vec<_>>(),
        [65, 77, 88, 99, 111]
    );
    assert!(sim.snapshot().lucid);
    assert_eq!(sim.snapshot().phase, RunPhase::Intro);
}

#[test]
fn late_join_clones_progression_loadout_and_stats_but_refreshes_combat() {
    let mut sim = party(39);
    edit_hero(&mut sim, 1, |h| {
        h.health.hp = 30.0;
        h.health.max_hp = 300.0;
        h.progression.level = 4;
        h.progression.xp = 12.0;
        h.view.attack_power = 2.0;
        h.view.shards = 77;
        h.motor.position = [3.0, 4.0];
        h.motor.dash_cooldown = 0.8;
        h.action.recovery = 0.7;
        h.combat.shield = 21.0;
        h.combat.shield_remaining = 3.0;
        h.loadout.0[0] = memory_slot(MemoryKind::Wisp);
        h.loadout.0[0].modifier = Some(EssenceKind::Twin);
        h.loadout.0[0].level = 3;
        h.loadout.0[0].cooldown = 5.0;
    });
    assert!(sim.add_player(33));
    let joined = sim.snapshot_for(33).hero;
    assert_eq!(joined.hp, 300.0);
    assert_eq!(joined.level, 4);
    assert_eq!(joined.xp, 12.0);
    assert_eq!(joined.attack_power, 2.0);
    assert_eq!(joined.shards, 77);
    assert_eq!(joined.position, [4.5, 4.0]);
    assert_eq!(joined.memories[0].kind, MemoryKind::Wisp);
    assert_eq!(joined.memories[0].essence, Some(EssenceKind::Twin));
    assert_eq!(joined.memories[0].level, 3);
    assert_eq!(joined.memories[0].cooldown, 0.0);
    assert_eq!(joined.attack_cooldown, 0.0);
    assert_eq!(joined.dash_cooldown, 0.0);
    assert_eq!(joined.shield, 0.0);
    assert!(joined.invulnerable);
}

#[test]
fn players_move_independently_and_duplicate_tick_inputs_execute_once() {
    let mut sim = party(11);
    let before = sim.snapshot();
    let right = DreamInput {
        action_sequences: [0; 5],
        movement: [1.0, 0.0],
        ..Default::default()
    };
    let left = DreamInput {
        action_sequences: [0; 5],
        movement: [-1.0, 0.0],
        ..Default::default()
    };
    sim.step_multiplayer(&[(1, right), (1, left), (22, left), (555, right)]);
    let after = sim.snapshot();
    assert!(after.heroes[0].position[0] > before.heroes[0].position[0]);
    assert!(after.heroes[1].position[0] < before.heroes[1].position[0]);
    assert!((after.heroes[0].position[0] - before.heroes[0].position[0] - 7.8 * DT).abs() < 0.001);
    assert_eq!(after.heroes.len(), 2);
    let h1 = sim.snapshot_for(1);
    let h2 = sim.snapshot_for(22);
    assert_eq!(h1.enemies, h2.enemies);
    assert_eq!(h1.heroes, h2.heroes);
    assert_ne!(h1.hero.id, h2.hero.id);
}

#[test]
fn two_attacks_damage_one_shared_enemy_and_have_distinct_owners() {
    let mut sim = party(4);
    empty_arena(&mut sim);
    add_enemy(&mut sim, EnemyKind::Boss, [0.0, -2.0]);
    for id in [1, 22] {
        edit_hero(&mut sim, id, |h| {
            h.motor.position = [0.0; 2];
            h.view.critical_chance = 0.0;
        });
    }
    let before = sim.snapshot().enemies[0].hp;
    let input = DreamInput {
        action_sequences: [0; 5],
        attack: true,
        aim: [0.0, -1.0],
        ..Default::default()
    };
    sim.step_multiplayer(&[(22, input), (1, input)]);
    let snapshot = sim.snapshot();
    assert!((before - snapshot.enemies[0].hp - 48.0).abs() < 0.01);
    assert!(snapshot.presentations.iter().any(|p| p.id.owner == 1));
    assert!(snapshot.presentations.iter().any(|p| p.id.owner == 22));
    let mut ids: Vec<_> = snapshot
        .presentations
        .iter()
        .map(|p| (p.id.owner, p.id.action_seq, p.id.slot))
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), snapshot.presentations.len());
}

#[test]
fn enemies_target_living_players_and_area_attacks_hit_the_party() {
    let mut sim = party(4);
    empty_arena(&mut sim);
    add_enemy(&mut sim, EnemyKind::Melee, [0.0, 0.5]);
    edit_hero(&mut sim, 1, |h| {
        h.motor.position = [12.0, 12.0];
        h.health.hp = 0.0;
    });
    edit_hero(&mut sim, 22, |h| {
        h.motor.position = [0.0; 2];
        h.combat.invulnerability_remaining = 0.0;
    });
    for mut e in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        e.action.recovery = 0.0;
    }
    sim.step_multiplayer(&[]);
    assert_eq!(sim.snapshot().enemies[0].target, [0.0; 2]);
    for id in [1, 22] {
        edit_hero(&mut sim, id, |h| {
            h.health.hp = 220.0;
            h.motor.position = [0.0; 2];
            h.combat.invulnerability_remaining = 0.0;
        });
    }
    for mut e in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        e.action.windup = DT * 0.5;
        e.action.target = [0.0; 2];
        e.view.warn_radius = 3.0;
    }
    sim.step_multiplayer(&[(
        22,
        DreamInput {
            action_sequences: [0; 5],
            dash: true,
            aim: [0.0, -1.0],
            ..Default::default()
        },
    )]);
    assert_eq!(sim.snapshot_for(1).hero.hp, 203.0);
    assert_eq!(sim.snapshot_for(22).hero.hp, 220.0);
}

#[test]
fn individual_rewards_readiness_and_departures_do_not_cross_assign() {
    let mut sim = party(44);
    empty_arena(&mut sim);
    sim.step_multiplayer(&[]);
    edit_hero(&mut sim, 1, |h| {
        h.rewards = vec![reward(RewardKind::Memory(MemoryKind::Wisp))]
    });
    edit_hero(&mut sim, 22, |h| {
        h.rewards = vec![reward(RewardKind::Essence(EssenceKind::Frost))]
    });
    let other_before = sim.snapshot_for(22).hero.memories;
    assert!(sim.choose_reward_for(1, 0, 2));
    assert!(!sim.choose_reward_for(1, 0, 2));
    assert!(sim.snapshot_for(1).awaiting_party);
    assert!(sim.snapshot_for(1).rewards.is_empty());
    assert_eq!(sim.snapshot_for(22).hero.memories, other_before);
    assert_eq!(sim.snapshot().phase, RunPhase::Reward);
    assert!(sim.choose_reward_for(22, 0, 1));
    assert_eq!(sim.snapshot().phase, RunPhase::Transition);
    assert_eq!(sim.snapshot_for(1).hero.memories[2].kind, MemoryKind::Wisp);
    assert_eq!(
        sim.snapshot_for(22).hero.memories[1].essence,
        Some(EssenceKind::Frost)
    );
    assert!(!sim.continue_run_for(900));
    assert!(sim.continue_run_for(1));
    assert_eq!(sim.snapshot().phase, RunPhase::Transition);
    assert!(sim.snapshot().awaiting_party);
    assert!(sim.remove_player(22));
    assert_eq!(sim.snapshot().phase, RunPhase::Combat);
    assert_eq!(sim.snapshot().room, 1);
}

#[test]
fn fallen_teammates_cannot_act_and_revive_only_after_shared_clear() {
    let mut sim = party(15);
    edit_hero(&mut sim, 1, |h| {
        h.health.hp = 0.0;
    });
    let before = sim.snapshot_for(1).hero.position;
    sim.step_multiplayer(&[(
        1,
        DreamInput {
            action_sequences: [0; 5],
            movement: [1.0; 2],
            attack: true,
            dash: true,
            casts: [true; 4],
            ..Default::default()
        },
    )]);
    assert_eq!(sim.snapshot().phase, RunPhase::Combat);
    assert_eq!(sim.snapshot_for(1).hero.hp, 0.0);
    assert_eq!(sim.snapshot_for(1).hero.position, before);
    empty_arena(&mut sim);
    sim.step_multiplayer(&[]);
    assert_eq!(sim.snapshot().phase, RunPhase::Reward);
    assert_eq!(
        sim.snapshot_for(1).hero.hp,
        sim.snapshot_for(1).hero.max_hp * 0.5
    );
    let mut doomed = party(16);
    for id in [1, 22] {
        edit_hero(&mut doomed, id, |h| h.health.hp = 0.0);
    }
    doomed.step_multiplayer(&[]);
    assert_eq!(doomed.snapshot().phase, RunPhase::Defeat);
}

#[test]
fn projectile_leech_and_summons_belong_to_the_casting_player() {
    let mut sim = party(9);
    empty_arena(&mut sim);
    add_enemy(&mut sim, EnemyKind::Boss, [0.0, -2.0]);
    for id in [1, 22] {
        edit_hero(&mut sim, id, |h| {
            h.health.hp = 100.0;
            h.motor.position = [0.0; 2];
            h.view.critical_chance = 0.0;
        });
    }
    edit_hero(&mut sim, 22, |h| {
        h.loadout.0[0] = memory_slot(MemoryKind::Starfall);
        h.loadout.0[0].modifier = Some(EssenceKind::Leech);
    });
    sim.step_multiplayer(&[(
        22,
        DreamInput {
            action_sequences: [0; 5],
            aim: [0.0, -1.0],
            casts: [true, false, false, false],
            ..Default::default()
        },
    )]);
    assert!(sim.snapshot_for(22).hero.hp > 100.0);
    assert_eq!(sim.snapshot_for(1).hero.hp, 100.0);
    edit_hero(&mut sim, 22, |h| {
        h.loadout.0[0] = memory_slot(MemoryKind::Wisp);
        h.loadout.0[0].modifier = Some(EssenceKind::Echo);
    });
    sim.step_multiplayer(&[(
        22,
        DreamInput {
            action_sequences: [0; 5],
            casts: [true, false, false, false],
            ..Default::default()
        },
    )]);
    assert_eq!(sim.snapshot().wisps[0].owner, 22);
    assert_eq!(sim.snapshot().state.delayed[0].payload.owner, 22);
    sim.remove_player(22);
    assert!(sim.snapshot().wisps.is_empty());
    assert!(sim.snapshot().state.delayed.is_empty());
}

#[test]
fn multiplayer_json_restore_and_reordered_input_replay_match_exactly() {
    let mut sim = party(188);
    sim.add_player(31);
    sim.add_player(43);
    let encoded = serde_json::to_vec(&sim.snapshot_for(22)).unwrap();
    let snapshot: DreamSnapshot = serde_json::from_slice(&encoded).unwrap();
    let mut replay = DreamSimulation::from_snapshot(&snapshot);
    for tick in 0..420 {
        let inputs: Vec<_> = [1, 22, 31, 43]
            .map(|id| {
                (
                    id,
                    DreamInput {
                        action_sequences: [0; 5],
                        movement: [(tick as f32 * 0.04 + id as f32).sin(), -0.3],
                        aim: [0.2, -1.0],
                        attack: true,
                        dash: tick % 119 == 0,
                        casts: [
                            tick % 41 == 0,
                            tick % 23 == 0,
                            tick % 71 == 0,
                            tick % 131 == 0,
                        ],
                    },
                )
            })
            .into();
        let mut reverse = inputs.clone();
        reverse.reverse();
        sim.step_multiplayer(&inputs);
        replay.step_multiplayer(&reverse);
        assert_eq!(
            sim.snapshot_for(22),
            replay.snapshot_for(22),
            "replay diverged tick {tick}"
        );
    }
}

fn bot_input(snapshot: &DreamSnapshot) -> DreamInput {
    if snapshot.hero.hp <= 0.0 {
        return DreamInput::default();
    }
    let Some(target) = snapshot.enemies.iter().min_by(|a, b| {
        dist(a.position, snapshot.hero.position)
            .total_cmp(&dist(b.position, snapshot.hero.position))
    }) else {
        return DreamInput::default();
    };
    let direction = norm([
        target.position[0] - snapshot.hero.position[0],
        target.position[1] - snapshot.hero.position[1],
    ]);
    let mut movement = if dist(target.position, snapshot.hero.position) > 2.6 {
        direction
    } else {
        [0.0; 2]
    };
    let danger = snapshot
        .enemies
        .iter()
        .filter(|e| e.windup > 0.0 && e.kind != EnemyKind::Ranged)
        .find(|e| dist(e.target, snapshot.hero.position) < e.warn_radius + 1.3);
    if let Some(danger) = danger {
        let away = [
            snapshot.hero.position[0] - danger.target[0],
            snapshot.hero.position[1] - danger.target[1],
        ];
        movement = if away[0].hypot(away[1]) > 0.2 {
            norm(away)
        } else {
            [-direction[1], direction[0]]
        };
    }
    DreamInput {
        action_sequences: [0; 5],
        movement,
        aim: direction,
        attack: true,
        dash: danger.is_some_and(|e| e.windup < 0.35),
        casts: [
            dist(target.position, snapshot.hero.position) < 4.8,
            true,
            dist(target.position, snapshot.hero.position) < 5.0,
            snapshot.hero.shield < 10.0,
        ],
    }
}

#[test]
fn full_shared_cooperative_runs_reach_every_boss_phase_and_victory() {
    for seed in [7, 83] {
        let mut sim = party(seed);
        let mut boss_phases = vec![];
        for _ in 0..100_000 {
            let snapshot = sim.snapshot();
            match snapshot.phase {
                RunPhase::Victory | RunPhase::Defeat => break,
                RunPhase::Combat => {
                    for boss in snapshot
                        .enemies
                        .iter()
                        .filter(|e| e.kind == EnemyKind::Boss)
                    {
                        if !boss_phases.contains(&boss.phase) {
                            boss_phases.push(boss.phase);
                        }
                    }
                    let inputs = [
                        (1, bot_input(&snapshot)),
                        (22, bot_input(&sim.snapshot_for(22))),
                    ];
                    sim.step_multiplayer(&inputs);
                }
                RunPhase::Intro | RunPhase::Transition => {
                    sim.continue_run_for(1);
                    sim.continue_run_for(22);
                }
                RunPhase::Rest | RunPhase::Reward => {
                    for id in [1, 22] {
                        let local = sim.snapshot_for(id);
                        if local.phase != snapshot.phase || local.ready {
                            continue;
                        }
                        if local.phase == RunPhase::Rest {
                            for slot in 0..4 {
                                sim.buy_memory_upgrade_for(id, slot);
                            }
                        }
                        let choice = local
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
                        sim.choose_reward_for(id, choice, 0);
                    }
                }
            }
        }
        let final_state = sim.snapshot();
        println!(
            "co-op seed{seed}: {:?} {:.1}s {}kills heroHP={:?}",
            final_state.phase,
            final_state.elapsed,
            final_state.kills,
            final_state.heroes.iter().map(|h| h.hp).collect::<Vec<_>>()
        );
        assert_eq!(final_state.phase, RunPhase::Victory);
        assert_eq!(boss_phases, [1, 2, 3]);
        assert_eq!(final_state.heroes.len(), 2);
        assert!(final_state.kills > 90);
        assert_eq!(sim.snapshot_for(1).enemies, sim.snapshot_for(22).enemies);
    }
}

#[test]
fn eight_player_inputs_shared_enemies_and_independent_rewards() {
    let mut sim = DreamSimulation::new(80, false);
    for id in 2..=8 {
        assert!(sim.add_player(id));
    }
    for id in 1..=8 {
        assert!(sim.continue_run_for(id));
    }
    assert_eq!(sim.snapshot().phase, RunPhase::Combat);
    assert_eq!(sim.snapshot().heroes.len(), 8);
    let before = sim.snapshot();
    let inputs: Vec<_> = (1..=8)
        .map(|id| {
            (
                id,
                DreamInput {
                    action_sequences: [0; 5],
                    movement: [if id % 2 == 0 { 1.0 } else { -1.0 }, 0.0],
                    aim: [0.0, -1.0],
                    casts: [false, true, false, false],
                    ..Default::default()
                },
            )
        })
        .collect();
    sim.step_multiplayer(&inputs);
    let after = sim.snapshot();
    for (before, after) in before.heroes.iter().zip(&after.heroes) {
        assert_ne!(before.position, after.position);
        assert!(after.memories[1].cooldown > 0.0);
    }
    assert_eq!(after.projectiles.len(), 8);
    for id in 1..=8 {
        assert!(after.projectiles.iter().any(|p| p.owner == id));
        assert_eq!(sim.snapshot_for(id).enemies, after.enemies);
    }
    empty_arena(&mut sim);
    sim.step_multiplayer(&[]);
    for id in 1..=8 {
        edit_hero(&mut sim, id, |h| {
            h.rewards = vec![reward(RewardKind::Essence(if id % 2 == 0 {
                EssenceKind::Frost
            } else {
                EssenceKind::Twin
            }))]
        });
    }
    for id in 1..=8 {
        assert!(sim.choose_reward_for(id, 0, (id as usize - 1) % 4));
        if id < 8 {
            assert_eq!(sim.snapshot().phase, RunPhase::Reward);
        }
    }
    assert_eq!(sim.snapshot().phase, RunPhase::Transition);
    for id in 1..=8 {
        let local = sim.snapshot_for(id);
        assert_eq!(
            local.hero.memories[(id as usize - 1) % 4].essence,
            Some(if id % 2 == 0 {
                EssenceKind::Frost
            } else {
                EssenceKind::Twin
            })
        );
        assert_eq!(
            local
                .hero
                .memories
                .iter()
                .filter(|m| m.essence.is_some())
                .count(),
            1
        );
    }
}

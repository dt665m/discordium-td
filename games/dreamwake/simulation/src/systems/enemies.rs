use super::*;

pub(crate) fn hit_party_in_area(
    batch: &mut CombatBatch,
    action: CombatActionKey,
    heroes: &mut Query<HeroActor, Without<Enemy>>,
    center: [f32; 2],
    radius: f32,
    damage: f32,
) {
    let mut party: Vec<_> = heroes.iter_mut().collect();
    party.sort_by_key(|h| h.view.id);
    for hero in party {
        if hero.active
            && engine_core::contains_circle(center, radius, planar_position(&hero.motion))
        {
            hit_hero(batch, action, &hero, damage);
        }
    }
}

pub(crate) fn enemy_actions(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut batch: ResMut<CombatBatch>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<(Entity, EnemyActor), Without<Hero>>,
) {
    let mut ordered: Vec<_> = enemies.iter().map(|(e, v)| (v.view.id, e)).collect();
    ordered.sort_unstable();
    let mut support_pulses = Vec::new();
    for (_, entity) in ordered {
        let Ok((_, mut enemy)) = enemies.get_mut(entity) else {
            continue;
        };
        if enemy.health.hp <= 0.0 {
            continue;
        }
        // Detection only acquires a target. A living active target stays latched
        // beyond detection range and is not replaced by a nearer party member.
        let retained = enemy.ai.target.and_then(|id| {
            heroes
                .iter()
                .find(|h| h.view.id == id && h.active && h.health.hp > 0.0)
                .map(|h| (h.view.id, planar_position(&h.motion)))
        });
        let target = retained.or_else(|| {
            engine_core::nearest_target(
                enemy.motor.position,
                f32::INFINITY,
                2,
                heroes.iter().map(|h| engine_core::CollisionTarget {
                    id: h.view.id,
                    faction: 1,
                    position: planar_position(&h.motion),
                    radius: 0.48,
                    // Detection includes the exact boundary.
                    active: h.active
                        && h.health.hp > 0.0
                        && Vec2::from_array(enemy.motor.position)
                            .distance_squared(Vec2::from_array(planar_position(&h.motion)))
                            <= ENEMY_AGGRO_RANGE * ENEMY_AGGRO_RANGE,
                }),
            )
            .map(|target| (target.id, target.position))
        });
        let Some((target_id, hero_position)) = target else {
            if enemy.ai.target.is_some() {
                enemy.ai.target = None;
            }
            if enemy.ai.awake {
                enemy.ai.awake = false;
            }
            if !enemy.action.idle() {
                enemy.action.cancel();
            }
            if enemy.view.warn_radius != 0.0 {
                enemy.view.warn_radius = 0.0;
            }
            if enemy.motor.velocity != [0.0; 2] {
                enemy.motor.velocity = [0.0; 2];
            }
            continue;
        };
        if enemy.ai.target != Some(target_id) {
            enemy.ai.target = Some(target_id);
        }
        if !enemy.ai.awake {
            let delay = enemy.ai.wake_delay;
            enemy.ai.awake = true;
            enemy.ai.wake_delay = 0.0;
            if delay > 0.0 && enemy.action.idle() {
                enemy.action.begin_recovery(delay);
            }
        }
        let kind = enemy.view.kind;
        if kind == EnemyKind::Boss {
            let phase = if enemy.health.hp < enemy.health.max_hp * 0.34 {
                3
            } else if enemy.health.hp < enemy.health.max_hp * 0.68 {
                2
            } else {
                1
            };
            if phase > enemy.view.phase {
                enemy.view.phase = phase;
                enemy.action.cancel();
                enemy.action.begin_recovery(1.4);
                run.message = format!(
                    "The Somnarch · Dream {} · {}",
                    phase,
                    if phase == 2 {
                        "Shattered Constellations"
                    } else {
                        "The Last Unraveling"
                    }
                );
                action_effect(
                    &mut commands,
                    &run,
                    enemy.view.id,
                    ActionEffect {
                        sequence: phase as u32,
                        slot: 0xc300,
                    },
                    enemy.motor.position,
                    6.0,
                    65,
                );
                for offset in [[-5.0, 0.0], [5.0, 0.0]] {
                    let pos = clamp(add(enemy.motor.position, offset));
                    encounters::spawn_enemy(
                        &mut commands,
                        &mut run,
                        if phase == 2 {
                            EnemyKind::Melee
                        } else {
                            EnemyKind::Ambusher
                        },
                        pos,
                    );
                }
            }
        }
        if enemy.action.windup > 0.0 || enemy.action.due {
            if enemy.action.take_due() {
                let origin = enemy.motor.position;
                let target = enemy.action.target;
                match kind {
                    EnemyKind::Ranged => {
                        projectile(
                            &mut commands,
                            &mut run,
                            enemy.view.id,
                            origin,
                            sub(target, origin),
                            17.0,
                            false,
                            None,
                            9.5,
                            0.35,
                        );
                    }
                    EnemyKind::Support => {
                        hit_party_in_area(
                            &mut batch,
                            CombatActionKey::new(
                                run.tick,
                                enemy.view.id,
                                enemy.combat_identity.generation,
                                2,
                                u64::from(enemy.attack_index),
                                0,
                            ),
                            &mut heroes,
                            target,
                            enemy.view.warn_radius,
                            20.0,
                        );
                        support_pulses.push(origin);
                        action_effect(
                            &mut commands,
                            &run,
                            enemy.view.id,
                            ActionEffect {
                                sequence: enemy.attack_index,
                                slot: 0xc200,
                            },
                            target,
                            enemy.view.warn_radius,
                            27,
                        );
                    }
                    EnemyKind::Boss => {
                        hit_party_in_area(
                            &mut batch,
                            CombatActionKey::new(
                                run.tick,
                                enemy.view.id,
                                enemy.combat_identity.generation,
                                2,
                                u64::from(enemy.attack_index),
                                0,
                            ),
                            &mut heroes,
                            target,
                            enemy.view.warn_radius,
                            33.0,
                        );
                        action_effect(
                            &mut commands,
                            &run,
                            enemy.view.id,
                            ActionEffect {
                                sequence: enemy.attack_index,
                                slot: 0xc200,
                            },
                            target,
                            enemy.view.warn_radius,
                            36,
                        );
                        let count = 8 + (enemy.view.phase as usize - 1) * 4;
                        let offset = enemy.attack_index as f32 * 0.31;
                        for i in 0..count {
                            let angle = i as f32 / count as f32 * TAU + offset;
                            projectile(
                                &mut commands,
                                &mut run,
                                enemy.view.id,
                                origin,
                                [angle.cos(), angle.sin()],
                                17.0,
                                false,
                                None,
                                7.0 + enemy.view.phase as f32 * 0.7,
                                0.32,
                            );
                        }
                        if enemy.view.phase == 3
                            && enemy.attack_index.is_multiple_of(3)
                            && enemy.attack_index < 15
                        {
                            encounters::spawn_enemy(
                                &mut commands,
                                &mut run,
                                EnemyKind::Ranged,
                                clamp(add(origin, [5.0, -3.0])),
                            );
                        }
                    }
                    _ => {
                        if kind == EnemyKind::Ambusher || kind == EnemyKind::Elite {
                            if let Err(error) = enemy.combat_identity.discontinuity(false) {
                                batch.error = Some(error);
                                continue;
                            }
                            enemy.motor.position = clamp(target);
                        }
                        let damage = match kind {
                            EnemyKind::Ambusher => 19.0,
                            EnemyKind::Elite => 32.0,
                            _ => 17.0,
                        };
                        hit_party_in_area(
                            &mut batch,
                            CombatActionKey::new(
                                run.tick,
                                enemy.view.id,
                                enemy.combat_identity.generation,
                                2,
                                u64::from(enemy.attack_index),
                                0,
                            ),
                            &mut heroes,
                            target,
                            enemy.view.warn_radius + 0.25,
                            damage,
                        );
                        action_effect(
                            &mut commands,
                            &run,
                            enemy.view.id,
                            ActionEffect {
                                sequence: enemy.attack_index,
                                slot: 0xc200,
                            },
                            target,
                            enemy.view.warn_radius,
                            22,
                        );
                        if kind == EnemyKind::Elite {
                            for i in 0..6 {
                                let angle = i as f32 / 6.0 * TAU + enemy.attack_index as f32 * 0.5;
                                projectile(
                                    &mut commands,
                                    &mut run,
                                    enemy.view.id,
                                    target,
                                    [angle.cos(), angle.sin()],
                                    12.0,
                                    false,
                                    None,
                                    7.0,
                                    0.30,
                                );
                            }
                        }
                    }
                }
                enemy.attack_index += 1;
                let recovery = match kind {
                    EnemyKind::Melee => 0.90,
                    EnemyKind::Ranged => 1.40,
                    EnemyKind::Ambusher => 1.45,
                    EnemyKind::Support => 2.8,
                    EnemyKind::Elite => 1.7,
                    EnemyKind::Boss => 2.0 - enemy.view.phase as f32 * 0.25,
                };
                enemy.action.begin_recovery(recovery);
            }
            continue;
        }
        if enemy.action.recovery > 0.0 {
            continue;
        }
        let to_hero = sub(hero_position, enemy.motor.position);
        let dist = length(to_hero);
        let direction = normalize(to_hero);
        enemy.motor.facing = direction;
        let (range, speed, windup, radius) = match kind {
            EnemyKind::Melee => (2.35, 3.1, 0.55, 1.9),
            EnemyKind::Ranged => (12.0, 2.5, 0.80, 1.0),
            EnemyKind::Ambusher => (6.5, 4.6, 0.72, 1.7),
            EnemyKind::Support => (12.0, 2.0, 1.15, 3.0),
            EnemyKind::Elite => (5.5, 2.9, 1.0, 3.1),
            EnemyKind::Boss => (25.0, 1.6, 1.05, 4.2 + enemy.view.phase as f32 * 0.35),
        };
        if dist < range && enemy.action.commit(hero_position, windup) {
            enemy.view.warn_radius = radius;
            continue;
        }
        let stop_range = if matches!(kind, EnemyKind::Ranged | EnemyKind::Support) {
            8.5
        } else if kind == EnemyKind::Boss {
            6.0
        } else {
            1.8
        };
        if dist > stop_range {
            let amount = speed
                * DT
                * if enemy.combat.status(FROST_STATUS).is_some() {
                    0.45
                } else {
                    1.0
                };
            enemy.motor.position = clamp(add(enemy.motor.position, scale(direction, amount)));
        } else if kind == EnemyKind::Ranged && dist < 5.5 {
            enemy.motor.position = clamp(add(
                enemy.motor.position,
                scale(direction, -speed * DT * 0.7),
            ));
        }
    }
    for center in support_pulses {
        for (_, mut enemy) in &mut enemies {
            if enemy.health.hp > 0.0
                && enemy.view.kind != EnemyKind::Boss
                && engine_core::contains_circle(center, 7.0, enemy.motor.position)
            {
                enemy.health.hp =
                    (enemy.health.hp + enemy.health.max_hp * 0.09).min(enemy.health.max_hp);
            }
        }
    }
}

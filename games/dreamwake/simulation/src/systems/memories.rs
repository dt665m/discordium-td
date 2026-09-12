use super::*;

pub(crate) fn execute_memory(
    commands: &mut Commands,
    run: &mut Run,
    batch: &mut CombatBatch,
    hero: &mut HeroActorItem<'_, '_>,
    enemies: &mut Query<(Entity, EnemyActor), Without<Hero>>,
    collision: &CollisionWorld,
    kind: MemoryKind,
    essence: Option<EssenceKind>,
    power: f32,
    origin: [f32; 2],
    direction: [f32; 2],
    source: ActionEffect,
) {
    let vast = if essence == Some(EssenceKind::Vast) {
        1.55
    } else {
        1.0
    };
    hero.view.attack_flash = 0.22;
    match kind {
        MemoryKind::Starfall => {
            action_effect(
                commands,
                run,
                hero.view.id,
                source,
                add(origin, scale(direction, 0.9)),
                0.8,
                12,
            );
            let angles: &[f32] = if essence == Some(EssenceKind::Twin) {
                &[-0.19, 0.0, 0.19]
            } else {
                &[0.0]
            };
            for angle in angles {
                projectile(
                    commands,
                    run,
                    hero.view.id,
                    add(origin, scale(direction, 0.9)),
                    rotate(direction, *angle),
                    38.0 * power,
                    true,
                    essence,
                    23.0,
                    0.34 * vast,
                );
            }
        }
        MemoryKind::Wisp => {
            let count = if essence == Some(EssenceKind::Twin) {
                2
            } else {
                1
            };
            for i in 0..count {
                spawn_wisp(
                    commands,
                    run,
                    hero.view.id,
                    origin,
                    essence,
                    15.0 * power,
                    i,
                );
            }
            action_effect(commands, run, hero.view.id, source, origin, 1.8, 28);
        }
        _ => {
            let (center, radius, damage) = match kind {
                MemoryKind::Crescent => (origin, 5.0 * vast, 48.0 * power),
                MemoryKind::Nova => (origin, 5.0 * vast, 52.0 * power),
                MemoryKind::Blink => {
                    if let Err(error) = collision.blink(&mut hero.motion, direction, 6.0 * vast) {
                        hero.motion_status.last_error = Some(error);
                        return;
                    }
                    hero.charge.state.interrupt();
                    if let Err(error) = hero.combat_identity.discontinuity(false) {
                        batch.error = Some(error);
                        return;
                    }
                    hero.combat
                        .grant_invulnerability(0.36, engine_core::DurationPolicy::Reset);
                    action_effect(
                        commands,
                        run,
                        hero.view.id,
                        source.offset(1),
                        origin,
                        1.5,
                        24,
                    );
                    (planar_position(&hero.motion), 2.8 * vast, 36.0 * power)
                }
                MemoryKind::Aegis => {
                    let amount = 55.0
                        * power
                        * if essence == Some(EssenceKind::Twin) {
                            1.5
                        } else {
                            vast
                        };
                    let duration = if essence == Some(EssenceKind::Haste) {
                        10.0
                    } else {
                        7.0
                    };
                    hero.combat.grant_shield(
                        amount,
                        duration,
                        engine_core::ShieldPolicy::AddCapped(hero.health.max_hp * 1.5),
                        engine_core::DurationPolicy::Reset,
                    );
                    if essence == Some(EssenceKind::Leech) {
                        heal(hero, 18.0 * power);
                    }
                    (origin, 3.8 * vast, 22.0 * power)
                }
                _ => unreachable!(),
            };
            let mut ordered: Vec<_> = enemies.iter().map(|(e, v)| (v.view.id, e)).collect();
            ordered.sort_unstable();
            for (_, entity) in ordered {
                let Ok((_, mut enemy)) = enemies.get_mut(entity) else {
                    continue;
                };
                let offset = sub(enemy.motor.position, center);
                let toward = normalize(offset);
                let in_area = if kind == MemoryKind::Crescent {
                    engine_core::contains_cone(
                        center,
                        direction,
                        radius + 0.5,
                        -0.30,
                        enemy.motor.position,
                    )
                } else {
                    engine_core::contains_circle(center, radius + 0.5, enemy.motor.position)
                };
                if in_area {
                    hit_enemy(
                        batch,
                        CombatActionKey::new(
                            run.tick,
                            hero.view.id,
                            hero.combat_identity.generation,
                            1,
                            u64::from(source.sequence),
                            source.slot,
                        ),
                        hero,
                        &mut enemy,
                        damage,
                        essence,
                        scale(toward, 1.1),
                        true,
                    );
                }
            }
            action_effect(
                commands,
                run,
                hero.view.id,
                source,
                center,
                radius,
                if kind == MemoryKind::Nova { 30 } else { 22 },
            );
            if essence == Some(EssenceKind::Twin) && kind != MemoryKind::Aegis {
                let id = run.id();
                commands.spawn((
                    DreamOwned,
                    DelayedCast::new(
                        0.18,
                        CastPayload {
                            spawn: None,
                            action_sequence: source.sequence,
                            presentation_slot: source.repeat().slot,
                            owner: hero.view.id,
                            id,
                            kind: if kind == MemoryKind::Blink {
                                MemoryKind::Nova
                            } else {
                                kind
                            },
                            essence: None,
                            power: power * 0.60,
                            origin: center,
                            direction,
                        },
                    ),
                ));
            }
        }
    }
}

pub(crate) fn delayed_casts(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut batch: ResMut<CombatBatch>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<(Entity, EnemyActor), Without<Hero>>,
    delayed: Query<(Entity, &DelayedCast)>,
    collision: Res<crate::platform::MotionEnvironment>,
) {
    let mut ordered: Vec<_> = delayed.iter().map(|(e, v)| (v.payload.id, e)).collect();
    ordered.sort_unstable();
    for (_, entity) in ordered {
        let Ok((_, delayed)) = delayed.get(entity) else {
            continue;
        };
        if delayed.ready {
            let cast = &delayed.payload;
            if let Some(mut hero) = heroes
                .iter_mut()
                .find(|h| h.view.id == cast.owner && h.health.hp > 0.0)
            {
                if let Some((key, origin_tick)) = cast.spawn {
                    let mut flight = crate::starfall::flight(
                        key,
                        origin_tick,
                        cast.origin,
                        cast.direction,
                        None,
                    );
                    flight.authority_id = Some(cast.id);
                    spawn_starfall(&mut commands, &mut run, flight, 38.0 * cast.power);
                    action_effect(
                        &mut commands,
                        &run,
                        cast.owner,
                        ActionEffect {
                            sequence: cast.action_sequence,
                            slot: cast.presentation_slot,
                        },
                        add(cast.origin, scale(cast.direction, 0.9)),
                        0.8,
                        12,
                    );
                } else {
                    execute_memory(
                        &mut commands,
                        &mut run,
                        &mut batch,
                        &mut hero,
                        &mut enemies,
                        collision.collision(),
                        cast.kind,
                        cast.essence,
                        cast.power,
                        cast.origin,
                        cast.direction,
                        ActionEffect {
                            sequence: cast.action_sequence,
                            slot: cast.presentation_slot,
                        },
                    );
                }
            }
            commands.entity(entity).despawn();
        }
    }
}

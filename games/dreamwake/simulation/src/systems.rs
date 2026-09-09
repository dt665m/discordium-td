use super::*;
use std::f32::consts::TAU;

use engine_core::spatial::{add, distance, length, normalize, scale, sub};

fn clamp(position: [f32; 2]) -> [f32; 2] {
    let len = length(position);
    if len > ARENA_RADIUS - 0.8 {
        scale(position, (ARENA_RADIUS - 0.8) / len)
    } else {
        position
    }
}
fn rotate(direction: [f32; 2], angle: f32) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    [
        direction[0] * c - direction[1] * s,
        direction[0] * s + direction[1] * c,
    ]
}
fn decrement(timer: &mut f32) {
    engine_core::advance_cooldown(timer, DT);
}

#[derive(Clone, Copy)]
struct ActionEffect {
    sequence: u32,
    slot: u16,
}
impl ActionEffect {
    fn input(sequence: u32, tick: u32, slot: u16) -> Self {
        Self {
            sequence: if sequence == 0 { tick } else { sequence },
            slot,
        }
    }
    fn repeat(self) -> Self {
        Self {
            slot: self.slot | 0x0800,
            ..self
        }
    }
    fn offset(self, offset: u16) -> Self {
        Self {
            slot: self.slot + offset,
            ..self
        }
    }
}

fn action_effect(
    commands: &mut Commands,
    run: &Run,
    owner: u64,
    source: ActionEffect,
    position: [f32; 2],
    radius: f32,
    duration: u32,
) {
    commands.spawn((
        DreamOwned,
        engine_core::PresentationInstance {
            id: engine_core::PresentationId {
                match_epoch: (run.seed ^ (run.seed >> 32)) as u32,
                owner,
                action_seq: source.sequence,
                slot: source.slot,
            },
            kind: engine_core::PresentationKind::RadialPulse,
            pos: position,
            radius,
            age_ticks: 0,
            duration_ticks: duration,
        },
    ));
}

fn number(
    commands: &mut Commands,
    run: &mut Run,
    position: [f32; 2],
    amount: f32,
    critical: bool,
    friendly: bool,
) {
    commands.spawn((
        DreamOwned,
        Number(DamageNumber {
            id: run.id(),
            position,
            amount,
            critical,
            friendly,
            age: 0.0,
        }),
    ));
}
fn heal(hero: &mut HeroActorItem<'_, '_>, amount: f32) {
    if hero.health.hp <= 0.0 {
        return;
    }
    hero.health.heal(amount);
}

fn hit_enemy(
    commands: &mut Commands,
    run: &mut Run,
    hero: &mut HeroActorItem<'_, '_>,
    enemy: &mut EnemyActorItem<'_, '_>,
    damage: f32,
    essence: Option<EssenceKind>,
    knockback: [f32; 2],
    ability: bool,
) {
    if enemy.health.hp <= 0.0 {
        return;
    }
    let critical = run.random() < hero.view.critical_chance;
    let amount = damage
        * if critical { 1.9 } else { 1.0 }
        * if ability && enemy.slow_left > 0.0 {
            1.20
        } else {
            1.0
        };
    let dealt = amount.min(enemy.health.hp);
    enemy.health.damage(amount);
    enemy.view.hit_flash = 0.14;
    if enemy.view.kind != EnemyKind::Boss {
        enemy.view.position = clamp(add(enemy.view.position, knockback));
    }
    if essence == Some(EssenceKind::Frost) {
        enemy.slow_left = 2.5;
        enemy.view.slowed = true;
    }
    if essence == Some(EssenceKind::Leech) {
        heal(hero, dealt * 0.12);
    }
    number(commands, run, enemy.view.position, amount, critical, true);
    // Impact flash lives on the predicted actor, so correction never restarts it
    // under a different transient identity.
}
fn hit_hero(commands: &mut Commands, run: &mut Run, hero: &mut HeroActorItem<'_, '_>, damage: f32) {
    if hero.dodge_left > 0.0 || hero.dash_left > 0.0 || hero.health.hp <= 0.0 {
        return;
    }
    let amount = damage * (1.0 - hero.view.defense) * if run.lucid { 1.22 } else { 1.0 };
    let absorbed = hero.view.shield.min(amount);
    hero.view.shield -= absorbed;
    hero.health.damage(amount - absorbed);
    hero.view.hit_flash = 0.22;
    hero.dodge_left = 0.23;
    hero.view.invulnerable = true;
    number(commands, run, hero.view.position, amount, false, false);
}
fn projectile(
    commands: &mut Commands,
    run: &mut Run,
    owner: u64,
    position: [f32; 2],
    direction: [f32; 2],
    damage: f32,
    friendly: bool,
    essence: Option<EssenceKind>,
    speed: f32,
    radius: f32,
) {
    commands.spawn((
        DreamOwned,
        Projectile {
            previous_position: position,
            view: ProjectileView {
                owner,
                id: run.id(),
                position,
                direction: normalize(direction),
                radius,
                friendly,
                essence,
            },
            speed,
            damage,
            lifetime: if friendly { 2.0 } else { 5.0 },
            pierce: if essence == Some(EssenceKind::Vast) && friendly {
                4
            } else {
                1
            },
            hit_ids: vec![],
        },
    ));
}

pub(super) fn begin_tick(
    mut run: ResMut<Run>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<EnemyActor, Without<Hero>>,
) {
    run.tick = run.tick.wrapping_add(1);
    run.elapsed += DT;
    for mut hero in &mut heroes {
        if hero.shield_left == 0.0 {
            hero.view.shield = 0.0;
        }
        hero.view.invulnerable = hero.dodge_left > 0.0 || hero.dash_left > 0.0;
        hero.view.dashing = hero.dash_left > 0.0;
    }
    for mut enemy in &mut enemies {
        enemy.view.slowed = enemy.slow_left > 0.0;
    }
}

/// Visual lifetimes continue through reward and ending scenes; pausing freezes them.
pub(super) fn age_transients(
    mut commands: Commands,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<EnemyActor, Without<Hero>>,
    mut numbers: Query<(Entity, &mut Number)>,
) {
    for mut hero in &mut heroes {
        decrement(&mut hero.view.hit_flash);
        decrement(&mut hero.view.attack_flash);
    }
    for mut enemy in &mut enemies {
        decrement(&mut enemy.view.hit_flash);
    }
    for (entity, mut value) in &mut numbers {
        value.0.age += DT;
        if value.0.age > 0.85 {
            commands.entity(entity).despawn();
        }
    }
}

/// A committed action uses a deterministic tick/owner/slot presentation identity.
pub(super) fn player_actions(
    mut commands: Commands,
    mut run: ResMut<Run>,
    input: Res<Input>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<(Entity, EnemyActor), Without<Hero>>,
) {
    let mut party: Vec<_> = heroes.iter_mut().collect();
    party.sort_by_key(|h| h.view.id);
    for mut hero in party {
        if hero.health.hp <= 0.0 {
            hero.view.velocity = [0.0; 2];
            continue;
        }
        let input = input
            .0
            .iter()
            .find(|(id, _)| *id == hero.view.id)
            .map(|(_, input)| *input)
            .unwrap_or_default();
        let movement = normalize(input.movement);
        let aim = normalize(input.aim);
        if length(aim) > 0.0 {
            hero.view.facing = aim;
        }
        if input.dash && hero.view.dash_cooldown <= 0.0 {
            hero.dash_left = 0.20;
            hero.dodge_left = 0.26;
            hero.view.dash_cooldown = 1.15;
            hero.dash_direction = if length(movement) > 0.0 {
                movement
            } else {
                hero.view.facing
            };
            hero.movement_lock = 0.0;
            hero.view.dashing = true;
            hero.view.invulnerable = true;
            action_effect(
                &mut commands,
                &run,
                hero.view.id,
                ActionEffect::input(input.action_sequences[0], run.tick, 0),
                hero.view.position,
                1.5,
                18,
            );
        }
        let velocity = if hero.dash_left > 0.0 {
            scale(hero.dash_direction, 24.0)
        } else if hero.movement_lock > 0.0 {
            [0.0; 2]
        } else {
            scale(movement, hero.view.movement_speed)
        };
        hero.view.velocity = velocity;
        hero.view.position = clamp(add(hero.view.position, scale(velocity, DT)));
        if input.attack && hero.view.attack_cooldown <= 0.0 && hero.dash_left <= 0.0 {
            hero.attack_sequence = hero.attack_sequence.wrapping_add(1);
            hero.view.attack_cooldown = 0.32;
            hero.view.attack_flash = 0.19;
            hero.movement_lock = 0.065;
            hero.view.combo = (hero.view.combo + 1) % 3;
            let empowered = hero.view.combo == 0;
            let reach = if empowered { 3.7 } else { 3.2 };
            let origin = hero.view.position;
            let direction = hero.view.facing;
            let damage = 24.0 * hero.view.attack_power * if empowered { 1.65 } else { 1.0 };
            let mut hit_any = false;
            let mut ordered: Vec<_> = enemies.iter().map(|(e, v)| (v.view.id, e)).collect();
            ordered.sort_unstable();
            for (_, entity) in ordered {
                let Ok((_, mut enemy)) = enemies.get_mut(entity) else {
                    continue;
                };
                let offset = sub(enemy.view.position, origin);
                let toward = normalize(offset);
                if enemy.health.hp > 0.0
                    && length(offset) < reach + 0.55
                    && toward[0] * direction[0] + toward[1] * direction[1] > -0.05
                {
                    hit_any = true;
                    hit_enemy(
                        &mut commands,
                        &mut run,
                        &mut hero,
                        &mut enemy,
                        damage,
                        None,
                        scale(toward, if empowered { 0.75 } else { 0.25 }),
                        false,
                    );
                }
            }
            if empowered && hit_any {
                heal(&mut hero, 5.0);
                for slot in &mut hero.view.memories {
                    slot.cooldown = (slot.cooldown - 0.50).max(0.0);
                }
            }
            action_effect(
                &mut commands,
                &run,
                hero.view.id,
                ActionEffect {
                    sequence: hero.attack_sequence,
                    slot: 0x8000,
                },
                add(origin, scale(direction, 1.2)),
                reach * 0.55,
                13,
            );
        }
        for (index, pressed) in input.casts.into_iter().enumerate() {
            if !pressed || hero.view.memories[index].cooldown > 0.0 {
                continue;
            }
            let memory = hero.view.memories[index];
            hero.view.memories[index].cooldown = memory.max_cooldown;
            let power = hero.view.ability_power * (1.0 + (memory.level - 1) as f32 * 0.25);
            let origin = hero.view.position;
            let direction = hero.view.facing;
            let source = ActionEffect::input(
                input.action_sequences[index + 1],
                run.tick,
                ((index + 1) as u16) << 12,
            );
            execute_memory(
                &mut commands,
                &mut run,
                &mut hero,
                &mut enemies,
                memory.kind,
                memory.essence,
                power,
                origin,
                direction,
                source,
            );
            if memory.essence == Some(EssenceKind::Echo) {
                let id = run.id();
                commands.spawn((
                    DreamOwned,
                    DelayedCast {
                        action_sequence: source.sequence,
                        presentation_slot: source.repeat().slot,
                        owner: hero.view.id,
                        id,
                        remaining: 0.55,
                        kind: memory.kind,
                        essence: None,
                        power: power * 0.65,
                        origin,
                        direction,
                    },
                ));
            }
        }
    }
}

fn execute_memory(
    commands: &mut Commands,
    run: &mut Run,
    hero: &mut HeroActorItem<'_, '_>,
    enemies: &mut Query<(Entity, EnemyActor), Without<Hero>>,
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
                commands.spawn((
                    DreamOwned,
                    Wisp {
                        view: WispView {
                            owner: hero.view.id,
                            id: run.id(),
                            position: origin,
                            remaining: 9.0,
                            essence,
                        },
                        damage: 15.0 * power,
                        fire_left: 0.15 + i as f32 * 0.2,
                        orbit: i as f32 * 3.14,
                    },
                ));
            }
            action_effect(commands, run, hero.view.id, source, origin, 1.8, 28);
        }
        _ => {
            let (center, radius, damage) = match kind {
                MemoryKind::Crescent => (origin, 5.0 * vast, 48.0 * power),
                MemoryKind::Nova => (origin, 5.0 * vast, 52.0 * power),
                MemoryKind::Blink => {
                    hero.view.position =
                        clamp(add(hero.view.position, scale(direction, 6.0 * vast)));
                    hero.dodge_left = 0.36;
                    hero.view.invulnerable = true;
                    action_effect(
                        commands,
                        run,
                        hero.view.id,
                        source.offset(1),
                        origin,
                        1.5,
                        24,
                    );
                    (hero.view.position, 2.8 * vast, 36.0 * power)
                }
                MemoryKind::Aegis => {
                    hero.view.shield = (hero.view.shield
                        + 55.0
                            * power
                            * if essence == Some(EssenceKind::Twin) {
                                1.5
                            } else {
                                vast
                            })
                    .min(hero.health.max_hp * 1.5);
                    hero.shield_left = if essence == Some(EssenceKind::Haste) {
                        10.0
                    } else {
                        7.0
                    };
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
                let offset = sub(enemy.view.position, center);
                let toward = normalize(offset);
                if length(offset) < radius + 0.5
                    && (kind != MemoryKind::Crescent
                        || toward[0] * direction[0] + toward[1] * direction[1] > -0.30)
                {
                    hit_enemy(
                        commands,
                        run,
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
                    DelayedCast {
                        action_sequence: source.sequence,
                        presentation_slot: source.repeat().slot,
                        owner: hero.view.id,
                        id,
                        remaining: 0.18,
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
                ));
            }
        }
    }
}

pub(super) fn delayed_casts(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<(Entity, EnemyActor), Without<Hero>>,
    mut delayed: Query<(Entity, &mut DelayedCast)>,
) {
    let mut ordered: Vec<_> = delayed.iter().map(|(e, v)| (v.id, e)).collect();
    ordered.sort_unstable();
    for (_, entity) in ordered {
        let Ok((_, mut cast)) = delayed.get_mut(entity) else {
            continue;
        };
        cast.remaining -= DT;
        if cast.remaining <= 0.0 {
            if let Some(mut hero) = heroes
                .iter_mut()
                .find(|h| h.view.id == cast.owner && h.health.hp > 0.0)
            {
                execute_memory(
                    &mut commands,
                    &mut run,
                    &mut hero,
                    &mut enemies,
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
            commands.entity(entity).despawn();
        }
    }
}

pub(super) fn wisp_actions(
    mut commands: Commands,
    mut run: ResMut<Run>,
    heroes: Query<HeroActorReadOnly, Without<Enemy>>,
    enemies: Query<EnemyActorReadOnly, Without<Hero>>,
    mut wisps: Query<(Entity, &mut Wisp)>,
) {
    let mut ordered: Vec<_> = wisps.iter().map(|(e, v)| (v.view.id, e)).collect();
    ordered.sort_unstable();
    for (_, entity) in ordered {
        let Ok((_, mut wisp)) = wisps.get_mut(entity) else {
            continue;
        };
        wisp.view.remaining -= DT;
        let Some(hero) = heroes.iter().find(|h| h.view.id == wisp.view.owner) else {
            commands.entity(entity).despawn();
            continue;
        };
        if wisp.view.remaining <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        if hero.health.hp <= 0.0 {
            continue;
        }
        wisp.orbit += DT * 1.8;
        wisp.view.position = add(
            hero.view.position,
            [wisp.orbit.cos() * 1.7, wisp.orbit.sin() * 1.7],
        );
        wisp.fire_left -= DT;
        if wisp.fire_left > 0.0 {
            continue;
        }
        let target = enemies.iter().filter(|e| e.health.hp > 0.0).min_by(|a, b| {
            distance(a.view.position, wisp.view.position)
                .total_cmp(&distance(b.view.position, wisp.view.position))
                .then(a.view.id.cmp(&b.view.id))
        });
        if let Some(target) = target
            && distance(target.view.position, wisp.view.position)
                < if wisp.view.essence == Some(EssenceKind::Vast) {
                    18.0
                } else {
                    12.0
                }
        {
            projectile(
                &mut commands,
                &mut run,
                hero.view.id,
                wisp.view.position,
                sub(target.view.position, wisp.view.position),
                wisp.damage,
                true,
                wisp.view.essence,
                20.0,
                0.24,
            );
            wisp.fire_left = if wisp.view.essence == Some(EssenceKind::Haste) {
                0.42
            } else {
                0.70
            };
        }
    }
}

fn hit_party_in_area(
    commands: &mut Commands,
    run: &mut Run,
    heroes: &mut Query<HeroActor, Without<Enemy>>,
    center: [f32; 2],
    radius: f32,
    damage: f32,
) {
    let mut party: Vec<_> = heroes.iter_mut().collect();
    party.sort_by_key(|h| h.view.id);
    for mut hero in party {
        if distance(hero.view.position, center) < radius {
            hit_hero(commands, run, &mut hero, damage);
        }
    }
}

pub(super) fn enemy_actions(
    mut commands: Commands,
    mut run: ResMut<Run>,
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
        let Some(hero_position) = heroes
            .iter()
            .filter(|h| h.health.hp > 0.0)
            .min_by(|a, b| {
                distance(a.view.position, enemy.view.position)
                    .total_cmp(&distance(b.view.position, enemy.view.position))
                    .then(a.view.id.cmp(&b.view.id))
            })
            .map(|h| h.view.position)
        else {
            continue;
        };
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
                enemy.recovery = 1.4;
                enemy.view.windup = 0.0;
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
                    enemy.view.position,
                    6.0,
                    65,
                );
                for offset in [[-5.0, 0.0], [5.0, 0.0]] {
                    let pos = clamp(add(enemy.view.position, offset));
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
        if enemy.view.windup > 0.0 {
            enemy.view.windup = (enemy.view.windup - DT).max(0.0);
            if enemy.view.windup == 0.0 {
                let origin = enemy.view.position;
                let target = enemy.view.target;
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
                            &mut commands,
                            &mut run,
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
                            &mut commands,
                            &mut run,
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
                            enemy.view.position = clamp(target);
                        }
                        let damage = match kind {
                            EnemyKind::Ambusher => 19.0,
                            EnemyKind::Elite => 32.0,
                            _ => 17.0,
                        };
                        hit_party_in_area(
                            &mut commands,
                            &mut run,
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
                enemy.recovery = match kind {
                    EnemyKind::Melee => 0.90,
                    EnemyKind::Ranged => 1.40,
                    EnemyKind::Ambusher => 1.45,
                    EnemyKind::Support => 2.8,
                    EnemyKind::Elite => 1.7,
                    EnemyKind::Boss => 2.0 - enemy.view.phase as f32 * 0.25,
                };
            }
            continue;
        }
        let to_hero = sub(hero_position, enemy.view.position);
        let dist = length(to_hero);
        let direction = normalize(to_hero);
        enemy.view.facing = direction;
        let (range, speed, windup, radius) = match kind {
            EnemyKind::Melee => (2.35, 3.1, 0.55, 1.9),
            EnemyKind::Ranged => (12.0, 2.5, 0.80, 1.0),
            EnemyKind::Ambusher => (6.5, 4.6, 0.72, 1.7),
            EnemyKind::Support => (12.0, 2.0, 1.15, 3.0),
            EnemyKind::Elite => (5.5, 2.9, 1.0, 3.1),
            EnemyKind::Boss => (25.0, 1.6, 1.05, 4.2 + enemy.view.phase as f32 * 0.35),
        };
        if enemy.recovery <= 0.0 && dist < range {
            enemy.view.target = hero_position;
            enemy.view.warn_radius = radius;
            enemy.view.windup = windup;
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
            let amount = speed * DT * if enemy.slow_left > 0.0 { 0.45 } else { 1.0 };
            enemy.view.position = clamp(add(enemy.view.position, scale(direction, amount)));
        } else if kind == EnemyKind::Ranged && dist < 5.5 {
            enemy.view.position = clamp(add(
                enemy.view.position,
                scale(direction, -speed * DT * 0.7),
            ));
        }
    }
    for center in support_pulses {
        for (_, mut enemy) in &mut enemies {
            if enemy.health.hp > 0.0
                && enemy.view.kind != EnemyKind::Boss
                && distance(enemy.view.position, center) < 7.0
            {
                enemy.health.hp =
                    (enemy.health.hp + enemy.health.max_hp * 0.09).min(enemy.health.max_hp);
            }
        }
    }
}

/// Segment collision avoids tunneling through thin enemies at high projectile speed.
fn segment_distance(point: [f32; 2], from: [f32; 2], to: [f32; 2]) -> f32 {
    let delta = sub(to, from);
    let offset = sub(point, from);
    let square = delta[0] * delta[0] + delta[1] * delta[1];
    let t = if square < 0.00001 {
        0.0
    } else {
        ((offset[0] * delta[0] + offset[1] * delta[1]) / square).clamp(0.0, 1.0)
    };
    distance(point, add(from, scale(delta, t)))
}

pub(super) fn projectile_actions(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<(Entity, EnemyActor), Without<Hero>>,
    mut projectiles: Query<(Entity, &mut Projectile)>,
) {
    let mut ordered: Vec<_> = projectiles.iter().map(|(e, v)| (v.view.id, e)).collect();
    ordered.sort_unstable();
    for (_, entity) in ordered {
        let Ok((_, mut bolt)) = projectiles.get_mut(entity) else {
            continue;
        };
        let from = bolt.previous_position;
        bolt.lifetime -= DT;
        if bolt.lifetime <= 0.0 || length(bolt.view.position) > ARENA_RADIUS + 4.0 {
            commands.entity(entity).despawn();
            continue;
        }
        if bolt.view.friendly {
            let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == bolt.view.owner) else {
                commands.entity(entity).despawn();
                continue;
            };
            let mut targets: Vec<_> = enemies
                .iter()
                .filter(|(_, e)| e.health.hp > 0.0 && !bolt.hit_ids.contains(&e.view.id))
                .map(|(entity, enemy)| (distance(enemy.view.position, from), enemy.view.id, entity))
                .collect();
            targets.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            for (_, id, target) in targets {
                let Ok((_, mut enemy)) = enemies.get_mut(target) else {
                    continue;
                };
                let radius = if enemy.view.kind == EnemyKind::Boss {
                    1.5
                } else if enemy.view.kind == EnemyKind::Elite {
                    0.9
                } else {
                    0.6
                };
                if segment_distance(enemy.view.position, from, bolt.view.position)
                    < bolt.view.radius + radius
                {
                    hit_enemy(
                        &mut commands,
                        &mut run,
                        &mut hero,
                        &mut enemy,
                        bolt.damage,
                        bolt.view.essence,
                        scale(bolt.view.direction, 0.25),
                        true,
                    );
                    bolt.hit_ids.push(id);
                    bolt.pierce -= 1;
                    if bolt.pierce == 0 {
                        commands.entity(entity).despawn();
                        break;
                    }
                }
            }
        } else {
            let struck = heroes
                .iter()
                .filter(|h| {
                    h.health.hp > 0.0
                        && segment_distance(h.view.position, from, bolt.view.position)
                            < bolt.view.radius + 0.48
                })
                .min_by(|a, b| {
                    distance(a.view.position, from)
                        .total_cmp(&distance(b.view.position, from))
                        .then(a.view.id.cmp(&b.view.id))
                })
                .map(|h| h.view.id);
            if let Some(id) = struck
                && let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == id)
            {
                hit_hero(&mut commands, &mut run, &mut hero, bolt.damage);
                commands.entity(entity).despawn();
            }
        }
    }
}

pub(super) fn resolve_deaths(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    enemies: Query<(Entity, EnemyActorReadOnly), (Without<Hero>, With<engine_core::Depleted>)>,
) {
    let mut dead: Vec<_> = enemies
        .iter()
        .filter(|(_, e)| e.health.hp <= 0.0)
        .map(|(entity, e)| (e.view.id, entity, e.view.kind, e.view.position))
        .collect();
    dead.sort_by_key(|v| v.0);
    let mut xp = 0.0;
    let mut shards = 0;
    let kills = dead.len();
    for (id, entity, kind, position) in dead {
        run.kills += 1;
        xp += match kind {
            EnemyKind::Boss => 160.0,
            EnemyKind::Elite => 60.0,
            EnemyKind::Support => 22.0,
            _ => 16.0,
        };
        shards += if run.lucid { 9 } else { 6 }
            * if matches!(kind, EnemyKind::Elite | EnemyKind::Boss) {
                4
            } else {
                1
            };
        action_effect(
            &mut commands,
            &run,
            id,
            ActionEffect {
                sequence: 0,
                slot: 0xc100,
            },
            position,
            if kind == EnemyKind::Boss { 9.0 } else { 1.6 },
            if kind == EnemyKind::Boss { 120 } else { 32 },
        );
        commands.entity(entity).despawn();
    }
    let mut party: Vec<_> = heroes.iter_mut().collect();
    party.sort_by_key(|h| h.view.id);
    let mut alive = 0;
    for mut hero in party {
        hero.view.xp += xp;
        hero.view.shards += shards;
        heal(&mut hero, kills as f32 * 1.5);
        while hero.view.xp >= hero.view.xp_next {
            hero.view.xp -= hero.view.xp_next;
            hero.view.level += 1;
            hero.view.xp_next = 45.0 + hero.view.level as f32 * 22.0;
            hero.health.max_hp += 10.0;
            heal(&mut hero, 25.0);
            hero.pending_levels += 1;
            if hero.health.hp > 0.0 {
                action_effect(
                    &mut commands,
                    &run,
                    hero.view.id,
                    ActionEffect {
                        sequence: hero.view.level,
                        slot: 0xc400,
                    },
                    hero.view.position,
                    3.5,
                    45,
                );
            }
        }
        if hero.health.hp > 0.0 {
            alive += 1;
        } else {
            hero.view.velocity = [0.0; 2];
            hero.view.dashing = false;
        }
    }
    if alive == 0 && run.party_size > 0 {
        run.phase = RunPhase::Defeat;
        run.message = "Every thread breaks. Begin another dream together.".into();
    }
}

pub(super) fn finish_encounter(
    mut commands: Commands,
    mut run: ResMut<Run>,
    enemies: Query<EnemyActorReadOnly, Without<Hero>>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
) {
    if run.phase != RunPhase::Combat {
        return;
    }
    if enemies.iter().len() <= 2 && run.reinforcements > 0 {
        let count = run.reinforcements.min(5 + run.room as u32 / 3);
        run.reinforcements -= count;
        run.message = "The dream fractures · A new wave approaches".into();
        let offset = run.random() * TAU;
        for i in 0..count {
            let angle = offset + i as f32 / count as f32 * TAU;
            let kind = match run.index(4) {
                0 => EnemyKind::Melee,
                1 => EnemyKind::Ranged,
                2 => EnemyKind::Ambusher,
                _ => EnemyKind::Melee,
            };
            let position = [angle.cos() * 15.5, angle.sin() * 15.5];
            encounters::spawn_enemy(&mut commands, &mut run, kind, position);
            action_effect(
                &mut commands,
                &run,
                0,
                ActionEffect {
                    sequence: run.room as u32,
                    slot: 0xc500 + (run.reinforcements * 16 + i) as u16,
                },
                position,
                1.6,
                42,
            );
        }
        return;
    }
    if !enemies.is_empty() {
        return;
    }
    run.cleared += 1;
    for mut hero in &mut heroes {
        hero.view.velocity = [0.0; 2];
        hero.view.dashing = false;
        hero.view.shards += if run.lucid { 30 } else { 20 };
        hero.ready = false;
        if hero.health.hp <= 0.0 {
            hero.health.hp = hero.health.max_hp * 0.5;
        }
    }
    if run.room == TOTAL_ROOMS - 1 {
        run.phase = RunPhase::Victory;
        run.message = "The Somnarch unravels. You carry the dawn home.".into();
    } else {
        run.phase = RunPhase::Reward;
        run.message = "Dream unraveled · Choose one gift to carry onward".into();
        run.rewards = encounters::make_rewards(&mut run, false);
        for mut hero in &mut heroes {
            hero.rewards = run.rewards.clone();
        }
    }
}

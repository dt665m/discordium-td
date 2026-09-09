use super::*;

pub(crate) fn player_actions(
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
            hero.motor.velocity = [0.0; 2];
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
            hero.motor.facing = aim;
        }
        if input.dash && hero.motor.dash_cooldown <= 0.0 {
            let direction = if length(movement) > 0.0 {
                movement
            } else {
                hero.motor.facing
            };
            hero.motor.start_dash(direction, 24.0, 0.20, 1.15);
            hero.combat
                .grant_invulnerability(0.26, engine_core::DurationPolicy::Reset);
            hero.motor.movement_lock = 0.0;
            action_effect(
                &mut commands,
                &run,
                hero.view.id,
                ActionEffect::input(input.action_sequences[0], run.tick, 0),
                hero.motor.position,
                1.5,
                18,
            );
        }
        let speed = hero.view.movement_speed;
        hero.motor.advance(
            input.movement,
            input.aim,
            speed,
            Some(engine_core::CircleBounds {
                center: [0.0; 2],
                radius: ARENA_RADIUS - 0.8,
            }),
            DT,
        );
        let target = hero.motor.position;
        if input.attack && hero.motor.dash_remaining <= 0.0 && hero.action.commit(target, 0.0) {
            hero.action.take_due();
            hero.action.begin_recovery(0.32);
            hero.view.attack_flash = 0.19;
            hero.motor.movement_lock = 0.065;
            hero.view.combo = (hero.view.combo + 1) % 3;
            let empowered = hero.view.combo == 0;
            let reach = if empowered { 3.7 } else { 3.2 };
            let origin = hero.motor.position;
            let direction = hero.motor.facing;
            let damage = 24.0 * hero.view.attack_power * if empowered { 1.65 } else { 1.0 };
            let mut hit_any = false;
            let mut ordered: Vec<_> = enemies.iter().map(|(e, v)| (v.view.id, e)).collect();
            ordered.sort_unstable();
            for (_, entity) in ordered {
                let Ok((_, mut enemy)) = enemies.get_mut(entity) else {
                    continue;
                };
                let offset = sub(enemy.motor.position, origin);
                let toward = normalize(offset);
                if enemy.health.hp > 0.0
                    && engine_core::contains_cone(
                        origin,
                        direction,
                        reach + 0.55,
                        -0.05,
                        enemy.motor.position,
                    )
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
                for slot in &mut hero.loadout.0 {
                    slot.reduce_cooldown(0.50);
                }
            }
            action_effect(
                &mut commands,
                &run,
                hero.view.id,
                ActionEffect {
                    sequence: hero.action.sequence,
                    slot: 0x8000,
                },
                add(origin, scale(direction, 1.2)),
                reach * 0.55,
                13,
            );
        }
        for (index, pressed) in input.casts.into_iter().enumerate() {
            if !pressed || !hero.loadout.0[index].activate() {
                continue;
            }
            let memory = hero.loadout.0[index];
            let power = hero.view.ability_power * (1.0 + (memory.level - 1) as f32 * 0.25);
            let origin = hero.motor.position;
            let direction = hero.motor.facing;
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
                memory.modifier,
                power,
                origin,
                direction,
                source,
            );
            if memory.modifier == Some(EssenceKind::Echo) {
                let id = run.id();
                commands.spawn((
                    DreamOwned,
                    DelayedCast::new(
                        0.55,
                        CastPayload {
                            action_sequence: source.sequence,
                            presentation_slot: source.repeat().slot,
                            owner: hero.view.id,
                            id,
                            kind: memory.kind,
                            essence: None,
                            power: power * 0.65,
                            origin,
                            direction,
                        },
                    ),
                ));
            }
        }
    }
}

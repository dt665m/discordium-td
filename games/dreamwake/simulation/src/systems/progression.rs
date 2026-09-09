use super::*;

pub(crate) fn resolve_deaths(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    enemies: Query<(Entity, EnemyActorReadOnly), (Without<Hero>, With<engine_core::Depleted>)>,
) {
    let mut dead: Vec<_> = enemies
        .iter()
        .filter(|(_, e)| e.health.hp <= 0.0)
        .map(|(entity, e)| (e.view.id, entity, e.view.kind, e.motor.position))
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
        hero.view.shards += shards;
        heal(&mut hero, kills as f32 * 1.5);
        let previous_level = hero.progression.level;
        let gained = hero
            .progression
            .gain(xp, crate::catalog::experience_threshold)
            .expect("Dreamwake XP curve is positive and finite");
        for offset in 1..=gained {
            hero.health.max_hp += 10.0;
            heal(&mut hero, 25.0);
            if hero.health.hp > 0.0 {
                action_effect(
                    &mut commands,
                    &run,
                    hero.view.id,
                    ActionEffect {
                        sequence: previous_level + offset,
                        slot: 0xc400,
                    },
                    hero.motor.position,
                    3.5,
                    45,
                );
            }
        }
        if hero.health.hp > 0.0 {
            alive += 1;
        } else {
            hero.motor.velocity = [0.0; 2];
            hero.motor.dash_remaining = 0.0;
        }
    }
    if alive == 0 && run.party_size > 0 {
        run.phase = RunPhase::Defeat;
        run.message = "Every thread breaks. Begin another dream together.".into();
    }
}

pub(crate) fn finish_encounter(
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
        hero.motor.velocity = [0.0; 2];
        hero.motor.dash_remaining = 0.0;
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

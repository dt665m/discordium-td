use super::*;

pub(crate) fn heal(hero: &mut HeroActorItem<'_, '_>, amount: f32) {
    if hero.health.hp <= 0.0 {
        return;
    }
    hero.health.heal(amount);
}

pub(crate) fn hit_enemy(
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
        * if ability && enemy.combat.status(FROST_STATUS).is_some() {
            1.20
        } else {
            1.0
        };
    let outcome = engine_core::resolve_damage(&mut enemy.health, &mut enemy.combat, amount, 1.0);
    if outcome.blocked {
        return;
    }
    enemy.view.hit_flash = 0.14;
    let knockback_response = if enemy.view.kind == EnemyKind::Boss {
        0.0
    } else {
        1.0
    };
    enemy.motor.displace(
        knockback,
        knockback_response,
        Some(engine_core::CircleBounds {
            center: [0.0; 2],
            radius: ARENA_RADIUS - 0.8,
        }),
    );
    if essence == Some(EssenceKind::Frost) {
        enemy
            .combat
            .apply_status(FROST_STATUS, 0.45, 2.5, engine_core::DurationPolicy::Reset);
    }
    if essence == Some(EssenceKind::Leech) {
        heal(hero, outcome.hp_damage * 0.12);
    }
    number(commands, run, enemy.motor.position, amount, critical, true);
    // Impact flash lives on the predicted actor, so correction never restarts it
    // under a different transient identity.
}
pub(crate) fn hit_hero(
    commands: &mut Commands,
    run: &mut Run,
    hero: &mut HeroActorItem<'_, '_>,
    damage: f32,
) {
    if hero.motor.dash_remaining > 0.0 {
        return;
    }
    let amount = damage * (1.0 - hero.view.defense) * if run.lucid { 1.22 } else { 1.0 };
    let outcome = engine_core::resolve_damage(&mut hero.health, &mut hero.combat, amount, 1.0);
    if outcome.blocked {
        return;
    }
    hero.view.hit_flash = 0.22;
    hero.combat
        .grant_invulnerability(0.23, engine_core::DurationPolicy::Reset);
    number(commands, run, hero.motor.position, amount, false, false);
}
pub(crate) fn begin_tick(mut run: ResMut<Run>) {
    run.tick = run.tick.wrapping_add(1);
    run.elapsed += DT;
}

/// Visual lifetimes continue through reward and ending scenes; pausing freezes them.
pub(crate) fn age_transients(
    mut commands: Commands,
    mut heroes: Query<&mut Hero>,
    mut enemies: Query<&mut Enemy>,
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

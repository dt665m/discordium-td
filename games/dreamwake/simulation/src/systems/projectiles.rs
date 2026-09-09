use super::*;
use engine_core::{CollisionTarget, ProjectileSourcePolicy, ProjectileState, TargetIndex};

pub(crate) fn projectile(
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
        Projectile { damage, essence },
        ProjectileState {
            id: run.id(),
            owner,
            faction: if friendly { 1 } else { 2 },
            position,
            previous_position: position,
            direction: normalize(direction),
            speed,
            remaining: if friendly { 2.0 } else { 5.0 },
            radius,
            hits_remaining: if essence == Some(EssenceKind::Vast) && friendly {
                4
            } else {
                1
            },
            hit_ids: vec![],
            max_distance: Some(ARENA_RADIUS + 4.0),
            source_policy: if friendly {
                ProjectileSourcePolicy::RequirePresent
            } else {
                ProjectileSourcePolicy::Independent
            },
            expired: false,
            pending_impacts: vec![],
        },
    ));
}

fn enemy_radius(kind: EnemyKind) -> f32 {
    match kind {
        EnemyKind::Boss => 1.5,
        EnemyKind::Elite => 0.9,
        _ => 0.6,
    }
}

/// This index is disposable and refreshed at each targeting phase.
pub(crate) fn refresh_target_index(
    heroes: Query<HeroActorReadOnly, Without<Enemy>>,
    enemies: Query<EnemyActorReadOnly, Without<Hero>>,
    mut index: ResMut<TargetIndex>,
) {
    index.0.clear();
    index.0.extend(heroes.iter().map(|h| CollisionTarget {
        id: h.view.id,
        faction: 1,
        position: h.motor.position,
        radius: 0.48,
        active: h.health.hp > 0.0,
    }));
    index.0.extend(enemies.iter().map(|e| CollisionTarget {
        id: e.view.id,
        faction: 2,
        position: e.motor.position,
        radius: enemy_radius(e.view.kind),
        active: e.health.hp > 0.0,
    }));
}

pub(crate) fn projectile_actions(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<EnemyActor, Without<Hero>>,
    mut projectiles: Query<(Entity, &Projectile, &mut ProjectileState)>,
) {
    let mut ordered: Vec<_> = projectiles
        .iter()
        .map(|(e, _, state)| (state.id, e))
        .collect();
    ordered.sort_unstable();
    for (_, entity) in ordered {
        let Ok((_, payload, mut bolt)) = projectiles.get_mut(entity) else {
            continue;
        };
        if bolt.finished() {
            commands.entity(entity).despawn();
            continue;
        }
        if bolt.faction == 1 {
            let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == bolt.owner) else {
                commands.entity(entity).despawn();
                continue;
            };
            let impacts = bolt.impacts(enemies.iter().map(|e| CollisionTarget {
                id: e.view.id,
                faction: 2,
                position: e.motor.position,
                radius: enemy_radius(e.view.kind),
                active: e.health.hp > 0.0,
            }));
            for impact in impacts {
                let Some(mut enemy) = enemies
                    .iter_mut()
                    .find(|e| e.view.id == impact.target_id && e.health.hp > 0.0)
                else {
                    continue;
                };
                hit_enemy(
                    &mut commands,
                    &mut run,
                    &mut hero,
                    &mut enemy,
                    payload.damage,
                    payload.essence,
                    scale(bolt.direction, 0.25),
                    true,
                );
                bolt.record_hit(impact.target_id);
                if bolt.finished() {
                    break;
                }
            }
        } else {
            let impacts = bolt.impacts(heroes.iter().map(|h| CollisionTarget {
                id: h.view.id,
                faction: 1,
                position: h.motor.position,
                radius: 0.48,
                active: h.health.hp > 0.0,
            }));
            if let Some(impact) = impacts.first()
                && let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == impact.target_id)
            {
                hit_hero(&mut commands, &mut run, &mut hero, payload.damage);
                bolt.record_hit(impact.target_id);
            }
        }
        bolt.pending_impacts.clear();
        if bolt.finished() {
            commands.entity(entity).despawn();
        }
    }
}

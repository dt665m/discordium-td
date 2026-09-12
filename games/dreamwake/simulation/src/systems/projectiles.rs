use super::*;
use engine_core::{
    ColliderKey, CollisionScene, CollisionTarget, ProjectileSourcePolicy, ProjectileState,
    SceneQueryBudget, StaticCollider, TargetIndex,
};

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
        Projectile {
            damage,
            essence,
            spawn: None,
        },
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

pub(super) fn enemy_radius(kind: EnemyKind) -> f32 {
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
        position: planar_position(&h.motion),
        radius: 0.48,
        active: h.active && h.health.hp > 0.0,
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
    run: Res<Run>,
    mut batch: ResMut<CombatBatch>,
    collision: Res<crate::platform::MotionEnvironment>,
    history: Res<crate::combat_history::CombatHistory>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<EnemyActor, Without<Hero>>,
    mut projectiles: Query<(Entity, &Projectile, &mut ProjectileState)>,
) {
    let poses = history.logical_frame(run.tick, collision.collision().manifest().scene_revision());
    let targets: Vec<_> = poses
        .into_iter()
        .flat_map(|frame| &frame.poses)
        .filter(|pose| pose.alive && (pose.damageable || pose.faction == 0))
        .map(|pose| {
            (
                pose.id,
                pose.generation,
                pose.faction,
                CollisionScene::new(
                    1,
                    vec![StaticCollider {
                        key: ColliderKey {
                            index: pose.id,
                            generation: pose.generation,
                        },
                        position: pose.position,
                        rotation: pose.rotation,
                        shape: pose.shape,
                    }],
                )
                .expect("captured combat pose is validated"),
            )
        })
        .collect();
    let mut query_budget =
        SceneQueryBudget::new(1_048_576, 65_536).expect("fixed projectile query limits");
    let mut ordered: Vec<_> = projectiles
        .iter()
        .map(|(e, _, state)| (state.id, e))
        .collect();
    ordered.sort_unstable();
    for (_, entity) in ordered {
        let Ok((_, payload, mut bolt)) = projectiles.get_mut(entity) else {
            continue;
        };
        if poses.is_none() || bolt.finished() {
            commands.entity(entity).despawn();
            continue;
        }
        // Resolve the complete sweep before recording hits. A bounded query failure
        // expires the projectile instead of allowing an untested continuation.
        let start = Vec3::new(bolt.previous_position[0], 0.9, bolt.previous_position[1]);
        let delta = Vec3::new(bolt.position[0], 0.9, bolt.position[1]) - start;
        let impacts = (|| {
            let mut wall =
                collision
                    .scene()
                    .sphere_cast(start, delta, bolt.radius, &mut query_budget)?;
            for (_, _, faction, scene) in &targets {
                if *faction == 0
                    && let Some(hit) =
                        scene.sphere_cast(start, delta, bolt.radius, &mut query_budget)?
                    && wall.as_ref().is_none_or(|wall| hit.toi < wall.toi)
                {
                    wall = Some(hit);
                }
            }
            let mut impacts = Vec::new();
            for (id, generation, faction, scene) in &targets {
                if *faction == 0
                    || *faction == bolt.faction
                    || *id == bolt.owner
                    || bolt.hit_ids.contains(id)
                {
                    continue;
                }
                if let Some(hit) =
                    scene.sphere_cast(start, delta, bolt.radius, &mut query_budget)?
                    && wall.as_ref().is_none_or(|wall| hit.toi < wall.toi)
                {
                    impacts.push((hit.toi, *id, *generation));
                }
            }
            impacts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            Ok::<_, engine_core::KinematicError>((impacts, wall.is_some()))
        })();
        let Ok((impacts, blocked)) = impacts else {
            commands.entity(entity).despawn();
            continue;
        };
        if bolt.faction == 1 {
            let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == bolt.owner) else {
                commands.entity(entity).despawn();
                continue;
            };
            for (_, target_id, generation) in &impacts {
                let Some(mut enemy) = enemies.iter_mut().find(|e| {
                    e.view.id == *target_id
                        && e.combat_identity.generation == *generation
                        && e.health.hp > 0.0
                }) else {
                    continue;
                };
                hit_enemy(
                    &mut batch,
                    CombatActionKey::new(
                        run.tick,
                        hero.view.id,
                        hero.combat_identity.generation,
                        3,
                        bolt.id,
                        0,
                    ),
                    &mut hero,
                    &mut enemy,
                    payload.damage,
                    payload.essence,
                    scale(bolt.direction, 0.25),
                    true,
                );
                bolt.record_hit(*target_id);
                if bolt.finished() {
                    break;
                }
            }
        } else {
            if let Some((_, target_id, generation)) = impacts.first()
                && let Some(hero) = heroes.iter_mut().find(|h| {
                    h.view.id == *target_id && h.combat_identity.generation == *generation
                })
            {
                hit_hero(
                    &mut batch,
                    CombatActionKey::new(run.tick, bolt.owner, 1, 3, bolt.id, 0),
                    &hero,
                    payload.damage,
                );
                bolt.record_hit(*target_id);
            }
        }
        bolt.pending_impacts.clear();
        if blocked || bolt.finished() {
            commands.entity(entity).despawn();
        }
    }
}

/// Shared spawn spec, with authority-only entity allocation.
pub(crate) fn spawn_starfall(
    commands: &mut Commands,
    run: &mut Run,
    mut flight: crate::starfall::StarfallFlight,
    damage: f32,
) {
    if flight.authority_id.is_none() {
        flight.authority_id = Some(run.id());
    }
    commands.spawn((
        DreamOwned,
        Projectile {
            damage,
            essence: flight.essence,
            spawn: Some((flight.key, flight.origin_tick)),
        },
        flight.engine_state(),
    ));
}

//! Independent per-actor updates read one immutable geometric frame.
use super::*;

#[derive(Resource, Default)]
pub(crate) struct EnemyFrame {
    pub navmesh: Option<NavMesh>,
    pub hero_targets: Vec<(u64, [f32; 2])>,
    pub motion_snapshots: Vec<EnemyMotionSnapshot>,
    pub separation_cells: BTreeMap<(i32, i32), Vec<usize>>,
    pub separation_cell_size: f32,
}
#[derive(Resource, Default)]
pub(crate) struct HeroFrame {
    pub enemy_positions: BTreeMap<u64, [f32; 2]>,
}

pub(crate) fn update_navigation(
    towers: Query<(&game_replication::NetId, &Position), With<TowerState>>,
    mut cache: ResMut<NavigationCache>,
) {
    if !cache.dirty && cache.mesh.is_some() {
        return;
    }
    let mut positions: Vec<_> = towers.iter().map(|(id, p)| (id.value, p.pos)).collect();
    positions.sort_unstable_by_key(|(id, _)| *id);
    cache.mesh = Some(build_navigation_mesh_from_positions(
        &positions.into_iter().map(|(_, p)| p).collect::<Vec<_>>(),
    ));
    cache.dirty = false;
}
pub(crate) fn prepare_enemies(
    cache: Res<NavigationCache>,
    heroes: Query<(&game_replication::NetId, &Position), With<HeroState>>,
    enemies: Query<(&EnemyState, &Position)>,
    mut frame: ResMut<EnemyFrame>,
) {
    frame.navmesh = cache.mesh.clone();
    frame.hero_targets.clear();
    frame
        .hero_targets
        .extend(heroes.iter().map(|(id, p)| (id.value, p.pos)));
    frame.hero_targets.sort_unstable_by_key(|(id, _)| *id);
    frame.motion_snapshots.clear();
    frame
        .motion_snapshots
        .extend(enemies.iter().map(|(enemy, p)| EnemyMotionSnapshot {
            id: enemy.id,
            pos: p.pos,
            radius: enemy.radius,
        }));
    frame.motion_snapshots.sort_unstable_by_key(|e| e.id);
    frame.separation_cell_size = separation_spatial_cell_size(&frame.motion_snapshots);
    frame.separation_cells =
        build_spatial_cells(&frame.motion_snapshots, frame.separation_cell_size, |e| {
            e.pos
        });
}

pub(crate) fn advance_enemies(
    frame: Res<EnemyFrame>,
    mut enemies: Query<(
        &mut EnemyState,
        &mut Position,
        &mut Health,
        &mut Facing,
        &mut Attack,
        &mut EnemyStep,
    )>,
) {
    let EnemyFrame {
        navmesh,
        hero_targets,
        motion_snapshots,
        separation_cells,
        separation_cell_size,
    } = frame.as_ref();
    let separation_cell_size = *separation_cell_size;
    let navmesh = navmesh
        .as_ref()
        .expect("navigation prepared before movement");
    let update = |(mut role, mut position, mut health, mut facing, mut attack, mut outcome): (
        Mut<'_, EnemyState>,
        Mut<'_, Position>,
        Mut<'_, Health>,
        Mut<'_, Facing>,
        Mut<'_, Attack>,
        Mut<'_, EnemyStep>,
    )| {
        let enemy = EnemyMut {
            role: &mut role,
            position: &mut position,
            health: &mut health,
            facing: &mut facing,
            attack: &mut attack,
        };
        *outcome = EnemyStep::default();
        enemy.role.lock_target =
            choose_enemy_target(enemy.position.pos, enemy.role.lock_target, &hero_targets);
        let target = resolve_enemy_target(enemy.role.lock_target, &hero_targets);
        let target_pos = enemy_target_position(target);
        let previous_target = enemy.role.target_pos;
        enemy.role.target_pos =
            enemy_target_attack_anchor(target, enemy.role.id, enemy.attack.profile.range);
        let facing_to_target = normalize_or_zero([
            target_pos[0] - enemy.position.pos[0],
            target_pos[1] - enemy.position.pos[1],
        ]);
        if facing_to_target != [0.0, 0.0] {
            // AI lock-on: face the active attack target (hero/base), not the steering anchor.
            enemy.facing.direction.dir = facing_to_target;
        }

        let attack_triggered = advance_attack_state(&mut enemy.attack.state, enemy.attack.profile);
        if attack_triggered {
            match target {
                EnemyTarget::Hero { client_id, pos } => {
                    if directional_attack_can_hit(
                        enemy.position.pos,
                        enemy.facing.direction.dir,
                        pos,
                        enemy.attack.profile,
                    ) {
                        outcome.hero_hit = Some((client_id, enemy.attack.profile.damage));
                    }
                }
                EnemyTarget::Base => {
                    if directional_attack_can_hit(
                        enemy.position.pos,
                        enemy.facing.direction.dir,
                        BASE_POSITION,
                        enemy.attack.profile,
                    ) {
                        outcome.objective_hit = true;
                    }
                }
            }
        }

        if is_attack_locked(enemy.attack.state) {
            enemy.role.vel = [0.0, 0.0];
            return;
        }

        if distance_sq(enemy.position.pos, target_pos)
            <= enemy.attack.profile.range * enemy.attack.profile.range
        {
            let _ = start_attack(&mut enemy.attack.state, enemy.attack.profile);
            enemy.role.vel = [0.0, 0.0];
            return;
        }

        let target_changed = distance_sq(enemy.role.target_pos, previous_target) > 0.5;

        let reached_waypoint = distance_sq(enemy.position.pos, enemy.role.waypoint)
            <= ENEMY_WAYPOINT_REACH_RADIUS.powi(2);
        if enemy.role.repath_cooldown == 0 || target_changed || reached_waypoint {
            enemy.role.waypoint =
                find_next_waypoint(enemy.position.pos, enemy.role.target_pos, &navmesh)
                    .unwrap_or(enemy.role.target_pos);
            enemy.role.repath_cooldown = ENEMY_REPATH_TICKS;
        } else {
            enemy.role.repath_cooldown -= 1;
        }

        let mut desired = normalize_or_zero([
            enemy.role.waypoint[0] - enemy.position.pos[0],
            enemy.role.waypoint[1] - enemy.position.pos[1],
        ]);
        if desired == [0.0, 0.0] {
            desired = normalize_or_zero([
                enemy.role.target_pos[0] - enemy.position.pos[0],
                enemy.role.target_pos[1] - enemy.position.pos[1],
            ]);
        }

        let separation = enemy_separation_direction(
            enemy.role.id,
            enemy.position.pos,
            enemy.role.radius,
            &motion_snapshots,
            &separation_cells,
            separation_cell_size,
        );
        let steer = normalize_or_zero([
            desired[0] * ENEMY_STEERING_TARGET_WEIGHT
                + separation[0] * ENEMY_STEERING_SEPARATION_WEIGHT,
            desired[1] * ENEMY_STEERING_TARGET_WEIGHT
                + separation[1] * ENEMY_STEERING_SEPARATION_WEIGHT,
        ]);

        enemy.role.vel[0] += steer[0] * enemy.role.speed * ENEMY_ACCEL_FACTOR * FIXED_DT_SECONDS;
        enemy.role.vel[1] += steer[1] * enemy.role.speed * ENEMY_ACCEL_FACTOR * FIXED_DT_SECONDS;

        let damping = (1.0 - ENEMY_DAMPING_FACTOR * FIXED_DT_SECONDS).clamp(0.0, 1.0);
        enemy.role.vel[0] *= damping;
        enemy.role.vel[1] *= damping;

        let vel_len_sq =
            enemy.role.vel[0] * enemy.role.vel[0] + enemy.role.vel[1] * enemy.role.vel[1];
        let max_len_sq = enemy.role.speed * enemy.role.speed;
        if vel_len_sq > max_len_sq {
            let scale = (max_len_sq / vel_len_sq).sqrt();
            enemy.role.vel[0] *= scale;
            enemy.role.vel[1] *= scale;
        }

        enemy.position.pos = clamp_to_world([
            enemy.position.pos[0] + enemy.role.vel[0] * FIXED_DT_SECONDS,
            enemy.position.pos[1] + enemy.role.vel[1] * FIXED_DT_SECONDS,
        ]);
    };
    if enemies.iter().len() >= 256 {
        enemies
            .par_iter_mut()
            .batching_strategy(bevy::ecs::batching::BatchingStrategy::new().min_batch_size(128))
            .for_each(update);
    } else {
        enemies.iter_mut().for_each(update);
    }
}

pub(crate) fn prepare_heroes(
    enemies: Query<(&EnemyState, &Position)>,
    mut frame: ResMut<HeroFrame>,
) {
    frame.enemy_positions.clear();
    frame
        .enemy_positions
        .extend(enemies.iter().map(|(enemy, pos)| (enemy.id, pos.pos)));
}
pub(crate) fn advance_heroes(
    frame: Res<HeroFrame>,
    mut heroes: Query<(
        &mut HeroState,
        &mut Position,
        &mut Health,
        &mut Facing,
        &mut Attack,
        &mut HeroStep,
    )>,
) {
    let enemy_positions = &frame.enemy_positions;
    let update = |(mut role, mut position, mut health, mut facing, mut attack, mut outcome): (
        Mut<'_, HeroState>,
        Mut<'_, Position>,
        Mut<'_, Health>,
        Mut<'_, Facing>,
        Mut<'_, Attack>,
        Mut<'_, HeroStep>,
    )| {
        let mut hero = HeroMut {
            role: &mut role,
            position: &mut position,
            health: &mut health,
            facing: &mut facing,
            attack: &mut attack,
        };
        *outcome = HeroStep::default();
        let regular_attack_profile = hero_regular_attack_profile(&hero.role, hero.attack.profile);

        if hero.role.lock_mode_active {
            if hero
                .role
                .lock_target_id
                .is_some_and(|target_id| !enemy_positions.contains_key(&target_id))
            {
                hero.role.lock_target_id = nearest_enemy_id(hero.position.pos, &enemy_positions);
            } else if hero.role.lock_target_id.is_none() {
                hero.role.lock_target_id = nearest_enemy_id(hero.position.pos, &enemy_positions);
            }

            if hero.role.lock_target_id.is_none() {
                hero.role.lock_mode_active = false;
            }
        } else {
            hero.role.lock_target_id = None;
        }

        let attack_triggered = advance_attack_state(&mut hero.attack.state, regular_attack_profile);
        if attack_triggered {
            outcome.attack_triggered = true;
        }
        advance_charge_state(&mut hero.role, &mut hero.health);

        if let Some(target_id) = hero.role.lock_target_id {
            if let Some(target_pos) = enemy_positions.get(&target_id).copied() {
                let to_target = normalize_or_zero([
                    target_pos[0] - hero.position.pos[0],
                    target_pos[1] - hero.position.pos[1],
                ]);
                if to_target != [0.0, 0.0] {
                    hero.facing.direction.dir = to_target;
                }
            }
        }

        let attack_locked = is_attack_locked(hero.attack.state);
        let charge_locked = is_charge_locked(hero.role.charge_state);
        if !attack_locked && !charge_locked {
            if hero.role.move_dir != [0.0, 0.0]
                && (!hero.role.lock_mode_active || hero.role.lock_target_id.is_none())
            {
                hero.facing.direction.dir = hero.role.move_dir;
            }

            hero.position.pos = clamp_to_world([
                hero.position.pos[0] + hero.role.move_dir[0] * HERO_SPEED * FIXED_DT_SECONDS,
                hero.position.pos[1] + hero.role.move_dir[1] * HERO_SPEED * FIXED_DT_SECONDS,
            ]);
        }

        hero.role.mana = (hero.role.mana + HERO_MANA_REGEN_PER_TICK).min(HERO_MAX_MANA);
        if hero.role.ability_cooldown_ticks > 0 {
            hero.role.ability_cooldown_ticks -= 1;
        }
    };
    if heroes.iter().len() >= 128 {
        heroes
            .par_iter_mut()
            .batching_strategy(bevy::ecs::batching::BatchingStrategy::new().min_batch_size(128))
            .for_each(update);
    } else {
        heroes.iter_mut().for_each(update);
    }
}

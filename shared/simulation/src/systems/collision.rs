use crate::storage::{Epoch, SimulationOwned};
use crate::*;
use game_replication::NetId;

impl Globals {
    pub(crate) fn resolve_hero_collisions(&mut self, world: &mut World) {
        #[derive(Clone, Copy)]
        struct HeroCollisionSnapshot {
            client_id: u64,
            pos: [f32; 2],
        }

        #[derive(Clone, Copy)]
        struct EnemyCollisionSnapshot {
            id: u64,
            pos: [f32; 2],
            radius: f32,
        }

        let epoch = world.resource::<Epoch>().0;
        let mut heroes =
            world.query_filtered::<(&NetId, &Position), (With<HeroState>, With<SimulationOwned>)>();
        let mut hero_snapshots: Vec<_> = heroes
            .iter(world)
            .filter(|(id, _)| id.epoch == epoch)
            .map(|(id, position)| HeroCollisionSnapshot {
                client_id: id.value,
                pos: position.pos,
            })
            .collect();
        hero_snapshots.sort_unstable_by_key(|hero| hero.client_id);
        let mut enemies =
            world.query_filtered::<(&NetId, &EnemyState, &Position), With<SimulationOwned>>();
        let mut enemy_snapshots: Vec<_> = enemies
            .iter(world)
            .filter(|(id, _, _)| id.epoch == epoch)
            .map(|(id, enemy, position)| EnemyCollisionSnapshot {
                id: id.value,
                pos: position.pos,
                radius: enemy.radius,
            })
            .collect();
        enemy_snapshots.sort_unstable_by_key(|enemy| enemy.id);
        let tower_positions = tower_positions(world, epoch);
        let mut displacements: BTreeMap<u64, [f32; 2]> = BTreeMap::new();

        for i in 0..hero_snapshots.len() {
            for j in (i + 1)..hero_snapshots.len() {
                let a = hero_snapshots[i];
                let b = hero_snapshots[j];

                let mut delta = [b.pos[0] - a.pos[0], b.pos[1] - a.pos[1]];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = HERO_COLLIDER_RADIUS * 2.0;
                let min_dist_sq = min_dist * min_dist;

                if dist_sq >= min_dist_sq {
                    continue;
                }

                if dist_sq <= f32::EPSILON {
                    let angle = ((a.client_id ^ (b.client_id << 1)) % 6283) as f32 * 0.001;
                    delta = [angle.cos(), angle.sin()];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                let push = overlap * 0.5;

                add_displacement(
                    &mut displacements,
                    a.client_id,
                    [-normal[0] * push, -normal[1] * push],
                );
                add_displacement(
                    &mut displacements,
                    b.client_id,
                    [normal[0] * push, normal[1] * push],
                );
            }
        }

        for snapshot in &hero_snapshots {
            for &(_, tower_pos) in &tower_positions {
                let mut delta = [
                    snapshot.pos[0] - tower_pos[0],
                    snapshot.pos[1] - tower_pos[1],
                ];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = HERO_COLLIDER_RADIUS + TOWER_COLLIDER_RADIUS;
                let min_dist_sq = min_dist * min_dist;
                if dist_sq >= min_dist_sq {
                    continue;
                }

                if dist_sq <= f32::EPSILON {
                    delta = [1.0, 0.0];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                add_displacement(
                    &mut displacements,
                    snapshot.client_id,
                    [normal[0] * overlap, normal[1] * overlap],
                );
            }
        }

        for snapshot in &hero_snapshots {
            let mut delta = [
                snapshot.pos[0] - BASE_POSITION[0],
                snapshot.pos[1] - BASE_POSITION[1],
            ];
            let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
            let min_dist = HERO_COLLIDER_RADIUS + OBJECTIVE_COLLIDER_RADIUS;
            let min_dist_sq = min_dist * min_dist;
            if dist_sq >= min_dist_sq {
                continue;
            }

            if dist_sq <= f32::EPSILON {
                delta = [1.0, 0.0];
                dist_sq = 1.0;
            }

            let dist = dist_sq.sqrt();
            let normal = [delta[0] / dist, delta[1] / dist];
            let overlap = (min_dist - dist).max(0.0);
            add_displacement(
                &mut displacements,
                snapshot.client_id,
                [normal[0] * overlap, normal[1] * overlap],
            );
        }

        for snapshot in &hero_snapshots {
            for enemy in &enemy_snapshots {
                let mut delta = [
                    snapshot.pos[0] - enemy.pos[0],
                    snapshot.pos[1] - enemy.pos[1],
                ];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = HERO_COLLIDER_RADIUS + enemy.radius;
                let min_dist_sq = min_dist * min_dist;
                if dist_sq >= min_dist_sq {
                    continue;
                }

                if dist_sq <= f32::EPSILON {
                    delta = [1.0, 0.0];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                add_displacement(
                    &mut displacements,
                    snapshot.client_id,
                    [normal[0] * overlap, normal[1] * overlap],
                );
            }
        }

        if displacements.is_empty() {
            return;
        }
        let mut heroes = world
            .query_filtered::<(&NetId, &mut Position), (With<HeroState>, With<SimulationOwned>)>();
        for (id, mut position) in heroes.iter_mut(world) {
            if id.epoch != epoch {
                continue;
            }
            let Some(&displacement) = displacements.get(&id.value) else {
                continue;
            };
            let limited =
                clamp_vector_length(displacement, HERO_COLLISION_MAX_DISPLACEMENT_PER_TICK);
            position.pos =
                clamp_to_world([position.pos[0] + limited[0], position.pos[1] + limited[1]]);
        }
    }

    pub(crate) fn resolve_enemy_collisions(&mut self, world: &mut World) {
        let epoch = world.resource::<Epoch>().0;
        let mut enemies =
            world.query_filtered::<(&NetId, &EnemyState, &Position), With<SimulationOwned>>();
        let mut snapshots: Vec<_> = enemies
            .iter(world)
            .filter(|(id, _, _)| id.epoch == epoch)
            .map(|(_, enemy, position)| EnemyCollisionSnapshot {
                id: enemy.id,
                pos: position.pos,
                radius: enemy.radius,
            })
            .collect();
        snapshots.sort_unstable_by_key(|enemy| enemy.id);
        let collision_cell_size = collision_spatial_cell_size(&snapshots);
        let collision_cells =
            build_spatial_cells(&snapshots, collision_cell_size, |snapshot| snapshot.pos);

        let tower_positions = tower_positions(world, epoch);
        let mut displacements: BTreeMap<u64, [f32; 2]> = BTreeMap::new();

        for i in 0..snapshots.len() {
            let a = snapshots[i];
            for_each_spatial_neighbor(&collision_cells, collision_cell_size, a.pos, |j| {
                if j <= i {
                    return;
                }
                let b = snapshots[j];

                let mut delta = [b.pos[0] - a.pos[0], b.pos[1] - a.pos[1]];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = a.radius + b.radius;
                let min_dist_sq = min_dist * min_dist;

                if dist_sq >= min_dist_sq {
                    return;
                }

                if dist_sq <= f32::EPSILON {
                    let angle = ((a.id ^ (b.id << 1)) % 6283) as f32 * 0.001;
                    delta = [angle.cos(), angle.sin()];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                let push = overlap * 0.5;

                add_displacement(
                    &mut displacements,
                    a.id,
                    [-normal[0] * push, -normal[1] * push],
                );
                add_displacement(
                    &mut displacements,
                    b.id,
                    [normal[0] * push, normal[1] * push],
                );
            });
        }

        for snapshot in &snapshots {
            for &(_, tower_pos) in &tower_positions {
                let mut delta = [
                    snapshot.pos[0] - tower_pos[0],
                    snapshot.pos[1] - tower_pos[1],
                ];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = snapshot.radius + TOWER_COLLIDER_RADIUS;
                let min_dist_sq = min_dist * min_dist;

                if dist_sq >= min_dist_sq {
                    continue;
                }

                if dist_sq <= f32::EPSILON {
                    delta = [1.0, 0.0];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                add_displacement(
                    &mut displacements,
                    snapshot.id,
                    [normal[0] * overlap, normal[1] * overlap],
                );
            }
        }

        for snapshot in &snapshots {
            let mut delta = [
                snapshot.pos[0] - BASE_POSITION[0],
                snapshot.pos[1] - BASE_POSITION[1],
            ];
            let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
            let min_dist = snapshot.radius + OBJECTIVE_COLLIDER_RADIUS;
            let min_dist_sq = min_dist * min_dist;

            if dist_sq >= min_dist_sq {
                continue;
            }

            if dist_sq <= f32::EPSILON {
                delta = [1.0, 0.0];
                dist_sq = 1.0;
            }

            let dist = dist_sq.sqrt();
            let normal = [delta[0] / dist, delta[1] / dist];
            let overlap = (min_dist - dist).max(0.0);
            add_displacement(
                &mut displacements,
                snapshot.id,
                [normal[0] * overlap, normal[1] * overlap],
            );
        }

        if displacements.is_empty() {
            return;
        }
        let mut enemies = world
            .query_filtered::<(&NetId, &mut EnemyState, &mut Position), With<SimulationOwned>>();
        for (id, mut enemy, mut position) in enemies.iter_mut(world) {
            if id.epoch != epoch {
                continue;
            }
            let Some(&displacement) = displacements.get(&id.value) else {
                continue;
            };
            position.pos = clamp_to_world([
                position.pos[0] + displacement[0],
                position.pos[1] + displacement[1],
            ]);
            if displacement[0].abs() > 0.0001 || displacement[1].abs() > 0.0001 {
                enemy.vel[0] *= ENEMY_COLLISION_DAMPING;
                enemy.vel[1] *= ENEMY_COLLISION_DAMPING;
            }
        }
    }
}

// Keep the canonical tower order: overlapping tower pushes accumulate floats in
// this order, independently of archetype storage or spawn order.
fn tower_positions(world: &mut World, epoch: u32) -> Vec<(u64, [f32; 2])> {
    let mut towers =
        world.query_filtered::<(&NetId, &Position), (With<TowerState>, With<SimulationOwned>)>();
    let mut positions: Vec<_> = towers
        .iter(world)
        .filter(|(id, _)| id.epoch == epoch)
        .map(|(id, position)| (id.value, position.pos))
        .collect();
    positions.sort_unstable_by_key(|(id, _)| *id);
    positions
}

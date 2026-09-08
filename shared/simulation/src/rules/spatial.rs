use crate::*;

pub(crate) fn enemy_separation_direction(
    enemy_id: u64,
    enemy_pos: [f32; 2],
    enemy_radius: f32,
    snapshots: &[EnemyMotionSnapshot],
    separation_cells: &BTreeMap<(i32, i32), Vec<usize>>,
    separation_cell_size: f32,
) -> [f32; 2] {
    let mut separation = [0.0, 0.0];

    for_each_spatial_neighbor(
        separation_cells,
        separation_cell_size,
        enemy_pos,
        |other_idx| {
            let other = snapshots[other_idx];
            if other.id == enemy_id {
                return;
            }

            let mut delta = [enemy_pos[0] - other.pos[0], enemy_pos[1] - other.pos[1]];
            let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
            let avoid_distance = enemy_radius + other.radius + ENEMY_SEPARATION_BUFFER;
            let avoid_distance_sq = avoid_distance * avoid_distance;
            if dist_sq >= avoid_distance_sq {
                return;
            }

            if dist_sq <= f32::EPSILON {
                let angle = ((enemy_id ^ (other.id << 1)) % 6283) as f32 * 0.001;
                delta = [angle.cos(), angle.sin()];
                dist_sq = 1.0;
            }

            let dist = dist_sq.sqrt();
            let weight = ((avoid_distance - dist) / avoid_distance).clamp(0.0, 1.0);
            separation[0] += (delta[0] / dist) * weight;
            separation[1] += (delta[1] / dist) * weight;
        },
    );

    normalize_or_zero(separation)
}

pub(crate) fn separation_spatial_cell_size(snapshots: &[EnemyMotionSnapshot]) -> f32 {
    let max_radius = snapshots
        .iter()
        .map(|snapshot| snapshot.radius)
        .fold(0.0, f32::max);
    (max_radius * 2.0 + ENEMY_SEPARATION_BUFFER).max(ENEMY_SPATIAL_CELL_MIN_SIZE)
}

pub(crate) fn collision_spatial_cell_size(snapshots: &[EnemyCollisionSnapshot]) -> f32 {
    let max_radius = snapshots
        .iter()
        .map(|snapshot| snapshot.radius)
        .fold(0.0, f32::max);
    (max_radius * 2.0).max(ENEMY_SPATIAL_CELL_MIN_SIZE)
}

pub(crate) fn build_spatial_cells<T>(
    values: &[T],
    cell_size: f32,
    pos_of: impl Fn(&T) -> [f32; 2],
) -> BTreeMap<(i32, i32), Vec<usize>> {
    let mut cells: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        let cell = spatial_cell_key(pos_of(value), cell_size);
        cells.entry(cell).or_default().push(index);
    }
    cells
}

pub(crate) fn for_each_spatial_neighbor(
    cells: &BTreeMap<(i32, i32), Vec<usize>>,
    cell_size: f32,
    pos: [f32; 2],
    mut f: impl FnMut(usize),
) {
    let (cx, cy) = spatial_cell_key(pos, cell_size);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let key = (cx + dx, cy + dy);
            let Some(indices) = cells.get(&key) else {
                continue;
            };
            for &index in indices {
                f(index);
            }
        }
    }
}

pub(crate) fn spatial_cell_key(pos: [f32; 2], cell_size: f32) -> (i32, i32) {
    (
        (pos[0] / cell_size).floor() as i32,
        (pos[1] / cell_size).floor() as i32,
    )
}

pub(crate) fn add_displacement(map: &mut BTreeMap<u64, [f32; 2]>, id: u64, delta: [f32; 2]) {
    let entry = map.entry(id).or_insert([0.0, 0.0]);
    entry[0] += delta[0];
    entry[1] += delta[1];
}

pub(crate) fn clamp_vector_length(delta: [f32; 2], max_length: f32) -> [f32; 2] {
    let length_sq = delta[0] * delta[0] + delta[1] * delta[1];
    let max_sq = max_length * max_length;
    if length_sq <= max_sq {
        return delta;
    }

    if length_sq <= f32::EPSILON {
        return [0.0, 0.0];
    }

    let scale = (max_sq / length_sq).sqrt();
    [delta[0] * scale, delta[1] * scale]
}

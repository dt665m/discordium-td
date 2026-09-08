use crate::*;

pub(crate) fn enemy_target_position(target: EnemyTarget) -> [f32; 2] {
    match target {
        EnemyTarget::Hero { pos, .. } => pos,
        EnemyTarget::Base => BASE_POSITION,
    }
}

pub(crate) fn choose_enemy_target(
    enemy_pos: [f32; 2],
    current_target: EnemyLockTarget,
    hero_targets: &[(u64, [f32; 2])],
) -> EnemyLockTarget {
    let hero_range_sq = ENEMY_HERO_AGGRO_RANGE * ENEMY_HERO_AGGRO_RANGE;
    let hero_disengage_sq = ENEMY_HERO_DISENGAGE_RANGE * ENEMY_HERO_DISENGAGE_RANGE;

    if let EnemyLockTarget::Hero(current_hero_id) = current_target {
        if let Some((_, current_hero_pos)) = hero_targets
            .iter()
            .find(|(hero_id, _)| *hero_id == current_hero_id)
        {
            if distance_sq(enemy_pos, *current_hero_pos) <= hero_disengage_sq {
                return current_target;
            }
        }
    }

    let mut best: Option<(u64, f32)> = None;
    for (hero_id, hero_pos) in hero_targets {
        let dist_sq = distance_sq(enemy_pos, *hero_pos);
        if dist_sq > hero_range_sq {
            continue;
        }

        match best {
            Some((_, best_dist_sq)) if dist_sq >= best_dist_sq => {}
            _ => best = Some((*hero_id, dist_sq)),
        }
    }

    if let Some((client_id, _)) = best {
        EnemyLockTarget::Hero(client_id)
    } else {
        EnemyLockTarget::Base
    }
}

pub(crate) fn resolve_enemy_target(
    target: EnemyLockTarget,
    hero_targets: &[(u64, [f32; 2])],
) -> EnemyTarget {
    match target {
        EnemyLockTarget::Hero(client_id) => hero_targets
            .iter()
            .find(|(hero_id, _)| *hero_id == client_id)
            .map(|(_, pos)| EnemyTarget::Hero {
                client_id,
                pos: *pos,
            })
            .unwrap_or(EnemyTarget::Base),
        EnemyLockTarget::Base => EnemyTarget::Base,
    }
}

pub(crate) fn enemy_target_attack_anchor(
    target: EnemyTarget,
    enemy_id: u64,
    attack_range: f32,
) -> [f32; 2] {
    let target_pos = enemy_target_position(target);
    let offset_radius = match target {
        EnemyTarget::Hero { .. } => (attack_range - 0.18).max(0.7),
        EnemyTarget::Base => (attack_range + ENEMY_BASE_ENGAGE_RING_PADDING).max(0.9),
    };

    let angle = enemy_slot_angle(enemy_id);
    clamp_to_world([
        target_pos[0] + angle.cos() * offset_radius,
        target_pos[1] + angle.sin() * offset_radius,
    ])
}

pub(crate) fn enemy_slot_angle(enemy_id: u64) -> f32 {
    // Golden-angle distribution produces stable spread around a target.
    let golden_angle = 2.399_963_1_f32;
    ((enemy_id as f32) * golden_angle) % TAU
}

pub(crate) fn nearest_enemy_id(
    hero_pos: [f32; 2],
    enemies: &BTreeMap<u64, [f32; 2]>,
) -> Option<u64> {
    let mut best: Option<(u64, f32)> = None;
    for (enemy_id, enemy_pos) in enemies {
        let dist_sq = distance_sq(hero_pos, *enemy_pos);
        match best {
            Some((_, best_dist_sq)) if dist_sq >= best_dist_sq => {}
            _ => best = Some((*enemy_id, dist_sq)),
        }
    }
    best.map(|(enemy_id, _)| enemy_id)
}

pub(crate) fn spawn_position_for_client(client_id: u64) -> [f32; 2] {
    let angle = ((client_id % 16) as f32 / 16.0) * TAU;
    let radius = 2.5 + ((client_id % 3) as f32 * 0.45);
    clamp_to_world([angle.cos() * radius, angle.sin() * radius])
}

use bevy::prelude::*;
use game_shared::distance_sq;

pub struct LockOnPlugin;

impl Plugin for LockOnPlugin {
    fn build(&self, _app: &mut App) {}
}

pub fn select_next_lock_target(
    current_target_id: Option<u64>,
    source_pos: [f32; 2],
    candidates: impl IntoIterator<Item = (u64, [f32; 2])>,
) -> Option<u64> {
    let mut sorted_candidates: Vec<(u64, f32)> = candidates
        .into_iter()
        .map(|(id, pos)| (id, distance_sq(source_pos, pos)))
        .collect();
    sorted_candidates.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    if sorted_candidates.is_empty() {
        return None;
    }

    if let Some(current_target_id) = current_target_id {
        if let Some(index) = sorted_candidates
            .iter()
            .position(|(candidate_id, _)| *candidate_id == current_target_id)
        {
            if sorted_candidates.len() == 1 {
                return Some(sorted_candidates[index].0);
            }
            if let Some((target_id, _)) = sorted_candidates
                .iter()
                .find(|(candidate_id, _)| *candidate_id != current_target_id)
            {
                return Some(*target_id);
            }
            return Some(sorted_candidates[index].0);
        }
    }

    Some(sorted_candidates[0].0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_next_lock_target_picks_nearest_then_id() {
        let selected = select_next_lock_target(
            None,
            [0.0, 0.0],
            vec![(30, [3.0, 0.0]), (10, [1.0, 0.0]), (20, [1.0, 0.0])],
        );
        assert_eq!(selected, Some(10));
    }

    #[test]
    fn select_next_lock_target_prefers_nearest_other_target() {
        let candidates = vec![(10, [1.0, 0.0]), (20, [2.0, 0.0]), (30, [3.0, 0.0])];
        assert_eq!(
            select_next_lock_target(Some(20), [0.0, 0.0], candidates.clone()),
            Some(10)
        );
        assert_eq!(
            select_next_lock_target(Some(10), [0.0, 0.0], candidates),
            Some(20)
        );
    }

    #[test]
    fn select_next_lock_target_uses_nearest_when_current_missing() {
        let selected = select_next_lock_target(
            Some(999),
            [0.0, 0.0],
            vec![(7, [4.0, 0.0]), (2, [1.0, 0.0])],
        );
        assert_eq!(selected, Some(2));
    }

    #[test]
    fn select_next_lock_target_keeps_only_target_when_single_candidate() {
        let selected = select_next_lock_target(Some(5), [0.0, 0.0], vec![(5, [2.0, 0.0])]);
        assert_eq!(selected, Some(5));
    }
}

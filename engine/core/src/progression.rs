use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Progression {
    pub level: u32,
    pub xp: f32,
    pub xp_next: f32,
    pub pending_levels: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressionError {
    InvalidExperience,
    InvalidThreshold,
    LevelOverflow,
}
impl Progression {
    /// Invalid curves are rejected atomically; game callbacks should be pure.
    /// The callback receives the new level and returns its next XP threshold.
    pub fn gain(
        &mut self,
        amount: f32,
        mut next_threshold: impl FnMut(u32) -> f32,
    ) -> Result<u32, ProgressionError> {
        if !amount.is_finite() || amount < 0.0 || !self.xp.is_finite() || self.xp < 0.0 {
            return Err(ProgressionError::InvalidExperience);
        }
        let mut updated = *self;
        updated.xp += amount;
        if !updated.xp.is_finite() {
            return Err(ProgressionError::InvalidExperience);
        }
        let mut gained = 0_u32;
        loop {
            if !updated.xp_next.is_finite() || updated.xp_next <= 0.0 {
                return Err(ProgressionError::InvalidThreshold);
            }
            if updated.xp < updated.xp_next {
                break;
            }
            let remainder = updated.xp - updated.xp_next;
            // A positive threshold below floating-point resolution cannot advance.
            if remainder == updated.xp {
                return Err(ProgressionError::InvalidThreshold);
            }
            updated.xp = remainder;
            updated.level = updated
                .level
                .checked_add(1)
                .ok_or(ProgressionError::LevelOverflow)?;
            updated.pending_levels = updated
                .pending_levels
                .checked_add(1)
                .ok_or(ProgressionError::LevelOverflow)?;
            gained = gained
                .checked_add(1)
                .ok_or(ProgressionError::LevelOverflow)?;
            updated.xp_next = next_threshold(updated.level);
        }
        *self = updated;
        Ok(gained)
    }
}
pub fn spend_currency(balance: &mut u32, cost: u32) -> bool {
    if *balance < cost {
        return false;
    }
    *balance -= cost;
    true
}
/// Validate both constraints before mutating either currency or rank.
pub fn purchase_rank(balance: &mut u32, rank: &mut u8, cost: u32, cap: u8) -> bool {
    if *rank >= cap || *balance < cost {
        return false;
    }
    *balance -= cost;
    *rank += 1;
    true
}
pub fn add_capped(value: f32, amount: f32, cap: f32) -> f32 {
    (value + amount).min(cap)
}
pub fn multiply_capped(value: f32, factor: f32, cap: f32) -> f32 {
    (value * factor).min(cap)
}
pub fn multiply_floored(value: f32, factor: f32, floor: f32) -> f32 {
    (value * factor).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn experience_uses_game_curve_and_rejects_invalid_curve_atomically() {
        let mut progress = Progression {
            level: 1,
            xp: 0.0,
            xp_next: 10.0,
            pending_levels: 0,
        };
        assert_eq!(progress.gain(35.0, |level| level as f32 * 10.0), Ok(2));
        assert_eq!(
            (progress.level, progress.xp, progress.pending_levels),
            (3, 5.0, 2)
        );
        let previous = progress;
        assert_eq!(
            progress.gain(100.0, |_| 0.0),
            Err(ProgressionError::InvalidThreshold)
        );
        assert_eq!(progress, previous);
    }
    #[test]
    fn rank_purchase_is_atomic_at_caps_and_insufficient_funds() {
        let (mut balance, mut rank) = (10, 1);
        assert!(!purchase_rank(&mut balance, &mut rank, 11, 2));
        assert_eq!((balance, rank), (10, 1));
        assert!(purchase_rank(&mut balance, &mut rank, 4, 2));
        assert!(!purchase_rank(&mut balance, &mut rank, 4, 2));
        assert_eq!((balance, rank), (6, 2));
    }
}

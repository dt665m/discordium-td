use super::components::*;
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

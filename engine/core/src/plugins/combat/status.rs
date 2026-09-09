use super::effects::{CombatState, nonnegative};
use serde::{Deserialize, Serialize};

/// A stable identifier allocated by the consuming game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusId(pub u16);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedStatus {
    pub id: StatusId,
    pub magnitude: f32,
    pub remaining: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum DurationPolicy {
    /// Replace the previous duration, even when the new duration is shorter.
    Reset,
    KeepLonger,
    Extend,
}

pub(super) fn duration(previous: f32, supplied: f32, policy: DurationPolicy) -> f32 {
    let previous = nonnegative(previous);
    let supplied = nonnegative(supplied);
    match policy {
        DurationPolicy::Reset => supplied,
        DurationPolicy::KeepLonger => previous.max(supplied),
        DurationPolicy::Extend => (previous + supplied).min(f32::MAX),
    }
}

impl CombatState {
    pub fn grant_invulnerability(&mut self, seconds: f32, policy: DurationPolicy) {
        self.invulnerability_remaining = duration(self.invulnerability_remaining, seconds, policy);
    }

    pub fn is_invulnerable(&self) -> bool {
        self.invulnerability_remaining > 0.0
    }

    /// Refreshes duration and replaces magnitude for an existing identity.
    /// Zero duration removes the status. Non-finite magnitudes become zero.
    pub fn apply_status(
        &mut self,
        id: StatusId,
        magnitude: f32,
        seconds: f32,
        policy: DurationPolicy,
    ) {
        let magnitude = if magnitude.is_finite() {
            magnitude
        } else {
            0.0
        };
        if let Some(status) = self.statuses.iter_mut().find(|status| status.id == id) {
            status.magnitude = magnitude;
            status.remaining = duration(status.remaining, seconds, policy);
        } else {
            self.statuses.push(TimedStatus {
                id,
                magnitude,
                remaining: duration(0.0, seconds, policy),
            });
        }
        self.statuses.retain(|status| status.remaining > 0.0);
    }

    pub fn status(&self, id: StatusId) -> Option<&TimedStatus> {
        self.statuses
            .iter()
            .find(|status| status.id == id && status.remaining > 0.0)
    }
}

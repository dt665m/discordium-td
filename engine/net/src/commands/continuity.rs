use serde::{Deserialize, Serialize};

/// Committed input state needed to reproduce bounded missing-input substitution.
/// Action lists are intentionally absent; games supply their held-input filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputContinuity<I> {
    pub last_input: Option<I>,
    pub missing_streak: u32,
}

impl<I> Default for InputContinuity<I> {
    fn default() -> Self {
        Self {
            last_input: None,
            missing_streak: 0,
        }
    }
}

impl<I> InputContinuity<I> {
    pub fn map<T>(&self, map: impl FnOnce(&I) -> T) -> InputContinuity<T> {
        InputContinuity {
            last_input: self.last_input.as_ref().map(map),
            missing_streak: self.missing_streak,
        }
    }

    pub fn commit_input(&mut self, input: I) {
        self.last_input = Some(input);
        self.missing_streak = 0;
    }

    /// Prepare without changing committed state. The caller commits the returned
    /// streak only after its simulation step succeeds.
    pub fn substitute(
        &self,
        grace_ticks: u8,
        held: impl FnOnce(&I) -> I,
        neutral: impl FnOnce() -> I,
    ) -> (I, u32) {
        let missing_streak = self.missing_streak.saturating_add(1);
        let input = if missing_streak <= u32::from(grace_ticks) {
            self.last_input.as_ref().map_or_else(neutral, held)
        } else {
            neutral()
        };
        (input, missing_streak)
    }
}

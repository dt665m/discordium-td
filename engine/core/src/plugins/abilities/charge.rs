//! Saved, episode-fenced charge timing. Games own costs, interruption policy and curves.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChargeCommand {
    #[default]
    None,
    Begin {
        episode: u64,
    },
    Release {
        episode: u64,
    },
    Cancel {
        episode: u64,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotionCurveKey {
    pub id: u16,
    pub version: u16,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ChargePhase {
    #[default]
    Idle,
    Charging {
        ticks: u16,
    },
    Executing {
        curve: MotionCurveKey,
        cursor: u16,
        direction: [f32; 2],
        distance: f32,
    },
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChargeState {
    pub episode: u64,
    pub phase: ChargePhase,
    pub cooldown_ticks: u16,
}
impl ChargeState {
    pub fn tick(&mut self, max_charge_ticks: u16) {
        self.cooldown_ticks = self.cooldown_ticks.saturating_sub(1);
        if let ChargePhase::Charging { ticks } = &mut self.phase {
            *ticks = ticks.saturating_add(1).min(max_charge_ticks);
        }
    }
    pub fn begin(&mut self, episode: u64) -> bool {
        if episode == 0
            || episode <= self.episode
            || self.phase != ChargePhase::Idle
            || self.cooldown_ticks != 0
        {
            return false;
        }
        self.episode = episode;
        self.phase = ChargePhase::Charging { ticks: 0 };
        true
    }
    pub fn cancel(&mut self, episode: u64) -> bool {
        if episode == 0 || episode != self.episode || self.phase == ChargePhase::Idle {
            return false;
        }
        self.phase = ChargePhase::Idle;
        true
    }
    pub fn interrupt(&mut self) {
        self.phase = ChargePhase::Idle;
    }
}

/// A monotonic cumulative planar distance curve. Adjacent samples are consumed
/// once per fixed tick; collision truncation never repeats a consumed sample.
pub fn authored_motion_delta(
    state: &mut ChargeState,
    key: MotionCurveKey,
    samples: &[f32],
) -> Option<[f32; 2]> {
    let ChargePhase::Executing {
        curve,
        cursor,
        direction,
        distance,
    } = state.phase
    else {
        return None;
    };
    let i = usize::from(cursor);
    if curve != key
        || samples.len() < 2
        || samples.len() > usize::from(u16::MAX) + 1
        || i + 1 >= samples.len()
        || samples.first() != Some(&0.0)
        || samples.last() != Some(&1.0)
        || samples.windows(2).any(|s| !s[0].is_finite() || s[0] > s[1])
        || !distance.is_finite()
        || distance < 0.0
        || direction.iter().any(|v| !v.is_finite())
        || ((direction[0] * direction[0] + direction[1] * direction[1]) - 1.0).abs() > 0.001
    {
        return None;
    }
    let amount = (samples[i + 1] - samples[i]) * distance;
    state.phase = if i + 2 == samples.len() {
        ChargePhase::Idle
    } else {
        ChargePhase::Executing {
            curve,
            cursor: cursor + 1,
            direction,
            distance,
        }
    };
    Some([direction[0] * amount, direction[1] * amount])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn episode_fences_cancel_and_curve_progress_survive_restore() {
        let mut state = ChargeState::default();
        assert!(state.begin(7));
        assert!(!state.cancel(6));
        state.tick(3);
        assert_eq!(state.phase, ChargePhase::Charging { ticks: 1 });
        assert!(state.cancel(7));
        assert!(!state.begin(7));
        assert!(state.begin(8));
        let curve = MotionCurveKey { id: 9, version: 2 };
        state.phase = ChargePhase::Executing {
            curve,
            cursor: 0,
            direction: [1.0, 0.0],
            distance: 4.0,
        };
        let before = state;
        assert_eq!(
            authored_motion_delta(
                &mut state,
                MotionCurveKey { id: 9, version: 3 },
                &[0.0, 0.25, 1.0]
            ),
            None
        );
        assert_eq!(state, before);
        assert_eq!(
            authored_motion_delta(&mut state, curve, &[0.0, 0.25, 1.0]),
            Some([1.0, 0.0])
        );
        let mut restored =
            serde_json::from_slice::<ChargeState>(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            authored_motion_delta(&mut restored, curve, &[0.0, 0.25, 1.0]),
            Some([3.0, 0.0])
        );
        assert_eq!(restored.phase, ChargePhase::Idle);
    }
}

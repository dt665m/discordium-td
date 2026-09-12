//! Timestamped hardware journal, separate from simulation and replay.
//!
//! A sample is consumed by exactly one fixed-step command. Held state persists;
//! mouse deltas and edges do not. On overflow the caller resynchronizes instead of
//! dropping an unknown button transition or duplicating input during catch-up.
use crate::codec::BoundedVec;
use std::{collections::VecDeque, time::Duration};

#[derive(Debug, Clone, PartialEq)]
pub struct HardwareSample<H, E> {
    pub at: Duration,
    pub held: Option<H>,
    pub edges: BoundedVec<E, 8>,
    pub mouse_delta: [f64; 2],
}
#[derive(Debug, Clone, PartialEq)]
pub struct JournalFrame<H, E> {
    pub deadline: Duration,
    pub held: H,
    pub edges: BoundedVec<E, 8>,
    pub mouse_delta: [f64; 2],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalError {
    InvalidCapacity,
    Full,
    NonMonotonicSample,
    ClosedSampleTime,
    NonMonotonicDeadline,
    NonFiniteMouseDelta,
    TooManyEdges,
}
impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "input journal: {self:?}")
    }
}
impl std::error::Error for JournalError {}

pub struct InputJournal<H, E> {
    held: H,
    samples: VecDeque<HardwareSample<H, E>>,
    capacity: usize,
    last_sample: Option<Duration>,
    closed_through: Option<Duration>,
}
impl<H: Clone, E: Clone> InputJournal<H, E> {
    pub fn new(neutral: H, capacity: usize) -> Result<Self, JournalError> {
        if capacity == 0 {
            return Err(JournalError::InvalidCapacity);
        }
        Ok(Self {
            held: neutral,
            samples: VecDeque::new(),
            capacity,
            last_sample: None,
            closed_through: None,
        })
    }
    pub fn pending_samples(&self) -> usize {
        self.samples.len()
    }
    pub fn capture(&mut self, sample: HardwareSample<H, E>) -> Result<(), JournalError> {
        if sample.mouse_delta.iter().any(|v| !v.is_finite()) {
            return Err(JournalError::NonFiniteMouseDelta);
        }
        if self.last_sample.is_some_and(|at| sample.at < at) {
            return Err(JournalError::NonMonotonicSample);
        }
        if self.closed_through.is_some_and(|at| sample.at <= at) {
            return Err(JournalError::ClosedSampleTime);
        }
        if self.samples.len() == self.capacity {
            return Err(JournalError::Full);
        }
        self.last_sample = Some(sample.at);
        self.samples.push_back(sample);
        Ok(())
    }
    /// Stage all events through this monotonic deadline and commit consumption only
    /// if the complete command fits its action budget. Timestamps at the boundary
    /// belong to this command. A failed drain consumes no input.
    pub fn drain_until(&mut self, deadline: Duration) -> Result<JournalFrame<H, E>, JournalError> {
        if self.closed_through.is_some_and(|at| deadline < at) {
            return Err(JournalError::NonMonotonicDeadline);
        }
        let mut held = self.held.clone();
        let mut edges = BoundedVec::default();
        let mut mouse_delta = [0.0; 2];
        let mut consumed = 0;
        for sample in self
            .samples
            .iter()
            .take_while(|sample| sample.at <= deadline)
        {
            if edges.len() + sample.edges.len() > 8 {
                return Err(JournalError::TooManyEdges);
            }
            for edge in sample.edges.as_slice() {
                edges.push(edge.clone()).expect("checked action bound");
            }
            if let Some(value) = &sample.held {
                held = value.clone();
            }
            for axis in 0..2 {
                mouse_delta[axis] += sample.mouse_delta[axis];
                if !mouse_delta[axis].is_finite() {
                    return Err(JournalError::NonFiniteMouseDelta);
                }
            }
            consumed += 1;
        }
        for _ in 0..consumed {
            self.samples.pop_front();
        }
        self.held = held.clone();
        self.closed_through = Some(deadline);
        Ok(JournalFrame {
            deadline,
            held,
            edges,
            mouse_delta,
        })
    }
    /// Use only at an explicit input-stream reset, after resolving old predicted
    /// actions. Suspension must not drain a stale backlog into new gameplay ticks.
    pub fn reset(&mut self, neutral: H) {
        self.held = neutral;
        self.samples.clear();
        self.last_sample = None;
        self.closed_through = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(
        ms: u64,
        held: Option<bool>,
        edges: Vec<bool>,
        mouse_delta: [f64; 2],
    ) -> HardwareSample<bool, bool> {
        HardwareSample {
            at: Duration::from_millis(ms),
            held,
            edges: BoundedVec::new(edges).unwrap(),
            mouse_delta,
        }
    }
    #[test]
    fn low_fps_press_release_and_mouse_are_consumed_once_across_catchup() {
        let mut journal = InputJournal::new(false, 32).unwrap();
        journal
            .capture(sample(5, Some(true), vec![true], [3.0, 4.0]))
            .unwrap();
        journal
            .capture(sample(9, Some(false), vec![false], [2.0, -1.0]))
            .unwrap();
        let first = journal.drain_until(Duration::from_millis(16)).unwrap();
        assert_eq!(first.edges.as_slice(), &[true, false]);
        assert!(!first.held);
        assert_eq!(first.mouse_delta, [5.0, 3.0]);
        for ms in [33, 50, 66] {
            let next = journal.drain_until(Duration::from_millis(ms)).unwrap();
            assert!(next.edges.is_empty());
            assert_eq!(next.mouse_delta, [0.0; 2]);
        }
    }
    #[test]
    fn future_held_samples_do_not_move_into_earlier_ticks() {
        let mut journal = InputJournal::new(false, 4).unwrap();
        journal
            .capture(sample(30, Some(true), vec![true], [2.0; 2]))
            .unwrap();
        assert!(!journal.drain_until(Duration::from_millis(16)).unwrap().held);
        let current = journal.drain_until(Duration::from_millis(30)).unwrap();
        assert!(current.held);
        let next = journal.drain_until(Duration::from_millis(50)).unwrap();
        assert!(next.held);
        assert!(next.edges.is_empty());
        assert_eq!(next.mouse_delta, [0.0; 2]);
        assert_eq!(
            journal.capture(sample(30, None, vec![], [0.0; 2])),
            Err(JournalError::ClosedSampleTime)
        );
    }
    #[test]
    fn action_overflow_is_transactional_and_reset_drops_stale_edges() {
        let mut journal = InputJournal::new(false, 2).unwrap();
        journal
            .capture(sample(1, Some(true), vec![true; 5], [2.0; 2]))
            .unwrap();
        journal
            .capture(sample(2, Some(false), vec![false; 4], [3.0; 2]))
            .unwrap();
        assert_eq!(
            journal.drain_until(Duration::from_millis(3)),
            Err(JournalError::TooManyEdges)
        );
        assert_eq!(journal.pending_samples(), 2);
        assert_eq!(
            journal.capture(sample(3, None, vec![], [0.0; 2])),
            Err(JournalError::Full)
        );
        assert_eq!(
            journal
                .drain_until(Duration::from_millis(1))
                .unwrap()
                .edges
                .len(),
            5
        );
        assert_eq!(
            journal
                .drain_until(Duration::from_millis(2))
                .unwrap()
                .edges
                .len(),
            4
        );
        journal
            .capture(sample(5, Some(true), vec![true], [8.0; 2]))
            .unwrap();
        journal.reset(false);
        let reset = journal.drain_until(Duration::from_secs(20)).unwrap();
        assert!(!reset.held);
        assert!(reset.edges.is_empty());
        assert_eq!(reset.mouse_delta, [0.0; 2]);
    }
    #[test]
    fn nonfinite_and_reordered_input_do_not_consume_journal_capacity() {
        let mut journal = InputJournal::new(false, 2).unwrap();
        assert_eq!(
            journal.capture(sample(1, None, vec![], [f64::NAN, 0.0])),
            Err(JournalError::NonFiniteMouseDelta)
        );
        journal.capture(sample(2, None, vec![], [0.0; 2])).unwrap();
        assert_eq!(
            journal.capture(sample(1, None, vec![], [0.0; 2])),
            Err(JournalError::NonMonotonicSample)
        );
        assert_eq!(journal.pending_samples(), 1);
    }
}

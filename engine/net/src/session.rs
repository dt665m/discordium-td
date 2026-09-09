//! Per-peer command sequencing independent of payloads, channels, or game policy.
use crate::newer;
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxError {
    MessageLimit,
    ActionLimit,
}

/// Latest held input and a bounded one-shot FIFO. Received sequence numbers are
/// distinct from applied acknowledgements so snapshots never acknowledge work
/// merely because a packet arrived. Limits and freshness policy come from games.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerInbox<I, A> {
    pub input: I,
    pub received_input: Option<u32>,
    pub applied_input: Option<u32>,
    pub received_action: Option<u32>,
    pub applied_action: Option<u32>,
    pub input_at: Duration,
    pub actions: VecDeque<(u32, A)>,
    pub budget: usize,
    epoch: u32,
    action_taken: bool,
}

impl<I: Default, A> PeerInbox<I, A> {
    pub fn new(epoch: u32) -> Self {
        Self {
            input: I::default(),
            received_input: None,
            applied_input: None,
            received_action: None,
            applied_action: None,
            input_at: Duration::ZERO,
            actions: VecDeque::new(),
            budget: 0,
            epoch,
            action_taken: false,
        }
    }

    /// Restart/re-admission cannot inherit held input, queued commands or ACKs.
    pub fn reset(&mut self, epoch: u32) {
        *self = Self::new(epoch);
    }
}

impl<I, A> PeerInbox<I, A> {
    /// Charge before channel or payload validation, including stale packets.
    pub fn consume_message(&mut self, limit: usize) -> Result<(), InboxError> {
        if self.budget >= limit {
            return Err(InboxError::MessageLimit);
        }
        self.budget += 1;
        Ok(())
    }

    pub fn accepts_input(&self, epoch: u32, seq: u32) -> bool {
        epoch == self.epoch && newer(seq, self.received_input)
    }

    /// Commit a game-validated held input; replayed or previous-epoch packets do nothing.
    pub fn receive_input(&mut self, epoch: u32, seq: u32, input: I, now: Duration) -> bool {
        if !self.accepts_input(epoch, seq) {
            return false;
        }
        self.input = input;
        self.received_input = Some(seq);
        self.input_at = now;
        true
    }

    pub fn accepts_action(&self, epoch: u32, seq: u32) -> bool {
        epoch == self.epoch && newer(seq, self.received_action)
    }

    /// Queue a validated command. A full queue leaves receive sequence unchanged
    /// so rejection cannot falsely acknowledge or silently discard a command.
    pub fn queue_action(
        &mut self,
        epoch: u32,
        seq: u32,
        action: A,
        limit: usize,
    ) -> Result<bool, InboxError> {
        if !self.accepts_action(epoch, seq) {
            return Ok(false);
        }
        if self.actions.len() >= limit {
            return Err(InboxError::ActionLimit);
        }
        self.received_action = Some(seq);
        self.actions.push_back((seq, action));
        Ok(true)
    }

    pub fn begin_tick(&mut self) {
        self.budget = 0;
        self.action_taken = false;
    }

    /// The game decides which held-input fields to neutralize when this is false.
    pub fn input_is_fresh(&self, now: Duration, timeout: Duration) -> bool {
        now.saturating_sub(self.input_at) <= timeout
    }

    /// At most one queued command is consumed per simulation tick.
    pub fn next_action(&mut self) -> Option<(u32, A)> {
        if self.action_taken {
            return None;
        }
        let action = self.actions.pop_front()?;
        self.action_taken = true;
        Some(action)
    }

    pub fn acknowledge_input(&mut self) {
        self.applied_input = self.received_input;
    }

    /// Acknowledge after the game processes a command, including a game-policy
    /// rejection. Games omit this call when the command resets the match epoch.
    pub fn acknowledge_action(&mut self, seq: u32) {
        if newer(seq, self.applied_action) {
            self.applied_action = Some(seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn received_and_applied_are_distinct_and_sequences_wrap() {
        let mut inbox = PeerInbox::<u8, u8>::new(4);
        assert!(inbox.receive_input(4, u32::MAX, 1, Duration::ZERO));
        assert_eq!(inbox.applied_input, None);
        assert!(inbox.receive_input(4, 0, 2, Duration::ZERO));
        assert!(!inbox.receive_input(4, u32::MAX, 3, Duration::ZERO));
        assert!(!inbox.receive_input(4, 1 << 31, 3, Duration::ZERO));
        inbox.acknowledge_input();
        assert_eq!((inbox.input, inbox.applied_input), (2, Some(0)));
        assert_eq!(inbox.queue_action(4, u32::MAX, 5, 4), Ok(true));
        assert_eq!(inbox.queue_action(4, 0, 6, 4), Ok(true));
        assert_eq!(inbox.queue_action(4, 0, 6, 4), Ok(false));
        assert_eq!(inbox.applied_action, None);
        let (seq, action) = inbox.next_action().unwrap();
        assert_eq!(action, 5);
        inbox.acknowledge_action(seq);
        assert!(inbox.next_action().is_none());
        inbox.begin_tick();
        let (seq, action) = inbox.next_action().unwrap();
        assert_eq!(action, 6);
        inbox.acknowledge_action(seq);
        assert_eq!(inbox.applied_action, Some(0));
    }

    #[test]
    fn limits_and_freshness_have_explicit_boundaries() {
        let mut inbox = PeerInbox::<u8, u8>::new(1);
        assert_eq!(inbox.consume_message(1), Ok(()));
        assert_eq!(inbox.consume_message(1), Err(InboxError::MessageLimit));
        assert_eq!(inbox.budget, 1);
        inbox.begin_tick();
        assert_eq!(inbox.consume_message(1), Ok(()));
        assert_eq!(inbox.queue_action(1, 1, 5, 1), Ok(true));
        assert_eq!(inbox.queue_action(1, 2, 6, 1), Err(InboxError::ActionLimit));
        assert_eq!(inbox.received_action, Some(1));
        inbox.next_action();
        assert_eq!(inbox.queue_action(1, 2, 6, 1), Ok(true));
        inbox.receive_input(1, 1, 4, Duration::from_millis(10));
        assert!(inbox.input_is_fresh(Duration::from_millis(210), Duration::from_millis(200)));
        assert!(!inbox.input_is_fresh(Duration::from_millis(211), Duration::from_millis(200)));
    }

    #[test]
    fn reset_discards_previous_admission_and_epoch_without_accepting_delayed_packets() {
        let mut inbox = PeerInbox::<u8, u8>::new(1);
        inbox.receive_input(1, 8, 9, Duration::from_secs(1));
        inbox.queue_action(1, 9, 10, 4).unwrap();
        inbox.acknowledge_input();
        inbox.consume_message(4).unwrap();
        inbox.reset(2);
        assert_eq!(inbox, PeerInbox::new(2));
        assert!(!inbox.receive_input(1, 10, 9, Duration::from_secs(2)));
        assert_eq!(inbox.queue_action(1, 10, 10, 4), Ok(false));
        assert!(inbox.receive_input(2, 0, 9, Duration::from_secs(2)));
    }

    #[test]
    fn serialized_inbox_replay_preserves_order_and_acknowledgements() {
        let mut inbox = PeerInbox::<u8, u8>::new(2);
        inbox.queue_action(2, 1, 5, 4).unwrap();
        inbox.queue_action(2, 2, 6, 4).unwrap();
        inbox.receive_input(2, 1, 3, Duration::ZERO);
        let mut restored: PeerInbox<u8, u8> = crate::decode(&crate::encode(&inbox)).unwrap();
        for _ in 0..3 {
            for state in [&mut inbox, &mut restored] {
                state.begin_tick();
                if let Some((seq, _)) = state.next_action() {
                    state.acknowledge_action(seq);
                }
                state.acknowledge_input();
            }
            assert_eq!(inbox, restored);
        }
        assert_eq!(inbox.applied_action, Some(2));
    }
}

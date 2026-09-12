//! Immutable target-tick command admission and explicit simulation finalization.
//!
//! One inbox belongs to one server-granted owner/stream incarnation. Receiving or
//! preparing input never advances authoritative time: only `commit` after the
//! simulation commits can move the finalized watermark. Transport ACKs are absent.
use crate::{codec::BoundedVec, types::*};
use bincode::Options;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, Write},
};
mod continuity;
pub use continuity::InputContinuity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerStream {
    pub connection: ConnectionId,
    pub epoch: ConnectionEpoch,
    /// Assign distinct streams to separately controlled owners on a connection.
    pub stream: CommandStream,
    pub owner: EntityId,
    pub ownership: OwnershipEpoch,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionEdge<A> {
    pub slot: u8,
    pub action: A,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command<I, A> {
    pub owner: OwnerStream,
    pub sequence: CommandSeq,
    pub target: TargetTick,
    pub input: I,
    pub actions: BoundedVec<ActionEdge<A>, 8>,
}
impl<I, A> Command<I, A> {
    pub fn action_key(&self, slot: u8) -> ActionKey {
        ActionKey {
            connection: self.owner.epoch,
            stream: self.owner.stream,
            command: self.sequence,
            slot,
        }
    }
}

/// Game/schema callbacks. Numeric ranges, enums, legal held bits and ownership at
/// the target tick are game decisions. Nested payload collections must use bounded
/// codecs before admission. `held_input` must clear per-sample deltas as well as
/// any input-level edges; the command's separate action list is always cleared.
pub trait CommandRules<I, A> {
    fn owns_at(&self, owner: OwnerStream, target: TargetTick) -> bool;
    fn valid_input(&self, input: &I) -> bool;
    fn valid_action(&self, action: &A) -> bool;
    fn held_input(&self, previous: &I) -> I;
    fn neutral_input(&self) -> I;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandLimits {
    pub future_ticks: u8,
    pub held_grace_ticks: u8,
    pub audit_records: usize,
    pub sequence_ahead: u64,
    pub max_command_bytes: usize,
    pub submissions_per_tick: usize,
}
impl Default for CommandLimits {
    fn default() -> Self {
        Self {
            future_ticks: 12,
            held_grace_ticks: 3,
            audit_records: 256,
            sequence_ahead: 256,
            max_command_bytes: 2048,
            submissions_per_tick: 64,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandError {
    InvalidConfiguration,
    WrongOwnerStream,
    OwnershipDenied,
    InvalidIdentity,
    InvalidInput,
    InvalidAction,
    DuplicateActionSlot,
    OversizedOrInvalidEncoding,
    Equivocation,
    SequenceWindow,
    AuditFull,
    RateLimit,
    TickInProgress,
    TickExhausted,
    NothingPrepared,
    WrongCommitTick,
    InvalidSubstitute,
    InvalidBundleLimit,
    NewestDoesNotFit,
}
impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command: {self:?}")
    }
}
impl std::error::Error for CommandError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Accepted { arrival_slack: u64 },
    Duplicate,
    Late,
    Future,
    TargetOccupied,
    RetiredSequence,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinalizedStatus {
    Executed,
    Substituted,
}
/// Executed means consumed by simulation, not that every requested game action was
/// accepted. Action outcomes are a separate game-authoritative result stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizedReceipt {
    pub tick: ServerTick,
    pub sequence: Option<CommandSeq>,
    pub status: FinalizedStatus,
}
#[derive(Debug, Clone)]
pub struct PreparedInput<I, A> {
    tick: ServerTick,
    sequence: Option<CommandSeq>,
    input: I,
    actions: BoundedVec<ActionEdge<A>, 8>,
    missing_streak: u32,
}
impl<I, A> PreparedInput<I, A> {
    pub fn tick(&self) -> ServerTick {
        self.tick
    }
    pub fn sequence(&self) -> Option<CommandSeq> {
        self.sequence
    }
    pub fn input(&self) -> &I {
        &self.input
    }
    pub fn actions(&self) -> &[ActionEdge<A>] {
        self.actions.as_slice()
    }
    pub fn is_substitute(&self) -> bool {
        self.sequence.is_none()
    }
}
struct AuditRecord {
    fingerprint: [u8; 32],
    admission: Admission,
    pending: bool,
}

pub struct CommandInbox<I, A> {
    owner: OwnerStream,
    limits: CommandLimits,
    finalized: ServerTick,
    pending: BTreeMap<TargetTick, Command<I, A>>,
    audit: BTreeMap<CommandSeq, AuditRecord>,
    audit_order: VecDeque<CommandSeq>,
    retired_sequence: Option<CommandSeq>,
    highest_received: Option<CommandSeq>,
    // Admission budget, not a received/applied acknowledgement. A committed
    // substitute grants one sequence slot just like a committed real command.
    sequence_window_floor: CommandSeq,
    receipts: VecDeque<FinalizedReceipt>,
    prepared: Option<PreparedInput<I, A>>,
    continuity: InputContinuity<I>,
    submissions: usize,
}
impl<I: Clone + Serialize, A: Clone + Serialize> CommandInbox<I, A> {
    /// Reset/reconnect constructs a new inbox under an explicit new stream epoch.
    /// Retained terminal gameplay outcomes must live outside this control inbox.
    pub fn new(
        owner: OwnerStream,
        initialized_at: ServerTick,
        limits: CommandLimits,
    ) -> Result<Self, CommandError> {
        if limits.future_ticks == 0
            || limits.future_ticks > 12
            || limits.held_grace_ticks > 3
            || limits.audit_records < usize::from(limits.future_ticks)
            || limits.sequence_ahead < u64::from(limits.future_ticks)
            || limits.max_command_bytes == 0
            || limits.submissions_per_tick == 0
        {
            return Err(CommandError::InvalidConfiguration);
        }
        if owner.epoch.0 == 0
            || owner.stream.0 == 0
            || owner.owner.generation == 0
            || owner.ownership.0 == 0
        {
            return Err(CommandError::InvalidIdentity);
        }
        Ok(Self {
            owner,
            limits,
            finalized: initialized_at,
            pending: BTreeMap::new(),
            audit: BTreeMap::new(),
            audit_order: VecDeque::new(),
            retired_sequence: None,
            highest_received: None,
            sequence_window_floor: CommandSeq(0),
            receipts: VecDeque::new(),
            prepared: None,
            continuity: InputContinuity::default(),
            submissions: 0,
        })
    }
    pub fn owner(&self) -> OwnerStream {
        self.owner
    }
    pub fn finalized_through(&self) -> ServerTick {
        self.finalized
    }
    /// Matches `finalized_through`; prepared or merely received input is excluded.
    pub fn input_continuity(&self) -> &InputContinuity<I> {
        &self.continuity
    }
    pub fn prepared_tick(&self) -> Option<ServerTick> {
        self.prepared.as_ref().map(|input| input.tick)
    }
    pub fn highest_received_sequence(&self) -> Option<CommandSeq> {
        self.highest_received
    }
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
    pub fn audit_len(&self) -> usize {
        self.audit.len()
    }
    /// Last admission retained by the existing bounded immutable-command audit.
    /// Inspect before `admit` to distinguish a first effective Late observation
    /// from retransmission, or a Future command becoming eligible later.
    pub fn retained_admission(&self, sequence: CommandSeq) -> Option<Admission> {
        self.audit.get(&sequence).map(|record| record.admission)
    }
    pub fn receipt(&self, tick: ServerTick) -> Option<FinalizedReceipt> {
        self.receipts
            .iter()
            .find(|receipt| receipt.tick == tick)
            .copied()
    }
    pub fn receipts(&self) -> impl ExactSizeIterator<Item = &FinalizedReceipt> {
        self.receipts.iter()
    }

    pub fn admit(
        &mut self,
        authenticated_connection: ConnectionId,
        command: Command<I, A>,
        rules: &impl CommandRules<I, A>,
    ) -> Result<Admission, CommandError> {
        if authenticated_connection != self.owner.connection || command.owner != self.owner {
            return Err(CommandError::WrongOwnerStream);
        }
        if self.submissions >= self.limits.submissions_per_tick {
            return Err(CommandError::RateLimit);
        }
        self.submissions += 1;
        if !rules.owns_at(command.owner, command.target) {
            return Err(CommandError::OwnershipDenied);
        }
        if command.sequence.0 == 0 || command.target.0 == 0 {
            return Err(CommandError::InvalidIdentity);
        }
        if !rules.valid_input(&command.input) {
            return Err(CommandError::InvalidInput);
        }
        let mut slots = BTreeSet::new();
        for edge in command.actions.as_slice() {
            if edge.slot >= 8 || !rules.valid_action(&edge.action) {
                return Err(CommandError::InvalidAction);
            }
            if !slots.insert(edge.slot) {
                return Err(CommandError::DuplicateActionSlot);
            }
        }
        let (fingerprint, _) = fingerprint(&command, self.limits.max_command_bytes)?;
        if let Some(previous) = self.audit.get(&command.sequence) {
            if previous.fingerprint != fingerprint {
                return Err(CommandError::Equivocation);
            }
            match previous.admission {
                Admission::Accepted { .. } => return Ok(Admission::Duplicate),
                Admission::Late => return Ok(Admission::Late),
                Admission::TargetOccupied => return Ok(Admission::TargetOccupied),
                // A too-early identical command may become eligible later. Its
                // original bytes remain immutable even before admission.
                _ => {}
            }
        } else {
            if self
                .retired_sequence
                .is_some_and(|floor| command.sequence <= floor)
            {
                return Ok(Admission::RetiredSequence);
            }
            if command.sequence.0
                > self
                    .sequence_window_floor
                    .0
                    .saturating_add(self.limits.sequence_ahead)
            {
                return Err(CommandError::SequenceWindow);
            }
        }
        let cutoff = self
            .prepared
            .as_ref()
            .map_or(self.finalized.0, |input| input.tick.0);
        let admission = if command.target.0 <= cutoff {
            Admission::Late
        } else if command.target.0
            > self
                .finalized
                .0
                .saturating_add(u64::from(self.limits.future_ticks))
        {
            Admission::Future
        } else if self.pending.contains_key(&command.target) {
            Admission::TargetOccupied
        } else {
            Admission::Accepted {
                arrival_slack: command.target.0 - self.finalized.0,
            }
        };
        self.reserve_audit(command.sequence)?;
        let accepted = matches!(admission, Admission::Accepted { .. });
        self.audit.insert(
            command.sequence,
            AuditRecord {
                fingerprint,
                admission,
                pending: accepted,
            },
        );
        if accepted {
            self.highest_received = Some(
                self.highest_received
                    .map_or(command.sequence, |seq| seq.max(command.sequence)),
            );
            self.pending.insert(command.target, command);
        }
        Ok(admission)
    }

    fn reserve_audit(&mut self, sequence: CommandSeq) -> Result<(), CommandError> {
        if self.audit.contains_key(&sequence) {
            return Ok(());
        }
        if self.audit.len() == self.limits.audit_records {
            let index = self
                .audit_order
                .iter()
                .position(|seq| !self.audit[seq].pending)
                .ok_or(CommandError::AuditFull)?;
            let evicted = self.audit_order.remove(index).expect("known audit index");
            self.audit.remove(&evicted);
            self.retired_sequence = Some(
                self.retired_sequence
                    .map_or(evicted, |old| old.max(evicted)),
            );
        }
        self.audit_order.push_back(sequence);
        Ok(())
    }

    /// Establish the next tick's input cutoff. Call exactly once before staging the
    /// shared simulation. Input arriving after this point cannot replace a substitute.
    pub fn prepare_next(
        &mut self,
        rules: &impl CommandRules<I, A>,
    ) -> Result<PreparedInput<I, A>, CommandError> {
        if self.prepared.is_some() {
            return Err(CommandError::TickInProgress);
        }
        let tick = self
            .finalized
            .checked_next()
            .ok_or(CommandError::TickExhausted)?;
        // Authority may revoke a previously admitted future command. Recheck
        // before selecting either explicit input or the last owner's held input.
        if !rules.owns_at(self.owner, TargetTick(tick.0)) {
            return Err(CommandError::OwnershipDenied);
        }
        let prepared = if let Some(command) = self.pending.get(&TargetTick(tick.0)) {
            PreparedInput {
                tick,
                sequence: Some(command.sequence),
                input: command.input.clone(),
                actions: command.actions.clone(),
                missing_streak: 0,
            }
        } else {
            let (input, missing_streak) = self.continuity.substitute(
                self.limits.held_grace_ticks,
                |last| rules.held_input(last),
                || rules.neutral_input(),
            );
            if !rules.valid_input(&input) {
                return Err(CommandError::InvalidSubstitute);
            }
            PreparedInput {
                tick,
                sequence: None,
                input,
                actions: BoundedVec::default(),
                missing_streak,
            }
        };
        self.prepared = Some(prepared.clone());
        Ok(prepared)
    }

    /// Invoke only AFTER S[tick] is committed. Simulation failures retain the
    /// prepared selection and leave the finalized watermark unchanged for recovery.
    pub fn commit(&mut self, tick: ServerTick) -> Result<FinalizedReceipt, CommandError> {
        let prepared = self
            .prepared
            .as_ref()
            .ok_or(CommandError::NothingPrepared)?;
        if prepared.tick != tick {
            return Err(CommandError::WrongCommitTick);
        }
        let prepared = self.prepared.take().expect("checked prepared tick");
        self.sequence_window_floor.0 = self.sequence_window_floor.0.saturating_add(1);
        if let Some(sequence) = prepared.sequence {
            self.continuity.commit_input(prepared.input);
            self.sequence_window_floor = self.sequence_window_floor.max(sequence);
            self.pending.remove(&TargetTick(tick.0));
            self.audit
                .get_mut(&sequence)
                .expect("pending audit retained")
                .pending = false;
        }
        self.continuity.missing_streak = prepared.missing_streak;
        self.finalized = tick;
        self.submissions = 0;
        let receipt = FinalizedReceipt {
            tick,
            sequence: prepared.sequence,
            status: if prepared.sequence.is_some() {
                FinalizedStatus::Executed
            } else {
                FinalizedStatus::Substituted
            },
        };
        if self.receipts.len() == self.limits.audit_records {
            self.receipts.pop_front();
        }
        self.receipts.push_back(receipt);
        Ok(receipt)
    }
}

struct FingerprintWriter {
    hasher: blake3::Hasher,
    length: usize,
    limit: usize,
}
impl Write for FingerprintWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.length) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "command encoding cap",
            ));
        }
        self.hasher.update(bytes);
        self.length += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn fingerprint<T: Serialize>(value: &T, limit: usize) -> Result<([u8; 32], usize), CommandError> {
    let mut writer = FingerprintWriter {
        hasher: blake3::Hasher::new(),
        length: 0,
        limit,
    };
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .serialize_into(&mut writer, value)
        .map_err(|_| CommandError::OversizedOrInvalidEncoding)?;
    Ok((*writer.hasher.finalize().as_bytes(), writer.length))
}

/// Independently decodable full records, newest first. `payload_budget` excludes
/// transport/application framing overhead. Never truncate the newest command or
/// its action list to fit; optional redundant records are omitted when necessary.
pub fn pack_redundant<I: Clone + Serialize, A: Clone + Serialize>(
    newest: &Command<I, A>,
    older: &[Command<I, A>],
    max_records: usize,
    payload_budget: usize,
) -> Result<BoundedVec<Command<I, A>, 8>, CommandError> {
    if !(1..=8).contains(&max_records) {
        return Err(CommandError::InvalidBundleLimit);
    }
    let mut result = BoundedVec::default();
    let (_, newest_bytes) =
        fingerprint(newest, payload_budget).map_err(|_| CommandError::NewestDoesNotFit)?;
    let mut remaining = payload_budget
        .checked_sub(8)
        .and_then(|n| n.checked_sub(newest_bytes))
        .ok_or(CommandError::NewestDoesNotFit)?;
    result
        .push(newest.clone())
        .expect("one record fits count bound");
    let mut seen = BTreeSet::from([(newest.owner.stream, newest.sequence)]);
    for command in older {
        if result.len() == max_records {
            break;
        }
        if command.owner != newest.owner || !seen.insert((command.owner.stream, command.sequence)) {
            continue;
        }
        if let Ok((_, bytes)) = fingerprint(command, remaining) {
            remaining -= bytes;
            result.push(command.clone()).expect("bounded record count");
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
    struct Input {
        movement: f32,
        mouse: f32,
        held: bool,
    }
    struct Rules;
    impl CommandRules<Input, bool> for Rules {
        fn owns_at(&self, _: OwnerStream, _: TargetTick) -> bool {
            true
        }
        fn valid_input(&self, input: &Input) -> bool {
            input.movement.is_finite() && input.movement.abs() <= 1.0 && input.mouse.is_finite()
        }
        fn valid_action(&self, _: &bool) -> bool {
            true
        }
        fn held_input(&self, previous: &Input) -> Input {
            Input {
                mouse: 0.0,
                ..*previous
            }
        }
        fn neutral_input(&self) -> Input {
            Input {
                movement: 0.0,
                mouse: 0.0,
                held: false,
            }
        }
    }
    fn owner() -> OwnerStream {
        OwnerStream {
            connection: ConnectionId(7),
            epoch: ConnectionEpoch(1),
            stream: CommandStream(1),
            owner: EntityId {
                index: 17,
                generation: 1,
            },
            ownership: OwnershipEpoch(1),
        }
    }
    fn command(sequence: u64, target: u64) -> Command<Input, bool> {
        Command {
            owner: owner(),
            sequence: CommandSeq(sequence),
            target: TargetTick(target),
            input: Input {
                movement: 0.5,
                mouse: 4.0,
                held: true,
            },
            actions: BoundedVec::new(vec![ActionEdge {
                slot: 0,
                action: true,
            }])
            .unwrap(),
        }
    }
    fn inbox() -> CommandInbox<Input, bool> {
        CommandInbox::new(owner(), ServerTick(0), CommandLimits::default()).unwrap()
    }
    fn advance(inbox: &mut CommandInbox<Input, bool>) -> PreparedInput<Input, bool> {
        let prepared = inbox.prepare_next(&Rules).unwrap();
        inbox.commit(prepared.tick()).unwrap();
        prepared
    }
    #[test]
    fn receiving_and_preparing_never_finalize_or_advance_world_ticks() {
        let mut inbox = inbox();
        assert_eq!(
            inbox.admit(ConnectionId(7), command(1, 1), &Rules),
            Ok(Admission::Accepted { arrival_slack: 1 })
        );
        assert_eq!(inbox.highest_received_sequence(), Some(CommandSeq(1)));
        assert_eq!(inbox.finalized_through(), ServerTick(0));
        let prepared = inbox.prepare_next(&Rules).unwrap();
        assert_eq!(prepared.actions().len(), 1);
        assert_eq!(inbox.finalized_through(), ServerTick(0));
        assert!(matches!(
            inbox.prepare_next(&Rules),
            Err(CommandError::TickInProgress)
        ));
        assert_eq!(
            inbox.commit(ServerTick(2)),
            Err(CommandError::WrongCommitTick)
        );
        assert_eq!(
            inbox.admit(ConnectionId(7), command(1, 1), &Rules),
            Ok(Admission::Duplicate)
        );
        assert_eq!(
            inbox.commit(ServerTick(1)).unwrap().status,
            FinalizedStatus::Executed
        );
        assert_eq!(inbox.finalized_through(), ServerTick(1));
        assert_eq!(
            inbox.admit(ConnectionId(7), command(1, 1), &Rules),
            Ok(Admission::Duplicate)
        );
        assert_eq!(
            inbox.commit(ServerTick(1)),
            Err(CommandError::NothingPrepared)
        );
    }
    #[test]
    fn immutable_payload_target_owner_and_numeric_checks_precede_execution() {
        let mut inbox = inbox();
        inbox.admit(ConnectionId(7), command(1, 2), &Rules).unwrap();
        let mut changed = command(1, 2);
        changed.input.movement = -0.5;
        assert_eq!(
            inbox.admit(ConnectionId(7), changed, &Rules),
            Err(CommandError::Equivocation)
        );
        assert_eq!(
            inbox.admit(ConnectionId(7), command(2, 2), &Rules),
            Ok(Admission::TargetOccupied)
        );
        let mut wrong_epoch = command(3, 3);
        wrong_epoch.owner.epoch = ConnectionEpoch(2);
        assert_eq!(
            inbox.admit(ConnectionId(7), wrong_epoch, &Rules),
            Err(CommandError::WrongOwnerStream)
        );
        assert_eq!(
            inbox.admit(ConnectionId(8), command(3, 3), &Rules),
            Err(CommandError::WrongOwnerStream)
        );
        let mut invalid = command(3, 3);
        invalid.input.movement = f32::NAN;
        assert_eq!(
            inbox.admit(ConnectionId(7), invalid, &Rules),
            Err(CommandError::InvalidInput)
        );
        let mut duplicate_slot = command(3, 3);
        duplicate_slot
            .actions
            .push(ActionEdge {
                slot: 0,
                action: false,
            })
            .unwrap();
        assert_eq!(
            inbox.admit(ConnectionId(7), duplicate_slot, &Rules),
            Err(CommandError::DuplicateActionSlot)
        );
        assert_eq!(inbox.pending_len(), 1);
        assert!(advance(&mut inbox).is_substitute());
        assert_eq!(advance(&mut inbox).input().movement, 0.5);
    }
    #[test]
    fn missing_input_grace_clears_edges_mouse_and_then_risky_held_controls() {
        let mut inbox = inbox();
        inbox.admit(ConnectionId(7), command(1, 1), &Rules).unwrap();
        advance(&mut inbox);
        for tick in 2..=4 {
            let input = advance(&mut inbox);
            assert!(input.is_substitute());
            assert_eq!(input.input().movement, 0.5);
            assert!(input.input().held);
            assert_eq!(input.input().mouse, 0.0);
            assert!(input.actions().is_empty());
            assert_eq!(
                inbox.receipt(ServerTick(tick)).unwrap().status,
                FinalizedStatus::Substituted
            );
        }
        let neutral = advance(&mut inbox);
        assert_eq!(*neutral.input(), Rules.neutral_input());
        assert_eq!(
            inbox.admit(ConnectionId(7), command(2, 3), &Rules),
            Ok(Admission::Late)
        );
        assert_eq!(
            inbox.admit(ConnectionId(7), command(2, 3), &Rules),
            Ok(Admission::Late)
        );
        assert!(advance(&mut inbox).actions().is_empty());
    }
    #[test]
    fn prepared_substitute_cannot_be_replaced_before_commit() {
        let mut inbox = inbox();
        assert!(inbox.prepare_next(&Rules).unwrap().is_substitute());
        assert_eq!(
            inbox.admit(ConnectionId(7), command(1, 1), &Rules),
            Ok(Admission::Late)
        );
        assert_eq!(inbox.finalized_through(), ServerTick(0));
        assert_eq!(
            inbox.commit(ServerTick(1)).unwrap().status,
            FinalizedStatus::Substituted
        );
        assert_eq!(inbox.pending_len(), 0);
    }
    #[test]
    fn future_and_sequence_windows_never_retarget_or_fast_forward() {
        let mut inbox = inbox();
        assert_eq!(
            inbox.admit(ConnectionId(7), command(1, 13), &Rules),
            Ok(Admission::Future)
        );
        assert_eq!(inbox.pending_len(), 0);
        assert_eq!(
            inbox.admit(ConnectionId(7), command(257, 1), &Rules),
            Err(CommandError::SequenceWindow)
        );
        let mut changed = command(1, 13);
        changed.target = TargetTick(12);
        assert_eq!(
            inbox.admit(ConnectionId(7), changed, &Rules),
            Err(CommandError::Equivocation)
        );
        advance(&mut inbox);
        assert_eq!(
            inbox.admit(ConnectionId(7), command(1, 13), &Rules),
            Ok(Admission::Accepted { arrival_slack: 12 })
        );
        for tick in 2..13 {
            assert!(advance(&mut inbox).is_substitute());
            assert_eq!(inbox.finalized_through(), ServerTick(tick));
        }
        assert_eq!(advance(&mut inbox).sequence(), Some(CommandSeq(1)));
    }
    #[test]
    fn bounded_audit_never_evicts_pending_inputs_and_retired_sequences_cannot_reexecute() {
        let mut inbox = CommandInbox::new(
            owner(),
            ServerTick(0),
            CommandLimits {
                audit_records: 12,
                ..Default::default()
            },
        )
        .unwrap();
        for seq in 1..=12 {
            inbox
                .admit(ConnectionId(7), command(seq, seq), &Rules)
                .unwrap();
        }
        assert_eq!(inbox.pending_len(), 12);
        assert_eq!(inbox.finalized_through(), ServerTick(0));
        assert_eq!(
            inbox.admit(ConnectionId(7), command(13, 13), &Rules),
            Err(CommandError::AuditFull)
        );
        assert_eq!(advance(&mut inbox).sequence(), Some(CommandSeq(1)));
        assert!(matches!(
            inbox.admit(ConnectionId(7), command(13, 13), &Rules),
            Ok(Admission::Accepted { .. })
        ));
        assert_eq!(inbox.audit_len(), 12);
        assert_eq!(
            inbox.admit(ConnectionId(7), command(1, 1), &Rules),
            Ok(Admission::RetiredSequence)
        );
        for _ in 0..20 {
            advance(&mut inbox);
        }
        assert_eq!(inbox.receipts().len(), 12);
        assert!(inbox.receipt(ServerTick(1)).is_none());
    }
    #[test]
    fn payload_and_per_tick_work_caps_are_explicit() {
        let mut tiny = CommandInbox::new(
            owner(),
            ServerTick(0),
            CommandLimits {
                max_command_bytes: 8,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            tiny.admit(ConnectionId(7), command(1, 1), &Rules),
            Err(CommandError::OversizedOrInvalidEncoding)
        );
        assert_eq!(tiny.pending_len(), 0);
        let mut capped = CommandInbox::new(
            owner(),
            ServerTick(0),
            CommandLimits {
                submissions_per_tick: 1,
                ..Default::default()
            },
        )
        .unwrap();
        capped
            .admit(ConnectionId(7), command(1, 1), &Rules)
            .unwrap();
        assert_eq!(
            capped.admit(ConnectionId(7), command(1, 1), &Rules),
            Err(CommandError::RateLimit)
        );
        advance(&mut capped);
        assert_eq!(
            capped.admit(ConnectionId(7), command(1, 1), &Rules),
            Ok(Admission::Duplicate)
        );
    }
    #[test]
    fn redundant_bundle_preserves_newest_complete_record_and_independent_decoding() {
        let newest = command(3, 3);
        let older = [command(2, 2), command(1, 1)];
        let all = pack_redundant(&newest, &older, 4, 2048).unwrap();
        assert_eq!(all.len(), 3);
        let bytes = bincode::serialize(&all).unwrap();
        let decoded: BoundedVec<Command<Input, bool>, 8> = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded, all);
        let single_bytes =
            bincode::serialize(&BoundedVec::<_, 8>::new(vec![newest.clone()]).unwrap())
                .unwrap()
                .len();
        let one = pack_redundant(&newest, &older, 4, single_bytes).unwrap();
        assert_eq!(one.as_slice(), &[newest.clone()]);
        assert_eq!(
            pack_redundant(&newest, &older, 4, single_bytes - 1),
            Err(CommandError::NewestDoesNotFit)
        );
        let mut inbox = inbox();
        for cmd in all.as_slice() {
            inbox.admit(ConnectionId(7), cmd.clone(), &Rules).unwrap();
        }
        for cmd in all.as_slice() {
            assert_eq!(
                inbox.admit(ConnectionId(7), cmd.clone(), &Rules),
                Ok(Admission::Duplicate)
            );
        }
        for seq in 1..=3 {
            assert_eq!(advance(&mut inbox).sequence(), Some(CommandSeq(seq)));
        }
    }
    #[test]
    fn committed_substitutes_advance_sequence_budget_but_packets_do_not() {
        let mut inbox = inbox();
        for tick in 1..=600 {
            assert!(advance(&mut inbox).is_substitute());
            assert_eq!(
                inbox.admit(ConnectionId(7), command(tick, tick), &Rules),
                Ok(Admission::Late)
            );
            assert_eq!(inbox.highest_received_sequence(), None);
        }
        assert_eq!(inbox.finalized_through(), ServerTick(600));
        assert_eq!(
            inbox.admit(ConnectionId(7), command(601, 602), &Rules),
            Ok(Admission::Accepted { arrival_slack: 2 })
        );
        assert!(advance(&mut inbox).is_substitute());
        assert_eq!(advance(&mut inbox).sequence(), Some(CommandSeq(601)));
        assert_eq!(
            inbox.admit(ConnectionId(7), command(602, 900), &Rules),
            Ok(Admission::Future)
        );
        let floor = inbox.sequence_window_floor;
        for sequence in 603..620 {
            assert_eq!(
                inbox.admit(ConnectionId(7), command(sequence, 900), &Rules),
                Ok(Admission::Future)
            );
        }
        assert_eq!(inbox.sequence_window_floor, floor);
        assert_eq!(
            inbox.admit(ConnectionId(7), command(floor.0 + 257, 603), &Rules),
            Err(CommandError::SequenceWindow)
        );
        assert_eq!(inbox.pending_len(), 0);
        // A retained command cannot be retargeted after the window advances;
        // an evicted older sequence remains retired rather than executing again.
        let mut changed = command(601, 603);
        assert_eq!(
            inbox.admit(ConnectionId(7), changed.clone(), &Rules),
            Err(CommandError::Equivocation)
        );
        changed.sequence = CommandSeq(1);
        assert_eq!(
            inbox.admit(ConnectionId(7), changed, &Rules),
            Ok(Admission::RetiredSequence)
        );
    }
    #[test]
    fn revocation_fences_already_queued_commands_and_held_substitutes() {
        struct Revoked;
        impl CommandRules<Input, bool> for Revoked {
            fn owns_at(&self, _: OwnerStream, _: TargetTick) -> bool {
                false
            }
            fn valid_input(&self, input: &Input) -> bool {
                Rules.valid_input(input)
            }
            fn valid_action(&self, action: &bool) -> bool {
                Rules.valid_action(action)
            }
            fn held_input(&self, input: &Input) -> Input {
                Rules.held_input(input)
            }
            fn neutral_input(&self) -> Input {
                Rules.neutral_input()
            }
        }
        let mut queued = inbox();
        queued
            .admit(ConnectionId(7), command(1, 1), &Rules)
            .unwrap();
        assert!(matches!(
            queued.prepare_next(&Revoked),
            Err(CommandError::OwnershipDenied)
        ));
        assert_eq!(queued.finalized_through(), ServerTick(0));
        assert_eq!(queued.pending_len(), 1);
        let mut held = inbox();
        held.admit(ConnectionId(7), command(1, 1), &Rules).unwrap();
        advance(&mut held);
        assert!(matches!(
            held.prepare_next(&Revoked),
            Err(CommandError::OwnershipDenied)
        ));
        assert_eq!(held.finalized_through(), ServerTick(1));
    }
}

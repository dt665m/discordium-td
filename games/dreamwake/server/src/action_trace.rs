//! Opt-in local authority traces. This resource has no network representation.
use bevy::prelude::Resource;
use dreamwake_protocol::{
    DreamAction,
    live::{ActionOutcome, CombatViewStamp, TickInput},
};
use engine_net::{commands::Command, types::*};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
const MAX_RECORDS: usize = 4096;
const MAX_BYTES: usize = 8 * 1024 * 1024;
// Ring capacity is charged separately, including vacant slots after eviction.
// Per-record allocator slack and game geometry allowances cover nested deques.
const RECORD_BYTES: usize = 256;
const BASE_BYTES: usize = std::mem::size_of::<State>() + 256;
#[derive(Debug, Clone, Serialize)]
pub struct AuthorityActionTrace {
    pub revision: u64,
    pub server_instance: u64,
    pub owner: u64,
    pub key: ActionKey,
    pub kind: &'static str,
    pub view: Option<CombatViewStamp>,
    pub held_beam: Option<dreamwake_protocol::live::BeamAimSample>,
    pub terminal_expected: bool,
    pub target_c: ServerTick,
    pub first_accepted_a: Option<ServerTick>,
    pub outcome: Option<ActionOutcome>,
    /// Restricted local diagnostics; never included in action outcome messages.
    pub combat: VecDeque<dreamwake_sim::combat_trace::CombatTraceRecord>,
}
impl AuthorityActionTrace {
    fn bytes(&self) -> usize {
        RECORD_BYTES
            + self
                .combat
                .iter()
                .map(|sample| sample.retained_bytes())
                .sum::<usize>()
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct AuthorityTraceBatch {
    pub records: Vec<AuthorityActionTrace>,
    pub through: u64,
    pub gap: bool,
    pub dropped: u64,
}
#[derive(Default)]
struct State {
    enabled: bool,
    records: VecDeque<AuthorityActionTrace>,
    dropped: u64,
    bytes: usize,
    revision: u64,
    evicted_through: u64,
}
#[derive(Resource, Clone, Default)]
pub struct ActionTrace(Arc<Mutex<State>>);
impl ActionTrace {
    pub fn set_enabled(&self, enabled: bool) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !enabled {
            state.evicted_through = state.revision;
            state.records = VecDeque::new();
            state.bytes = 0;
        } else if !state.enabled {
            state.bytes = retained(&state);
        }
        state.enabled = enabled;
    }
    pub fn enabled(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .enabled
    }
    pub fn dropped(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .dropped
    }
    pub fn retained_bytes(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .bytes
    }
    pub fn trace(&self, owner: u64, limit: usize) -> Vec<AuthorityActionTrace> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .records
            .iter()
            .rev()
            .filter(|record| record.owner == owner)
            .take(limit.min(128))
            .cloned()
            .collect()
    }
    /// Coalesces intermediate revisions of an action. A gap explicitly reports
    /// eviction rather than implying a complete forensic record. Export clones
    /// at most 128 records and no more than the 8MiB retained allocation budget;
    /// sorting uses at most 4096 borrowed references (32KiB on 64-bit hosts).
    pub fn snapshot_since(&self, after: u64, limit: usize) -> AuthorityTraceBatch {
        let state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let mut selected: Vec<_> = state
            .records
            .iter()
            .filter(|record| record.revision > after)
            .collect();
        selected.sort_unstable_by_key(|record| record.revision);
        let records: Vec<_> = selected.into_iter().take(limit.min(128)).cloned().collect();
        let through = records
            .last()
            .map_or(if limit == 0 { after } else { state.revision }, |record| {
                record.revision
            });
        AuthorityTraceBatch {
            records,
            through,
            gap: after < state.evicted_through,
            dropped: state.dropped,
        }
    }
    pub(crate) fn command(
        &self,
        instance: u64,
        owner: u64,
        command: &Command<TickInput, DreamAction>,
        arrival: Option<ServerTick>,
    ) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !state.enabled {
            return;
        }
        let mut declarations = Vec::with_capacity(8);
        for edge in command.actions.as_slice() {
            let (kind, view) = match edge.action {
                DreamAction::Dreamlance { view, .. } => ("ray", Some(view)),
                DreamAction::BeamBegin { view, .. } => ("beam_begin", Some(view)),
                DreamAction::BeamStop => ("beam_stop", None),
                DreamAction::Dash { .. } => ("dash", None),
                DreamAction::Cast { .. } => ("cast", None),
                _ => ("invalid", None),
            };
            declarations.push((command.action_key(edge.slot), kind, view, true));
        }
        if let Some(sample) = command.input.beam {
            if !command.actions.as_slice().iter().any(|edge| edge.slot == 0) {
                declarations.push((command.action_key(0), "beam_aim", Some(sample.view), false));
            }
        }
        for (key, kind, view, terminal_expected) in declarations {
            if state
                .records
                .iter()
                .any(|record| record.server_instance == instance && record.key == key)
            {
                continue;
            }
            while state.records.len() >= MAX_RECORDS {
                evict(&mut state);
            }
            let growth = if state.records.len() == state.records.capacity() {
                state.records.capacity().max(4) * std::mem::size_of::<AuthorityActionTrace>()
            } else {
                0
            };
            while state.bytes + RECORD_BYTES + growth > MAX_BYTES {
                evict(&mut state);
            }
            let Some(revision) = next_revision(&mut state) else {
                break;
            };
            state.records.push_back(AuthorityActionTrace {
                revision,
                server_instance: instance,
                owner,
                key,
                kind,
                view,
                held_beam: command.input.beam,
                terminal_expected,
                target_c: ServerTick(command.target.0),
                first_accepted_a: arrival,
                outcome: None,
                combat: VecDeque::new(),
            });
            state.bytes = retained(&state);
        }
    }
    pub(crate) fn complete(&self, mut lookup: impl FnMut(u64, ActionKey) -> Option<ActionOutcome>) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !state.enabled {
            return;
        }
        for index in 0..state.records.len() {
            let record = &state.records[index];
            if record.outcome.is_some() || !record.terminal_expected {
                continue;
            }
            let Some(outcome) = lookup(record.owner, record.key) else {
                continue;
            };
            let Some(revision) = next_revision(&mut state) else {
                break;
            };
            state.records[index].outcome = Some(outcome);
            state.records[index].revision = revision;
        }
    }
    pub(crate) fn combat(
        &self,
        instance: u64,
        samples: Vec<dreamwake_sim::combat_trace::CombatTraceRecord>,
    ) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !state.enabled {
            return;
        }
        for sample in samples
            .into_iter()
            .take(dreamwake_sim::combat_trace::MAX_COMBAT_TRACE_RECORDS)
        {
            let added = sample.retained_bytes();
            let fixed =
                BASE_BYTES + state.records.capacity() * std::mem::size_of::<AuthorityActionTrace>();
            if sample.shield_credits.len() > dreamwake_sim::combat_trace::MAX_COMBAT_TRACE_CREDITS
                || added > MAX_BYTES.saturating_sub(fixed)
            {
                state.dropped = state.dropped.saturating_add(1);
                continue;
            }
            while state.bytes + added > MAX_BYTES {
                evict(&mut state);
            }
            let key = ActionKey {
                connection: ConnectionEpoch(sample.key.connection_epoch),
                stream: CommandStream(sample.key.command_stream),
                command: CommandSeq(sample.key.command_sequence),
                slot: sample.key.action_slot,
            };
            let Some(index) = state
                .records
                .iter()
                .position(|record| record.server_instance == instance && record.key == key)
            else {
                continue;
            };
            let Some(revision) = next_revision(&mut state) else {
                break;
            };
            let record = &mut state.records[index];
            let removed = if record.combat.len() == 8 {
                record
                    .combat
                    .pop_front()
                    .map_or(0, |sample| sample.retained_bytes())
            } else {
                0
            };
            record.combat.push_back(sample);
            record.revision = revision;
            state.bytes = state.bytes - removed + added;
            if removed != 0 {
                state.dropped = state.dropped.saturating_add(1);
            }
        }
    }
}
fn next_revision(state: &mut State) -> Option<u64> {
    match state.revision.checked_add(1) {
        Some(revision) => {
            state.revision = revision;
            Some(revision)
        }
        None => {
            state.enabled = false;
            None
        }
    }
}
fn evict(state: &mut State) {
    if let Some(record) = state.records.pop_front() {
        state.bytes -= record.bytes();
        state.evicted_through = state.evicted_through.max(record.revision);
        state.dropped = state.dropped.saturating_add(1);
    }
}
fn retained(state: &State) -> usize {
    BASE_BYTES
        + state.records.capacity() * std::mem::size_of::<AuthorityActionTrace>()
        + state
            .records
            .iter()
            .map(AuthorityActionTrace::bytes)
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_net::{
        codec::BoundedVec,
        commands::{ActionEdge, OwnerStream},
    };
    fn command(sequence: u64) -> Command<TickInput, DreamAction> {
        Command {
            owner: OwnerStream {
                connection: ConnectionId(1),
                epoch: ConnectionEpoch(1),
                stream: CommandStream(1),
                owner: EntityId {
                    index: 7,
                    generation: 1,
                },
                ownership: OwnershipEpoch(1),
            },
            sequence: CommandSeq(sequence),
            target: TargetTick(sequence + 5),
            input: TickInput::default(),
            actions: BoundedVec::new(vec![ActionEdge {
                slot: 0,
                action: DreamAction::Dash {
                    direction: [1.0, 0.0],
                },
            }])
            .unwrap(),
        }
    }
    #[test]
    fn default_off_first_arrival_and_cursor_export_are_bounded() {
        let trace = ActionTrace::default();
        let command = command(1);
        trace.command(17, 7, &command, Some(ServerTick(2)));
        assert!(trace.snapshot_since(0, 128).records.is_empty());
        trace.set_enabled(true);
        trace.command(17, 7, &command, Some(ServerTick(2)));
        let first = trace.snapshot_since(0, 128);
        assert_eq!(first.records.len(), 1);
        trace.command(17, 7, &command, Some(ServerTick(4)));
        assert!(trace.snapshot_since(first.through, 128).records.is_empty());
        assert_eq!(trace.trace(7, 1)[0].first_accepted_a, Some(ServerTick(2)));
        assert!(trace.trace(8, 128).is_empty());
        trace.set_enabled(false);
        assert_eq!(trace.retained_bytes(), 0);
        assert!(trace.snapshot_since(0, 128).gap);
    }
    #[test]
    fn eviction_is_explicit_and_export_does_not_omit_unread_current_revisions() {
        let trace = ActionTrace::default();
        trace.set_enabled(true);
        for sequence in 1..=MAX_RECORDS as u64 + 20 {
            trace.command(17, 7, &command(sequence), Some(ServerTick(sequence)));
        }
        assert!(trace.retained_bytes() <= MAX_BYTES);
        assert!(trace.dropped() > 0);
        let mut cursor = 0;
        let mut count = 0;
        loop {
            let batch = trace.snapshot_since(cursor, 999);
            assert!(batch.records.len() <= 128);
            if batch.records.is_empty() {
                break;
            }
            assert!(
                batch
                    .records
                    .windows(2)
                    .all(|pair| pair[0].revision < pair[1].revision)
            );
            assert!(batch.through > cursor);
            cursor = batch.through;
            count += batch.records.len();
        }
        assert_eq!(count, trace.0.lock().unwrap().records.len());
    }
    #[test]
    fn geometry_eviction_charges_vacant_ring_capacity_and_nested_allocations() {
        use dreamwake_sim::{combat::*, combat_trace::*};
        let trace = ActionTrace::default();
        trace.set_enabled(true);
        for sequence in 1..=100 {
            trace.command(17, 7, &command(sequence), Some(ServerTick(sequence)));
        }
        for sequence in 1..=100 {
            let key = RayActionKey {
                match_epoch: 1,
                connection_epoch: 1,
                command_stream: 1,
                ownership_epoch: 1,
                actor: 7,
                actor_generation: 1,
                command_sequence: sequence,
                action_slot: 0,
            };
            let verdict = CombatVerdict {
                key,
                execution_server_tick: sequence,
                execution_gameplay_tick: 1,
                query_server_tick: 1,
                query_gameplay_tick: 1,
                reason: CombatReason::Miss,
                target: None,
                hit_region: 0,
                damage: 0.0,
                transaction: None,
            };
            let sample = CombatTraceRecord {
                key,
                sample_key: key,
                command_fraction: None,
                query_fraction: 0,
                execution_server_tick: sequence,
                execution_gameplay_tick: 1,
                query_server_tick: 1,
                query_gameplay_tick: 1,
                muzzle: Some([0.0; 3]),
                aim: [1.0, 0.0],
                selected: None,
                shield_credits: vec![
                    CombatTraceShieldCredit {
                        id: 1,
                        activated_tick: 1,
                        expires_tick: 2,
                        granted: 1.0,
                        spent_before: 0.0,
                        spent_after: 0.0
                    };
                    MAX_COMBAT_TRACE_CREDITS
                ],
                verdict,
            };
            assert!(
                sample.retained_bytes()
                    >= 4 * std::mem::size_of::<CombatTraceRecord>()
                        + sample.shield_credits.capacity()
                            * std::mem::size_of::<CombatTraceShieldCredit>()
                        + 64
            );
            trace.combat(17, vec![sample; 8]);
            let state = trace.0.lock().unwrap();
            assert_eq!(state.bytes, retained(&state));
            assert!(state.bytes <= MAX_BYTES);
        }
        assert!(trace.dropped() > 0);
        let state = trace.0.lock().unwrap();
        assert!(state.records.capacity() > state.records.len());
    }
}

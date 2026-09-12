//! Bounded local action tracking survives transport reconnects for owner-bound lookup.
use bevy::prelude::Resource;
use dreamwake_protocol::{DreamAction, live::*};
#[cfg(test)]
use dreamwake_sim::DreamInput;
use engine_net::{codec::BoundedVec, commands::Command, types::*};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

const RETENTION: Duration = Duration::from_secs(30);
const CAPACITY: usize = 16_384;
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct TraceObservation {
    pub tick: ServerTick,
    pub at: Duration,
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ClientActionTrace {
    pub server_instance: u64,
    pub owner: u64,
    pub key: ActionKey,
    pub kind: &'static str,
    pub view: Option<CombatViewStamp>,
    pub target_c: ServerTick,
    pub created_at: Duration,
    /// First enqueue on the authenticated command lane, not physical UDP send.
    pub first_enqueued_at: Option<Duration>,
    pub outcome: Option<ActionOutcome>,
    pub outcome_received_at: Option<Duration>,
    pub checkpoint: Option<TraceObservation>,
    /// Timeline observation only; it does not prove an action visual appeared.
    pub presented_timeline: Option<TraceObservation>,
    pub graphics_seen: Option<TraceObservation>,
    pub graphics_id: Option<engine_core::GraphicsId>,
    pub lookup_unavailable: bool,
}
#[derive(Clone)]
struct Tracked {
    owner: u64,
    target: ServerTick,
    created: Duration,
    looked_up: Duration,
    outcome: Option<ActionOutcome>,
    unavailable: bool,
    kind: &'static str,
    view: Option<CombatViewStamp>,
    first_enqueued: Option<Duration>,
    received: Option<Duration>,
    checkpoint: Option<TraceObservation>,
    presented: Option<TraceObservation>,
    graphics_seen: Option<TraceObservation>,
    graphics_id: Option<engine_core::GraphicsId>,
}
#[derive(Resource, Default)]
pub(crate) struct ClientOutcomes {
    server_instance: Option<u64>,
    entries: BTreeMap<ActionKey, Tracked>,
    observations: BTreeSet<ActionKey>,
    last_lookup: Duration,
}
impl ClientOutcomes {
    pub fn bind_instance(&mut self, instance: u64) {
        if self.server_instance != Some(instance) {
            self.entries.clear();
            self.observations.clear();
            self.last_lookup = Duration::ZERO;
            self.server_instance = Some(instance);
        }
    }
    pub fn track(
        &mut self,
        owner: u64,
        command: &Command<TickInput, DreamAction>,
        now: Duration,
    ) -> Result<(), String> {
        self.entries
            .retain(|_, value| now.saturating_sub(value.created) < RETENTION);
        self.observations
            .retain(|key| self.entries.contains_key(key));
        if self.entries.len().saturating_add(command.actions.len()) > CAPACITY {
            return Err("Local action outcome capacity".into());
        }
        for edge in command.actions.as_slice() {
            let (kind, view) = match edge.action {
                DreamAction::Dreamlance { view, .. } => ("ray", Some(view)),
                DreamAction::BeamBegin { view, .. } => ("beam_begin", Some(view)),
                DreamAction::BeamStop => ("beam_stop", None),
                DreamAction::ChargeBegin => ("charge_begin", None),
                DreamAction::ChargeRelease { .. } => ("charge_release", None),
                DreamAction::ChargeCancel { .. } => ("charge_cancel", None),
                DreamAction::Dash { .. } => ("dash", None),
                DreamAction::Cast { .. } => ("cast", None),
                _ => ("invalid", None),
            };
            self.observations.insert(command.action_key(edge.slot));
            self.entries
                .entry(command.action_key(edge.slot))
                .or_insert(Tracked {
                    owner,
                    target: ServerTick(command.target.0),
                    created: now,
                    looked_up: now,
                    outcome: None,
                    unavailable: false,
                    kind,
                    view,
                    first_enqueued: None,
                    received: None,
                    checkpoint: None,
                    presented: None,
                    graphics_seen: None,
                    graphics_id: None,
                });
        }
        Ok(())
    }
    pub fn receive_batch(
        &mut self,
        owner: u64,
        records: &[ActionOutcome],
        now: Duration,
    ) -> Result<(), String> {
        for (index, outcome) in records.iter().enumerate() {
            if let Some(previous) = self.entries.get(&outcome.key) {
                if previous.owner != owner
                    || previous
                        .outcome
                        .as_ref()
                        .is_some_and(|value| value != outcome)
                {
                    return Err("Conflicting terminal action outcome".into());
                }
            }
            if records[..index]
                .iter()
                .any(|previous| previous.key == outcome.key && previous != outcome)
            {
                return Err("Conflicting terminal action batch".into());
            }
        }
        for outcome in records {
            self.receive(owner, outcome.clone(), now)?;
        }
        Ok(())
    }
    pub fn receive(
        &mut self,
        owner: u64,
        outcome: ActionOutcome,
        now: Duration,
    ) -> Result<(), String> {
        let Some(tracked) = self.entries.get_mut(&outcome.key) else {
            return Ok(());
        };
        if tracked.owner != owner {
            return Err("Action outcome owner mismatch".into());
        }
        if let Some(previous) = &tracked.outcome {
            if previous != &outcome {
                return Err("Conflicting terminal action outcome".into());
            }
        } else {
            tracked.outcome = Some(outcome);
            tracked.received = Some(now);
        }
        Ok(())
    }
    pub fn unavailable(&mut self, owner: u64, keys: &[ActionKey]) {
        for key in keys {
            if let Some(value) = self
                .entries
                .get_mut(key)
                .filter(|value| value.owner == owner)
            {
                // This means unknown/expired lookup, never a gameplay rejection.
                value.unavailable = true;
            }
        }
    }
    pub fn lookup(
        &mut self,
        owner: u64,
        connection: ConnectionEpoch,
        finalized: ServerTick,
        now: Duration,
    ) -> Option<ClientControl> {
        self.entries
            .retain(|_, value| now.saturating_sub(value.created) < RETENTION);
        self.observations
            .retain(|key| self.entries.contains_key(key));
        if now.saturating_sub(self.last_lookup) < Duration::from_millis(250) {
            return None;
        }
        let keys: Vec<_> = self
            .entries
            .iter_mut()
            .filter(|(key, value)| {
                value.owner == owner
                    && value.outcome.is_none()
                    && !value.unavailable
                    && (key.connection != connection || value.target <= finalized)
                    && now.saturating_sub(value.looked_up) >= Duration::from_secs(1)
            })
            .take(8)
            .map(|(key, value)| {
                value.looked_up = now;
                *key
            })
            .collect();
        if keys.is_empty() {
            return None;
        }
        self.last_lookup = now;
        Some(ClientControl::OutcomeLookup {
            keys: BoundedVec::new(keys).unwrap(),
        })
    }
    #[cfg(test)]
    fn latest(&self, owner: u64) -> Option<&ActionOutcome> {
        self.entries
            .values()
            .filter(|value| value.owner == owner)
            .filter_map(|value| value.outcome.as_ref())
            .max_by_key(|outcome| outcome.sequence)
    }
    pub fn enqueued(&mut self, keys: impl Iterator<Item = ActionKey>, now: Duration) {
        for key in keys {
            if let Some(value) = self.entries.get_mut(&key) {
                value.first_enqueued.get_or_insert(now);
            }
        }
    }
    pub fn observe(
        &mut self,
        owner: u64,
        connection: ConnectionEpoch,
        tick: ServerTick,
        now: Duration,
        presented: bool,
    ) {
        self.observations.retain(|key| {
            let Some(value) = self.entries.get_mut(key) else {
                return false;
            };
            if value.owner == owner && key.connection == connection && value.target <= tick {
                let observation = if presented {
                    &mut value.presented
                } else {
                    &mut value.checkpoint
                };
                observation.get_or_insert(TraceObservation { tick, at: now });
            }
            value.checkpoint.is_none() || value.presented.is_none()
        });
    }
    pub fn graphics_seen(
        &mut self,
        owner: u64,
        key: ActionKey,
        graphics_id: engine_core::GraphicsId,
        tick: ServerTick,
        now: Duration,
    ) {
        if let Some(value) = self
            .entries
            .get_mut(&key)
            .filter(|value| value.owner == owner)
        {
            value
                .graphics_seen
                .get_or_insert(TraceObservation { tick, at: now });
            value.graphics_id.get_or_insert(graphics_id);
        }
    }
    fn trace_entry(&self, key: ActionKey, value: &Tracked) -> ClientActionTrace {
        ClientActionTrace {
            server_instance: self.server_instance.unwrap_or(0),
            owner: value.owner,
            key,
            kind: value.kind,
            view: value.view,
            target_c: value.target,
            created_at: value.created,
            first_enqueued_at: value.first_enqueued,
            outcome: value.outcome.clone(),
            outcome_received_at: value.received,
            checkpoint: value.checkpoint,
            presented_timeline: value.presented,
            graphics_seen: value.graphics_seen,
            graphics_id: value.graphics_id,
            lookup_unavailable: value.unavailable,
        }
    }
    pub fn trace(&self, owner: u64, limit: usize) -> Vec<ClientActionTrace> {
        self.entries
            .iter()
            .rev()
            .filter(|(_, value)| value.owner == owner)
            .take(limit.min(128))
            .map(|(key, value)| self.trace_entry(*key, value))
            .collect()
    }
    pub fn latest_trace(&self, owner: u64) -> Option<ClientActionTrace> {
        self.entries
            .iter()
            .rev()
            .find(|(_, value)| value.owner == owner)
            .map(|(key, value)| self.trace_entry(*key, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_net::commands::{ActionEdge, OwnerStream};
    fn command() -> Command<TickInput, DreamAction> {
        Command {
            owner: OwnerStream {
                connection: ConnectionId(1),
                epoch: ConnectionEpoch(1),
                stream: CommandStream(1),
                owner: EntityId {
                    index: 1,
                    generation: 1,
                },
                ownership: OwnershipEpoch(1),
            },
            sequence: CommandSeq(1),
            target: TargetTick(9),
            input: DreamInput::default().into(),
            actions: BoundedVec::new(vec![ActionEdge {
                slot: 0,
                action: DreamAction::Dash {
                    direction: [1.0, 0.0],
                },
            }])
            .unwrap(),
        }
    }
    fn terminal(key: ActionKey) -> ActionOutcome {
        ActionOutcome {
            sequence: 1,
            key,
            execution_tick: ServerTick(9),
            data: ActionOutcomeData {
                status: TerminalStatus::Rejected,
                reason: OutcomeReason::Timing,
                query_tick: None,
                target: None,
                hit_region: 0,
                damage: 0.0,
                transaction: None,
                binding: None,
                spawn_bindings: Default::default(),
            },
        }
    }
    #[test]
    fn finalized_is_not_terminal_and_reconnect_lookup_is_owner_bound() {
        let mut state = ClientOutcomes::default();
        let command = command();
        state.track(7, &command, Duration::ZERO).unwrap();
        assert!(
            state
                .lookup(7, ConnectionEpoch(1), ServerTick(8), Duration::from_secs(2))
                .is_none()
        );
        assert!(
            state
                .lookup(8, ConnectionEpoch(2), ServerTick(0), Duration::from_secs(2))
                .is_none()
        );
        assert!(
            state
                .lookup(7, ConnectionEpoch(2), ServerTick(0), Duration::from_secs(2))
                .is_some()
        );
        assert!(state.latest(7).is_none());
        let outcome = terminal(command.action_key(0));
        state
            .receive(7, outcome.clone(), Duration::from_secs(2))
            .unwrap();
        state
            .receive(7, outcome.clone(), Duration::from_secs(2))
            .unwrap();
        assert_eq!(state.latest(7), Some(&outcome));
        let mut conflict = outcome;
        conflict.sequence = 2;
        assert!(state.receive(7, conflict, Duration::from_secs(2)).is_err());
        assert!(
            state
                .lookup(
                    7,
                    ConnectionEpoch(2),
                    ServerTick(10),
                    Duration::from_secs(3)
                )
                .is_none()
        );
    }
    #[test]
    fn unavailable_and_retention_expiry_never_create_rejection() {
        let mut state = ClientOutcomes::default();
        let command = command();
        state.track(7, &command, Duration::ZERO).unwrap();
        state.unavailable(7, &[command.action_key(0)]);
        assert!(state.latest(7).is_none());
        assert!(
            state
                .lookup(
                    7,
                    ConnectionEpoch(2),
                    ServerTick(10),
                    Duration::from_secs(2)
                )
                .is_none()
        );
        state.lookup(7, ConnectionEpoch(2), ServerTick(10), RETENTION);
        assert!(state.entries.is_empty());
    }
    #[test]
    fn authenticated_instance_restart_discards_results_before_key_reuse() {
        let mut state = ClientOutcomes::default();
        state.bind_instance(1);
        let command = command();
        state.track(7, &command, Duration::ZERO).unwrap();
        state
            .receive(7, terminal(command.action_key(0)), Duration::from_secs(2))
            .unwrap();
        state.bind_instance(1);
        assert!(
            state.latest(7).is_some(),
            "ordinary reconnect preserves results"
        );
        state.bind_instance(2);
        assert!(state.latest(7).is_none());
        assert!(
            state
                .lookup(
                    7,
                    ConnectionEpoch(1),
                    ServerTick(10),
                    Duration::from_secs(2)
                )
                .is_none()
        );
        state.track(7, &command, Duration::from_secs(2)).unwrap();
        let mut new = terminal(command.action_key(0));
        new.sequence = 99;
        state
            .receive(7, new.clone(), Duration::from_secs(2))
            .unwrap();
        assert_eq!(state.latest(7), Some(&new));
    }
    #[test]
    fn conflicting_batch_is_atomic_and_timeline_is_not_graphics_proof() {
        let mut state = ClientOutcomes::default();
        state.bind_instance(1);
        let first = command();
        let mut second = command();
        second.sequence = CommandSeq(2);
        state.track(7, &first, Duration::ZERO).unwrap();
        state.track(7, &second, Duration::ZERO).unwrap();
        let saved = terminal(second.action_key(0));
        state
            .receive(7, saved.clone(), Duration::from_secs(1))
            .unwrap();
        let mut conflict = saved;
        conflict.sequence += 1;
        assert!(
            state
                .receive_batch(
                    7,
                    &[terminal(first.action_key(0)), conflict],
                    Duration::from_secs(2)
                )
                .is_err()
        );
        assert!(state.entries[&first.action_key(0)].outcome.is_none());
        state.observe(
            7,
            ConnectionEpoch(2),
            ServerTick(10),
            Duration::from_secs(2),
            false,
        );
        assert!(state.latest_trace(7).unwrap().checkpoint.is_none());
        state.observe(
            7,
            ConnectionEpoch(1),
            ServerTick(10),
            Duration::from_secs(2),
            true,
        );
        let trace = state.latest_trace(7).unwrap();
        assert!(trace.presented_timeline.is_some());
        assert!(trace.graphics_seen.is_none());
        assert!(trace.graphics_id.is_none());
    }
}

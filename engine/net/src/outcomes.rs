//! Bounded terminal action results, independent of command receipt/finalization.
//! Callers authenticate owner lookup and forbid old-stream gameplay resubmission.
use crate::{replication::Payload, types::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone)]
pub struct OutcomeSequencer(Arc<AtomicU64>);
impl Default for OutcomeSequencer {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(1)))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OutcomeLimits {
    pub records: usize,
    pub records_per_owner: usize,
    pub retained_bytes: usize,
    pub maximum_payload_bytes: usize,
    pub retention_ticks: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeError {
    Invalid,
    Capacity,
    Unknown,
    Conflict,
    SequenceExhausted,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalOutcome<P> {
    pub sequence: u64,
    pub key: ActionKey,
    pub at: ServerTick,
    pub payload: P,
}
struct Record<P> {
    owner: u64,
    terminal: Option<TerminalOutcome<P>>,
    reported: bool,
    delivery_closed: bool,
}
pub struct OutcomeLedger<P> {
    limits: OutcomeLimits,
    records: BTreeMap<ActionKey, Record<P>>,
    owners: BTreeMap<u64, usize>,
    delivery: BTreeMap<(u64, ConnectionEpoch), BTreeSet<(u64, ActionKey)>>,
    sequence: OutcomeSequencer,
    charge: usize,
}
impl<P: Payload> OutcomeLedger<P> {
    pub fn new(limits: OutcomeLimits) -> Result<Self, OutcomeError> {
        Self::with_sequencer(limits, OutcomeSequencer::default())
    }
    pub fn with_sequencer(
        limits: OutcomeLimits,
        sequence: OutcomeSequencer,
    ) -> Result<Self, OutcomeError> {
        let charge = std::mem::size_of::<Record<P>>()
            .checked_add(256)
            .and_then(|bytes| bytes.checked_add(limits.maximum_payload_bytes))
            .ok_or(OutcomeError::Invalid)?;
        if limits.records == 0
            || limits.records_per_owner == 0
            || limits.retention_ticks == 0
            || limits.maximum_payload_bytes == 0
            || limits.retained_bytes < charge
        {
            return Err(OutcomeError::Invalid);
        }
        Ok(Self {
            limits,
            records: BTreeMap::new(),
            owners: BTreeMap::new(),
            delivery: BTreeMap::new(),
            sequence,
            charge,
        })
    }
    pub fn retained_bytes(&self) -> usize {
        self.records.len() * self.charge
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn available_for(&self, owner: u64) -> usize {
        self.limits
            .records
            .saturating_sub(self.records.len())
            .min(
                self.limits
                    .records_per_owner
                    .saturating_sub(self.owners.get(&owner).copied().unwrap_or(0)),
            )
            .min(
                self.limits
                    .retained_bytes
                    .saturating_sub(self.retained_bytes())
                    / self.charge,
            )
    }
    pub fn pending(&self, owner: u64, connection: ConnectionEpoch) -> Vec<ActionKey> {
        self.records
            .iter()
            .filter_map(|(key, record)| {
                (record.owner == owner && key.connection == connection && record.terminal.is_none())
                    .then_some(*key)
            })
            .collect()
    }
    /// A disconnected transport has no delivery path. Preserve terminal results
    /// for the full declared reconnect lookup window, then permit expiry.
    pub fn close_delivery(&mut self, owner: u64, connection: ConnectionEpoch) {
        self.delivery.remove(&(owner, connection));
        for (key, record) in &mut self.records {
            if record.owner == owner && key.connection == connection {
                record.delivery_closed = true;
            }
        }
    }
    /// Reserve every result slot before admitting the immutable command. The
    /// returned keys are new reservations, released if command admission fails.
    pub fn reserve(
        &mut self,
        owner: u64,
        keys: &[ActionKey],
    ) -> Result<Vec<ActionKey>, OutcomeError> {
        if owner == 0 || keys.is_empty() || keys.len() > 8 {
            return Err(OutcomeError::Invalid);
        }
        let unique: std::collections::BTreeSet<_> = keys.iter().copied().collect();
        if unique.len() != keys.len()
            || keys.iter().any(|key| {
                key.connection.0 == 0 || key.stream.0 == 0 || key.command.0 == 0 || key.slot >= 8
            })
        {
            return Err(OutcomeError::Invalid);
        }
        if keys.iter().any(|key| {
            self.records
                .get(key)
                .is_some_and(|record| record.owner != owner)
        }) {
            return Err(OutcomeError::Conflict);
        }
        let new: Vec<_> = keys
            .iter()
            .filter(|key| !self.records.contains_key(key))
            .copied()
            .collect();
        let total = self
            .records
            .len()
            .checked_add(new.len())
            .ok_or(OutcomeError::Capacity)?;
        let owned = self.owners.get(&owner).copied().unwrap_or(0) + new.len();
        if total > self.limits.records
            || owned > self.limits.records_per_owner
            || total
                .checked_mul(self.charge)
                .is_none_or(|bytes| bytes > self.limits.retained_bytes)
        {
            return Err(OutcomeError::Capacity);
        }
        for &key in &new {
            self.records.insert(
                key,
                Record {
                    owner,
                    terminal: None,
                    reported: false,
                    delivery_closed: false,
                },
            );
        }
        if owned != 0 {
            self.owners.insert(owner, owned);
        }
        Ok(new)
    }
    /// Only a failed admission may release still-pending reservations.
    pub fn cancel_reservations(&mut self, keys: &[ActionKey]) {
        for key in keys {
            if self
                .records
                .get(key)
                .is_some_and(|record| record.terminal.is_none())
            {
                self.remove(*key);
            }
        }
    }
    pub fn finish(
        &mut self,
        owner: u64,
        key: ActionKey,
        at: ServerTick,
        payload: P,
    ) -> Result<u64, OutcomeError> {
        if payload.retained_bytes() > self.limits.maximum_payload_bytes {
            return Err(OutcomeError::Capacity);
        }
        let record = self.records.get_mut(&key).ok_or(OutcomeError::Unknown)?;
        if record.owner != owner {
            return Err(OutcomeError::Unknown);
        }
        if let Some(old) = &record.terminal {
            return if old.at == at && old.payload == payload {
                Ok(old.sequence)
            } else {
                Err(OutcomeError::Conflict)
            };
        }
        let sequence = self
            .sequence
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| OutcomeError::SequenceExhausted)?;
        record.terminal = Some(TerminalOutcome {
            sequence,
            key,
            at,
            payload,
        });
        if !record.delivery_closed {
            self.delivery
                .entry((owner, key.connection))
                .or_default()
                .insert((sequence, key));
        }
        Ok(sequence)
    }
    pub fn lookup(&self, owner: u64, key: ActionKey) -> Option<&TerminalOutcome<P>> {
        self.records
            .get(&key)
            .filter(|record| record.owner == owner)?
            .terminal
            .as_ref()
    }
    pub fn unreported(
        &self,
        owner: u64,
        connection: ConnectionEpoch,
    ) -> impl Iterator<Item = &TerminalOutcome<P>> {
        self.delivery
            .get(&(owner, connection))
            .into_iter()
            .flat_map(|keys| keys.iter())
            .filter_map(|(_, key)| self.records.get(key)?.terminal.as_ref())
    }
    /// Mark only after the complete bounded reliable message has been enqueued.
    pub fn reported(&mut self, owner: u64, keys: &[ActionKey]) -> Result<(), OutcomeError> {
        if keys.iter().any(|key| {
            self.records
                .get(key)
                .is_none_or(|record| record.owner != owner || record.terminal.is_none())
        }) {
            return Err(OutcomeError::Unknown);
        }
        for key in keys {
            let record = self.records.get_mut(key).unwrap();
            record.reported = true;
            let sequence = record.terminal.as_ref().unwrap().sequence;
            if let Some(delivery) = self.delivery.get_mut(&(owner, key.connection)) {
                delivery.remove(&(sequence, *key));
                if delivery.is_empty() {
                    self.delivery.remove(&(owner, key.connection));
                }
            }
        }
        Ok(())
    }
    /// Never evict a pending or unreported result to make room. An unavailable
    /// retired lookup is not a new rejection and never authorizes re-execution.
    pub fn expire(&mut self, now: ServerTick) {
        let expired: Vec<_> = self
            .records
            .iter()
            .filter_map(|(key, record)| {
                ((record.reported || record.delivery_closed)
                    && record.terminal.as_ref().is_some_and(|terminal| {
                        now.0.saturating_sub(terminal.at.0) >= self.limits.retention_ticks
                    }))
                .then_some(*key)
            })
            .collect();
        for key in expired {
            self.remove(key);
        }
    }
    fn remove(&mut self, key: ActionKey) {
        if let Some(record) = self.records.remove(&key) {
            if let Some(terminal) = &record.terminal {
                if let Some(delivery) = self.delivery.get_mut(&(record.owner, key.connection)) {
                    delivery.remove(&(terminal.sequence, key));
                    if delivery.is_empty() {
                        self.delivery.remove(&(record.owner, key.connection));
                    }
                }
            }
            let count = self.owners.get_mut(&record.owner).unwrap();
            *count -= 1;
            if *count == 0 {
                self.owners.remove(&record.owner);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn key(sequence: u64) -> ActionKey {
        ActionKey {
            connection: ConnectionEpoch(1),
            stream: CommandStream(1),
            command: CommandSeq(sequence),
            slot: 0,
        }
    }
    fn ledger() -> OutcomeLedger<Vec<u8>> {
        OutcomeLedger::new(OutcomeLimits {
            records: 3,
            records_per_owner: 2,
            retained_bytes: 8192,
            maximum_payload_bytes: 64,
            retention_ticks: 10,
        })
        .unwrap()
    }
    #[test]
    fn admission_reservation_is_atomic_and_bounded_by_owner_and_global_memory() {
        let mut store = ledger();
        assert_eq!(store.reserve(7, &[key(1), key(2)]).unwrap().len(), 2);
        assert_eq!(store.reserve(7, &[key(3)]), Err(OutcomeError::Capacity));
        assert_eq!(
            store.reserve(8, &[key(3), key(4)]),
            Err(OutcomeError::Capacity)
        );
        assert_eq!(store.len(), 2);
        store.cancel_reservations(&[key(1)]);
        store.reserve(8, &[key(3), key(4)]).unwrap();
        assert!(store.retained_bytes() <= 8192);
    }
    #[test]
    fn terminal_result_never_changes_and_lookup_is_owner_bound_across_reconnect() {
        let mut store = ledger();
        store.reserve(7, &[key(1)]).unwrap();
        store.finish(7, key(1), ServerTick(1), vec![1]).unwrap();
        assert_eq!(
            store.finish(7, key(1), ServerTick(1), vec![2]),
            Err(OutcomeError::Conflict)
        );
        assert!(store.lookup(8, key(1)).is_none());
        store.expire(ServerTick(100));
        assert!(
            store.lookup(7, key(1)).is_some(),
            "unreported results are not evicted"
        );
        store.reported(7, &[key(1)]).unwrap();
        store.expire(ServerTick(100));
        assert!(store.lookup(7, key(1)).is_none());
    }
    #[test]
    fn delivery_is_indexed_by_owner_and_monotonic_outcome_sequence() {
        let mut store = ledger();
        store.reserve(7, &[key(1), key(2)]).unwrap();
        store.finish(7, key(2), ServerTick(1), vec![2]).unwrap();
        store.finish(7, key(1), ServerTick(2), vec![1]).unwrap();
        assert_eq!(
            store
                .unreported(7, ConnectionEpoch(1))
                .map(|value| value.key)
                .collect::<Vec<_>>(),
            vec![key(2), key(1)]
        );
        assert_eq!(store.unreported(8, ConnectionEpoch(1)).count(), 0);
        store.reported(7, &[key(2)]).unwrap();
        assert_eq!(store.unreported(7, ConnectionEpoch(1)).count(), 1);
        store.close_delivery(7, ConnectionEpoch(1));
        assert_eq!(store.unreported(7, ConnectionEpoch(1)).count(), 0);
        assert!(store.lookup(7, key(1)).is_some());
        store.expire(ServerTick(100));
        assert!(store.delivery.is_empty());
        assert_eq!(store.len(), 0);
    }
}

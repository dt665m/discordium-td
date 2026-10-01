//! A per-connection stream of entity states over an unreliable channel.
//!
//! Each packet carries a sequence number. The receiver acknowledges packets
//! with the newest sequence it has seen and a bitmask of the 64 before it,
//! sent back with its own traffic, so one lost acknowledgement costs nothing.
//! An entity's state is encoded against the newest state of that entity the
//! receiver has acknowledged, so a packet carries only fields that changed
//! since then. When a packet is lost nothing is resent: the next packet is
//! encoded against the older acknowledged state and therefore carries every
//! field that changed since, with its current value. An entity the receiver
//! has never acknowledged is sent whole. Removals ride every packet until one
//! carrying them is acknowledged.
//!
//! Both sides agree on which states can serve as a base without negotiating:
//! the sender only uses an acknowledged state from the last [`STREAM_WINDOW`]
//! packets, and the receiver keeps every state it accepted from that window.
//!
//! This module tracks identities, sequences and bases. The caller owns the
//! byte encoding, so a game supplies its own schema and field deltas.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Packets an acknowledgement covers and a base may be drawn from.
pub const STREAM_WINDOW: u32 = 64;

/// The receiver's view of the stream: the newest packet it accepted and, in
/// bit `i`, whether it accepted packet `latest - 1 - i`. Zero means nothing
/// received yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StreamAck {
    pub latest: u32,
    pub received: u64,
}
impl StreamAck {
    /// Whether this acknowledgement reports packet `seq` as received.
    pub fn covers(&self, seq: u32) -> bool {
        if seq == 0 || seq > self.latest {
            return false;
        }
        let behind = self.latest - seq;
        behind == 0 || (behind <= 64 && self.received & (1 << (behind - 1)) != 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamError {
    /// A packet must be finished before another begins, and only then.
    PacketState,
    /// The acknowledgement names a packet that was never sent.
    FutureAck,
    /// The sequence space is exhausted; the connection must start over.
    Exhausted,
    /// A delta names a base the receiver does not hold. The packet must be
    /// dropped unacknowledged; the sender recovers by itself.
    MissingBase,
    /// The packet repeats an entity or is otherwise inconsistent.
    Malformed,
    Capacity,
}
impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
impl std::error::Error for StreamError {}

struct SentPacket<K> {
    seq: u32,
    removals: Vec<K>,
}
struct SentEntity<S> {
    /// Newest acknowledged state and the packet that carried it.
    base: Option<(u32, S)>,
    /// States sent since the base, awaiting acknowledgement.
    in_flight: VecDeque<(u32, S)>,
}

/// Sender side. One per connection.
pub struct ServerStream<K, S> {
    next: u32,
    pending: Option<u32>,
    pending_included: Vec<K>,
    pending_removals: Vec<K>,
    sent: VecDeque<SentPacket<K>>,
    entities: BTreeMap<K, SentEntity<S>>,
    /// Entities the receiver may hold that have since left this connection.
    removals: BTreeSet<K>,
    acked: StreamAck,
    max_entities: usize,
}
impl<K: Copy + Ord, S: Clone + PartialEq> ServerStream<K, S> {
    pub fn new(max_entities: usize) -> Self {
        Self {
            next: 1,
            pending: None,
            pending_included: Vec::new(),
            pending_removals: Vec::new(),
            sent: VecDeque::new(),
            entities: BTreeMap::new(),
            removals: BTreeSet::new(),
            acked: StreamAck::default(),
            max_entities,
        }
    }
    /// The newest acknowledgement received.
    pub fn acknowledged(&self) -> StreamAck {
        self.acked
    }
    /// Packets sent and not yet covered by an acknowledgement window.
    pub fn unacknowledged(&self) -> usize {
        self.sent
            .iter()
            .filter(|packet| !self.acked.covers(packet.seq))
            .count()
    }
    /// Start a packet and return its sequence number.
    pub fn begin(&mut self) -> Result<u32, StreamError> {
        if self.pending.is_some() {
            return Err(StreamError::PacketState);
        }
        let seq = self.next;
        self.next = seq.checked_add(1).ok_or(StreamError::Exhausted)?;
        self.pending = Some(seq);
        // Bases and in-flight states older than the window can never be used.
        let oldest = seq.saturating_sub(STREAM_WINDOW - 1);
        while self.sent.front().is_some_and(|packet| packet.seq < oldest) {
            self.sent.pop_front();
        }
        for entity in self.entities.values_mut() {
            while entity.in_flight.front().is_some_and(|(at, _)| *at < oldest) {
                entity.in_flight.pop_front();
            }
            if entity.base.as_ref().is_some_and(|(at, _)| *at < oldest) {
                entity.base = None;
            }
        }
        Ok(seq)
    }
    /// The acknowledged state to encode `key` against in the current packet,
    /// with the sequence the packet must name. `None` means send it whole.
    pub fn base(&self, key: K) -> Option<(u32, &S)> {
        self.entities
            .get(&key)?
            .base
            .as_ref()
            .map(|(seq, state)| (*seq, state))
    }
    /// The newest state sent for `key` that is not yet acknowledged, with the
    /// packet carrying it.
    pub fn in_flight(&self, key: K) -> Option<(u32, &S)> {
        self.entities
            .get(&key)?
            .in_flight
            .back()
            .map(|(seq, state)| (*seq, state))
    }
    /// Whether the receiver reported packets after `seq` without `seq`, so
    /// `seq` is lost. A packet not yet covered by any acknowledgement is not.
    pub fn lost(&self, seq: u32) -> bool {
        self.acked.latest > seq && !self.acked.covers(seq)
    }
    /// Record that the current packet carries `state` for `key`.
    pub fn include(&mut self, key: K, state: S) -> Result<(), StreamError> {
        let seq = self.pending.ok_or(StreamError::PacketState)?;
        if self.pending_included.contains(&key) || self.pending_removals.contains(&key) {
            return Err(StreamError::Malformed);
        }
        if !self.entities.contains_key(&key) && self.entities.len() >= self.max_entities {
            return Err(StreamError::Capacity);
        }
        self.entities
            .entry(key)
            .or_insert(SentEntity {
                base: None,
                in_flight: VecDeque::new(),
            })
            .in_flight
            .push_back((seq, state));
        self.pending_included.push(key);
        Ok(())
    }
    /// Removals the receiver has not yet acknowledged, oldest key first,
    /// except keys the current packet carries again.
    pub fn pending_removals(&self) -> impl Iterator<Item = K> + '_ {
        self.removals
            .iter()
            .copied()
            .filter(|key| !self.pending_included.contains(key))
    }
    /// Record that the current packet carries the removal of `key`.
    pub fn include_removal(&mut self, key: K) -> Result<(), StreamError> {
        if self.pending.is_none()
            || !self.removals.contains(&key)
            || self.pending_included.contains(&key)
            || self.pending_removals.contains(&key)
        {
            return Err(StreamError::PacketState);
        }
        self.pending_removals.push(key);
        Ok(())
    }
    /// Close the current packet. Nothing it records is lost if the bytes are.
    pub fn finish(&mut self) -> Result<(), StreamError> {
        let seq = self.pending.take().ok_or(StreamError::PacketState)?;
        // A key that is back before its removal was acknowledged starts over;
        // the receiver orders that removal and this state by packet sequence.
        for key in self.pending_included.drain(..) {
            self.removals.remove(&key);
        }
        self.sent.push_back(SentPacket {
            seq,
            removals: std::mem::take(&mut self.pending_removals),
        });
        Ok(())
    }
    /// Abandon the current packet when its bytes could not be sent.
    pub fn cancel(&mut self) {
        let Some(seq) = self.pending.take() else {
            return;
        };
        self.pending_removals.clear();
        for key in self.pending_included.drain(..) {
            if let Some(entity) = self.entities.get_mut(&key) {
                if entity.in_flight.back().is_some_and(|(at, _)| *at == seq) {
                    entity.in_flight.pop_back();
                }
                if entity.base.is_none() && entity.in_flight.is_empty() {
                    self.entities.remove(&key);
                }
            }
        }
    }
    /// `key` has left this connection. The receiver is told until it confirms.
    pub fn remove(&mut self, key: K) {
        if self.entities.remove(&key).is_some() {
            self.removals.insert(key);
        }
    }
    /// Whether `key` is currently part of the stream.
    pub fn contains(&self, key: K) -> bool {
        self.entities.contains_key(&key)
    }
    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.entities.keys().copied()
    }
    /// Apply an acknowledgement. Older or repeated ones are harmless.
    pub fn acknowledge(&mut self, ack: StreamAck) -> Result<(), StreamError> {
        if ack.latest >= self.next {
            return Err(StreamError::FutureAck);
        }
        if ack.latest < self.acked.latest {
            // Reordered: still learn which packets it reports.
        } else {
            self.acked = ack;
        }
        for packet in &mut self.sent {
            if !ack.covers(packet.seq) {
                continue;
            }
            for key in packet.removals.drain(..) {
                // A key that came back after this removal is no longer pending.
                if !self.entities.contains_key(&key) {
                    self.removals.remove(&key);
                }
            }
        }
        for entity in self.entities.values_mut() {
            let newest = entity
                .in_flight
                .iter()
                .rposition(|(seq, _)| ack.covers(*seq));
            if let Some(index) = newest {
                let promoted = entity.in_flight.drain(..=index).next_back();
                if let Some((seq, state)) = promoted
                    && entity.base.as_ref().is_none_or(|(at, _)| *at < seq)
                {
                    entity.base = Some((seq, state));
                }
            }
        }
        Ok(())
    }
}

struct ReceivedEntity<S> {
    /// Accepted states by packet sequence; the newest is the current state.
    history: BTreeMap<u32, S>,
}

/// How the receiver decodes one entry.
#[derive(Debug, PartialEq, Eq)]
pub enum Base<'a, S> {
    /// The entry is a whole state.
    Whole,
    /// The entry carries changes against this state.
    Delta(&'a S),
    /// A newer packet removed the entity; skip the entry.
    Superseded,
}

/// What one accepted packet changed.
#[derive(Debug, PartialEq, Eq)]
pub struct StreamChanges<K> {
    /// Keys whose newest state is now from this packet.
    pub updated: Vec<K>,
    /// Keys this packet removed.
    pub removed: Vec<K>,
}

/// Receiver side. One per connection.
pub struct ClientStream<K, S> {
    ack: StreamAck,
    entities: BTreeMap<K, ReceivedEntity<S>>,
    /// Sequence of the newest removal of each key no longer held, so an older
    /// packet arriving late cannot bring a removed entity back.
    removed_at: BTreeMap<K, u32>,
    max_entities: usize,
}
impl<K: Copy + Ord, S: Clone> ClientStream<K, S> {
    pub fn new(max_entities: usize) -> Self {
        Self {
            ack: StreamAck::default(),
            entities: BTreeMap::new(),
            removed_at: BTreeMap::new(),
            max_entities,
        }
    }
    /// The acknowledgement to send with outgoing traffic.
    pub fn ack(&self) -> StreamAck {
        self.ack
    }
    /// Whether a packet with this sequence would be accepted.
    pub fn accepts(&self, seq: u32) -> bool {
        seq != 0
            && !self.ack.covers(seq)
            && seq.saturating_add(STREAM_WINDOW) > self.ack.latest
    }
    /// How to decode the entry for `key` in packet `seq`, which names `base`.
    pub fn base(&self, key: K, base: Option<u32>, seq: u32) -> Result<Base<'_, S>, StreamError> {
        if self.removed_at.get(&key).is_some_and(|at| *at > seq) {
            return Ok(Base::Superseded);
        }
        let Some(base) = base else {
            return Ok(Base::Whole);
        };
        if base >= seq {
            return Err(StreamError::Malformed);
        }
        self.entities
            .get(&key)
            .and_then(|entity| entity.history.get(&base))
            .map(Base::Delta)
            .ok_or(StreamError::MissingBase)
    }
    /// The newest state of `key` and the packet that carried it.
    pub fn current(&self, key: K) -> Option<(u32, &S)> {
        self.entities
            .get(&key)?
            .history
            .last_key_value()
            .map(|(seq, state)| (*seq, state))
    }
    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.entities.keys().copied()
    }
    /// Commit a packet whose entries were all decoded against [`Self::base`].
    /// Only an accepted packet is acknowledged.
    pub fn accept(
        &mut self,
        seq: u32,
        entries: Vec<(K, S)>,
        removals: &[K],
    ) -> Result<StreamChanges<K>, StreamError> {
        if !self.accepts(seq) {
            return Err(StreamError::Malformed);
        }
        let mut keys = BTreeSet::new();
        if entries
            .iter()
            .map(|(key, _)| *key)
            .chain(removals.iter().copied())
            .any(|key| !keys.insert(key))
        {
            return Err(StreamError::Malformed);
        }
        let new = entries
            .iter()
            .filter(|(key, _)| !self.entities.contains_key(key))
            .count();
        if self.entities.len() + new > self.max_entities {
            return Err(StreamError::Capacity);
        }
        let mut changes = StreamChanges {
            updated: Vec::new(),
            removed: Vec::new(),
        };
        for (key, state) in entries {
            if self.removed_at.get(&key).is_some_and(|at| *at > seq) {
                continue;
            }
            self.removed_at.remove(&key);
            let entity = self.entities.entry(key).or_insert(ReceivedEntity {
                history: BTreeMap::new(),
            });
            let newest = entity
                .history
                .last_key_value()
                .is_none_or(|(at, _)| *at < seq);
            entity.history.insert(seq, state);
            if newest {
                changes.updated.push(key);
            }
        }
        for key in removals {
            let current = self
                .entities
                .get(key)
                .and_then(|entity| entity.history.last_key_value().map(|(at, _)| *at));
            if current.is_some_and(|at| at > seq) {
                continue;
            }
            if self.entities.remove(key).is_some() {
                changes.removed.push(*key);
            }
            self.removed_at.insert(*key, seq);
        }
        self.record(seq);
        Ok(changes)
    }
    fn record(&mut self, seq: u32) {
        if seq > self.ack.latest {
            let shift = seq - self.ack.latest;
            let previous = u64::from(self.ack.latest != 0);
            self.ack.received = if shift >= 64 {
                if shift == 64 { previous << 63 } else { 0 }
            } else {
                (self.ack.received << shift) | (previous << (shift - 1))
            };
            self.ack.latest = seq;
        } else {
            let behind = self.ack.latest - seq;
            self.ack.received |= 1 << (behind - 1);
        }
        // Keep the newest state of each entity for presentation, and every
        // state the sender may still name as a base.
        let oldest = self.ack.latest.saturating_sub(STREAM_WINDOW - 1);
        for entity in self.entities.values_mut() {
            while entity.history.len() > 1
                && entity
                    .history
                    .first_key_value()
                    .is_some_and(|(at, _)| *at < oldest)
            {
                entity.history.pop_first();
            }
        }
        self.removed_at.retain(|_, at| *at >= oldest);
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;

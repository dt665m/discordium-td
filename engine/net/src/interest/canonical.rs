//! Bounded reuse of connection-independent positive payloads at one committed barrier.
use super::EligibleSet;
use crate::types::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CanonicalCacheStats {
    pub hits: u64,
    pub encodes: u64,
    pub uncached: u64,
    pub entries: usize,
    pub retained_bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CanonicalCacheError<E> {
    StaleBarrier,
    MissingGrant,
    Encode(E),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Barrier {
    frame: ReplicationFrame,
    tick: ServerTick,
    world: u64,
    policy: PolicyRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    entity: EntityId,
    state: StateVersion,
    scene: SceneRevision,
    schema: u64,
    representation: RepresentationRevision,
    fields: u64,
    prediction_allowed: bool,
}

/// The game must explicitly opt in only payloads whose bytes depend exclusively
/// on the committed entity and exact representation grant. In particular, owner
/// data, source-redacted views, attachment scope references, baselines and packets
/// do not meet this contract. A hit is not authorization: delivery must still go
/// through the recipient's current `EligibleSet` and scope barrier.
///
/// No data survives `begin_frame`, even when an entity version is unchanged. The
/// entry and allocation ceilings bound memory; a full cache simply encodes without
/// retaining the result. The cache never removes an eligible delivery.
pub struct CanonicalPayloadCache {
    barrier: Option<Barrier>,
    entries: BTreeMap<Key, Vec<u8>>,
    max_entries: usize,
    max_bytes: usize,
    stats: CanonicalCacheStats,
}
impl CanonicalPayloadCache {
    // Includes the fixed key/value and conservative B-tree node allocation share.
    const ENTRY_BYTES: usize = std::mem::size_of::<Key>() + std::mem::size_of::<Vec<u8>>() + 128;

    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            barrier: None,
            entries: BTreeMap::new(),
            max_entries,
            max_bytes,
            stats: CanonicalCacheStats::default(),
        }
    }
    pub fn clear(&mut self) {
        self.barrier = None;
        self.entries.clear();
        self.stats = CanonicalCacheStats::default();
    }
    pub fn begin_frame(
        &mut self,
        frame: ReplicationFrame,
        tick: ServerTick,
        world_revision: u64,
        policy_revision: PolicyRevision,
    ) {
        self.clear();
        self.barrier = Some(Barrier {
            frame,
            tick,
            world: world_revision,
            policy: policy_revision,
        });
    }
    pub fn stats(&self) -> CanonicalCacheStats {
        self.stats
    }
    pub fn encode<E>(
        &mut self,
        eligible: &EligibleSet,
        entity: EntityId,
        encode: impl FnOnce() -> Result<Vec<u8>, E>,
    ) -> Result<Vec<u8>, CanonicalCacheError<E>> {
        if self.barrier
            != Some(Barrier {
                frame: eligible.frame(),
                tick: eligible.tick(),
                world: eligible.world_revision(),
                policy: eligible.policy_revision(),
            })
        {
            return Err(CanonicalCacheError::StaleBarrier);
        }
        let entry = eligible
            .get(entity)
            .ok_or(CanonicalCacheError::MissingGrant)?;
        let grant = entry.representation();
        let key = Key {
            entity,
            state: entry.state_version(),
            scene: entry.scene_revision(),
            schema: grant.schema_id,
            representation: grant.revision,
            fields: grant.fields,
            prediction_allowed: grant.prediction_allowed,
        };
        if let Some(bytes) = self.entries.get(&key) {
            self.stats.hits = self.stats.hits.saturating_add(1);
            return Ok(bytes.clone());
        }
        self.stats.encodes = self.stats.encodes.saturating_add(1);
        let bytes = encode().map_err(CanonicalCacheError::Encode)?;
        let charge = bytes.len().saturating_add(Self::ENTRY_BYTES);
        if self.entries.len() >= self.max_entries
            || charge > self.max_bytes.saturating_sub(self.stats.retained_bytes)
        {
            self.stats.uncached = self.stats.uncached.saturating_add(1);
            return Ok(bytes);
        }
        // Clone allocates only the visible bytes, not the encoder's spare capacity.
        let retained = bytes.clone();
        let charge = retained.capacity().saturating_add(Self::ENTRY_BYTES);
        if charge > self.max_bytes.saturating_sub(self.stats.retained_bytes) {
            self.stats.uncached = self.stats.uncached.saturating_add(1);
            return Ok(bytes);
        }
        self.entries.insert(key, retained);
        self.stats.entries = self.entries.len();
        self.stats.retained_bytes += charge;
        Ok(bytes)
    }
}

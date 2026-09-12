use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupPublication {
    pub connection: ConnectionEpoch,
    pub group: GroupId,
    pub revision: GroupRevision,
    pub snapshot: SnapshotId,
}

/// Each fragment carries whole canonical members; splitting one member's byte
/// codec is left to a separately bounded decoder. All fragments repeat the same
/// sorted, unique full member manifest and committed tick.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupChunk<P> {
    pub publication: GroupPublication,
    pub end_tick: ServerTick,
    pub manifest: Vec<ScopeIdentity>,
    pub index: usize,
    pub count: usize,
    pub members: Vec<FullState<P>>,
}
#[derive(Debug, Clone, Copy)]
pub struct GroupLimits {
    pub members: usize,
    pub chunks: usize,
    pub incomplete: usize,
    pub known_groups: usize,
    /// Charges payload capacities, FullState structs and manifest storage.
    /// Fixed map/vector overhead is additionally bounded by count limits.
    pub staging_bytes: usize,
    pub group_bytes: usize,
    pub max_age_ms: u64,
}
impl Default for GroupLimits {
    fn default() -> Self {
        Self {
            members: 32,
            chunks: 16,
            incomplete: 8,
            known_groups: 8,
            staging_bytes: 128 * 1024,
            group_bytes: 16384,
            max_age_ms: 2000,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupProgress {
    Staged,
    Published(Vec<StateReceipt>),
}
#[derive(Debug)]
struct Assembly<P> {
    end_tick: ServerTick,
    manifest: Vec<ScopeIdentity>,
    chunks: Vec<Option<Vec<FullState<P>>>>,
    started_ms: u64,
    bytes: usize,
}
#[derive(Debug)]
struct GroupFence {
    publication: GroupPublication,
    end_tick: ServerTick,
    manifest: Vec<ScopeIdentity>,
    receipts: Option<Vec<StateReceipt>>,
}

/// Independent bounded chunk staging and publication fences. A newer snapshot
/// supersedes an unfinished full transfer without extending its original deadline.
/// Expired incomplete groups require explicit reset before accepting more data.
#[derive(Debug)]
pub struct GroupAssembler<P> {
    connection: ConnectionEpoch,
    limits: GroupLimits,
    assemblies: BTreeMap<GroupId, Assembly<P>>,
    fences: BTreeMap<GroupId, GroupFence>,
    bytes: usize,
    now_ms: u64,
}
impl<P: Payload> GroupAssembler<P> {
    pub fn new(connection: ConnectionEpoch, limits: GroupLimits) -> Result<Self, ReplicationError> {
        if connection.0 == 0
            || limits.members == 0
            || limits.chunks == 0
            || limits.incomplete == 0
            || limits.known_groups < limits.incomplete
            || limits.group_bytes == 0
            || limits.staging_bytes < limits.group_bytes
            || limits.max_age_ms == 0
        {
            return Err(ReplicationError::InvalidConfiguration);
        }
        Ok(Self {
            connection,
            limits,
            assemblies: BTreeMap::new(),
            fences: BTreeMap::new(),
            bytes: 0,
            now_ms: 0,
        })
    }
    pub fn staged_bytes(&self) -> usize {
        self.bytes
    }
    pub fn incomplete(&self) -> usize {
        self.assemblies.len()
    }
    pub fn publication(&self, group: GroupId) -> Option<GroupPublication> {
        self.fences.get(&group).map(|fence| fence.publication)
    }
    /// Expiration retains the publication fence, so late fragments cannot
    /// recreate an expired assembly under the same identity.
    pub fn expire(&mut self, now_ms: u64) -> Result<usize, ReplicationError> {
        if now_ms < self.now_ms {
            return Err(ReplicationError::Stale);
        }
        self.now_ms = now_ms;
        let before = self.assemblies.len();
        self.assemblies.retain(|_, assembly| {
            let keep = now_ms - assembly.started_ms < self.limits.max_age_ms;
            if !keep {
                self.bytes -= assembly.bytes;
            }
            keep
        });
        Ok(before - self.assemblies.len())
    }
    pub fn receive(
        &mut self,
        chunk: GroupChunk<P>,
        now_ms: u64,
        client: &mut ClientScopes<P>,
        validate: impl Fn(&P) -> bool,
    ) -> Result<GroupProgress, ReplicationError> {
        self.expire(now_ms)?;
        let key = chunk.publication;
        if key.connection != self.connection || client.connection() != self.connection {
            return Err(ReplicationError::Stale);
        }
        if key.revision.0 == 0
            || key.snapshot.0 == 0
            || chunk.manifest.is_empty()
            || chunk.manifest.len() > self.limits.members
            || chunk.count == 0
            || chunk.count > self.limits.chunks
            || chunk.count > chunk.manifest.len()
            || chunk.index >= chunk.count
            || chunk.members.is_empty()
            || chunk.members.len() > chunk.manifest.len()
        {
            return Err(ReplicationError::InvalidIdentity);
        }
        let mut indices = BTreeSet::new();
        for (i, scope) in chunk.manifest.iter().enumerate() {
            validate_scope(*scope, self.connection)?;
            if !indices.insert(scope.entity.index) || (i > 0 && chunk.manifest[i - 1] >= *scope) {
                return Err(ReplicationError::Conflict);
            }
        }
        let mut member_ids = BTreeSet::new();
        let mut member_bytes = chunk
            .members
            .capacity()
            .checked_mul(std::mem::size_of::<FullState<P>>())
            .ok_or(ReplicationError::Capacity)?;
        for state in &chunk.members {
            state.validate_identity(self.connection)?;
            if state.end_tick != chunk.end_tick
                || chunk.manifest.binary_search(&state.scope).is_err()
                || !member_ids.insert(state.scope)
            {
                return Err(ReplicationError::Conflict);
            }
            if !validate(&state.payload) {
                return Err(ReplicationError::InvalidPayload);
            }
            member_bytes = member_bytes
                .checked_add(state.payload.retained_bytes())
                .ok_or(ReplicationError::Capacity)?;
        }
        let mut replace = false;
        let mut started_ms = now_ms;
        if let Some(fence) = self.fences.get(&key.group) {
            let old = fence.publication;
            if key.revision == old.revision {
                if chunk.manifest != fence.manifest
                    || (key.snapshot == old.snapshot && chunk.end_tick != fence.end_tick)
                    || (key.snapshot < old.snapshot && chunk.end_tick > fence.end_tick)
                    || (key.snapshot > old.snapshot && chunk.end_tick < fence.end_tick)
                {
                    return Err(ReplicationError::Conflict);
                }
            }
            if key.revision < old.revision
                || (key.revision == old.revision && key.snapshot < old.snapshot)
            {
                return Err(ReplicationError::Obsolete(ObsoleteReason::Publication));
            }
            if fence.receipts.is_none() && !self.assemblies.contains_key(&key.group) {
                return Err(ReplicationError::Expired);
            }
            if key == old {
                if let Some(receipts) = &fence.receipts {
                    // Repeat ACK only while every named full baseline is still
                    // retained; never promise an already superseded generation.
                    if !receipts.iter().all(|r| {
                        client
                            .state(r.scope.entity)
                            .is_some_and(|s| s.receipt() == *r)
                    }) {
                        return Err(ReplicationError::Stale);
                    }
                    if !chunk
                        .members
                        .iter()
                        .all(|s| client.state(s.scope.entity) == Some(s))
                    {
                        return Err(ReplicationError::Conflict);
                    }
                    return Ok(GroupProgress::Published(receipts.clone()));
                }
                if !self.assemblies.contains_key(&key.group) {
                    return Err(ReplicationError::Expired);
                }
            }
            if key != old {
                if let Some(assembly) = self.assemblies.get(&key.group) {
                    started_ms = assembly.started_ms;
                }
                replace = true;
            }
        } else if self.fences.len() >= self.limits.known_groups {
            return Err(ReplicationError::ResetRequired);
        }
        let existing = self.assemblies.get(&key.group).filter(|_| !replace);
        if let Some(assembly) = existing {
            if assembly.end_tick != chunk.end_tick
                || assembly.chunks.len() != chunk.count
                || assembly.manifest != chunk.manifest
            {
                return Err(ReplicationError::Conflict);
            }
            if let Some(previous) = &assembly.chunks[chunk.index] {
                return if *previous == chunk.members {
                    Ok(GroupProgress::Staged)
                } else {
                    Err(ReplicationError::Conflict)
                };
            }
            if assembly
                .chunks
                .iter()
                .flatten()
                .flatten()
                .any(|s| member_ids.contains(&s.scope))
            {
                return Err(ReplicationError::Conflict);
            }
        } else if !self.assemblies.contains_key(&key.group)
            && self.assemblies.len() >= self.limits.incomplete
        {
            return Err(ReplicationError::Capacity);
        }
        let old_bytes = if replace {
            self.assemblies.get(&key.group).map_or(0, |a| a.bytes)
        } else {
            0
        };
        let added = member_bytes
            .checked_add(if existing.is_none() {
                chunk
                    .manifest
                    .capacity()
                    .checked_mul(std::mem::size_of::<ScopeIdentity>())
                    .ok_or(ReplicationError::Capacity)?
            } else {
                0
            })
            .ok_or(ReplicationError::Capacity)?;
        let group_bytes = existing
            .map_or(0, |a| a.bytes)
            .checked_add(added)
            .ok_or(ReplicationError::Capacity)?;
        let bytes = self
            .bytes
            .checked_sub(old_bytes)
            .and_then(|n| n.checked_add(added))
            .ok_or(ReplicationError::Capacity)?;
        if group_bytes > self.limits.group_bytes || bytes > self.limits.staging_bytes {
            return Err(ReplicationError::Capacity);
        }
        if replace {
            self.assemblies.remove(&key.group);
        }
        let assembly = self
            .assemblies
            .entry(key.group)
            .or_insert_with(|| Assembly {
                end_tick: chunk.end_tick,
                manifest: chunk.manifest.clone(),
                chunks: (0..chunk.count).map(|_| None).collect(),
                started_ms,
                bytes: 0,
            });
        assembly.chunks[chunk.index] = Some(chunk.members);
        assembly.bytes = group_bytes;
        self.bytes = bytes;
        self.fences.insert(
            key.group,
            GroupFence {
                publication: key,
                end_tick: chunk.end_tick,
                manifest: chunk.manifest.clone(),
                receipts: None,
            },
        );
        if assembly.chunks.iter().any(Option::is_none) {
            return Ok(GroupProgress::Staged);
        }
        // All fragments are present; coverage must be exact, not merely a count
        // of bytes or arrival of the last numbered fragment.
        let count: usize = assembly.chunks.iter().flatten().map(Vec::len).sum();
        if count != assembly.manifest.len() {
            let removed = self.assemblies.remove(&key.group).unwrap();
            self.bytes -= removed.bytes;
            return Err(ReplicationError::Incomplete);
        }
        let mut staged_client = client.clone();
        let result = assembly
            .chunks
            .iter()
            .flatten()
            .flatten()
            .map(|state| staged_client.apply_full(state.clone(), &validate))
            .collect::<Result<Vec<_>, _>>();
        let removed = self.assemblies.remove(&key.group).unwrap();
        self.bytes -= removed.bytes;
        let receipts = result?;
        *client = staged_client;
        self.fences.get_mut(&key.group).unwrap().receipts = Some(receipts.clone());
        Ok(GroupProgress::Published(receipts))
    }
    pub fn reset(&mut self, connection: ConnectionEpoch) -> Result<(), ReplicationError> {
        if connection <= self.connection {
            return Err(ReplicationError::Stale);
        }
        self.connection = connection;
        self.assemblies.clear();
        self.fences.clear();
        self.bytes = 0;
        self.now_ms = 0;
        Ok(())
    }
}

/// Borrowed proof that an entire group's exact decoded publication is still
/// retained. The immutable ClientScopes borrow prevents a member from exiting or
/// being replaced while a predictor decodes it. No public constructor exists.
pub struct PublishedGroup<'a, P> {
    publication: GroupPublication,
    end_tick: ServerTick,
    manifest: &'a [ScopeIdentity],
    client: &'a ClientScopes<P>,
}
impl<P: Payload> PublishedGroup<'_, P> {
    pub fn publication(&self) -> GroupPublication {
        self.publication
    }
    pub fn end_tick(&self) -> ServerTick {
        self.end_tick
    }
    pub fn manifest(&self) -> &[ScopeIdentity] {
        self.manifest
    }
    pub fn states(&self) -> impl ExactSizeIterator<Item = &FullState<P>> {
        self.manifest.iter().map(|scope| {
            self.client
                .state(scope.entity)
                .expect("immutable fully retained group proof")
        })
    }
}
impl<P: Payload> GroupAssembler<P> {
    pub fn published_group<'a>(
        &'a self,
        group: GroupId,
        client: &'a ClientScopes<P>,
    ) -> Option<PublishedGroup<'a, P>> {
        if client.connection() != self.connection {
            return None;
        }
        let fence = self.fences.get(&group)?;
        let receipts = fence.receipts.as_ref()?;
        if fence.manifest.is_empty() || receipts.len() != fence.manifest.len() {
            return None;
        }
        let end_tick = client.state(fence.manifest[0].entity)?.end_tick;
        for scope in &fence.manifest {
            let state = client.state(scope.entity)?;
            if state.scope != *scope
                || state.end_tick != end_tick
                || !receipts.iter().any(|receipt| *receipt == state.receipt())
            {
                return None;
            }
        }
        Some(PublishedGroup {
            publication: fence.publication,
            end_tick,
            manifest: &fence.manifest,
            client,
        })
    }
}

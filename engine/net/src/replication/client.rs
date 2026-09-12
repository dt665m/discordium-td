use super::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
struct Replica<P> {
    scope: ScopeIdentity,
    closed: bool,
    destroyed: bool,
    state: Option<FullState<P>>,
}

/// Bounded, read-only identity and publication metadata for rejection diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplicaFence {
    pub scope: ScopeIdentity,
    pub closed: bool,
    pub destroyed: bool,
    pub publication: Option<(StateReceipt, ServerTick)>,
}

/// Sparse state and high-water fences per entity index. Closed records consume
/// the independent identity cap until a new authenticated connection epoch.
/// Group publication preflights on a bounded clone, so peak working replica
/// storage is at most twice retained_payload_bytes plus bounded staging storage.
#[derive(Debug, Clone)]
pub struct ClientScopes<P> {
    connection: ConnectionEpoch,
    limits: ScopeLimits,
    records: BTreeMap<u64, Replica<P>>,
    bytes: usize,
}
impl<P: Payload> ClientScopes<P> {
    pub fn new(connection: ConnectionEpoch, limits: ScopeLimits) -> Result<Self, ReplicationError> {
        if connection.0 == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        Ok(Self {
            connection,
            limits: limits.validate()?,
            records: BTreeMap::new(),
            bytes: 0,
        })
    }
    pub fn connection(&self) -> ConnectionEpoch {
        self.connection
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub fn known_entities(&self) -> usize {
        self.records.len()
    }
    pub fn fence(&self, entity_index: u64) -> Option<ReplicaFence> {
        let replica = self.records.get(&entity_index)?;
        Some(ReplicaFence {
            scope: replica.scope,
            closed: replica.closed,
            destroyed: replica.destroyed,
            publication: replica.state.as_ref().map(|s| (s.receipt(), s.end_tick)),
        })
    }
    pub fn state(&self, entity: EntityId) -> Option<&FullState<P>> {
        self.records
            .get(&entity.index)?
            .state
            .as_ref()
            .filter(|s| s.scope.entity == entity)
    }
    fn capacity_for(&self, index: u64) -> Result<(), ReplicationError> {
        if !self.records.contains_key(&index) && self.records.len() >= self.limits.known_entities {
            Err(ReplicationError::ResetRequired)
        } else {
            Ok(())
        }
    }
    /// Validate a canonical payload before any published field or fence changes.
    /// Returning a receipt promises this full state is published and retained.
    pub fn apply_full(
        &mut self,
        state: FullState<P>,
        validate: impl FnOnce(&P) -> bool,
    ) -> Result<StateReceipt, ReplicationError> {
        state.validate_identity(self.connection)?;
        self.capacity_for(state.scope.entity.index)?;
        // Even a superseded packet must satisfy the caller's canonical payload
        // validation; malformed data must never acquire an obsolete classification.
        if state.payload.retained_bytes() > self.limits.payload_bytes {
            return Err(ReplicationError::Capacity);
        }
        if !validate(&state.payload) {
            return Err(ReplicationError::InvalidPayload);
        }
        let mut replaced_bytes = 0;
        if let Some(old) = self.records.get(&state.scope.entity.index) {
            if state.scope.entity.generation < old.scope.entity.generation {
                return Err(ReplicationError::Obsolete(ObsoleteReason::Scope));
            }
            if state.scope.entity.generation == old.scope.entity.generation {
                if old.destroyed || state.scope.scope < old.scope.scope {
                    return Err(ReplicationError::Obsolete(ObsoleteReason::Scope));
                }
                if state.scope.scope == old.scope.scope {
                    if state.scope != old.scope {
                        return Err(ReplicationError::Conflict);
                    }
                    if old.closed {
                        return Err(ReplicationError::Obsolete(ObsoleteReason::Scope));
                    }
                    if let Some(previous) = &old.state {
                        if state.snapshot == previous.snapshot && state != *previous {
                            return Err(ReplicationError::Conflict);
                        }
                        if state.version == previous.version && state.payload != previous.payload {
                            return Err(ReplicationError::Conflict);
                        }
                        if state.snapshot < previous.snapshot
                            && state.baseline_generation <= previous.baseline_generation
                            && state.version <= previous.version
                            && state.end_tick <= previous.end_tick
                        {
                            return Err(ReplicationError::Obsolete(ObsoleteReason::Publication));
                        }
                        // A newer snapshot cannot move any watermark backward,
                        // and an older snapshot cannot claim newer metadata.
                        if state.baseline_generation < previous.baseline_generation
                            || state.snapshot < previous.snapshot
                            || state.version < previous.version
                            || state.end_tick < previous.end_tick
                        {
                            return Err(ReplicationError::Conflict);
                        }
                    }
                }
            }
            replaced_bytes = old.state.as_ref().map_or(0, |s| s.payload.retained_bytes());
        }
        let size = state.payload.retained_bytes();
        let bytes = self
            .bytes
            .checked_sub(replaced_bytes)
            .and_then(|n| n.checked_add(size))
            .ok_or(ReplicationError::Capacity)?;
        if size > self.limits.payload_bytes || bytes > self.limits.retained_payload_bytes {
            return Err(ReplicationError::Capacity);
        }
        let receipt = state.receipt();
        self.records.insert(
            state.scope.entity.index,
            Replica {
                scope: state.scope,
                closed: false,
                destroyed: false,
                state: Some(state),
            },
        );
        self.bytes = bytes;
        Ok(receipt)
    }
    /// Exit-before-entry records a fence. Delayed old exits are acknowledged
    /// idempotently without deleting a newer generation/incarnation.
    pub fn apply_exit(&mut self, exit: ScopeExit) -> Result<ScopeExit, ReplicationError> {
        validate_scope(exit.scope, self.connection)?;
        self.capacity_for(exit.scope.entity.index)?;
        if let Some(old) = self.records.get(&exit.scope.entity.index) {
            if exit.scope.entity.generation < old.scope.entity.generation
                || (exit.scope.entity.generation == old.scope.entity.generation
                    && exit.scope.scope < old.scope.scope)
            {
                return Ok(exit);
            }
            if exit.scope.entity.generation == old.scope.entity.generation
                && exit.scope.scope == old.scope.scope
                && exit.scope != old.scope
            {
                return Err(ReplicationError::Conflict);
            }
            if old.destroyed && exit.scope.entity.generation == old.scope.entity.generation {
                return Ok(exit);
            }
        }
        let previous = self.records.insert(
            exit.scope.entity.index,
            Replica {
                scope: exit.scope,
                closed: true,
                destroyed: false,
                state: None,
            },
        );
        self.bytes -= previous
            .and_then(|r| r.state)
            .map_or(0, |s| s.payload.retained_bytes());
        Ok(exit)
    }
    /// Authoritative death fences the entity generation regardless of scope.
    /// The server must separately authorize the audience for this fact.
    pub fn apply_destroy(&mut self, destroy: Destroy) -> Result<Destroy, ReplicationError> {
        if destroy.connection != self.connection {
            return Err(ReplicationError::Stale);
        }
        if destroy.entity.generation == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        self.capacity_for(destroy.entity.index)?;
        if self
            .records
            .get(&destroy.entity.index)
            .is_some_and(|r| r.scope.entity.generation > destroy.entity.generation)
        {
            return Ok(destroy);
        }
        let scope = ScopeIdentity {
            connection: destroy.connection,
            entity: destroy.entity,
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let previous = self.records.insert(
            destroy.entity.index,
            Replica {
                scope,
                closed: true,
                destroyed: true,
                state: None,
            },
        );
        self.bytes -= previous
            .and_then(|r| r.state)
            .map_or(0, |s| s.payload.retained_bytes());
        Ok(destroy)
    }
    pub fn reset(&mut self, connection: ConnectionEpoch) -> Result<(), ReplicationError> {
        if connection <= self.connection {
            return Err(ReplicationError::Stale);
        }
        self.connection = connection;
        self.records.clear();
        self.bytes = 0;
        Ok(())
    }
}

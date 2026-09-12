use super::*;
use crate::interest::{EligibleSet, RepresentationGrant};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryPhase {
    Entering,
    Active,
    DormantKnown,
    Leaving,
    Forgotten,
}
#[derive(Debug, Clone)]
struct Delivery<P> {
    identity: ScopeIdentity,
    desired: Option<FullState<P>>,
    sent: BTreeMap<SnapshotId, StateReceipt>,
    decoded: Option<StateReceipt>,
    dormant: bool,
    phase: DeliveryPhase,
    exit_sent: bool,
    destroyed: bool,
}

/// One authenticated peer's sparse delivery state. Caller performs bounded
/// projection from a graph grant; no API accepts an unrestricted actor array.
/// This full-state-only implementation resets the baseline generation on every
/// new publication. Delta encoding/retirement negotiation is intentionally absent.
#[derive(Debug)]
pub struct ServerScopes<P> {
    connection: ConnectionId,
    epoch: ConnectionEpoch,
    world_revision: u64,
    policy: PolicyRevision,
    limits: ScopeLimits,
    records: BTreeMap<u64, Delivery<P>>,
    next_snapshot: SnapshotId,
    bytes: usize,
    reset_required: bool,
}
impl<P: Payload> ServerScopes<P> {
    pub fn new(
        connection: ConnectionId,
        epoch: ConnectionEpoch,
        world_revision: u64,
        policy: PolicyRevision,
        limits: ScopeLimits,
    ) -> Result<Self, ReplicationError> {
        if epoch.0 == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        Ok(Self {
            connection,
            epoch,
            world_revision,
            policy,
            limits: limits.validate()?,
            records: BTreeMap::new(),
            next_snapshot: SnapshotId(1),
            bytes: 0,
            reset_required: false,
        })
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub fn known_entities(&self) -> usize {
        self.records.len()
    }
    pub fn phase(&self, entity: EntityId) -> Option<DeliveryPhase> {
        self.records
            .get(&entity.index)
            .filter(|r| r.identity.entity == entity)
            .map(|r| r.phase)
    }
    pub fn pending_transitions(&self) -> usize {
        self.records
            .values()
            .filter(|r| matches!(r.phase, DeliveryPhase::Entering | DeliveryPhase::Leaving))
            .count()
    }
    /// Fail closed if required revocations cannot fit their independent cap.
    /// The owner must reset/disconnect the peer; no old payload remains sendable.
    fn require_reset(&mut self) -> ReplicationError {
        self.reset_required = true;
        for record in self.records.values_mut() {
            record.desired = None;
            record.sent.clear();
            record.decoded = None;
            record.phase = DeliveryPhase::Forgotten;
        }
        self.bytes = 0;
        ReplicationError::ResetRequired
    }
    /// Advance the committed barrier before gather/encoding. A policy change
    /// conservatively revokes every old representation, cancels unsent payloads
    /// and retains retryable exits. Reoffer current grants under fresh scopes.
    /// Bytes already handed to transport cannot be retracted by this API.
    pub fn advance_barrier(
        &mut self,
        world_revision: u64,
        policy: PolicyRevision,
    ) -> Result<(), ReplicationError> {
        if self.reset_required {
            return Err(ReplicationError::ResetRequired);
        }
        if world_revision < self.world_revision || policy < self.policy {
            return Err(ReplicationError::Stale);
        }
        if policy != self.policy {
            if self
                .records
                .values()
                .filter(|r| r.phase != DeliveryPhase::Forgotten)
                .count()
                > self.limits.pending_transitions
            {
                return Err(self.require_reset());
            }
            for record in self.records.values_mut() {
                if record.phase != DeliveryPhase::Forgotten {
                    record.phase = if record.destroyed {
                        DeliveryPhase::Forgotten
                    } else {
                        DeliveryPhase::Leaving
                    };
                    record.exit_sent = false;
                    record.desired = None;
                    record.sent.clear();
                    record.decoded = None;
                }
            }
            self.bytes = 0;
        }
        self.world_revision = world_revision;
        self.policy = policy;
        Ok(())
    }
    /// `project` must positively select only the grant's schema/field mask, and
    /// never return the complete unrestricted game checkpoint. It runs only
    /// after connection and barrier validation. The graph supplies dirty version
    /// and committed tick; a mutation must publish a larger version before offer.
    pub fn offer(
        &mut self,
        eligible: &EligibleSet,
        entity: EntityId,
        dormant: bool,
        project: impl FnOnce(RepresentationGrant) -> P,
    ) -> Result<ScopeIdentity, ReplicationError> {
        if self.reset_required {
            return Err(ReplicationError::ResetRequired);
        }
        if eligible.connection() != self.connection
            || !eligible.is_current(self.world_revision, self.policy)
        {
            return Err(ReplicationError::Unauthorized);
        }
        let entry = eligible.get(entity).ok_or(ReplicationError::Unauthorized)?;
        let grant = entry.representation();
        self.offer_projected(
            entity,
            grant.revision,
            entry.state_version(),
            eligible.tick(),
            dormant,
            project(grant),
        )
    }
    fn offer_projected(
        &mut self,
        entity: EntityId,
        representation: RepresentationRevision,
        version: StateVersion,
        end_tick: ServerTick,
        dormant: bool,
        payload: P,
    ) -> Result<ScopeIdentity, ReplicationError> {
        if self.reset_required {
            return Err(ReplicationError::ResetRequired);
        }
        if entity.generation == 0 || representation.0 == 0 || version.0 == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        if !self.records.contains_key(&entity.index)
            && self.records.len() >= self.limits.known_entities
        {
            return Err(ReplicationError::ResetRequired);
        }
        let old = self.records.get(&entity.index);
        if old.is_some_and(|r| {
            entity.generation < r.identity.entity.generation
                || (entity.generation == r.identity.entity.generation && r.destroyed)
        }) {
            return Err(ReplicationError::Stale);
        }
        let new_scope = old.is_none_or(|r| {
            r.identity.entity != entity
                || r.identity.representation != representation
                || matches!(r.phase, DeliveryPhase::Leaving | DeliveryPhase::Forgotten)
        });
        if new_scope
            && old.is_none_or(|r| {
                !matches!(r.phase, DeliveryPhase::Entering | DeliveryPhase::Leaving)
            })
            && self.pending_transitions() >= self.limits.pending_transitions
        {
            return Err(ReplicationError::Capacity);
        }
        if !new_scope {
            let previous = old
                .and_then(|r| r.desired.as_ref())
                .ok_or(ReplicationError::Unknown)?;
            if version < previous.version || end_tick < previous.end_tick {
                return Err(ReplicationError::Stale);
            }
            if version == previous.version {
                if payload != previous.payload {
                    return Err(ReplicationError::Conflict);
                }
                let record = self.records.get_mut(&entity.index).unwrap();
                record.dormant = dormant;
                Self::refresh_phase(record);
                return Ok(record.identity);
            }
        }
        let size = payload.retained_bytes();
        let old_size = old
            .and_then(|r| r.desired.as_ref())
            .map_or(0, |s| s.payload.retained_bytes());
        let bytes = self
            .bytes
            .checked_sub(old_size)
            .and_then(|n| n.checked_add(size))
            .ok_or(ReplicationError::Capacity)?;
        if size > self.limits.payload_bytes || bytes > self.limits.retained_payload_bytes {
            return Err(ReplicationError::Capacity);
        }
        let scope = if new_scope {
            old.map_or(Some(ScopeEpoch(1)), |r| r.identity.scope.checked_next())
                .ok_or(ReplicationError::ResetRequired)?
        } else {
            old.unwrap().identity.scope
        };
        let next_snapshot = self
            .next_snapshot
            .checked_next()
            .ok_or(ReplicationError::ResetRequired)?;
        let identity = ScopeIdentity {
            connection: self.epoch,
            entity,
            scope,
            representation,
        };
        let state = FullState {
            scope: identity,
            baseline_generation: BaselineGeneration(self.next_snapshot.0),
            snapshot: self.next_snapshot,
            version,
            end_tick,
            payload,
        };
        if new_scope {
            self.records.insert(
                entity.index,
                Delivery {
                    identity,
                    desired: Some(state),
                    sent: BTreeMap::new(),
                    decoded: None,
                    dormant,
                    phase: DeliveryPhase::Entering,
                    exit_sent: false,
                    destroyed: false,
                },
            );
        } else {
            let record = self.records.get_mut(&entity.index).unwrap();
            record.desired = Some(state);
            record.dormant = dormant;
            Self::refresh_phase(record);
        }
        self.bytes = bytes;
        self.next_snapshot = next_snapshot;
        Ok(identity)
    }
    fn refresh_phase(record: &mut Delivery<P>) {
        record.phase = if record.decoded.is_none() {
            DeliveryPhase::Entering
        } else if record.dormant
            && record
                .desired
                .as_ref()
                .is_some_and(|s| Some(s.receipt()) == record.decoded)
        {
            DeliveryPhase::DormantKnown
        } else {
            DeliveryPhase::Active
        };
    }
    /// Read-only selection. Repeated calls do not acknowledge or mark as sent.
    pub fn state(&self, entity: EntityId) -> Option<&FullState<P>> {
        let record = self.records.get(&entity.index)?;
        if record.identity.entity != entity
            || matches!(
                record.phase,
                DeliveryPhase::Leaving | DeliveryPhase::Forgotten
            )
        {
            return None;
        }
        record.desired.as_ref()
    }
    /// Read-only selection of current state still awaiting its decoded receipt.
    pub fn pending(&self, entity: EntityId) -> Option<&FullState<P>> {
        let record = self.records.get(&entity.index)?;
        if record.identity.entity != entity
            || matches!(
                record.phase,
                DeliveryPhase::Leaving | DeliveryPhase::Forgotten
            )
        {
            return None;
        }
        record
            .desired
            .as_ref()
            .filter(|state| Some(state.receipt()) != record.decoded)
    }
    /// Invoke only once transport has accepted these exact bytes. Retry uses the
    /// same identity. A canceled/stale selection can never become a sent proof.
    pub fn mark_sent(&mut self, receipt: StateReceipt) -> Result<(), ReplicationError> {
        let record = self
            .records
            .get_mut(&receipt.scope.entity.index)
            .ok_or(ReplicationError::Unknown)?;
        if record.identity != receipt.scope
            || matches!(
                record.phase,
                DeliveryPhase::Leaving | DeliveryPhase::Forgotten
            )
            || record.desired.as_ref().map(FullState::receipt) != Some(receipt)
        {
            return Err(ReplicationError::Stale);
        }
        // Full states are independent reset generations. Forget an obsolete
        // proof under loss; a late ACK for it is safely ignored. Never stall the
        // latest repair forever because older datagrams were unacknowledged.
        if !record.sent.contains_key(&receipt.snapshot)
            && record.sent.len() >= self.limits.sent_proofs_per_scope
        {
            record.sent.pop_first();
        }
        record.sent.insert(receipt.snapshot, receipt);
        Ok(())
    }
    /// Preflight identity/capacity before the callback hands bytes to transport.
    /// The callback returns true only on accepted send, and must not retain an
    /// unrestricted queue bypassing subsequent policy barriers.
    pub fn transmit_pending(
        &mut self,
        entity: EntityId,
        send: impl FnOnce(&FullState<P>) -> bool,
    ) -> Result<bool, ReplicationError> {
        if self.reset_required {
            return Err(ReplicationError::ResetRequired);
        }
        let Some(state) = self.pending(entity) else {
            return Ok(false);
        };
        let receipt = state.receipt();
        if !send(state) {
            return Ok(false);
        }
        self.mark_sent(receipt)?;
        Ok(true)
    }
    /// Validate a complete sent proof without advancing decoded state. Atomic
    /// group adapters must preflight all members before acknowledging any member.
    pub fn validate_acknowledgment(&self, receipt: StateReceipt) -> Result<(), ReplicationError> {
        let record = self
            .records
            .get(&receipt.scope.entity.index)
            .ok_or(ReplicationError::Unknown)?;
        if receipt.scope != record.identity
            || matches!(
                record.phase,
                DeliveryPhase::Leaving | DeliveryPhase::Forgotten
            )
        {
            return Err(ReplicationError::Stale);
        }
        if record.decoded == Some(receipt) {
            return Ok(());
        }
        if record.sent.get(&receipt.snapshot) != Some(&receipt) {
            return Err(ReplicationError::Unknown);
        }
        if record
            .decoded
            .is_some_and(|old| receipt.snapshot < old.snapshot || receipt.version < old.version)
        {
            return Err(ReplicationError::Stale);
        }
        Ok(())
    }
    pub fn acknowledge(&mut self, receipt: StateReceipt) -> Result<(), ReplicationError> {
        self.validate_acknowledgment(receipt)?;
        let record = self
            .records
            .get_mut(&receipt.scope.entity.index)
            .ok_or(ReplicationError::Unknown)?;
        if record.decoded == Some(receipt) {
            return Ok(());
        }
        record.decoded = Some(receipt);
        record
            .sent
            .retain(|snapshot, _| *snapshot >= receipt.snapshot);
        Self::refresh_phase(record);
        Ok(())
    }
    /// Exact current full-state decode proof. This module enables no deltas;
    /// future delta support must additionally negotiate baseline retention.
    pub fn decoded_current(&self, entity: EntityId) -> Option<StateReceipt> {
        let record = self.records.get(&entity.index)?;
        if record.identity.entity != entity {
            return None;
        }
        let receipt = record.desired.as_ref()?.receipt();
        (record.decoded == Some(receipt)).then_some(receipt)
    }
    pub fn exit(&mut self, entity: EntityId) -> Result<ScopeExit, ReplicationError> {
        if self.reset_required {
            return Err(ReplicationError::ResetRequired);
        }
        let old = self
            .records
            .get(&entity.index)
            .filter(|r| r.identity.entity == entity)
            .ok_or(ReplicationError::Unknown)?;
        if old.destroyed {
            return Err(ReplicationError::Conflict);
        }
        if !matches!(
            old.phase,
            DeliveryPhase::Entering | DeliveryPhase::Leaving | DeliveryPhase::Forgotten
        ) && self.pending_transitions() >= self.limits.pending_transitions
        {
            return Err(self.require_reset());
        }
        let record = self
            .records
            .get_mut(&entity.index)
            .filter(|r| r.identity.entity == entity)
            .ok_or(ReplicationError::Unknown)?;
        if record.phase != DeliveryPhase::Leaving && record.phase != DeliveryPhase::Forgotten {
            record.phase = DeliveryPhase::Leaving;
            record.exit_sent = false;
            self.bytes -= record
                .desired
                .take()
                .map_or(0, |s| s.payload.retained_bytes());
            record.sent.clear();
            record.decoded = None;
        }
        Ok(ScopeExit {
            scope: record.identity,
        })
    }
    pub fn pending_exits(&self) -> impl Iterator<Item = ScopeExit> + '_ {
        self.records
            .values()
            .filter(|r| r.phase == DeliveryPhase::Leaving && !r.destroyed)
            .map(|r| ScopeExit { scope: r.identity })
    }
    /// Fences not yet accepted by a reliable transport. Once marked sent, the
    /// transport owns retransmission until the application acknowledgement.
    pub fn unsent_exits(&self) -> impl Iterator<Item = ScopeExit> + '_ {
        self.records
            .values()
            .filter(|r| r.phase == DeliveryPhase::Leaving && !r.destroyed && !r.exit_sent)
            .map(|r| ScopeExit { scope: r.identity })
    }
    pub fn mark_exit_sent(&mut self, exit: ScopeExit) -> Result<(), ReplicationError> {
        let record = self
            .records
            .get_mut(&exit.scope.entity.index)
            .ok_or(ReplicationError::Unknown)?;
        if record.identity != exit.scope
            || record.destroyed
            || record.phase != DeliveryPhase::Leaving
        {
            return Err(ReplicationError::Stale);
        }
        record.exit_sent = true;
        Ok(())
    }
    pub fn acknowledge_exit(&mut self, exit: ScopeExit) -> Result<(), ReplicationError> {
        let record = self
            .records
            .get_mut(&exit.scope.entity.index)
            .ok_or(ReplicationError::Unknown)?;
        if record.identity != exit.scope
            || record.destroyed
            || !record.exit_sent
            || !matches!(
                record.phase,
                DeliveryPhase::Leaving | DeliveryPhase::Forgotten
            )
        {
            return Err(ReplicationError::Stale);
        }
        record.phase = DeliveryPhase::Forgotten;
        Ok(())
    }
    /// Emit death only to an existing still-entitled scope. A policy barrier
    /// changes that scope to Leaving first, so revoked peers receive no death.
    /// Same-generation state cannot be reoffered after destruction.
    pub fn destroy(&mut self, entity: EntityId) -> Result<Destroy, ReplicationError> {
        if self.reset_required {
            return Err(ReplicationError::ResetRequired);
        }
        let record = self
            .records
            .get(&entity.index)
            .filter(|r| r.identity.entity == entity)
            .ok_or(ReplicationError::Unknown)?;
        if record.destroyed {
            return if record.phase == DeliveryPhase::Leaving {
                Ok(Destroy {
                    connection: self.epoch,
                    entity,
                })
            } else {
                Err(ReplicationError::Unauthorized)
            };
        }
        if matches!(
            record.phase,
            DeliveryPhase::Leaving | DeliveryPhase::Forgotten
        ) {
            return Err(ReplicationError::Unauthorized);
        }
        self.exit(entity)?;
        self.records.get_mut(&entity.index).unwrap().destroyed = true;
        Ok(Destroy {
            connection: self.epoch,
            entity,
        })
    }
    pub fn pending_destroys(&self) -> impl Iterator<Item = Destroy> + '_ {
        self.records
            .values()
            .filter(|r| r.phase == DeliveryPhase::Leaving && r.destroyed)
            .map(|r| Destroy {
                connection: self.epoch,
                entity: r.identity.entity,
            })
    }
    /// Destructions not yet accepted by a reliable transport.
    pub fn unsent_destroys(&self) -> impl Iterator<Item = Destroy> + '_ {
        self.records
            .values()
            .filter(|r| r.phase == DeliveryPhase::Leaving && r.destroyed && !r.exit_sent)
            .map(|r| Destroy {
                connection: self.epoch,
                entity: r.identity.entity,
            })
    }
    pub fn mark_destroy_sent(&mut self, destroy: Destroy) -> Result<(), ReplicationError> {
        if destroy.connection != self.epoch {
            return Err(ReplicationError::Stale);
        }
        let record = self
            .records
            .get_mut(&destroy.entity.index)
            .ok_or(ReplicationError::Unknown)?;
        if record.identity.entity != destroy.entity
            || !record.destroyed
            || record.phase != DeliveryPhase::Leaving
        {
            return Err(ReplicationError::Stale);
        }
        record.exit_sent = true;
        Ok(())
    }
    pub fn acknowledge_destroy(&mut self, destroy: Destroy) -> Result<(), ReplicationError> {
        if destroy.connection != self.epoch {
            return Err(ReplicationError::Stale);
        }
        let record = self
            .records
            .get_mut(&destroy.entity.index)
            .ok_or(ReplicationError::Unknown)?;
        if record.identity.entity != destroy.entity || !record.destroyed || !record.exit_sent {
            return Err(ReplicationError::Stale);
        }
        record.phase = DeliveryPhase::Forgotten;
        Ok(())
    }
    pub fn reset(&mut self, epoch: ConnectionEpoch) -> Result<(), ReplicationError> {
        if epoch <= self.epoch {
            return Err(ReplicationError::Stale);
        }
        self.epoch = epoch;
        self.records.clear();
        self.bytes = 0;
        self.next_snapshot = SnapshotId(1);
        self.reset_required = false;
        Ok(())
    }
}

#[cfg(test)]
impl<P: Payload> ServerScopes<P> {
    pub(super) fn fixture_offer(
        &mut self,
        entity: EntityId,
        representation: u32,
        version: u64,
        dormant: bool,
        payload: P,
    ) -> Result<ScopeIdentity, ReplicationError> {
        self.offer_projected(
            entity,
            RepresentationRevision(representation),
            StateVersion(version),
            ServerTick(version),
            dormant,
            payload,
        )
    }
}

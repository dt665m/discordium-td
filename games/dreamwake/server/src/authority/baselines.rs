//! Exact per-connection owner-group retention and synchronous send accounting.
use super::*;
use engine_net::replication::{FullState, GroupPublication, baselines::*};

#[derive(Default)]
pub(super) struct OwnerBaselines {
    caches: BTreeMap<EntityId, (BaselineContext, ServerBaselines<Vec<u8>>)>,
    pub reset_pending: Option<Vec<BaselineReset>>,
    pub reset_queued: bool,
}

impl OwnerBaselines {
    pub fn check_peak(&self, staged_bytes: usize) -> Result<(), String> {
        let resident = self
            .caches
            .values()
            .map(|(_, cache)| cache.retained_bytes())
            .sum::<usize>();
        if resident
            .checked_add(staged_bytes)
            .is_none_or(|bytes| bytes > live::baselines::GROUP_PEAK_BYTES)
        {
            return Err("Owner baseline staging capacity".into());
        }
        Ok(())
    }
    pub fn prepare_contexts(
        &mut self,
        members: &[FullState<Vec<u8>>],
        publication: GroupPublication,
    ) -> Result<(), String> {
        if !live::baselines::valid_member_count(members.len())
            || members
                .iter()
                .map(|state| state.scope.entity)
                .collect::<BTreeSet<_>>()
                .len()
                != members.len()
        {
            return Err("Invalid owner baseline manifest".into());
        }
        let changed = self.caches.len() != members.len()
            || members.iter().any(|state| {
                self.caches
                    .get(&state.scope.entity)
                    .is_none_or(|(context, _)| {
                        *context != live::baselines::context(state, publication)
                    })
            });
        if changed {
            self.close();
            for state in members {
                let context = live::baselines::context(state, publication);
                self.caches.insert(
                    state.scope.entity,
                    (
                        context,
                        ServerBaselines::new(
                            context,
                            BaselineGeneration(1),
                            live::baselines::limits(),
                        )
                        .map_err(failure)?,
                    ),
                );
            }
        }
        self.check_budget()
    }
    fn check_budget(&self) -> Result<(), String> {
        if self.caches.len() > live::baselines::MEMBERS
            || self
                .caches
                .values()
                .map(|(_, cache)| cache.retained_bytes())
                .sum::<usize>()
                > live::baselines::GROUP_RESIDENT_BYTES
        {
            return Err("Owner baseline aggregate capacity".into());
        }
        Ok(())
    }
    pub fn prepare(
        &mut self,
        members: &[FullState<Vec<u8>>],
        publication: GroupPublication,
    ) -> Result<Option<Vec<BaselinePacket>>, String> {
        self.prepare_contexts(members, publication)?;
        // Three projected/encoded payload copies, group codec + fragmentation,
        // framed transport scratch, and per-codec single-packet scratch are all
        // charged before building the publication; no staging clone is unbounded.
        self.check_peak(6 * live::baselines::MEMBER_BYTES * members.len() + 2 * 65_536)?;
        let result = self.preview(members, publication);
        match result {
            Ok(packets) => Ok(Some(packets)),
            Err(engine_net::replication::ReplicationError::Capacity) => {
                if self.reset_pending.is_some() {
                    return Ok(None);
                }
                self.reset_pending = Some(
                    self.caches
                        .values_mut()
                        .map(|(_, cache)| cache.begin_reset())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(failure)?,
                );
                self.reset_queued = false;
                self.preview(members, publication)
                    .map(Some)
                    .map_err(failure)
            }
            Err(error) => Err(failure(error)),
        }
    }
    fn preview(
        &mut self,
        members: &[FullState<Vec<u8>>],
        publication: GroupPublication,
    ) -> Result<Vec<BaselinePacket>, engine_net::replication::ReplicationError> {
        let mut packets = Vec::with_capacity(members.len());
        for state in members {
            let (_, cache) = self
                .caches
                .get_mut(&state.scope.entity)
                .ok_or(engine_net::replication::ReplicationError::Unknown)?;
            let receipt = live::baselines::target(state, publication, cache.generation());
            let base = cache
                .newest_decoded()
                .filter(|base| base.snapshot < receipt.snapshot);
            cache.transmit(
                BaselineState {
                    receipt,
                    payload: state.payload.clone(),
                },
                base,
                &BytePatch,
                |packet| {
                    packets.push(packet.clone());
                    false
                },
            )?;
        }
        Ok(packets)
    }
    /// Called only after all preflighted group fragments were queued successfully.
    pub fn mark_sent(
        &mut self,
        members: &[FullState<Vec<u8>>],
        packets: &[BaselinePacket],
    ) -> Result<(), String> {
        for (state, packet) in members.iter().zip(packets) {
            let (_, cache) = self
                .caches
                .get_mut(&state.scope.entity)
                .ok_or("Missing baseline cache")?;
            let base = match packet.encoding {
                BaselineEncoding::Full => None,
                BaselineEncoding::Delta { base } => Some(base),
            };
            if !cache
                .transmit(
                    BaselineState {
                        receipt: packet.target,
                        payload: state.payload.clone(),
                    },
                    base,
                    &BytePatch,
                    |actual| actual == packet,
                )
                .map_err(failure)?
            {
                return Err("Changed baseline publication after preflight".into());
            }
        }
        self.check_budget()
    }
    pub fn validate_proofs(
        &self,
        proofs: &[live::baselines::DecodedBaseline],
    ) -> Result<bool, String> {
        // Reliable proofs can arrive after a stream recovery or a prediction
        // group membership change. Fence obsolete contexts before comparing the
        // manifest size: a valid old three-member proof is not a malformed new
        // four-member proof (and a fresh peer may have no caches yet).
        if self.caches.is_empty()
            || proofs.iter().any(|proof| {
                self.caches
                    .get(&proof.state.scope.entity)
                    .is_none_or(|(context, cache)| {
                        proof.baseline.context != *context
                            || proof.baseline.generation != cache.generation()
                    })
            })
        {
            return Ok(false);
        }
        if proofs.len() != self.caches.len()
            || proofs
                .iter()
                .map(|p| p.state.scope.entity)
                .collect::<BTreeSet<_>>()
                .len()
                != self.caches.len()
        {
            return Err("Incomplete owner baseline proof".into());
        }
        let Some(first) = proofs.first() else {
            return Ok(false);
        };
        for proof in proofs {
            let Some((context, cache)) = self.caches.get(&proof.state.scope.entity) else {
                return Ok(false);
            };
            if proof.baseline.context != *context || proof.baseline.generation != cache.generation()
            {
                return Ok(false);
            }
            if proof.baseline.end_tick != first.baseline.end_tick
                || proof.baseline.context.group != first.baseline.context.group
            {
                return Err("Mixed owner baseline proof".into());
            }
            if cache.validate_acknowledgment(proof.baseline).is_err() {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn acknowledge(
        &mut self,
        proofs: &[live::baselines::DecodedBaseline],
    ) -> Result<bool, String> {
        if !self.validate_proofs(proofs)? {
            return Ok(false);
        }
        for proof in proofs {
            self.caches
                .get_mut(&proof.state.scope.entity)
                .unwrap()
                .1
                .acknowledge(proof.baseline)
                .map_err(failure)?;
        }
        self.reset_pending = None;
        self.reset_queued = false;
        Ok(true)
    }
    pub fn retire(
        &mut self,
        requests: &[BaselineRetirement],
    ) -> Result<Option<Vec<BaselineRetirement>>, String> {
        if requests
            .iter()
            .map(|r| r.context.scope.entity)
            .collect::<BTreeSet<_>>()
            .len()
            != requests.len()
        {
            return Err("Duplicate baseline retirement".into());
        }
        for request in requests {
            let Some((context, cache)) = self.caches.get(&request.context.scope.entity) else {
                return Ok(None);
            };
            if *context != request.context || cache.generation() != request.generation {
                return Ok(None);
            }
            cache.validate_retirement(*request).map_err(failure)?;
        }
        for request in requests {
            self.caches
                .get_mut(&request.context.scope.entity)
                .unwrap()
                .1
                .retire(*request)
                .map_err(failure)?;
        }
        Ok(Some(requests.to_vec()))
    }
    pub fn repair(&mut self, requests: &[BaselineRepair]) -> Result<(), String> {
        if self.reset_pending.is_some() {
            return Ok(());
        }
        if !requests.iter().any(|request| {
            self.caches
                .get(&request.context.scope.entity)
                .is_some_and(|(context, cache)| {
                    *context == request.context && cache.generation() == request.generation
                })
        }) {
            return Ok(());
        }
        self.reset_pending = Some(
            self.caches
                .values_mut()
                .map(|(_, cache)| cache.begin_reset())
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?,
        );
        self.reset_queued = false;
        Ok(())
    }
    pub fn close(&mut self) {
        for (_, cache) in self.caches.values_mut() {
            cache.close();
        }
        self.caches.clear();
        self.reset_pending = None;
        self.reset_queued = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Vec<FullState<Vec<u8>>>, GroupPublication) {
        let members = (1..=3)
            .map(|index| FullState {
                scope: ScopeIdentity {
                    connection: ConnectionEpoch(1),
                    entity: EntityId {
                        index,
                        generation: 1,
                    },
                    scope: ScopeEpoch(1),
                    representation: RepresentationRevision(1),
                },
                baseline_generation: BaselineGeneration(index),
                snapshot: SnapshotId(index),
                version: StateVersion(1),
                end_tick: ServerTick(1),
                payload: vec![index as u8; 100],
            })
            .collect();
        (
            members,
            GroupPublication {
                connection: ConnectionEpoch(1),
                group: live::OWNER_GROUP,
                revision: GroupRevision(1),
                snapshot: SnapshotId(2),
            },
        )
    }
    #[test]
    fn platform_membership_revision_replaces_every_baseline_context_atomically() {
        let (mut members, mut publication) = fixture();
        let mut caches = OwnerBaselines::default();
        let first = caches.prepare(&members, publication).unwrap().unwrap();
        caches.mark_sent(&members, &first).unwrap();
        let old_proofs: Vec<_> = members
            .iter()
            .zip(&first)
            .map(|(state, packet)| live::baselines::proof(state, packet.target))
            .collect();
        assert!(caches.acknowledge(&old_proofs).unwrap());
        let mut platform = members[0].clone();
        platform.scope.entity.index = 4;
        members.push(platform);
        publication.revision = GroupRevision(2);
        publication.snapshot = SnapshotId(4);
        let next = caches.prepare(&members, publication).unwrap().unwrap();
        assert_eq!(next.len(), 4);
        assert!(
            next.iter()
                .all(|packet| matches!(packet.encoding, BaselineEncoding::Full))
        );
        assert!(!caches.validate_proofs(&old_proofs).unwrap());
        caches.mark_sent(&members, &next).unwrap();
        let proofs: Vec<_> = members
            .iter()
            .zip(&next)
            .map(|(state, packet)| live::baselines::proof(state, packet.target))
            .collect();
        assert!(caches.acknowledge(&proofs).unwrap());
        members.pop();
        publication.revision = GroupRevision(3);
        publication.snapshot = SnapshotId(5);
        let reduced = caches.prepare(&members, publication).unwrap().unwrap();
        assert_eq!(caches.caches.len(), 3);
        assert!(
            reduced
                .iter()
                .all(|packet| matches!(packet.encoding, BaselineEncoding::Full))
        );
        assert!(!caches.validate_proofs(&proofs).unwrap());
    }

    #[test]
    fn delayed_proof_after_stream_recovery_is_ignored_before_manifest_validation() {
        let (mut members, mut publication) = fixture();
        let mut caches = OwnerBaselines::default();
        let packets = caches.prepare(&members, publication).unwrap().unwrap();
        caches.mark_sent(&members, &packets).unwrap();
        let stale: Vec<_> = members
            .iter()
            .zip(&packets)
            .map(|(state, packet)| live::baselines::proof(state, packet.target))
            .collect();
        // Ready can precede the first replacement publication while reliable
        // old controls are still in flight.
        caches.close();
        assert!(!caches.acknowledge(&stale).unwrap());
        publication.connection = ConnectionEpoch(2);
        for member in &mut members {
            member.scope.connection = ConnectionEpoch(2);
        }
        let packets = caches.prepare(&members, publication).unwrap().unwrap();
        caches.mark_sent(&members, &packets).unwrap();
        assert!(!caches.acknowledge(&stale).unwrap());
        assert!(
            caches
                .caches
                .values()
                .all(|(_, cache)| cache.newest_decoded().is_none())
        );
        let current: Vec<_> = members
            .iter()
            .zip(&packets)
            .map(|(state, packet)| live::baselines::proof(state, packet.target))
            .collect();
        assert!(caches.validate_proofs(&current[..2]).is_err());
        let mut duplicate = current.clone();
        duplicate[2] = duplicate[1];
        assert!(caches.validate_proofs(&duplicate).is_err());
        assert!(caches.acknowledge(&current).unwrap());
    }

    #[test]
    fn unsent_preview_and_mixed_proof_never_seed_any_member() {
        let (members, publication) = fixture();
        let mut caches = OwnerBaselines::default();
        let packets = caches.prepare(&members, publication).unwrap().unwrap();
        assert!(
            caches
                .caches
                .values()
                .all(|(_, cache)| cache.retained_states() == 0)
        );
        caches.mark_sent(&members, &packets).unwrap();
        let mut proofs: Vec<_> = members
            .iter()
            .zip(&packets)
            .map(|(state, packet)| live::baselines::proof(state, packet.target))
            .collect();
        proofs[2].baseline.version = StateVersion(2);
        assert!(!caches.acknowledge(&proofs).unwrap());
        assert!(
            caches
                .caches
                .values()
                .all(|(_, cache)| cache.newest_decoded().is_none())
        );
        proofs[2] = live::baselines::proof(&members[2], packets[2].target);
        assert!(caches.acknowledge(&proofs).unwrap());
        assert!(
            caches
                .caches
                .values()
                .all(|(_, cache)| cache.newest_decoded().is_some())
        );
    }
}

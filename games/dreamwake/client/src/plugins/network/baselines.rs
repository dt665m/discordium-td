//! Declared owner-member caches staged together with atomic group publication.
use dreamwake_protocol::live::baselines::{
    GROUP_PEAK_BYTES, GROUP_RESIDENT_BYTES, MEMBERS, limits,
};
use engine_net::{
    replication::{FullState, ReplicationError, baselines::*},
    types::*,
};
use std::collections::BTreeMap;

#[derive(Clone)]
struct Member {
    context: BaselineContext,
    cache: ClientBaselines<Vec<u8>>,
    retirement: Option<BaselineRetirement>,
}
#[derive(Clone, Default)]
pub(super) struct OwnerBaselines {
    members: BTreeMap<EntityId, Member>,
}
#[derive(Debug)]
pub(super) struct RetirementRejection {
    pub(super) error: ReplicationError,
    pub(super) fence: Option<BaselineRetirement>,
}
impl OwnerBaselines {
    pub(super) fn identity(
        &self,
        entity: EntityId,
    ) -> Option<(BaselineContext, BaselineGeneration)> {
        let member = self.members.get(&entity)?;
        Some((member.context, member.cache.generation()))
    }
    /// Keep a complete potentially rejected seed for atomic game validation.
    /// Ordinary advancing states are reconstructed directly into the group.
    pub(super) fn rejected_full_seed(&self, packet: &BaselinePacket) -> Option<Vec<u8>> {
        let member = self.members.get(&packet.target.context.scope.entity)?;
        (member.context == packet.target.context
            && (member.cache.generation() != packet.target.generation
                || member
                    .cache
                    .current()
                    .is_some_and(|current| packet.target.snapshot <= current.receipt.snapshot))
            && matches!(packet.encoding, BaselineEncoding::Full))
        .then(|| packet.bytes.clone())
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.members
            .values()
            .map(|m| m.cache.retained_bytes() + std::mem::size_of::<Member>() + 128)
            .sum()
    }
    /// Clone only after charging resident, staging and packet scratch envelopes.
    pub(super) fn stage(&self) -> Result<Self, ReplicationError> {
        if self.members.len() > MEMBERS
            || self.retained_bytes().saturating_mul(2) + 256 * 1024 > GROUP_PEAK_BYTES
            || self
                .members
                .values()
                .map(|m| m.cache.retained_bytes())
                .sum::<usize>()
                > GROUP_RESIDENT_BYTES
        {
            return Err(ReplicationError::Capacity);
        }
        Ok(self.clone())
    }
    pub(super) fn reconstruct(
        &mut self,
        state: &mut FullState<Vec<u8>>,
        packet: BaselinePacket,
    ) -> Result<BaselineReceipt, ReplicationError> {
        let entity = state.scope.entity;
        if self
            .members
            .get(&entity)
            .is_some_and(|m| m.context != packet.target.context)
        {
            self.members.remove(&entity);
        }
        if !self.members.contains_key(&entity) {
            if self.members.len() >= MEMBERS || !matches!(packet.encoding, BaselineEncoding::Full) {
                return Err(ReplicationError::InvalidIdentity);
            }
            self.members.insert(
                entity,
                Member {
                    context: packet.target.context,
                    cache: ClientBaselines::new(
                        packet.target.context,
                        packet.target.generation,
                        limits(),
                    )?,
                    retirement: None,
                },
            );
        }
        let member = self.members.get_mut(&entity).unwrap();
        // Reliable retirement fences may overtake older unreliable deltas. A
        // retired base is expected stale traffic: retain the usable replacement
        // and await the next publication instead of resetting every generation.
        let receipt = member.cache.receive(&packet, &BytePatch, |_| true)?;
        state.payload = member.cache.state(receipt)?.payload.clone();
        Ok(receipt)
    }
    pub(super) fn retain_manifest(&mut self, scopes: &[ScopeIdentity]) {
        self.members
            .retain(|_, m| scopes.contains(&m.context.scope));
    }
    pub(super) fn revoke(&mut self, entity: EntityId) {
        self.members.remove(&entity);
    }
    pub(super) fn retirements(&mut self) -> Result<Vec<BaselineRetirement>, ReplicationError> {
        let mut requests = Vec::new();
        for member in self.members.values_mut() {
            // The reliable transport already retains this request until delivery.
            // Remote actor receipts must not enqueue duplicate retirement controls.
            if member.retirement.is_some() {
                continue;
            }
            if member.cache.retained_states() < 2 {
                continue;
            }
            let latest = member
                .cache
                .current()
                .ok_or(ReplicationError::Unknown)?
                .receipt
                .snapshot;
            // Cumulative retirement leaves the latest decoded snapshot intact.
            if latest.0 > 1 {
                let request = member.cache.propose_retirement(SnapshotId(latest.0 - 1))?;
                member.retirement = Some(request);
                requests.push(request);
            }
        }
        Ok(requests)
    }
    fn control_context(
        member: &Member,
        context: BaselineContext,
        generation: BaselineGeneration,
        reset: bool,
    ) -> Result<(), ReplicationError> {
        use engine_net::replication::ObsoleteReason;
        let current = member.context;
        // Check unrelated and malformed dimensions before considering age.
        ClientBaselines::<Vec<u8>>::new(context, generation, limits())?;
        if context.scope.connection != current.scope.connection
            || context.scope.entity != current.scope.entity
            || context.schema != current.schema
            || context.group.map(|g| g.0) != current.group.map(|g| g.0)
        {
            return Err(ReplicationError::Conflict);
        }
        let revision = context.group.map(|g| g.1);
        let current_revision = current.group.map(|g| g.1);
        if context.scope.scope < current.scope.scope
            && context.scope.representation <= current.scope.representation
            && revision <= current_revision
        {
            return Err(ReplicationError::Obsolete(ObsoleteReason::Scope));
        }
        if context.scope != current.scope {
            return Err(ReplicationError::Conflict);
        }
        if revision < current_revision {
            return Err(ReplicationError::Obsolete(ObsoleteReason::Publication));
        }
        if revision > current_revision {
            if reset {
                return Ok(());
            }
            return Err(ReplicationError::Conflict);
        }
        let current_generation = member.cache.generation();
        if generation < current_generation {
            return Err(ReplicationError::Obsolete(ObsoleteReason::RetiredBaseline));
        }
        if generation != current_generation
            && !(reset && current_generation.0.checked_add(1) == Some(generation.0))
        {
            return Err(ReplicationError::Conflict);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn acknowledge_retirements(
        &mut self,
        fences: &[BaselineRetirement],
    ) -> Result<(), ReplicationError> {
        self.acknowledge_retirements_checked(fences, None)
            .map_err(|rejection| rejection.error)
    }
    pub(super) fn acknowledge_retirements_with_scopes(
        &mut self,
        fences: &[BaselineRetirement],
        scopes: &engine_net::replication::ClientScopes<Vec<u8>>,
    ) -> Result<(), RetirementRejection> {
        self.acknowledge_retirements_checked(fences, Some(scopes))
    }
    fn acknowledge_retirements_checked(
        &mut self,
        fences: &[BaselineRetirement],
        scopes: Option<&engine_net::replication::ClientScopes<Vec<u8>>>,
    ) -> Result<(), RetirementRejection> {
        use engine_net::replication::ObsoleteReason;
        if fences.is_empty() || fences.len() > MEMBERS {
            return Err(RetirementRejection {
                error: ReplicationError::InvalidIdentity,
                fence: None,
            });
        }
        let first = fences[0].context;
        let mut entities = std::collections::BTreeSet::new();
        // Validate the entire batch before using any old member as age evidence.
        for &fence in fences {
            let rejected = |error| RetirementRejection {
                error,
                fence: Some(fence),
            };
            ClientBaselines::<Vec<u8>>::new(fence.context, fence.generation, limits())
                .map_err(rejected)?;
            if fence.through.0 == 0 || !entities.insert(fence.context.scope.entity) {
                return Err(rejected(ReplicationError::InvalidIdentity));
            }
            if fence.context.group != first.group
                || fence.context.scope.connection != first.scope.connection
                || fence.context.schema != SchemaId(1)
                || fence.context.group.map(|group| group.0)
                    != Some(dreamwake_protocol::live::OWNER_GROUP)
            {
                return Err(rejected(ReplicationError::Conflict));
            }
        }
        // Retirement requests can be a subset of the owner manifest. The current
        // group, rather than an in-batch mandatory member, supplies the age proof.
        let older_known_group = self.members.values().any(|member| {
            member.context.scope.connection == first.scope.connection
                && member
                    .context
                    .group
                    .zip(first.group)
                    .is_some_and(|(current, incoming)| {
                        current.0 == incoming.0 && incoming.1 < current.1
                    })
        });
        let mut next = self
            .stage()
            .map_err(|error| RetirementRejection { error, fence: None })?;
        let mut obsolete = None;
        for &fence in fences {
            let rejected = |error| RetirementRejection {
                error,
                fence: Some(fence),
            };
            let Some(member) = next.members.get_mut(&fence.context.scope.entity) else {
                let known = scopes
                    .filter(|scopes| scopes.connection() == fence.context.scope.connection)
                    .and_then(|scopes| scopes.fence(fence.context.scope.entity.index))
                    .ok_or_else(|| rejected(ReplicationError::Unknown))?;
                if known.scope.entity != fence.context.scope.entity
                    || known.scope.connection != fence.context.scope.connection
                    || !(known.scope == fence.context.scope
                        || (known.closed
                            && fence.context.scope.scope < known.scope.scope
                            && fence.context.scope.representation <= known.scope.representation))
                {
                    return Err(rejected(ReplicationError::Conflict));
                }
                if !older_known_group {
                    return Err(rejected(ReplicationError::Unknown));
                }
                // The newer owner manifest already removed this known scope.
                // Ignore its old publication; do not acknowledge a current proposal.
                obsolete = Some(rejected(ReplicationError::Obsolete(
                    ObsoleteReason::Publication,
                )));
                continue;
            };
            match Self::control_context(member, fence.context, fence.generation, false) {
                Err(error @ ReplicationError::Obsolete(_)) => {
                    obsolete = Some(rejected(error));
                    continue;
                }
                result => result.map_err(rejected)?,
            }
            member
                .cache
                .acknowledge_retirement(fence)
                .map_err(rejected)?;
            if member.retirement == Some(fence) {
                member.retirement = None;
            }
        }
        if let Some(error) = obsolete {
            return Err(error);
        }
        *self = next;
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn apply_resets(
        &mut self,
        resets: &[BaselineReset],
    ) -> Result<(), ReplicationError> {
        self.apply_resets_checked(resets, None)
    }
    pub(super) fn apply_resets_with_scopes(
        &mut self,
        resets: &[BaselineReset],
        scopes: &engine_net::replication::ClientScopes<Vec<u8>>,
    ) -> Result<(), ReplicationError> {
        self.apply_resets_checked(resets, Some(scopes))
    }
    fn apply_resets_checked(
        &mut self,
        resets: &[BaselineReset],
        scopes: Option<&engine_net::replication::ClientScopes<Vec<u8>>>,
    ) -> Result<(), ReplicationError> {
        let mut next = self.stage()?;
        let mut obsolete = None;
        let mut entities = std::collections::BTreeSet::new();
        let older_known_group = resets.iter().any(|reset| {
            self.members
                .get(&reset.context.scope.entity)
                .is_some_and(|member| {
                    reset.context.group.zip(member.context.group).is_some_and(
                        |(incoming, current)| {
                            incoming.0 == dreamwake_protocol::live::OWNER_GROUP
                                && incoming.0 == current.0
                                && incoming.1 < current.1
                        },
                    ) && matches!(
                        Self::control_context(member, reset.context, reset.generation, true),
                        Err(ReplicationError::Obsolete(_))
                    )
                })
        });
        for &reset in resets {
            ClientBaselines::<Vec<u8>>::new(reset.context, reset.generation, limits())?;
            if !entities.insert(reset.context.scope.entity)
                || resets
                    .first()
                    .is_some_and(|first| first.context.group != reset.context.group)
            {
                return Err(ReplicationError::InvalidIdentity);
            }
            if let Some(member) = self.members.get(&reset.context.scope.entity) {
                match Self::control_context(member, reset.context, reset.generation, true) {
                    Err(error @ ReplicationError::Obsolete(_)) => obsolete = Some(error),
                    result => result?,
                }
            } else if let Some(member) = self.members.values().next() {
                // A new topology may add a validated member, but arbitrary group
                // identities cannot clear caches. Unreliable topology steps may be skipped.
                let current = member.context;
                let old_optional = older_known_group
                    && reset.context.schema == SchemaId(1)
                    && scopes
                        .and_then(|scopes| scopes.fence(reset.context.scope.entity.index))
                        .is_some_and(|fence| {
                            fence.scope.entity == reset.context.scope.entity
                                && fence.scope.connection == reset.context.scope.connection
                                && (fence.scope == reset.context.scope
                                    || (fence.closed
                                        && reset.context.scope.scope < fence.scope.scope
                                        && reset.context.scope.representation
                                            <= fence.scope.representation))
                        });
                if old_optional {
                    obsolete = Some(ReplicationError::Obsolete(
                        engine_net::replication::ObsoleteReason::Publication,
                    ));
                    continue;
                }
                if reset.context.scope.connection != current.scope.connection
                    || reset.context.schema != SchemaId(1)
                    || !current
                        .group
                        .zip(reset.context.group)
                        .is_some_and(|(old, new)| old.0 == new.0 && new.1 > old.1)
                {
                    return Err(ReplicationError::Conflict);
                }
            }
        }
        if let Some(error) = obsolete {
            return Err(error);
        }
        if let Some(first) = resets.first() {
            if next
                .members
                .values()
                .any(|member| member.context.group != first.context.group)
            {
                next.members.clear();
            }
        }
        for &reset in resets {
            let entity = reset.context.scope.entity;
            if let Some(member) = next.members.get_mut(&entity) {
                let changed = member.cache.generation() != reset.generation;
                member.cache.apply_reset(reset)?;
                if changed {
                    member.retirement = None;
                }
            } else {
                if next.members.len() >= MEMBERS {
                    return Err(ReplicationError::Capacity);
                }
                next.members.insert(
                    entity,
                    Member {
                        context: reset.context,
                        cache: ClientBaselines::new(reset.context, reset.generation, limits())?,
                        retirement: None,
                    },
                );
            }
        }
        *self = next;
        Ok(())
    }
    pub(super) fn repairs(&mut self, now_ms: u64) -> Vec<BaselineRepair> {
        self.members
            .values_mut()
            .filter_map(|m| {
                m.cache.request_reset();
                m.cache.repair_request(now_ms)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(snapshot: u64) -> FullState<Vec<u8>> {
        let mut payload = vec![1; 100];
        payload[40] = snapshot as u8;
        FullState {
            scope: ScopeIdentity {
                connection: ConnectionEpoch(1),
                entity: EntityId {
                    index: 1,
                    generation: 1,
                },
                scope: ScopeEpoch(1),
                representation: RepresentationRevision(1),
            },
            baseline_generation: BaselineGeneration(1),
            snapshot: SnapshotId(snapshot),
            version: StateVersion(snapshot),
            end_tick: ServerTick(snapshot),
            payload,
        }
    }
    fn packet(
        server: &mut ServerBaselines<Vec<u8>>,
        target: &FullState<Vec<u8>>,
        base: Option<BaselineReceipt>,
    ) -> BaselinePacket {
        let context = BaselineContext {
            scope: target.scope,
            schema: SchemaId(1),
            group: Some((GroupId(1), GroupRevision(1))),
        };
        let mut result = None;
        server
            .transmit(
                BaselineState {
                    receipt: BaselineReceipt {
                        context,
                        generation: BaselineGeneration(1),
                        snapshot: target.snapshot,
                        version: target.version,
                        end_tick: target.end_tick,
                    },
                    payload: target.payload.clone(),
                },
                base,
                &BytePatch,
                |packet| {
                    result = Some(packet.clone());
                    true
                },
            )
            .unwrap();
        result.unwrap()
    }
    #[test]
    fn retired_delta_overtaken_by_reliable_fence_is_stale_without_reset_loop() {
        let first = state(1);
        let context = BaselineContext {
            scope: first.scope,
            schema: SchemaId(1),
            group: Some((GroupId(1), GroupRevision(1))),
        };
        let mut server = ServerBaselines::new(context, BaselineGeneration(1), limits()).unwrap();
        let mut client = OwnerBaselines::default();
        let full = packet(&mut server, &first, None);
        let base = client.reconstruct(&mut first.clone(), full).unwrap();
        server.acknowledge(base).unwrap();
        let second = packet(&mut server, &state(2), Some(base));
        let delayed = packet(&mut server, &state(3), Some(base));
        assert!(matches!(delayed.encoding, BaselineEncoding::Delta { .. }));
        let replacement = client.reconstruct(&mut state(2), second).unwrap();
        server.acknowledge(replacement).unwrap();
        let retire = client.retirements().unwrap();
        assert_eq!(retire.len(), 1);
        // Repeated unrelated remote receipts do not amplify reliable CONTROL.
        for _ in 0..100 {
            assert!(client.retirements().unwrap().is_empty());
        }
        server.retire(retire[0]).unwrap();
        client.acknowledge_retirements(&retire).unwrap();
        let mut staged = client.stage().unwrap();
        assert_eq!(
            staged.reconstruct(&mut state(3), delayed),
            Err(ReplicationError::Obsolete(
                engine_net::replication::ObsoleteReason::RetiredBaseline
            ))
        );
        // The next publication uses the retained decoded replacement immediately.
        let next = packet(&mut server, &state(4), Some(replacement));
        client.reconstruct(&mut state(4), next).unwrap();
        assert_eq!(
            client.members[&first.scope.entity]
                .cache
                .current()
                .unwrap()
                .receipt
                .generation,
            BaselineGeneration(1)
        );
    }
    fn control_fixture() -> (OwnerBaselines, BaselineContext, BaselineRetirement) {
        let mut scope = state(1).scope;
        scope.scope = ScopeEpoch(3);
        scope.representation = RepresentationRevision(3);
        let context = BaselineContext {
            scope,
            schema: SchemaId(1),
            group: Some((GroupId(1), GroupRevision(3))),
        };
        let mut cache = ClientBaselines::new(context, BaselineGeneration(3), limits()).unwrap();
        for snapshot in 1..=2 {
            cache
                .receive(
                    &BaselinePacket {
                        target: BaselineReceipt {
                            context,
                            generation: BaselineGeneration(3),
                            snapshot: SnapshotId(snapshot),
                            version: StateVersion(snapshot),
                            end_tick: ServerTick(snapshot),
                        },
                        encoding: BaselineEncoding::Full,
                        bytes: state(snapshot).payload,
                    },
                    &BytePatch,
                    |_| true,
                )
                .unwrap();
        }
        let retirement = cache.propose_retirement(SnapshotId(1)).unwrap();
        let mut client = OwnerBaselines::default();
        client.members.insert(
            scope.entity,
            Member {
                context,
                cache,
                retirement: Some(retirement),
            },
        );
        (client, context, retirement)
    }
    #[test]
    fn delayed_baseline_controls_preserve_current_cache_and_pending_retirement() {
        use engine_net::replication::ObsoleteReason;
        let (mut client, context, retirement) = control_fixture();
        let before = client.retained_bytes();
        let current = client.members[&context.scope.entity]
            .cache
            .current()
            .unwrap()
            .clone();
        let mut old_scope = context;
        old_scope.scope.scope = ScopeEpoch(2);
        old_scope.scope.representation = RepresentationRevision(2);
        let mut old_group = context;
        old_group.group = Some((GroupId(1), GroupRevision(2)));
        for (incoming, generation, reason) in [
            (old_scope, BaselineGeneration(3), ObsoleteReason::Scope),
            (
                old_group,
                BaselineGeneration(3),
                ObsoleteReason::Publication,
            ),
            (
                context,
                BaselineGeneration(2),
                ObsoleteReason::RetiredBaseline,
            ),
        ] {
            assert_eq!(
                client.apply_resets(&[BaselineReset {
                    context: incoming,
                    generation
                }]),
                Err(ReplicationError::Obsolete(reason))
            );
            assert_eq!(
                client.acknowledge_retirements(&[BaselineRetirement {
                    context: incoming,
                    generation,
                    ..retirement
                }]),
                Err(ReplicationError::Obsolete(reason))
            );
            assert_eq!(client.retained_bytes(), before);
            let member = &client.members[&context.scope.entity];
            assert_eq!(member.cache.current(), Some(&current));
            assert_eq!(member.retirement, Some(retirement));
            assert_eq!(member.context, context);
        }
        // A duplicate reset cannot discard the outstanding retirement promise.
        client
            .apply_resets(&[BaselineReset {
                context,
                generation: BaselineGeneration(3),
            }])
            .unwrap();
        assert_eq!(
            client.members[&context.scope.entity].retirement,
            Some(retirement)
        );
    }
    #[test]
    fn baseline_control_batch_hard_errors_take_precedence_over_obsolete_entries() {
        let (mut client, context, retirement) = control_fixture();
        let before = client.retained_bytes();
        let old = BaselineReset {
            context,
            generation: BaselineGeneration(2),
        };
        let mut wrong_schema = context;
        wrong_schema.schema = SchemaId(99);
        let mut wrong_group = context;
        wrong_group.group = Some((GroupId(99), GroupRevision(3)));
        let mut future_scope = context;
        future_scope.scope.scope = ScopeEpoch(4);
        for incoming in [wrong_schema, wrong_group, future_scope] {
            assert!(matches!(
                client.apply_resets(&[BaselineReset {
                    context: incoming,
                    generation: BaselineGeneration(3)
                }]),
                Err(ReplicationError::Conflict)
            ));
        }
        assert_eq!(
            client.apply_resets(&[BaselineReset {
                context,
                generation: BaselineGeneration(5)
            }]),
            Err(ReplicationError::Conflict)
        );
        assert_eq!(
            client.apply_resets(&[
                old,
                BaselineReset {
                    context,
                    generation: BaselineGeneration(0)
                }
            ]),
            Err(ReplicationError::InvalidIdentity)
        );
        assert_eq!(
            client.acknowledge_retirements(&[
                BaselineRetirement {
                    generation: BaselineGeneration(2),
                    ..retirement
                },
                BaselineRetirement {
                    through: SnapshotId(0),
                    ..retirement
                },
            ]),
            Err(ReplicationError::InvalidIdentity)
        );
        assert_eq!(
            client.acknowledge_retirements(&[BaselineRetirement {
                generation: BaselineGeneration(4),
                ..retirement
            },]),
            Err(ReplicationError::Conflict)
        );
        assert_eq!(client.retained_bytes(), before);
        assert_eq!(
            client.members[&context.scope.entity].retirement,
            Some(retirement)
        );
        assert_eq!(
            client.identity(context.scope.entity),
            Some((context, BaselineGeneration(3)))
        );
    }
    #[test]
    fn reset_accepts_next_generation_and_skipped_newer_topology_without_rollback() {
        let (mut client, context, _) = control_fixture();
        client
            .apply_resets(&[BaselineReset {
                context,
                generation: BaselineGeneration(4),
            }])
            .unwrap();
        assert_eq!(
            client.identity(context.scope.entity),
            Some((context, BaselineGeneration(4)))
        );
        let mut newer = context;
        newer.group = Some((GroupId(1), GroupRevision(7)));
        client
            .apply_resets(&[BaselineReset {
                context: newer,
                generation: BaselineGeneration(1),
            }])
            .unwrap();
        assert_eq!(
            client.identity(context.scope.entity),
            Some((newer, BaselineGeneration(1)))
        );
        assert!(
            client.members[&context.scope.entity]
                .cache
                .current()
                .is_none()
        );
        assert_eq!(
            client.apply_resets(&[BaselineReset {
                context,
                generation: BaselineGeneration(4)
            }]),
            Err(ReplicationError::Obsolete(
                engine_net::replication::ObsoleteReason::Publication
            ))
        );
        assert_eq!(
            client.identity(context.scope.entity),
            Some((newer, BaselineGeneration(1)))
        );
    }
    #[test]
    fn optional_member_reset_preseed_and_retained_scope_proof_are_bounded() {
        use engine_net::replication::{ClientScopes, ObsoleteReason, ScopeExit, ScopeLimits};
        let (mut client, context, _) = control_fixture();
        let mut new_context = context;
        new_context.group = Some((dreamwake_protocol::live::OWNER_GROUP, GroupRevision(7)));
        let mut optional = new_context;
        optional.scope.entity.index = 99;
        optional.schema = SchemaId(1);
        client
            .apply_resets(&[
                BaselineReset {
                    context: new_context,
                    generation: BaselineGeneration(1),
                },
                BaselineReset {
                    context: optional,
                    generation: BaselineGeneration(1),
                },
            ])
            .unwrap();
        assert_eq!(
            client.identity(optional.scope.entity),
            Some((optional, BaselineGeneration(1)))
        );
        let mut scopes = ClientScopes::new(ConnectionEpoch(1), ScopeLimits::default()).unwrap();
        let mut public = state(2);
        public.scope = optional.scope;
        scopes.apply_full(public, |_| true).unwrap();
        scopes
            .apply_exit(ScopeExit {
                scope: optional.scope,
            })
            .unwrap();
        client.revoke(optional.scope.entity);
        let mut newest = new_context;
        newest.group = Some((dreamwake_protocol::live::OWNER_GROUP, GroupRevision(9)));
        client
            .apply_resets(&[BaselineReset {
                context: newest,
                generation: BaselineGeneration(1),
            }])
            .unwrap();
        let late = [
            BaselineReset {
                context: new_context,
                generation: BaselineGeneration(1),
            },
            BaselineReset {
                context: optional,
                generation: BaselineGeneration(1),
            },
        ];
        let before = client.retained_bytes();
        assert_eq!(
            client.apply_resets_with_scopes(&late, &scopes),
            Err(ReplicationError::Obsolete(ObsoleteReason::Publication))
        );
        assert_eq!(client.retained_bytes(), before);
        assert_eq!(
            client.identity(context.scope.entity),
            Some((newest, BaselineGeneration(1)))
        );
        assert!(client.identity(optional.scope.entity).is_none());
        let absent = ClientScopes::new(ConnectionEpoch(1), ScopeLimits::default()).unwrap();
        assert_eq!(
            client.apply_resets_with_scopes(&late, &absent),
            Err(ReplicationError::Conflict)
        );
        let mut malformed = late;
        malformed[1].generation = BaselineGeneration(0);
        assert_eq!(
            client.apply_resets_with_scopes(&malformed, &scopes),
            Err(ReplicationError::InvalidIdentity)
        );
        assert_eq!(client.retained_bytes(), before);
    }
}

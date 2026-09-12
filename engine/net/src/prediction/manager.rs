use super::*;
use crate::{
    replication::{Payload, PublishedGroup},
    types::*,
};
use std::collections::{BTreeMap, BTreeSet};

struct HistoryFrame<M: PredictionModel> {
    state: M::State,
    input: Option<ReplayInput<M::Command>>,
    dependencies: DependencyFrame<M::Dependency>,
    events: BTreeSet<ActionKey>,
    bytes: usize,
}
struct GroupHistory<M: PredictionModel> {
    identity: PredictionIdentity,
    frames: BTreeMap<ServerTick, HistoryFrame<M>>,
    last_snapshot: SnapshotId,
    last_checkpoint: ServerTick,
    last_sequence: Option<CommandSeq>,
    dirty_from: Option<ServerTick>,
    recovery: Option<RecoveryReason>,
    bytes: usize,
}

#[derive(Clone, Default)]
struct TopologyFences {
    groups: BTreeMap<GroupId, (GroupRevision, SnapshotId, ServerTick)>,
    members: BTreeMap<u64, (ScopeIdentity, SceneRevision, Option<PredictionBinding>)>,
}
impl TopologyFences {
    fn bytes_for(groups: usize, members: usize) -> Result<usize, PredictionError> {
        groups
            .checked_mul(
                ENTRY_OVERHEAD
                    + std::mem::size_of::<(GroupId, GroupRevision, SnapshotId, ServerTick)>(),
            )
            .and_then(|n| {
                members
                    .checked_mul(
                        ENTRY_OVERHEAD
                            + std::mem::size_of::<(
                                u64,
                                ScopeIdentity,
                                SceneRevision,
                                Option<PredictionBinding>,
                            )>(),
                    )
                    .and_then(|m| n.checked_add(m))
            })
            .ok_or(PredictionError::Capacity)
    }
    fn bytes(&self) -> usize {
        Self::bytes_for(self.groups.len(), self.members.len()).expect("bounded topology metadata")
    }
    fn record(&mut self, identity: &PredictionIdentity, snapshot: SnapshotId, tick: ServerTick) {
        self.groups
            .insert(identity.group, (identity.group_revision, snapshot, tick));
        for scope in &identity.scopes {
            let previous_owner = self.members.get(&scope.entity.index).and_then(|v| v.2);
            let owner = if scope.entity.index == identity.binding.owner.owner.index {
                Some(identity.binding)
            } else {
                previous_owner
            };
            self.members.insert(
                scope.entity.index,
                (*scope, identity.binding.scene_revision, owner),
            );
        }
    }
    fn validate<P: Payload>(
        &self,
        proof: &PublishedGroup<'_, P>,
        binding: PredictionBinding,
    ) -> Result<(), PredictionError> {
        let publication = proof.publication();
        if self
            .groups
            .get(&publication.group)
            .is_some_and(|(revision, snapshot, tick)| {
                publication.revision <= *revision
                    || publication.snapshot <= *snapshot
                    || proof.end_tick() < *tick
            })
        {
            return Err(PredictionError::InvalidIdentity);
        }
        for scope in proof.manifest() {
            if let Some((old, scene, owner)) = self.members.get(&scope.entity.index) {
                if scope.entity.generation < old.entity.generation
                    || scope.scope < old.scope
                    || scope.representation < old.representation
                    || binding.scene_revision < *scene
                {
                    return Err(PredictionError::InvalidIdentity);
                }
                if scope.entity.index == binding.owner.owner.index {
                    if let Some(previous) = owner {
                        if binding.owner.connection != previous.owner.connection
                            || binding.owner.owner.generation < previous.owner.owner.generation
                            || binding.owner.ownership < previous.owner.ownership
                            || binding.owner.stream < previous.owner.stream
                            || binding.teleport_segment < previous.teleport_segment
                        {
                            return Err(PredictionError::InvalidIdentity);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Owns all predicted groups so per-frame work and memory budgets cannot be reset
/// separately by each group. Baseline caches and visible replicas have independent
/// accounting; this manager owns copies of every replay dependency it retains.
pub struct PredictionManager<M: PredictionModel> {
    connection: ConnectionEpoch,
    limits: PredictionLimits,
    groups: BTreeMap<GroupId, GroupHistory<M>>,
    bytes: usize,
    entities: usize,
    frame: Option<ReplicationFrame>,
    work: PredictionWork,
    // Compact high-water metadata survives merge/split; old state/commands do not.
    // IDs are not silently evicted and reused when this bounded table fills.
    topology_fences: TopologyFences,
}
impl<M: PredictionModel> PredictionManager<M> {
    pub fn new(
        connection: ConnectionEpoch,
        limits: PredictionLimits,
    ) -> Result<Self, PredictionError> {
        if connection.0 == 0
            || [
                limits.groups,
                limits.entities_per_group,
                limits.total_entities,
                limits.history_ticks,
                limits.checkpoint_bytes,
                limits.command_bytes,
                limits.dependency_entities,
                limits.dependency_bytes,
                limits.events_per_tick,
                limits.history_bytes,
                limits.replay_ticks_per_frame,
                limits.replay_entity_steps_per_frame,
                limits.prediction_ticks_per_frame,
                limits.prediction_entity_steps_per_frame,
            ]
            .contains(&0)
            || limits.entities_per_group > limits.total_entities
            || limits.history_ticks == usize::MAX
            || limits.checkpoint_bytes > limits.history_bytes
        {
            return Err(PredictionError::InvalidConfiguration);
        }
        Ok(Self {
            connection,
            limits,
            groups: BTreeMap::new(),
            bytes: 0,
            entities: 0,
            frame: None,
            work: PredictionWork::default(),
            topology_fences: TopologyFences::default(),
        })
    }
    pub fn begin_frame(&mut self, frame: ReplicationFrame) -> Result<(), PredictionError> {
        if self.frame.is_some_and(|previous| frame <= previous) {
            return Err(PredictionError::NonMonotonicFrame);
        }
        self.frame = Some(frame);
        self.work = PredictionWork::default();
        Ok(())
    }
    pub fn work(&self) -> PredictionWork {
        self.work
    }
    /// Remaining per-frame prediction work for this group's complete membership.
    /// This is independent of the server's future-command admission horizon.
    pub fn remaining_prediction_ticks(&self, group: GroupId) -> Option<usize> {
        let history = self.groups.get(&group)?;
        if self.frame.is_none() || history.recovery.is_some() {
            return Some(0);
        }
        let ticks = self
            .limits
            .prediction_ticks_per_frame
            .saturating_sub(self.work.predicted_ticks);
        let entities = self
            .limits
            .prediction_entity_steps_per_frame
            .saturating_sub(self.work.predicted_entity_steps);
        Some(
            ticks.min(
                entities
                    .checked_div(history.identity.scopes.len())
                    .unwrap_or(0),
            ),
        )
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }
    pub fn predicted_entities(&self) -> usize {
        self.entities
    }
    pub fn state(&self, group: GroupId) -> Option<&M::State> {
        Some(&self.groups.get(&group)?.frames.last_key_value()?.1.state)
    }
    pub fn predicted_tick(&self, group: GroupId) -> Option<ServerTick> {
        self.groups
            .get(&group)?
            .frames
            .last_key_value()
            .map(|(tick, _)| *tick)
    }
    pub fn identity(&self, group: GroupId) -> Option<&PredictionIdentity> {
        self.groups.get(&group).map(|g| &g.identity)
    }
    pub fn recovery(&self, group: GroupId) -> Option<RecoveryReason> {
        self.groups.get(&group).and_then(|g| g.recovery)
    }
    pub fn history_state(&self, group: GroupId, tick: ServerTick) -> Option<&M::State> {
        Some(&self.groups.get(&group)?.frames.get(&tick)?.state)
    }
    /// Borrow an exact retained command. Substitutes and checkpoints have no
    /// command; use `replay_input_history` to distinguish recorded substitutes.
    /// Callers staging an authorized topology replacement account their copies.
    pub fn command_history(&self, group: GroupId, tick: ServerTick) -> Option<&M::Command> {
        self.replay_input_history(group, tick)?.command()
    }
    /// Borrow the exact transition after a checkpoint. `None` denotes a
    /// checkpoint or unavailable history, never an implicit substitute.
    pub fn replay_input_history(
        &self,
        group: GroupId,
        tick: ServerTick,
    ) -> Option<&ReplayInput<M::Command>> {
        self.groups.get(&group)?.frames.get(&tick)?.input.as_ref()
    }
    pub fn dependency_history(
        &self,
        group: GroupId,
        tick: ServerTick,
    ) -> Option<&DependencyFrame<M::Dependency>> {
        Some(&self.groups.get(&group)?.frames.get(&tick)?.dependencies)
    }
    /// Dependencies from the latest admitted authoritative checkpoint, for new
    /// prediction commands. Existing commands retain their exact historical
    /// dependencies during replay; they must not inherit these newer samples.
    pub fn checkpoint_dependencies(
        &self,
        group: GroupId,
    ) -> Option<&DependencyFrame<M::Dependency>> {
        let history = self.groups.get(&group)?;
        Some(&history.frames.get(&history.last_checkpoint)?.dependencies)
    }

    pub fn initialize(
        &mut self,
        proof: &PublishedGroup<'_, M::Wire>,
        binding: PredictionBinding,
        model: &M,
    ) -> Result<(), PredictionError> {
        let publication = proof.publication();
        if self.groups.contains_key(&publication.group) {
            return Err(PredictionError::AlreadyInitialized);
        }
        self.topology_fences.validate(proof, binding)?;
        if publication.connection != self.connection
            || binding.owner.epoch != self.connection
            || binding.owner.owner.generation == 0
            || binding.owner.ownership.0 == 0
            || binding.owner.stream.0 == 0
            || !proof
                .manifest()
                .iter()
                .any(|scope| scope.entity == binding.owner.owner)
        {
            return Err(PredictionError::InvalidIdentity);
        }
        if self.groups.len() >= self.limits.groups
            || proof.manifest().len() > self.limits.entities_per_group
            || self
                .entities
                .checked_add(proof.manifest().len())
                .is_none_or(|n| n > self.limits.total_entities)
        {
            return Err(PredictionError::Capacity);
        }
        // A predicted entity has one writer, even if several replication groups
        // would otherwise be eligible for delivery on this connection.
        if self.groups.values().any(|group| {
            group.identity.scopes.iter().any(|old| {
                proof
                    .manifest()
                    .iter()
                    .any(|new| old.entity.index == new.entity.index)
            })
        }) {
            return Err(PredictionError::InvalidIdentity);
        }
        self.reserve(
            self.limits
                .checkpoint_bytes
                .checked_add(self.limits.dependency_bytes)
                .ok_or(PredictionError::Capacity)?,
        )?;
        let decoded = model.decode_group(proof, binding)?;
        self.validate_state(&decoded.state, model)?;
        self.validate_dependencies(binding, &decoded.state, None, &decoded.dependencies, model)?;
        let mut frame = HistoryFrame::<M> {
            state: decoded.state,
            input: None,
            dependencies: decoded.dependencies,
            events: BTreeSet::new(),
            bytes: 0,
        };
        frame.bytes = frame_bytes(&frame)?;
        let identity = PredictionIdentity {
            binding,
            group: publication.group,
            group_revision: publication.revision,
            scopes: proof.manifest().iter().copied().collect(),
        };
        let identity_bytes = identity
            .scopes
            .len()
            .checked_mul(ENTRY_OVERHEAD + std::mem::size_of::<ScopeIdentity>())
            .and_then(|n| n.checked_add(std::mem::size_of::<GroupHistory<M>>() + ENTRY_OVERHEAD))
            .ok_or(PredictionError::Capacity)?;
        let bytes = identity_bytes
            .checked_add(frame.bytes)
            .ok_or(PredictionError::Capacity)?;
        self.reserve(bytes)?;
        self.groups.insert(
            publication.group,
            GroupHistory {
                identity,
                frames: BTreeMap::from([(proof.end_tick(), frame)]),
                last_snapshot: publication.snapshot,
                last_checkpoint: proof.end_tick(),
                last_sequence: None,
                dirty_from: None,
                recovery: None,
                bytes,
            },
        );
        self.bytes += bytes;
        self.entities += proof.manifest().len();
        Ok(())
    }

    /// Atomically retire exactly these identities and rebase their replacements
    /// from complete authority publications at one committed tick. No commands or
    /// dependency samples migrate across incompatible group topology. Old groups
    /// remain byte-for-byte intact on failure, including recovery flags; staged
    /// decoded state coexists with them under the aggregate memory budget.
    ///
    /// All manifest members are writable for this manager. Read-only dependency
    /// closure must be supplied as dependency records, not duplicate membership.
    /// Presentation remains owned by the connection's existing EventJournal;
    /// callers must retain it across this operation and apply speculative absence
    /// there. Returned keys are cancellation candidates only: the journal retains
    /// confirmed outcomes and one-shot delivery receipts, including after K.
    pub fn replace_groups(
        &mut self,
        retired: &[PredictionIdentity],
        replacements: &[(&PublishedGroup<'_, M::Wire>, PredictionBinding)],
        model: &M,
    ) -> Result<AuthoritativeRebase, PredictionError> {
        self.require_frame()?;
        if retired.is_empty()
            || replacements.is_empty()
            || retired.len() > self.limits.groups
            || replacements.len() > self.limits.groups
        {
            return Err(PredictionError::Capacity);
        }
        let metadata_bytes = retired
            .len()
            .checked_mul(ENTRY_OVERHEAD + std::mem::size_of::<GroupId>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .ok_or(PredictionError::Capacity)?;
        self.reserve(metadata_bytes)?;
        let mut retiring = BTreeSet::new();
        for identity in retired {
            if !retiring.insert(identity.group) || self.identity(identity.group) != Some(identity) {
                return Err(PredictionError::InvalidIdentity);
            }
        }
        let tick = replacements[0].0.end_tick();
        if self.groups.len() - retired.len() + replacements.len() > self.limits.groups {
            return Err(PredictionError::Capacity);
        }
        let removed_entities: usize = retired.iter().map(|id| id.scopes.len()).sum();
        let added_entities = replacements.iter().try_fold(0usize, |count, (proof, _)| {
            count
                .checked_add(proof.manifest().len())
                .ok_or(PredictionError::Capacity)
        })?;
        if (self.entities - removed_entities)
            .checked_add(added_entities)
            .is_none_or(|count| count > self.limits.total_entities)
        {
            return Err(PredictionError::Capacity);
        }
        let fence_groups = self
            .topology_fences
            .groups
            .len()
            .checked_add(retired.len())
            .and_then(|n| n.checked_add(replacements.len()))
            .ok_or(PredictionError::Capacity)?;
        let fence_members = self
            .topology_fences
            .members
            .len()
            .checked_add(removed_entities)
            .and_then(|n| n.checked_add(added_entities))
            .ok_or(PredictionError::Capacity)?;
        let fence_scratch = TopologyFences::bytes_for(fence_groups, fence_members)?;
        self.reserve(
            metadata_bytes
                .checked_add(fence_scratch)
                .ok_or(PredictionError::Capacity)?,
        )?;
        let mut fences = self.topology_fences.clone();
        for old in retired {
            let history = &self.groups[&old.group];
            fences.record(old, history.last_snapshot, history.last_checkpoint);
        }
        for (proof, binding) in replacements {
            fences.validate(proof, *binding)?;
            let publication = proof.publication();
            if proof.end_tick() != tick {
                return Err(PredictionError::WrongTick);
            }
            if self.groups.contains_key(&publication.group)
                && !retiring.contains(&publication.group)
            {
                return Err(PredictionError::InvalidIdentity);
            }
            for old in retired {
                let history = &self.groups[&old.group];
                if tick < history.last_checkpoint {
                    return Err(PredictionError::WrongTick);
                }
                if binding.scene_revision < old.binding.scene_revision
                    || (publication.group == old.group
                        && (publication.revision <= old.group_revision
                            || publication.snapshot <= history.last_snapshot))
                {
                    return Err(PredictionError::InvalidIdentity);
                }
                if binding.owner.owner.index == old.binding.owner.owner.index
                    && (binding.owner.connection != old.binding.owner.connection
                        || binding.owner.owner.generation < old.binding.owner.owner.generation
                        || binding.owner.ownership < old.binding.owner.ownership
                        || binding.owner.stream < old.binding.owner.stream
                        || binding.teleport_segment < old.binding.teleport_segment)
                {
                    return Err(PredictionError::InvalidIdentity);
                }
                for new in proof.manifest() {
                    if old.scopes.iter().any(|previous| {
                        previous.entity.index == new.entity.index
                            && (new.entity.generation < previous.entity.generation
                                || new.scope < previous.scope
                                || new.representation < previous.representation)
                    }) {
                        return Err(PredictionError::InvalidIdentity);
                    }
                }
            }
            if self
                .groups
                .iter()
                .filter(|(id, _)| !retiring.contains(id))
                .any(|(_, history)| {
                    history.identity.scopes.iter().any(|old| {
                        proof
                            .manifest()
                            .iter()
                            .any(|new| old.entity.index == new.entity.index)
                    })
                })
            {
                return Err(PredictionError::InvalidIdentity);
            }
        }
        // Reuse ordinary complete-proof decode and validation without borrowing
        // any old mutable group. Seeding bytes charges old + all staged copies;
        // this staging manager never begins a frame or executes simulation work.
        let mut staged = Self::new(self.connection, self.limits)?;
        staged.bytes = self.bytes + metadata_bytes + fence_scratch;
        for (proof, binding) in replacements {
            staged.initialize(proof, *binding, model)?;
        }
        for history in staged.groups.values() {
            fences.record(
                &history.identity,
                history.last_snapshot,
                history.last_checkpoint,
            );
        }
        if self
            .limits
            .groups
            .checked_mul(self.limits.history_ticks + 1)
            .is_none_or(|cap| fences.groups.len() > cap)
            || self
                .limits
                .total_entities
                .checked_mul(self.limits.history_ticks + 1)
                .is_none_or(|cap| fences.members.len() > cap)
        {
            return Err(PredictionError::Capacity);
        }
        let event_count = retired.iter().try_fold(0usize, |count, identity| {
            self.groups[&identity.group]
                .frames
                .range((std::ops::Bound::Excluded(tick), std::ops::Bound::Unbounded))
                .try_fold(count, |count, (_, frame)| {
                    count
                        .checked_add(frame.events.len())
                        .ok_or(PredictionError::Capacity)
                })
        })?;
        staged.reserve(
            event_count
                .checked_mul(ENTRY_OVERHEAD + std::mem::size_of::<ActionKey>())
                .ok_or(PredictionError::Capacity)?,
        )?;
        let canceled = retired
            .iter()
            .flat_map(|identity| {
                self.groups[&identity.group]
                    .frames
                    .range((std::ops::Bound::Excluded(tick), std::ops::Bound::Unbounded))
                    .flat_map(|(_, frame)| frame.events.iter().copied())
            })
            .filter(|event| {
                !self.groups.iter().any(|(id, history)| {
                    history.frames.iter().any(|(at, frame)| {
                        (!retiring.contains(id) || *at <= tick) && frame.events.contains(event)
                    })
                })
            })
            .collect();
        let staged_bytes = staged.bytes - self.bytes - metadata_bytes - fence_scratch;
        for identity in retired {
            self.remove_group(identity.group);
        }
        self.groups.append(&mut staged.groups);
        self.bytes = self.bytes - self.topology_fences.bytes() + staged_bytes + fences.bytes();
        self.topology_fences = fences;
        self.entities += added_entities;
        Ok(AuthoritativeRebase {
            tick,
            events: EventChanges {
                started: BTreeSet::new(),
                canceled,
            },
        })
    }

    /// Explicit lifecycle removal drops history only when its owner no longer needs
    /// replay. A mere visible-replica scope exit must not call this method.
    pub fn remove_group(&mut self, group: GroupId) -> bool {
        if let Some(old) = self.groups.remove(&group) {
            self.bytes -= old.bytes;
            self.entities -= old.identity.scopes.len();
            true
        } else {
            false
        }
    }

    pub fn predict(
        &mut self,
        group: GroupId,
        command: M::Command,
        dependencies: DependencyFrame<M::Dependency>,
        model: &M,
    ) -> Result<EventChanges, PredictionError> {
        let (_, _, target) = model.command_identity(&command);
        self.predict_input(
            group,
            ServerTick(target.0),
            ReplayInput::Command(command),
            dependencies,
            model,
        )
    }

    /// Record an explicit transition without authoring a command. Substitutes
    /// consume ordinary prediction/history budgets and retain replay dependencies
    /// but do not advance the last actual command sequence.
    pub fn predict_substitute(
        &mut self,
        group: GroupId,
        target: ServerTick,
        dependencies: DependencyFrame<M::Dependency>,
        model: &M,
    ) -> Result<EventChanges, PredictionError> {
        self.predict_input(group, target, ReplayInput::Substitute, dependencies, model)
    }

    fn predict_input(
        &mut self,
        group: GroupId,
        target: ServerTick,
        input: ReplayInput<M::Command>,
        dependencies: DependencyFrame<M::Dependency>,
        model: &M,
    ) -> Result<EventChanges, PredictionError> {
        self.with_group(group, |manager, history| {
            manager.require_frame()?;
            active(history)?;
            let sequence = if let Some(command) = input.command() {
                let (owner, sequence, command_target) = model.command_identity(command);
                if owner != history.identity.binding.owner
                    || sequence.0 == 0
                    || command_target.0 != target.0
                    || !model.valid_command(command)
                {
                    return Err(PredictionError::InvalidCommand);
                }
                Some(sequence)
            } else {
                None
            };
            if let Some(old) = history
                .frames
                .get(&target)
                .and_then(|frame| frame.input.as_ref())
            {
                return if old == &input {
                    Ok(EventChanges::default())
                } else {
                    Err(PredictionError::Equivocation)
                };
            }
            let (&current, previous) = history.frames.last_key_value().expect("initialized group");
            if current.checked_next() != Some(target)
                || sequence.is_some_and(|sequence| {
                    history.last_sequence.is_some_and(|old| sequence <= old)
                })
            {
                return Err(PredictionError::WrongTick);
            }
            let command_bytes = input.command().map(payload_bytes).transpose()?.unwrap_or(0);
            if command_bytes > manager.limits.command_bytes {
                return Err(PredictionError::Capacity);
            }
            if let Err(error) = manager.validate_dependencies(
                history.identity.binding,
                &previous.state,
                input.command(),
                &dependencies,
                model,
            ) {
                return recover(history, reason(error, RecoveryReason::MissingDependency));
            }
            let members = history.identity.scopes.len();
            if manager.work.predicted_ticks >= manager.limits.prediction_ticks_per_frame
                || manager
                    .work
                    .predicted_entity_steps
                    .checked_add(members)
                    .is_none_or(|n| n > manager.limits.prediction_entity_steps_per_frame)
            {
                return recover(history, RecoveryReason::ReplayBudget);
            }
            let scratch = manager
                .limits
                .checkpoint_bytes
                .checked_add(dependency_bytes(&dependencies)?)
                .and_then(|n| n.checked_add(command_bytes))
                .and_then(|n| n.checked_add(manager.event_scratch(2).ok()?))
                .and_then(|n| {
                    n.checked_add(std::mem::size_of::<HistoryFrame<M>>() + ENTRY_OVERHEAD)
                })
                .ok_or(PredictionError::Capacity)?;
            if manager.reserve(scratch).is_err() {
                return recover(history, RecoveryReason::MemoryBudget);
            }
            manager.work.predicted_ticks += 1;
            manager.work.predicted_entity_steps += members;
            let mut staging = previous.state.clone();
            let events = match step_input(model, &mut staging, target, &input, &dependencies) {
                Ok(events) => events,
                Err(error) => {
                    return recover(history, reason(error, RecoveryReason::InvalidState));
                }
            };
            if manager.validate_state(&staging, model).is_err()
                || !manager.valid_events(history.identity.binding.owner.epoch, &events)
            {
                return recover(history, RecoveryReason::InvalidState);
            }
            let started = events
                .iter()
                .filter(|event| {
                    !history
                        .frames
                        .values()
                        .any(|frame| frame.events.contains(event))
                })
                .copied()
                .collect();
            let mut frame = HistoryFrame::<M> {
                state: staging,
                input: Some(input),
                dependencies,
                events,
                bytes: 0,
            };
            frame.bytes = frame_bytes(&frame)?;
            if manager.reserve(frame.bytes).is_err() {
                return recover(history, RecoveryReason::MemoryBudget);
            }
            history.bytes += frame.bytes;
            history.frames.insert(target, frame);
            if let Some(sequence) = sequence {
                history.last_sequence = Some(sequence);
            }
            while history.frames.len() > manager.limits.history_ticks + 1 {
                let (_, removed) = history.frames.pop_first().expect("history exceeds bound");
                history.bytes -= removed.bytes;
            }
            Ok(EventChanges {
                started,
                canceled: BTreeSet::new(),
            })
        })
    }

    /// Replace a decoded, ticked dependency sample without immediately mutating the
    /// published predicted world. Reconciliation must replay even if owner pose at
    /// the checkpoint still compares equal. Historical scope identities cannot be
    /// replaced by a new incarnation masquerading as an old sample.
    pub fn update_dependencies(
        &mut self,
        group: GroupId,
        tick: ServerTick,
        dependencies: DependencyFrame<M::Dependency>,
    ) -> Result<(), PredictionError> {
        self.with_group(group, |manager, history| {
            active(history)?;
            manager.validate_dependency_records(history.identity.binding, &dependencies)?;
            let old = history
                .frames
                .get(&tick)
                .ok_or(PredictionError::ResyncRequired(
                    RecoveryReason::MissingHistory,
                ))?;
            for (entity, record) in &dependencies.records {
                if let Some(previous) = old.dependencies.records.get(entity) {
                    if record.scope != previous.scope
                        || record.scene_revision != previous.scene_revision
                    {
                        return Err(PredictionError::InvalidDependency);
                    }
                    if record.version < previous.version {
                        return Err(PredictionError::StaleDependency);
                    }
                    if record.version == previous.version && record != previous {
                        return Err(PredictionError::Equivocation);
                    }
                }
            }
            if dependencies == old.dependencies {
                return Ok(());
            }
            manager.reserve(dependency_bytes(&dependencies)?)?;
            let old_size = old.bytes;
            let new_size =
                frame_parts_bytes::<M>(&old.state, old.input.as_ref(), &dependencies, &old.events)?;
            let frame = history
                .frames
                .get_mut(&tick)
                .expect("checked history frame");
            frame.dependencies = dependencies;
            frame.bytes = new_size;
            history.bytes = history.bytes - old_size + new_size;
            history.dirty_from = Some(history.dirty_from.map_or(tick, |old| old.min(tick)));
            Ok(())
        })
    }

    pub fn reconcile(
        &mut self,
        proof: &PublishedGroup<'_, M::Wire>,
        binding: PredictionBinding,
        finalized_through: ServerTick,
        model: &M,
    ) -> Result<Correction, PredictionError> {
        let publication = proof.publication();
        if publication.connection != self.connection {
            return Err(PredictionError::InvalidIdentity);
        }
        self.with_group(publication.group, |manager, history| {
            manager.require_frame()?;
            active(history)?;
            if publication.revision < history.identity.group_revision
                || (publication.revision == history.identity.group_revision
                    && publication.snapshot <= history.last_snapshot)
                || proof.end_tick() < history.last_checkpoint
            {
                return Ok(Correction::Stale);
            }
            if publication.revision != history.identity.group_revision
                || binding != history.identity.binding
                || proof.manifest().iter().copied().collect::<BTreeSet<_>>()
                    != history.identity.scopes
            {
                return recover(history, RecoveryReason::IdentityChanged);
            }
            let checkpoint_tick = proof.end_tick();
            let predicted_tick = *history
                .frames
                .last_key_value()
                .expect("initialized history")
                .0;
            if checkpoint_tick > predicted_tick {
                return recover(history, RecoveryReason::CheckpointAhead);
            }
            if finalized_through < checkpoint_tick {
                return Err(PredictionError::InvalidCommand);
            }
            let Some(at_checkpoint) = history.frames.get(&checkpoint_tick) else {
                return recover(history, RecoveryReason::MissingHistory);
            };
            if manager
                .reserve(
                    manager
                        .limits
                        .checkpoint_bytes
                        .checked_add(manager.limits.dependency_bytes)
                        .ok_or(PredictionError::Capacity)?,
                )
                .is_err()
            {
                return recover(history, RecoveryReason::MemoryBudget);
            }
            let decoded = match model.decode_group(proof, binding) {
                Ok(value) => value,
                Err(error) => return recover(history, reason(error, RecoveryReason::InvalidState)),
            };
            if manager.validate_state(&decoded.state, model).is_err() {
                return recover(history, RecoveryReason::InvalidState);
            }
            if let Err(error) = manager.validate_dependencies(
                binding,
                &decoded.state,
                None,
                &decoded.dependencies,
                model,
            ) {
                return recover(history, reason(error, RecoveryReason::MissingDependency));
            }
            let needs_replay = decoded.state != at_checkpoint.state
                || decoded.dependencies != at_checkpoint.dependencies
                || history.dirty_from.is_some();
            if !needs_replay {
                // Retain authoritative K, and retire only commands reflected in K.
                // A later receipt watermark never deletes commands K+1..P.
                let new_bytes = frame_parts_bytes::<M>(
                    &decoded.state,
                    None,
                    &decoded.dependencies,
                    &BTreeSet::new(),
                )?;
                let frame = history
                    .frames
                    .get_mut(&checkpoint_tick)
                    .expect("checked frame");
                let old_bytes = frame.bytes;
                frame.state = decoded.state;
                frame.dependencies = decoded.dependencies;
                frame.input = None;
                frame.events.clear();
                frame.bytes = new_bytes;
                history.bytes = history.bytes - old_bytes + frame.bytes;
                retire_before(history, checkpoint_tick);
                history.last_snapshot = publication.snapshot;
                history.last_checkpoint = checkpoint_tick;
                return Ok(Correction::Equal);
            }
            let replay_ticks = usize::try_from(predicted_tick.0 - checkpoint_tick.0)
                .map_err(|_| PredictionError::Capacity)?;
            let steps = replay_ticks
                .checked_mul(history.identity.scopes.len())
                .ok_or(PredictionError::Capacity)?;
            if manager
                .work
                .replay_ticks
                .checked_add(replay_ticks)
                .is_none_or(|n| n > manager.limits.replay_ticks_per_frame)
                || manager
                    .work
                    .replay_entity_steps
                    .checked_add(steps)
                    .is_none_or(|n| n > manager.limits.replay_entity_steps_per_frame)
            {
                return recover(history, RecoveryReason::ReplayBudget);
            }
            let per_frame = manager
                .limits
                .checkpoint_bytes
                .checked_add(manager.event_scratch(5)?)
                .and_then(|n| n.checked_add(ENTRY_OVERHEAD))
                .ok_or(PredictionError::Capacity)?;
            let scratch = (replay_ticks + 1)
                .checked_mul(per_frame)
                .and_then(|n| n.checked_add(manager.limits.dependency_bytes))
                .ok_or(PredictionError::Capacity)?;
            if manager.reserve(scratch).is_err() {
                return recover(history, RecoveryReason::MemoryBudget);
            }
            // Charge planned work before stepping: a failed kernel cannot repeatedly
            // consume unaccounted replay work in the same rendered frame.
            manager.work.replay_ticks += replay_ticks;
            manager.work.replay_entity_steps += steps;
            let mut staged = Vec::with_capacity(replay_ticks + 1);
            let checkpoint_bytes = frame_parts_bytes::<M>(
                &decoded.state,
                None,
                &decoded.dependencies,
                &BTreeSet::new(),
            )?;
            staged.push((
                checkpoint_tick,
                decoded.state,
                BTreeSet::new(),
                checkpoint_bytes,
            ));
            for offset in 1..=replay_ticks {
                let tick = ServerTick(checkpoint_tick.0 + offset as u64);
                let Some(frame) = history.frames.get(&tick) else {
                    return recover(history, RecoveryReason::MissingHistory);
                };
                let Some(input) = &frame.input else {
                    return recover(history, RecoveryReason::MissingHistory);
                };
                let previous = &staged.last().expect("checkpoint staged").1;
                if let Err(error) = manager.validate_dependencies(
                    binding,
                    previous,
                    input.command(),
                    &frame.dependencies,
                    model,
                ) {
                    return recover(history, reason(error, RecoveryReason::MissingDependency));
                }
                let mut state = previous.clone();
                let events = match step_input(model, &mut state, tick, input, &frame.dependencies) {
                    Ok(events) => events,
                    Err(error) => {
                        return recover(history, reason(error, RecoveryReason::InvalidState));
                    }
                };
                if manager.validate_state(&state, model).is_err()
                    || !manager.valid_events(binding.owner.epoch, &events)
                {
                    return recover(history, RecoveryReason::InvalidState);
                }
                let bytes = frame_parts_bytes::<M>(
                    &state,
                    frame.input.as_ref(),
                    &frame.dependencies,
                    &events,
                )?;
                staged.push((tick, state, events, bytes));
            }
            let old_events: BTreeSet<_> = history
                .frames
                .range((
                    std::ops::Bound::Excluded(checkpoint_tick),
                    std::ops::Bound::Included(predicted_tick),
                ))
                .flat_map(|(_, frame)| frame.events.iter().copied())
                .collect();
            let new_events: BTreeSet<_> = staged
                .iter()
                .skip(1)
                .flat_map(|(_, _, events, _)| events.iter().copied())
                .collect();
            let changes = EventChanges {
                started: new_events.difference(&old_events).copied().collect(),
                canceled: old_events.difference(&new_events).copied().collect(),
            };
            // No published state has changed before this point. Commit the complete
            // correction at P; commands and pinned dependency samples stay immutable.
            for (tick, state, events, bytes) in staged {
                let frame = history
                    .frames
                    .get_mut(&tick)
                    .expect("validated contiguous replay");
                let old_bytes = frame.bytes;
                frame.state = state;
                frame.events = events;
                if tick == checkpoint_tick {
                    frame.input = None;
                }
                frame.bytes = bytes;
                history.bytes = history.bytes - old_bytes + bytes;
            }
            history
                .frames
                .get_mut(&checkpoint_tick)
                .expect("checkpoint retained")
                .dependencies = decoded.dependencies;
            retire_before(history, checkpoint_tick);
            history.last_snapshot = publication.snapshot;
            history.last_checkpoint = checkpoint_tick;
            history.dirty_from = None;
            if replay_ticks == 0 {
                Ok(Correction::Replaced)
            } else {
                Ok(Correction::Replayed {
                    from: ServerTick(checkpoint_tick.0 + 1),
                    through: predicted_tick,
                    events: changes,
                })
            }
        })
    }

    /// Hard entitlement revocation stops affected prediction immediately. It does
    /// not claim to erase already received data; history remains bounded until the
    /// explicit resynchronization lifecycle replaces/removes this group.
    pub fn revoke_scope(&mut self, scope: ScopeIdentity) {
        for history in self.groups.values_mut() {
            if history.identity.scopes.contains(&scope)
                || history.frames.values().any(|frame| {
                    frame
                        .dependencies
                        .records
                        .values()
                        .any(|record| record.scope == scope)
                })
            {
                history.recovery = Some(RecoveryReason::DisclosureRevoked);
            }
        }
    }

    fn with_group<R>(
        &mut self,
        group: GroupId,
        apply: impl FnOnce(&mut Self, &mut GroupHistory<M>) -> Result<R, PredictionError>,
    ) -> Result<R, PredictionError> {
        let mut history = self
            .groups
            .remove(&group)
            .ok_or(PredictionError::UnknownGroup)?;
        let before = history.bytes;
        let result = apply(self, &mut history);
        self.bytes = self.bytes - before + history.bytes;
        self.groups.insert(group, history);
        result
    }
    fn require_frame(&self) -> Result<(), PredictionError> {
        self.frame.map_or(Err(PredictionError::NoFrame), |_| Ok(()))
    }
    fn reserve(&self, bytes: usize) -> Result<(), PredictionError> {
        if self
            .bytes
            .checked_add(bytes)
            .is_none_or(|n| n > self.limits.history_bytes)
        {
            Err(PredictionError::Capacity)
        } else {
            Ok(())
        }
    }
    fn event_scratch(&self, copies: usize) -> Result<usize, PredictionError> {
        self.limits
            .events_per_tick
            .checked_mul(std::mem::size_of::<ActionKey>() + ENTRY_OVERHEAD)
            .and_then(|n| n.checked_mul(copies))
            .ok_or(PredictionError::Capacity)
    }
    fn valid_events(&self, connection: ConnectionEpoch, events: &BTreeSet<ActionKey>) -> bool {
        events.len() <= self.limits.events_per_tick
            && events.iter().all(|event| {
                event.connection == connection && event.stream.0 != 0 && event.command.0 != 0
            })
    }
    fn validate_state(&self, state: &M::State, model: &M) -> Result<(), PredictionError> {
        if !model.valid_state(state) || payload_bytes(state)? > self.limits.checkpoint_bytes {
            Err(PredictionError::InvalidState)
        } else {
            Ok(())
        }
    }
    fn validate_dependency_records(
        &self,
        binding: PredictionBinding,
        frame: &DependencyFrame<M::Dependency>,
    ) -> Result<(), PredictionError> {
        if frame.records.len() > self.limits.dependency_entities
            || dependency_bytes(frame)? > self.limits.dependency_bytes
        {
            return Err(PredictionError::Capacity);
        }
        for (entity, record) in &frame.records {
            if record.scope.entity != *entity
                || record.scope.connection != self.connection
                || entity.generation == 0
                || record.scope.scope.0 == 0
                || record.scope.representation.0 == 0
                || record.version.0 == 0
                || record.scene_revision != binding.scene_revision
            {
                return Err(PredictionError::InvalidDependency);
            }
        }
        Ok(())
    }
    fn validate_dependencies(
        &self,
        binding: PredictionBinding,
        state: &M::State,
        command: Option<&M::Command>,
        frame: &DependencyFrame<M::Dependency>,
        model: &M,
    ) -> Result<(), PredictionError> {
        self.validate_dependency_records(binding, frame)?;
        let required = model.required_dependencies(state, command);
        if required.len() > self.limits.dependency_entities {
            return Err(PredictionError::Capacity);
        }
        let mut seen = BTreeSet::new();
        for entity in required {
            if !seen.insert(entity) {
                return Err(PredictionError::InvalidDependency);
            }
            if frame
                .value(entity)
                .is_none_or(|value| matches!(value, DependencyValue::Unavailable))
            {
                return Err(PredictionError::ResyncRequired(
                    RecoveryReason::MissingDependency,
                ));
            }
        }
        Ok(())
    }
}
fn active<M: PredictionModel>(history: &GroupHistory<M>) -> Result<(), PredictionError> {
    history.recovery.map_or(Ok(()), |reason| {
        Err(PredictionError::ResyncRequired(reason))
    })
}
fn recover<M: PredictionModel, R>(
    history: &mut GroupHistory<M>,
    reason: RecoveryReason,
) -> Result<R, PredictionError> {
    history.recovery = Some(reason);
    Err(PredictionError::ResyncRequired(reason))
}
fn reason(error: PredictionError, fallback: RecoveryReason) -> RecoveryReason {
    match error {
        PredictionError::ResyncRequired(reason) => reason,
        PredictionError::Capacity => RecoveryReason::MemoryBudget,
        _ => fallback,
    }
}
fn retire_before<M: PredictionModel>(history: &mut GroupHistory<M>, tick: ServerTick) {
    while history
        .frames
        .first_key_value()
        .is_some_and(|(old, _)| *old < tick)
    {
        let (_, frame) = history.frames.pop_first().expect("checked old history");
        history.bytes -= frame.bytes;
    }
}
fn frame_bytes<M: PredictionModel>(frame: &HistoryFrame<M>) -> Result<usize, PredictionError> {
    frame_parts_bytes::<M>(
        &frame.state,
        frame.input.as_ref(),
        &frame.dependencies,
        &frame.events,
    )
}
fn frame_parts_bytes<M: PredictionModel>(
    state: &M::State,
    input: Option<&ReplayInput<M::Command>>,
    dependencies: &DependencyFrame<M::Dependency>,
    events: &BTreeSet<ActionKey>,
) -> Result<usize, PredictionError> {
    (std::mem::size_of::<HistoryFrame<M>>() + ENTRY_OVERHEAD)
        .checked_add(state.retained_bytes())
        .and_then(|n| {
            n.checked_add(
                input
                    .and_then(ReplayInput::command)
                    .map_or(0, Payload::retained_bytes),
            )
        })
        .and_then(|n| n.checked_add(dependency_bytes(dependencies).ok()?))
        .and_then(|n| n.checked_add(event_bytes(events).ok()?))
        .ok_or(PredictionError::Capacity)
}

fn step_input<M: PredictionModel>(
    model: &M,
    state: &mut M::State,
    tick: ServerTick,
    input: &ReplayInput<M::Command>,
    dependencies: &DependencyFrame<M::Dependency>,
) -> Result<BTreeSet<ActionKey>, PredictionError> {
    match input {
        ReplayInput::Command(command) => model.step(state, tick, command, dependencies),
        ReplayInput::Substitute => model.step_substitute(state, tick, dependencies),
    }
}

//! Pure restricted-owner adapter. Replica bytes never become a full simulation.
use dreamwake_protocol::{
    DreamAction,
    live::{TickInput, Welcome},
    replication::{ReplicaPayload, decode_replica},
};
use dreamwake_sim::replication::{OwnerExpectation, OwnerPredictionState};
use engine_net::{
    commands::{Command, OwnerStream},
    prediction::*,
    replication::{FullState, Payload, PublishedGroup},
    types::*,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OwnerState(
    pub OwnerPredictionState,
    pub BTreeMap<engine_core::ColliderKey, EntityId>,
);
impl Payload for OwnerState {
    fn retained_bytes(&self) -> usize {
        // Owner schema bounds all owned vectors/strings. Charge its entire allowed
        // decoded allocation envelope rather than only the current JSON length.
        48 * 1024
    }
}
#[derive(Clone, PartialEq)]
pub(super) struct InputCommand(pub Command<TickInput, DreamAction>);
impl Payload for InputCommand {
    fn retained_bytes(&self) -> usize {
        1024
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(super) enum CollisionDependency {
    StaticCollision(dreamwake_sim::collision::CollisionWorld),
    Platform(dreamwake_sim::replication::PublicPlatformView),
}
impl Payload for CollisionDependency {
    fn retained_bytes(&self) -> usize {
        match self {
            Self::StaticCollision(world) => world.retained_bytes(),
            Self::Platform(_) => 1024,
        }
    }
}
// 64 permitted colliders, conservative prepared geometry envelope plus the
// dependency record/map overhead. This is independent of wire group size.
pub(super) const COLLISION_HISTORY_BYTES: usize = 4096 + 64 * 1024 + 2048;

pub(super) fn validate_owner_group<'a>(
    model: &OwnerModel,
    group: GroupId,
    end_tick: ServerTick,
    states: impl IntoIterator<Item = &'a FullState<Vec<u8>>>,
) -> Result<DecodedPrediction<OwnerState, CollisionDependency>, PredictionError> {
    if group != dreamwake_protocol::live::OWNER_GROUP {
        return Err(PredictionError::InvalidIdentity);
    }
    let mut owner = None;
    let mut global = None;
    let mut collision = None;
    let mut count = 0;
    let mut platform = None;
    for state in states {
        count += 1;
        if state.end_tick != end_tick || state.scope.connection != model.welcome.stream.epoch {
            return Err(PredictionError::InvalidState);
        }
        match decode_replica(&state.payload, Some(model.expectation()))
            .map_err(|_| PredictionError::InvalidState)?
        {
            ReplicaPayload::Owner(value)
                if state.scope.entity == model.welcome.owner_entity && owner.is_none() =>
            {
                owner = Some(value)
            }
            ReplicaPayload::Global(value)
                if state.scope.entity == model.welcome.global_entity && global.is_none() =>
            {
                global = Some(value)
            }
            ReplicaPayload::Collision(value)
                if state.scope.entity == model.welcome.collision_entity && collision.is_none() =>
            {
                collision = Some((state.scope, state.version, value))
            }
            ReplicaPayload::Actor(dreamwake_sim::replication::PublicReplica::Platform(value))
                if platform.is_none()
                    && state.scope.entity != model.welcome.owner_entity
                    && state.scope.entity != model.welcome.global_entity
                    && state.scope.entity != model.welcome.collision_entity =>
            {
                platform = Some((state.scope, state.version, value))
            }
            _ => return Err(PredictionError::InvalidIdentity),
        }
    }
    if !dreamwake_protocol::live::baselines::valid_member_count(count) {
        return Err(PredictionError::InvalidIdentity);
    }
    let owner = owner.ok_or(PredictionError::InvalidState)?;
    let global = global.ok_or(PredictionError::InvalidState)?;
    let (scope, version, manifest) = collision.ok_or(PredictionError::InvalidState)?;
    if owner.context() != &global
        || global.stamp.server_tick != end_tick.0
        || manifest.scene_revision() != u64::from(model.welcome.content.scene_revision.0)
        || owner.collision_identity() != manifest.identity()
        || manifest.identity() != model.welcome.content.scene
    {
        return Err(PredictionError::InvalidState);
    }
    let state = OwnerPredictionState::from_checkpoint(&owner, model.expectation())
        .map_err(|_| PredictionError::InvalidState)?;
    let world = manifest
        .build()
        .map_err(|_| PredictionError::InvalidState)?;
    let required = owner.required_bases();
    if count != 3 + required.len() || required.len() > 1 {
        return Err(PredictionError::InvalidIdentity);
    }
    let validation_bases: Vec<_> = platform
        .as_ref()
        .map(|(_, _, view)| view.clone())
        .into_iter()
        .collect();
    state
        .validate_collision_with_bases(&world, &validation_bases)
        .map_err(|_| PredictionError::InvalidState)?;
    let mut bases = BTreeMap::new();
    let mut records = BTreeMap::from([(
        scope.entity,
        DependencyRecord {
            scope,
            version,
            scene_revision: model.welcome.content.scene_revision,
            value: DependencyValue::Known(CollisionDependency::StaticCollision(world)),
        },
    )]);
    if let Some((scope, version, view)) = platform {
        if required != [view.collider]
            || view.scene_revision != global.stamp.scene_revision
            || view.gameplay_tick != global.stamp.gameplay_tick
        {
            return Err(PredictionError::InvalidIdentity);
        }
        bases.insert(view.collider, scope.entity);
        records.insert(
            scope.entity,
            DependencyRecord {
                scope,
                version,
                scene_revision: model.welcome.content.scene_revision,
                value: DependencyValue::Known(CollisionDependency::Platform(view)),
            },
        );
    } else if !required.is_empty() {
        return Err(PredictionError::InvalidState);
    }
    Ok(DecodedPrediction {
        state: OwnerState(state, bases),
        dependencies: DependencyFrame { records },
    })
}
pub(super) struct OwnerModel {
    pub welcome: Welcome,
}
impl OwnerModel {
    pub fn expectation(&self) -> OwnerExpectation {
        OwnerExpectation {
            owner: self.welcome.player.get(),
            match_epoch: self.welcome.match_epoch,
            ownership_revision: self.welcome.stream.ownership.0,
            scene_revision: u64::from(self.welcome.content.scene_revision.0),
            minimum_revision: 0,
        }
    }
    pub fn binding(&self) -> PredictionBinding {
        PredictionBinding {
            owner: self.welcome.stream,
            scene_revision: self.welcome.content.scene_revision,
            teleport_segment: 0,
        }
    }
}
impl PredictionModel for OwnerModel {
    type Wire = Vec<u8>;
    type State = OwnerState;
    type Command = InputCommand;
    type Dependency = CollisionDependency;
    fn decode_group(
        &self,
        group: &PublishedGroup<'_, Vec<u8>>,
        binding: PredictionBinding,
    ) -> Result<DecodedPrediction<OwnerState, CollisionDependency>, PredictionError> {
        if binding != self.binding()
            || !dreamwake_protocol::live::baselines::valid_member_count(group.manifest().len())
        {
            return Err(PredictionError::InvalidIdentity);
        }
        validate_owner_group(
            self,
            group.publication().group,
            group.end_tick(),
            group.states(),
        )
    }
    fn valid_state(&self, state: &OwnerState) -> bool {
        state.1.len() <= 1
            && state
                .1
                .keys()
                .copied()
                .eq(state.0.required_bases().iter().copied())
            && state
                .0
                .checkpoint()
                .validate_for(self.expectation())
                .is_ok()
    }
    fn command_identity(&self, command: &InputCommand) -> (OwnerStream, CommandSeq, TargetTick) {
        (command.0.owner, command.0.sequence, command.0.target)
    }
    fn valid_command(&self, command: &InputCommand) -> bool {
        dreamwake_protocol::live::valid_tick_input(&command.0.input)
            && command
                .0
                .actions
                .as_slice()
                .iter()
                .all(|e| dreamwake_protocol::live::valid_tick_action(&e.action))
    }
    fn required_dependencies(&self, state: &OwnerState, _: Option<&InputCommand>) -> Vec<EntityId> {
        std::iter::once(self.welcome.collision_entity)
            .chain(state.1.values().copied())
            .collect()
    }
    fn step(
        &self,
        state: &mut OwnerState,
        _: ServerTick,
        command: &InputCommand,
        dependencies: &DependencyFrame<CollisionDependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        self.step_owner(state, Some(command), dependencies)
    }
    fn step_substitute(
        &self,
        state: &mut OwnerState,
        _: ServerTick,
        dependencies: &DependencyFrame<CollisionDependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        self.step_owner(state, None, dependencies)
    }
}
impl OwnerModel {
    fn step_owner(
        &self,
        state: &mut OwnerState,
        command: Option<&InputCommand>,
        dependencies: &DependencyFrame<CollisionDependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        let record = dependencies
            .records
            .get(&self.welcome.collision_entity)
            .ok_or(PredictionError::InvalidState)?;
        if record.scope.entity != self.welcome.collision_entity
            || record.scope.connection != self.welcome.stream.epoch
            || record.scene_revision != self.welcome.content.scene_revision
        {
            return Err(PredictionError::InvalidIdentity);
        }
        let DependencyValue::Known(CollisionDependency::StaticCollision(collision)) = &record.value
        else {
            return Err(PredictionError::InvalidState);
        };
        let mut bases = Vec::with_capacity(1);
        for (&key, &entity) in &state.1 {
            let record = dependencies
                .records
                .get(&entity)
                .ok_or(PredictionError::InvalidState)?;
            if record.scope.entity != entity
                || record.scope.connection != self.welcome.stream.epoch
                || record.scene_revision != self.welcome.content.scene_revision
            {
                return Err(PredictionError::InvalidIdentity);
            }
            let DependencyValue::Known(CollisionDependency::Platform(view)) = &record.value else {
                return Err(PredictionError::InvalidState);
            };
            if view.collider != key {
                return Err(PredictionError::InvalidIdentity);
            }
            bases.push(view.clone());
        }
        let mut input = match command {
            Some(command) => command.0.input.held,
            None => state.0.substitute_predicted_input(),
        };
        input.dash = false;
        input.casts = [false; 4];
        use dreamwake_sim::{combat::RayActionKey, replication::OwnerCombatAction};
        let mut action = None;
        let mut beam = None;
        if let Some(command) = command {
            let key = |slot| RayActionKey {
                match_epoch: self.welcome.match_epoch,
                connection_epoch: self.welcome.stream.epoch.0,
                command_stream: self.welcome.stream.stream.0,
                ownership_epoch: self.welcome.stream.ownership.0,
                actor: self.welcome.player.get(),
                actor_generation: state.0.combat_generation(),
                command_sequence: command.0.sequence.0,
                action_slot: slot,
            };
            if let Some(edge) = command
                .0
                .actions
                .as_slice()
                .iter()
                .min_by_key(|edge| edge.slot)
            {
                match edge.action {
                    DreamAction::Cast { slot, aim } => {
                        input.aim = aim;
                        action = Some(OwnerCombatAction::Cast {
                            key: key(edge.slot),
                            slot,
                            aim,
                        });
                    }
                    DreamAction::Dash { direction } => {
                        input.dash = true;
                        input.movement = direction;
                    }
                    DreamAction::Dreamlance { aim, .. } => {
                        action = Some(OwnerCombatAction::Ray {
                            key: key(edge.slot),
                            aim,
                        })
                    }
                    DreamAction::BeamBegin { aim, .. } => {
                        action = Some(OwnerCombatAction::BeamBegin {
                            key: key(edge.slot),
                            aim,
                        })
                    }
                    DreamAction::ChargeBegin
                    | DreamAction::ChargeRelease { .. }
                    | DreamAction::ChargeCancel { .. } => {
                        action = Some(OwnerCombatAction::Charge {
                            key: key(edge.slot),
                            command: match edge.action {
                                DreamAction::ChargeBegin => engine_core::ChargeCommand::Begin {
                                    episode: command.0.sequence.0,
                                },
                                DreamAction::ChargeRelease { episode } => {
                                    engine_core::ChargeCommand::Release { episode }
                                }
                                DreamAction::ChargeCancel { episode } => {
                                    engine_core::ChargeCommand::Cancel { episode }
                                }
                                _ => unreachable!(),
                            },
                            aim: input.aim,
                        })
                    }
                    DreamAction::BeamStop => {
                        action = Some(OwnerCombatAction::BeamStop {
                            key: key(edge.slot),
                        })
                    }
                    _ => {}
                }
            }
            beam = command.0.input.beam.map(|sample| (key(0), sample.aim));
            state.0.record_predicted_input(command.0.input.held);
        }
        state
            .0
            .step_restricted_combat_with_bases(input, collision, &bases, action, beam)
            .map_err(|_| PredictionError::InvalidState)?;
        Ok(state
            .0
            .starfall_cues()
            .iter()
            .map(|cue| ActionKey {
                connection: ConnectionEpoch(cue.action.connection_epoch),
                stream: CommandStream(cue.action.command_stream),
                command: CommandSeq(cue.action.command_sequence),
                slot: cue.action.action_slot,
            })
            .collect())
    }
}

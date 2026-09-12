use crate::{
    commands::OwnerStream,
    replication::{Payload, PublishedGroup},
    types::*,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub enum DependencyValue<D> {
    Known(D),
    ExplicitlyAbsent,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DependencyRecord<D> {
    pub scope: ScopeIdentity,
    pub scene_revision: SceneRevision,
    pub version: StateVersion,
    pub value: DependencyValue<D>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DependencyFrame<D> {
    pub records: BTreeMap<EntityId, DependencyRecord<D>>,
}
impl<D> Default for DependencyFrame<D> {
    fn default() -> Self {
        Self {
            records: BTreeMap::new(),
        }
    }
}
impl<D> DependencyFrame<D> {
    /// Missing is unavailable, never evidence that an entity/component was removed.
    pub fn value(&self, entity: EntityId) -> Option<&DependencyValue<D>> {
        self.records.get(&entity).map(|record| &record.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictionBinding {
    pub owner: OwnerStream,
    pub scene_revision: SceneRevision,
    pub teleport_segment: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionIdentity {
    pub binding: PredictionBinding,
    pub group: GroupId,
    pub group_revision: GroupRevision,
    pub scopes: BTreeSet<ScopeIdentity>,
}

pub struct DecodedPrediction<S, D> {
    pub state: S,
    pub dependencies: DependencyFrame<D>,
}

/// The immutable transition recorded after a checkpoint. A substitute derives
/// held/idle behavior from restored simulation state without inventing a command
/// sequence or action identity. A checkpoint itself has no transition.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplayInput<C> {
    Command(C),
    Substitute,
}
impl<C> ReplayInput<C> {
    pub(crate) fn command(&self) -> Option<&C> {
        match self {
            Self::Command(command) => Some(command),
            Self::Substitute => None,
        }
    }
}

/// A model restores every predicted field, including latent RNG/cooldown/physics
/// state. Equality is canonical gameplay equality, not approximate position error.
/// Wire decoding and all model allocations must already obey registered schema
/// limits; retained_bytes reports owned heap capacity (not only encoded size).
pub trait PredictionModel {
    type Wire: Payload;
    type State: Payload;
    type Command: Payload;
    type Dependency: Payload;

    fn decode_group(
        &self,
        group: &PublishedGroup<'_, Self::Wire>,
        binding: PredictionBinding,
    ) -> Result<DecodedPrediction<Self::State, Self::Dependency>, PredictionError>;
    fn valid_state(&self, state: &Self::State) -> bool;
    fn command_identity(&self, command: &Self::Command) -> (OwnerStream, CommandSeq, TargetTick);
    fn valid_command(&self, command: &Self::Command) -> bool;
    /// Return the complete required dependency identities for this transition.
    /// A bounded duplicate-free declaration is required; omission is a model bug.
    fn required_dependencies(
        &self,
        state: &Self::State,
        command: Option<&Self::Command>,
    ) -> Vec<EntityId>;
    fn step(
        &self,
        state: &mut Self::State,
        tick: ServerTick,
        command: &Self::Command,
        dependencies: &DependencyFrame<Self::Dependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError>;
    /// Advance without newly authored input, using only the restored state and
    /// pinned dependencies. Models must preserve their authoritative held/idle
    /// rules and must not allocate command sequences or synthesize action edges.
    /// `required_dependencies` receives `None` for this transition as it does
    /// for checkpoint validation. Unsupported models reject substitutes.
    fn step_substitute(
        &self,
        _state: &mut Self::State,
        _tick: ServerTick,
        _dependencies: &DependencyFrame<Self::Dependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        Err(PredictionError::InvalidCommand)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
    IdentityChanged,
    CheckpointAhead,
    MissingHistory,
    MissingDependency,
    ReplayBudget,
    MemoryBudget,
    InvalidState,
    DisclosureRevoked,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionError {
    InvalidConfiguration,
    InvalidIdentity,
    InvalidState,
    InvalidCommand,
    InvalidDependency,
    Equivocation,
    WrongTick,
    AlreadyInitialized,
    UnknownGroup,
    NoFrame,
    NonMonotonicFrame,
    StaleDependency,
    Capacity,
    ResyncRequired(RecoveryReason),
}
impl std::fmt::Display for PredictionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "prediction: {self:?}")
    }
}
impl std::error::Error for PredictionError {}

#[derive(Debug, Clone, Copy)]
pub struct PredictionLimits {
    pub groups: usize,
    pub entities_per_group: usize,
    pub total_entities: usize,
    pub history_ticks: usize,
    pub checkpoint_bytes: usize,
    pub command_bytes: usize,
    pub dependency_entities: usize,
    pub dependency_bytes: usize,
    pub events_per_tick: usize,
    pub history_bytes: usize,
    pub replay_ticks_per_frame: usize,
    pub replay_entity_steps_per_frame: usize,
    pub prediction_ticks_per_frame: usize,
    pub prediction_entity_steps_per_frame: usize,
}
impl Default for PredictionLimits {
    fn default() -> Self {
        Self {
            groups: 8,
            entities_per_group: 32,
            total_entities: 128,
            history_ticks: 256,
            checkpoint_bytes: 16_384,
            command_bytes: 2048,
            dependency_entities: 128,
            dependency_bytes: 16_384,
            events_per_tick: 32,
            history_bytes: 64 * 1024 * 1024,
            replay_ticks_per_frame: 32,
            replay_entity_steps_per_frame: 2048,
            prediction_ticks_per_frame: 32,
            prediction_entity_steps_per_frame: 2048,
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PredictionWork {
    pub replay_ticks: usize,
    pub replay_entity_steps: usize,
    pub predicted_ticks: usize,
    pub predicted_entity_steps: usize,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventChanges {
    pub started: BTreeSet<ActionKey>,
    pub canceled: BTreeSet<ActionKey>,
}
/// Topology changes discard incompatible command/dependency histories. The caller
/// resumes from this authoritative tick; no replay or neutral inputs are implied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoritativeRebase {
    pub tick: ServerTick,
    pub events: EventChanges,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Correction {
    Stale,
    Equal,
    Replaced,
    Replayed {
        from: ServerTick,
        through: ServerTick,
        events: EventChanges,
    },
}

// Conservatively charge allocator/map metadata as well as payload capacity. Count
// caps independently bound node/index overhead; this deliberately overaccounts small
// payloads rather than pretending their encoded length equals retained memory.
pub(crate) const ENTRY_OVERHEAD: usize = 128;
pub(crate) fn payload_bytes<P: Payload>(payload: &P) -> Result<usize, PredictionError> {
    std::mem::size_of::<P>()
        .checked_add(payload.retained_bytes())
        .ok_or(PredictionError::Capacity)
}
pub(crate) fn dependency_bytes<D: Payload>(
    frame: &DependencyFrame<D>,
) -> Result<usize, PredictionError> {
    let mut bytes = std::mem::size_of::<DependencyFrame<D>>();
    for record in frame.records.values() {
        bytes = bytes
            .checked_add(
                ENTRY_OVERHEAD
                    + std::mem::size_of::<EntityId>()
                    + std::mem::size_of::<DependencyRecord<D>>(),
            )
            .and_then(|n| {
                n.checked_add(match &record.value {
                    DependencyValue::Known(value) => value.retained_bytes(),
                    _ => 0,
                })
            })
            .ok_or(PredictionError::Capacity)?;
    }
    Ok(bytes)
}
pub(crate) fn event_bytes(events: &BTreeSet<ActionKey>) -> Result<usize, PredictionError> {
    events
        .len()
        .checked_mul(ENTRY_OVERHEAD + std::mem::size_of::<ActionKey>())
        .ok_or(PredictionError::Capacity)
}

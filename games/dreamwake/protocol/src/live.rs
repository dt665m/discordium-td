//! Authenticated readiness, immutable commands, scoped state and distinct proofs.
//! No live message can contain DreamSnapshot or arbitrary simulation components.
pub mod baselines;
pub mod combat;
pub use combat::{
    ActionOutcome, ActionOutcomeData, BeamAimSample, CombatViewStamp, OutcomeReason,
    TerminalStatus, TickInput,
};
#[cfg(test)]
mod tests;
use crate::{DreamAction, PROTOCOL_ID};
use dreamwake_sim::{CheckpointIdentity, DreamInput};
use engine_net::replication::baselines::{BaselineRepair, BaselineReset, BaselineRetirement};
use engine_net::{
    codec::{self, BoundedVec, CodecError, FrameHeader, FrameLimit, Lane, Validate},
    commands::{Command, FinalizedReceipt, OwnerStream},
    replication::*,
    types::*,
};
use serde::{Deserialize, Serialize};

pub const OWNER_GROUP: GroupId = GroupId(1);
const WELCOME_MAGIC: &[u8; 4] = b"DW13";
pub const WELCOME_BYTES: usize = 1024;

/// A first effective command arrival, tied to the immutable command that caused
/// it. Server-observed values never include client-supplied lead policy metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrivalSample {
    pub sequence: CommandSeq,
    pub target: TargetTick,
    pub arrival_tick: ServerTick,
    /// Zero means a missed deadline; accepted commands have 1..=12 ticks left.
    pub slack: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrivalFeedback {
    pub sample: Option<ArrivalSample>,
    /// Cumulative first-effective observations in this owner stream.
    pub observed_commands: u64,
    /// Every first-effective Late admission, including old policy and reordered input.
    pub late_commands: u64,
}
impl ArrivalFeedback {
    pub fn validate(&self) -> bool {
        self.late_commands <= self.observed_commands
            && self.sample.is_none_or(|sample| {
                self.observed_commands != 0
                    && sample.sequence.0 != 0
                    && sample.target.0 != 0
                    && sample.slack <= 12
                    && sample.target.0.saturating_sub(sample.arrival_tick.0)
                        == u64::from(sample.slack)
                    && if sample.slack == 0 {
                        self.late_commands != 0
                    } else {
                        self.observed_commands > self.late_commands
                    }
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentIdentity {
    pub ruleset: [u8; 32],
    pub schema: [u8; 32],
    pub scene: [u8; 32],
    pub scene_revision: SceneRevision,
}
impl ContentIdentity {
    pub fn current(scene_revision: SceneRevision) -> Self {
        let text = CheckpointIdentity::current(u64::from(scene_revision.0)).ruleset;
        let ruleset = std::array::from_fn(|i| {
            u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("BLAKE3 hexadecimal")
        });
        // Exact collision primitives and movement profile, independent of art.
        Self {
            ruleset,
            schema: dreamwake_sim::replication::schema_identity(),
            scene: dreamwake_sim::collision::CollisionManifest::current(u64::from(
                scene_revision.0,
            ))
            .identity(),
            scene_revision,
        }
    }
}

/// Issued over authenticated reliable transport. The initial handshake is the
/// only envelope allowed before the recipient knows its connection epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// Process/service incarnation; independent of connection and match resets.
    pub server_instance: u64,
    pub protocol: u64,
    pub client: ConnectionId,
    /// Stable game identity verified by bootstrap, independent of transport ID.
    pub player: crate::player::PlayerId,
    pub stream: OwnerStream,
    pub match_epoch: u32,
    pub tick: ServerTick,
    pub server_time_nanos: u64,
    pub content: ContentIdentity,
    pub global_entity: EntityId,
    pub owner_entity: EntityId,
    pub collision_entity: EntityId,
    pub application_frame_bytes: u16,
}
impl Validate for Welcome {
    fn validate(&self) -> Result<(), CodecError> {
        if self.protocol != PROTOCOL_ID
            || self.server_instance == 0
            || self.client.0 == 0
            || self.stream.connection != self.client
            || self.stream.epoch.0 == 0
            || self.stream.stream.0 == 0
            || self.stream.owner != self.owner_entity
            || self.stream.ownership.0 == 0
            || self.match_epoch == 0
            || self.global_entity.generation == 0
            || self.owner_entity.generation == 0
            || self.collision_entity.generation == 0
            || self.collision_entity == self.global_entity
            || self.collision_entity == self.owner_entity
            || self.owner_entity == self.global_entity
            || self.content.scene_revision.0 == 0
        {
            return Err(invalid("invalid session descriptor"));
        }
        byte_limits(self.frame_limit()?)
            .map_err(|_| invalid("unusable negotiated fragmentation allowance"))?;
        Ok(())
    }
}
impl Welcome {
    pub fn frame_limit(&self) -> Result<FrameLimit, CodecError> {
        FrameLimit::new(usize::from(self.application_frame_bytes))
    }
    pub fn compatible(&self, client: ConnectionId) -> bool {
        self.validate().is_ok()
            && self.client == client
            && self.content == ContentIdentity::current(self.content.scene_revision)
    }
}
pub fn encode_welcome(value: &Welcome) -> Result<Vec<u8>, CodecError> {
    let payload = codec::encode_payload(value, WELCOME_BYTES - 4)?;
    let mut out = WELCOME_MAGIC.to_vec();
    out.extend_from_slice(&payload);
    Ok(out)
}
pub fn is_welcome(bytes: &[u8]) -> bool {
    bytes.starts_with(WELCOME_MAGIC)
}
pub fn decode_welcome(bytes: &[u8]) -> Result<Welcome, CodecError> {
    if bytes.len() > WELCOME_BYTES {
        return Err(CodecError::Oversized);
    }
    if !is_welcome(bytes) {
        return Err(CodecError::ProtocolMismatch);
    }
    codec::decode_payload(&bytes[4..], WELCOME_BYTES - 4)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResyncReason {
    MissingHistory,
    MissingDependency,
    ReplayBudget,
    MissingScope,
    ExpiredGroup,
    InvalidCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientControl {
    /// Delivery feedback for a validated Finalized batch, never execution proof.
    FinalizedAck {
        through: ServerTick,
    },
    OutcomesAck {
        through: u64,
    },
    /// Lookup only; an old command key never becomes a fresh execution request.
    OutcomeLookup {
        keys: BoundedVec<ActionKey, 8>,
    },
    Ready {
        content: ContentIdentity,
    },
    Decoded {
        receipts: BoundedVec<StateReceipt, 12>,
    },
    OwnerDecoded {
        proofs: BoundedVec<baselines::DecodedBaseline, { baselines::MEMBERS }>,
    },
    BaselineRetire {
        requests: BoundedVec<BaselineRetirement, { baselines::MEMBERS }>,
    },
    BaselineRepair {
        requests: BoundedVec<BaselineRepair, { baselines::MEMBERS }>,
    },
    Exited(ScopeExit),
    Destroyed(Destroy),
    /// Menu transactions have reliable ordered delivery. Tick-critical actions
    /// are only legal in target-tick command bundles below.
    Menu {
        sequence: CommandSeq,
        action: DreamAction,
    },
    ClockProbe {
        client_send_nanos: u64,
    },
    Resync {
        reason: ResyncReason,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerControl {
    ActionOutcomes {
        batch: u64,
        records: BoundedVec<ActionOutcome, 4>,
    },
    OutcomeUnavailable {
        keys: BoundedVec<ActionKey, 8>,
    },
    BaselineRetired {
        fences: BoundedVec<BaselineRetirement, { baselines::MEMBERS }>,
    },
    BaselineReset {
        resets: BoundedVec<BaselineReset, { baselines::MEMBERS }>,
    },
    Finalized {
        through: ServerTick,
        receipts: BoundedVec<FinalizedReceipt, 32>,
        menu_applied: Option<CommandSeq>,
        arrival: ArrivalFeedback,
    },
    Active {
        owner_snapshot: SnapshotId,
        through: ServerTick,
    },
    Exit(ScopeExit),
    Destroy(Destroy),
    ClockReply {
        client_send_nanos: u64,
        server_receive_nanos: u64,
        server_send_nanos: u64,
        tick: ServerTick,
    },
    Notice {
        utf8: BoundedVec<u8, 512>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandBundle {
    pub records: BoundedVec<Command<TickInput, DreamAction>, 8>,
}

fn invalid(message: &str) -> CodecError {
    CodecError::InvalidPayload(message.into())
}
fn valid_scope(scope: ScopeIdentity) -> bool {
    scope.connection.0 != 0
        && scope.entity.generation != 0
        && scope.scope.0 != 0
        && scope.representation.0 != 0
}
fn valid_receipt(receipt: &StateReceipt) -> bool {
    valid_scope(receipt.scope)
        && receipt.snapshot.0 != 0
        && receipt.baseline_generation.0 != 0
        && receipt.version.0 != 0
}
pub fn valid_direction(direction: [f32; 2]) -> bool {
    direction.iter().all(|v| v.is_finite())
        && f64::from(direction[0]).hypot(f64::from(direction[1])) <= 1.000_001
}
pub fn valid_held_input(input: &DreamInput) -> bool {
    valid_direction(input.movement)
        && valid_direction(input.aim)
        && !input.dash
        && input.casts == [false; 4]
        && input.action_sequences == [0; 5]
        && input.charge == dreamwake_sim::ChargeCommand::None
}
pub fn valid_tick_input(input: &TickInput) -> bool {
    valid_held_input(&input.held)
        && input
            .beam
            .is_none_or(|sample| valid_direction(sample.aim) && sample.view.validate().is_ok())
}
pub fn valid_tick_action(action: &DreamAction) -> bool {
    match action {
        DreamAction::Cast { slot, aim } => *slot < 4 && valid_direction(*aim),
        DreamAction::Dash { direction } => valid_direction(*direction),
        DreamAction::Dreamlance { aim, view } | DreamAction::BeamBegin { aim, view } => {
            valid_direction(*aim) && view.validate().is_ok()
        }
        DreamAction::BeamStop | DreamAction::ChargeBegin => true,
        DreamAction::ChargeRelease { episode } | DreamAction::ChargeCancel { episode } => {
            *episode != 0
        }
        _ => false,
    }
}
fn valid_menu(action: &DreamAction) -> bool {
    match action {
        DreamAction::Choose { choice, slot } => *choice < 3 && *slot < 4,
        DreamAction::Swap { a, b } => *a < 4 && *b < 4,
        DreamAction::BuyMemoryUpgrade { slot } => *slot < 4,
        DreamAction::Cast { .. }
        | DreamAction::Dash { .. }
        | DreamAction::Dreamlance { .. }
        | DreamAction::BeamBegin { .. }
        | DreamAction::BeamStop
        | DreamAction::ChargeBegin
        | DreamAction::ChargeRelease { .. }
        | DreamAction::ChargeCancel { .. } => false,
        _ => true,
    }
}
impl Validate for CommandBundle {
    fn validate(&self) -> Result<(), CodecError> {
        if self.records.is_empty() {
            return Err(invalid("empty command bundle"));
        }
        for command in self.records.as_slice() {
            let owner = command.owner;
            if owner.connection.0 == 0
                || owner.epoch.0 == 0
                || owner.owner.generation == 0
                || owner.stream.0 == 0
                || owner.ownership.0 == 0
                || command.sequence.0 == 0
                || command.target.0 == 0
                || !valid_tick_input(&command.input)
            {
                return Err(invalid("invalid command"));
            }
            let mut slots = 0_u8;
            for edge in command.actions.as_slice() {
                if edge.slot >= 8
                    || slots & (1 << edge.slot) != 0
                    || !valid_tick_action(&edge.action)
                {
                    return Err(invalid("invalid action edge"));
                }
                slots |= 1 << edge.slot;
            }
        }
        Ok(())
    }
}
impl Validate for ClientControl {
    fn validate(&self) -> Result<(), CodecError> {
        let valid = match self {
            Self::FinalizedAck { through } => through.0 != 0,
            Self::OutcomesAck { through } => *through != 0,
            Self::OutcomeLookup { keys } => {
                !keys.is_empty() && keys.as_slice().iter().all(combat::valid_action_key)
            }
            Self::Ready { content } => content.scene_revision.0 != 0,
            Self::Decoded { receipts } => {
                !receipts.is_empty() && receipts.as_slice().iter().all(valid_receipt)
            }
            Self::OwnerDecoded { proofs } => {
                baselines::valid_member_count(proofs.len())
                    && proofs.as_slice().iter().all(baselines::valid_proof)
            }
            Self::BaselineRetire { requests } => {
                !requests.is_empty()
                    && requests.as_slice().iter().all(|r| {
                        baselines::valid_context(r.context)
                            && r.generation.0 != 0
                            && r.through.0 != 0
                    })
            }
            Self::BaselineRepair { requests } => {
                !requests.is_empty()
                    && requests
                        .as_slice()
                        .iter()
                        .all(|r| baselines::valid_context(r.context) && r.generation.0 != 0)
            }
            Self::Exited(exit) => valid_scope(exit.scope),
            Self::Destroyed(destroy) => destroy.connection.0 != 0 && destroy.entity.generation != 0,
            Self::Menu { sequence, action } => sequence.0 != 0 && valid_menu(action),
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(invalid("invalid client control"))
        }
    }
}
impl Validate for ServerControl {
    fn validate(&self) -> Result<(), CodecError> {
        let valid = match self {
            Self::ActionOutcomes { batch, records } => {
                *batch != 0
                    && !records.is_empty()
                    && records
                        .as_slice()
                        .iter()
                        .all(|record| record.validate().is_ok())
            }
            Self::OutcomeUnavailable { keys } => {
                !keys.is_empty() && keys.as_slice().iter().all(combat::valid_action_key)
            }
            Self::BaselineRetired { fences } => {
                !fences.is_empty()
                    && fences.as_slice().iter().all(|r| {
                        baselines::valid_context(r.context)
                            && r.generation.0 != 0
                            && r.through.0 != 0
                    })
            }
            Self::BaselineReset { resets } => {
                !resets.is_empty()
                    && resets
                        .as_slice()
                        .iter()
                        .all(|r| baselines::valid_context(r.context) && r.generation.0 != 0)
            }
            Self::Finalized {
                through,
                receipts,
                arrival,
                ..
            } => {
                arrival.validate()
                    && !receipts.is_empty()
                    && receipts
                        .as_slice()
                        .last()
                        .is_some_and(|receipt| receipt.tick == *through)
                    && receipts
                        .as_slice()
                        .iter()
                        .all(|r| r.tick <= *through && r.sequence.is_none_or(|s| s.0 != 0))
                    && receipts
                        .as_slice()
                        .windows(2)
                        .all(|pair| pair[0].tick.checked_next() == Some(pair[1].tick))
            }
            Self::Active { owner_snapshot, .. } => owner_snapshot.0 != 0,
            Self::Exit(exit) => valid_scope(exit.scope),
            Self::Destroy(destroy) => destroy.connection.0 != 0 && destroy.entity.generation != 0,
            Self::ClockReply {
                server_receive_nanos,
                server_send_nanos,
                ..
            } => server_receive_nanos <= server_send_nanos,
            Self::Notice { utf8 } => std::str::from_utf8(utf8.as_slice()).is_ok(),
        };
        if valid {
            Ok(())
        } else {
            Err(invalid("invalid server control"))
        }
    }
}
pub fn encode_control<T: Serialize + Validate>(
    value: &T,
    epoch: ConnectionEpoch,
    sequence: u32,
    limit: FrameLimit,
) -> Result<Vec<u8>, CodecError> {
    let payload = codec::encode_payload(value, limit.payload_bytes())?;
    codec::encode_frame(
        FrameHeader {
            lane: Lane::Control,
            connection: epoch,
            sequence,
        },
        &payload,
        limit,
    )
}
pub fn decode_control<T: serde::de::DeserializeOwned + Validate>(
    bytes: &[u8],
    epoch: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<T, CodecError> {
    let (header, payload) = codec::decode_frame(bytes, epoch, limit)?;
    if header.lane != Lane::Control {
        return Err(CodecError::InvalidLane);
    }
    codec::decode_payload(payload, limit.payload_bytes())
}
pub fn encode_commands(
    value: &CommandBundle,
    epoch: ConnectionEpoch,
    sequence: u32,
    limit: FrameLimit,
) -> Result<Vec<u8>, CodecError> {
    if value
        .records
        .as_slice()
        .iter()
        .any(|r| r.owner.epoch != epoch)
    {
        return Err(CodecError::WrongEpoch);
    }
    let payload = codec::encode_payload(value, limit.payload_bytes())?;
    codec::encode_frame(
        FrameHeader {
            lane: Lane::Input,
            connection: epoch,
            sequence,
        },
        &payload,
        limit,
    )
}
pub fn decode_commands(
    bytes: &[u8],
    epoch: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<CommandBundle, CodecError> {
    let (header, payload) = codec::decode_frame(bytes, epoch, limit)?;
    if header.lane != Lane::Input {
        return Err(CodecError::InvalidLane);
    }
    let bundle: CommandBundle = codec::decode_payload(payload, limit.payload_bytes())?;
    if bundle
        .records
        .as_slice()
        .iter()
        .any(|r| r.owner.epoch != epoch)
    {
        return Err(CodecError::WrongEpoch);
    }
    Ok(bundle)
}

/// Complete game prediction publications include the exact declared dependency
/// closure; per-member payload bounds remain independent of group capacity.
pub fn group_limits() -> GroupLimits {
    GroupLimits {
        members: baselines::MEMBERS,
        group_bytes: baselines::GROUP_BYTES,
        staging_bytes: baselines::GROUP_STAGING_BYTES,
        incomplete: baselines::GROUP_INCOMPLETE,
        ..Default::default()
    }
}

/// Limits are derived from the negotiated application frame allowance, including
/// our state discriminator and byte-fragment header. Smaller unsupported paths
/// fail admission instead of silently exceeding their allowance.
pub fn byte_limits(frame: FrameLimit) -> Result<ByteLimits, ReplicationError> {
    ByteLimits {
        fragment_payload_bytes: frame
            .payload_bytes()
            .checked_sub(BYTE_FRAGMENT_HEADER_BYTES + 1)
            .ok_or(ReplicationError::Capacity)?,
        group_bytes: baselines::GROUP_BYTES,
        fragments: baselines::GROUP_FRAGMENT_COUNT,
        incomplete: baselines::GROUP_INCOMPLETE,
        known_groups: 8,
        staging_bytes: baselines::GROUP_STAGING_BYTES,
        max_age_ms: 2000,
    }
    .validate()
}
pub enum StateFrame {
    Full(FullState<Vec<u8>>),
    Fragment(ByteFragment),
}
pub fn encode_state(
    value: &StateFrame,
    epoch: ConnectionEpoch,
    sequence: u32,
    limit: FrameLimit,
) -> Result<Vec<u8>, CodecError> {
    let (tag, bytes) = match value {
        StateFrame::Full(state) if state.scope.connection == epoch => (
            0,
            encode_compact_full_state(state, limit.payload_bytes().saturating_sub(1)),
        ),
        StateFrame::Fragment(fragment) if fragment.header.publication.connection == epoch => (
            1,
            fragment.encode(byte_limits(limit).map_err(|_| invalid("fragment limit"))?),
        ),
        _ => return Err(CodecError::WrongEpoch),
    };
    let bytes = bytes.map_err(|_| invalid("invalid scope state"))?;
    let mut payload = Vec::with_capacity(bytes.len() + 1);
    payload.push(tag);
    payload.extend_from_slice(&bytes);
    codec::encode_frame(
        FrameHeader {
            lane: Lane::State,
            connection: epoch,
            sequence,
        },
        &payload,
        limit,
    )
}
pub fn decode_state(
    bytes: &[u8],
    epoch: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<StateFrame, CodecError> {
    let (header, bytes) = codec::decode_frame(bytes, epoch, limit)?;
    if header.lane != Lane::State {
        return Err(CodecError::InvalidLane);
    }
    let (tag, body) = bytes.split_first().ok_or(CodecError::Truncated)?;
    match tag {
        0 => decode_compact_full_state(body, epoch, limit.payload_bytes().saturating_sub(1))
            .map(StateFrame::Full),
        1 => ByteFragment::decode(
            body,
            byte_limits(limit).map_err(|_| invalid("fragment limit"))?,
        )
        .and_then(|fragment| {
            if fragment.header.publication.connection == epoch {
                Ok(StateFrame::Fragment(fragment))
            } else {
                Err(ReplicationError::Stale)
            }
        }),
        _ => Err(ReplicationError::InvalidPayload),
    }
    .map_err(|_| invalid("invalid scoped frame"))
}

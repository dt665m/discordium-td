//! Dreamwake's live messages.
//!
//! Three kinds of traffic share the connection:
//!
//! * reliable controls in both directions (readiness, menu choices, terminal
//!   action outcomes, clock probes, notices);
//! * unreliable client input packets, each carrying the newest commands and
//!   the oldest ones the server has not yet consumed, plus the client's
//!   acknowledgement of state packets;
//! * unreliable server state packets: the owner's status and changed entity
//!   states, each encoded against the newest state the client acknowledged.
//!
//! Nothing here waits for application receipts: the state stream recovers
//! from loss by itself, and the owner's state in every packet supersedes the
//! previous one.
pub mod combat;
pub use combat::{
    ActionOutcome, ActionOutcomeData, BeamAimSample, CombatViewStamp, OutcomeReason,
    TerminalStatus, TickInput,
};
#[cfg(test)]
mod tests;
use crate::{DreamAction, PROTOCOL_ID};
use dreamwake_sim::{CheckpointIdentity, DreamInput};
use engine_net::{
    codec::{self, BoundedVec, ByteReader, CodecError, FrameHeader, FrameLimit, Lane, Validate},
    commands::{ActionEdge, Command, OwnerStream},
    replication::stream::{STREAM_WINDOW, StreamAck},
    types::*,
};
use serde::{Deserialize, Serialize};

/// Largest application frame, including the frame header. It keeps every
/// message inside one Renet packet, so one lost datagram loses one message.
pub const APPLICATION_FRAME_BYTES: usize = 1100;
const WELCOME_MAGIC: &[u8; 4] = b"DW17";
pub const WELCOME_BYTES: usize = 1024;
/// Commands per input packet: every unfinalized command, newest first, up to
/// the client's lead cap, so each command still ahead of the server travels in
/// every packet until its target passes.
pub const MAX_INPUT_RECORDS: usize = engine_net::types::MAX_COMMAND_LEAD_TICKS as usize;
/// A state packet is normally kept within [`APPLICATION_FRAME_BYTES`]; one
/// entity's whole state that alone exceeds it, such as a joining owner's
/// first checkpoint, travels in a larger packet that Renet slices.
pub const MAX_STATE_FRAME_BYTES: usize = 32 * 1024;
pub const MAX_STATE_ENTRIES: usize = 255;
pub const MAX_STATE_REMOVALS: usize = 255;

/// A first effective command arrival, tied to the immutable command that caused
/// it. Server-observed values never include client-supplied lead policy metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrivalSample {
    pub sequence: CommandSeq,
    pub target: TargetTick,
    pub arrival_tick: ServerTick,
    /// Zero means a missed deadline; accepted commands have 1 to
    /// `MAX_COMMAND_LEAD_TICKS` ticks left. More headroom than the lead
    /// controller can use is reported at that cap.
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
                    && sample.slack <= engine_net::types::MAX_COMMAND_LEAD_TICKS
                    && {
                        let headroom = sample.target.0.saturating_sub(sample.arrival_tick.0);
                        headroom == u64::from(sample.slack)
                            || (sample.slack == engine_net::types::MAX_COMMAND_LEAD_TICKS
                                && headroom > u64::from(sample.slack))
                    }
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
    /// The stream entity carrying the match summary.
    pub global_entity: EntityId,
    /// The stream entity carrying this connection's own hero.
    pub owner_entity: EntityId,
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
            || self.owner_entity == self.global_entity
            || self.content.scene_revision.0 == 0
        {
            return Err(invalid("invalid session descriptor"));
        }
        self.frame_limit()?;
        if !(1088..=1200).contains(&usize::from(self.application_frame_bytes)) {
            return Err(invalid("unusable application frame allowance"));
        }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientControl {
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
    /// Menu transactions have reliable ordered delivery. Tick-critical actions
    /// are only legal in target-tick command records.
    Menu {
        sequence: CommandSeq,
        action: DreamAction,
    },
    ClockProbe {
        client_send_nanos: u64,
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

fn invalid(message: &str) -> CodecError {
    CodecError::InvalidPayload(message.into())
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
impl Validate for ClientControl {
    fn validate(&self) -> Result<(), CodecError> {
        let valid = match self {
            Self::OutcomesAck { through } => *through != 0,
            Self::OutcomeLookup { keys } => {
                !keys.is_empty() && keys.as_slice().iter().all(combat::valid_action_key)
            }
            Self::Ready { content } => content.scene_revision.0 != 0,
            Self::Menu { sequence, action } => sequence.0 != 0 && valid_menu(action),
            Self::ClockProbe { .. } => true,
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
    codec::encode_frame_deflated(
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
    let (header, payload) = codec::decode_frame_inflated(bytes, epoch, limit)?;
    if header.lane != Lane::Control {
        return Err(CodecError::InvalidLane);
    }
    codec::decode_payload(&payload, limit.payload_bytes())
}

/// One immutable command. Its owner stream is the connection's own, so the
/// wire carries only what changes from command to command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRecord {
    pub sequence: CommandSeq,
    pub target: TargetTick,
    pub input: TickInput,
    pub actions: BoundedVec<ActionEdge<DreamAction>, 8>,
}
impl InputRecord {
    pub fn from_command(command: &Command<TickInput, DreamAction>) -> Self {
        Self {
            sequence: command.sequence,
            target: command.target,
            input: command.input,
            actions: command.actions.clone(),
        }
    }
    pub fn command(&self, owner: OwnerStream) -> Command<TickInput, DreamAction> {
        Command {
            owner,
            sequence: self.sequence,
            target: self.target,
            input: self.input,
            actions: self.actions.clone(),
        }
    }
    fn valid(&self) -> bool {
        let mut slots = 0_u8;
        self.sequence.0 != 0
            && self.target.0 != 0
            && valid_tick_input(&self.input)
            && self.actions.as_slice().iter().all(|edge| {
                if edge.slot >= 8 || slots & (1 << edge.slot) != 0 {
                    return false;
                }
                slots |= 1 << edge.slot;
                valid_tick_action(&edge.action)
            })
    }
}

/// Client to server, unreliable, sent every simulation tick the client runs.
/// A packet with no records still delivers the state acknowledgement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputPacket {
    pub ack: StreamAck,
    pub records: BoundedVec<InputRecord, MAX_INPUT_RECORDS>,
}
impl Validate for InputPacket {
    fn validate(&self) -> Result<(), CodecError> {
        let records = self.records.as_slice();
        if !records.iter().all(InputRecord::valid)
            || records
                .iter()
                .enumerate()
                .any(|(i, a)| records[..i].iter().any(|b| b.sequence == a.sequence))
        {
            return Err(invalid("invalid input packet"));
        }
        Ok(())
    }
}
/// Steps per unit for movement and aim on the wire. The client quantizes held
/// input when it creates a command, so prediction and the server read the
/// same values, and consecutive records differ by small integers.
pub const INPUT_AXIS_STEPS: f32 = 4096.0;

/// Round a direction toward zero onto the input grid, which never lengthens it.
pub fn quantize_direction(direction: [f32; 2]) -> [f32; 2] {
    direction.map(|axis| (axis * INPUT_AXIS_STEPS).trunc() / INPUT_AXIS_STEPS)
}
/// The held input a command carries, on the input grid.
pub fn quantize_tick_input(mut input: TickInput) -> TickInput {
    input.held.movement = quantize_direction(input.held.movement);
    input.held.aim = quantize_direction(input.held.aim);
    if let Some(beam) = &mut input.beam {
        beam.aim = quantize_direction(beam.aim);
    }
    input
}
/// `bounded` refuses steps beyond a unit axis; unbounded encoding exists only
/// so codec tests can send what the decoder must reject.
fn axis_steps(axis: f32, bounded: bool) -> Result<i64, CodecError> {
    let steps = axis * INPUT_AXIS_STEPS;
    let limit = if bounded {
        INPUT_AXIS_STEPS
    } else {
        i32::MAX as f32
    };
    if !steps.is_finite() || steps.fract() != 0.0 || steps.abs() > limit {
        return Err(invalid("input axis off the input grid"));
    }
    Ok(steps as i64)
}
fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}
fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}
fn put_offset(out: &mut Vec<u8>, first: u64, this: u64) -> Result<(), CodecError> {
    let offset = i64::try_from(i128::from(first) - i128::from(this))
        .map_err(|_| invalid("input records too far apart"))?;
    codec::put_varint(out, zigzag(offset));
    Ok(())
}
fn read_offset(input: &mut ByteReader, first: u64) -> Result<u64, CodecError> {
    let offset = unzigzag(input.varint()?);
    u64::try_from(i128::from(first) - i128::from(offset))
        .map_err(|_| invalid("input record outside the stream"))
}
const INPUT_CHANGED: u8 = 1;
const INPUT_ATTACK: u8 = 2;
const INPUT_BEAM: u8 = 4;
const INPUT_ACTIONS: u8 = 8;
const INPUT_ACTION_BYTES: usize = 512;

/// The input packet as sent: the acknowledgement, then each record as offsets
/// from the first record's sequence and target, and its movement and aim as
/// grid steps relative to the record before it. A record whose movement and
/// aim repeat the one before carries neither, so a held direction costs a few
/// bytes per command before deflate.
fn input_body(value: &InputPacket, bounded: bool) -> Result<Vec<u8>, CodecError> {
    let records = value.records.as_slice();
    let mut out = Vec::with_capacity(16 + records.len() * 8);
    codec::put_varint(&mut out, u64::from(value.ack.latest));
    codec::put_varint(&mut out, value.ack.received);
    out.push(records.len() as u8);
    let Some(first) = records.first() else {
        return Ok(out);
    };
    codec::put_varint(&mut out, first.sequence.0);
    codec::put_varint(&mut out, first.target.0);
    let mut previous: Option<[i64; 4]> = None;
    for record in records {
        put_offset(&mut out, first.sequence.0, record.sequence.0)?;
        put_offset(&mut out, first.target.0, record.target.0)?;
        let held = &record.input.held;
        let steps = [
            axis_steps(held.movement[0], bounded)?,
            axis_steps(held.movement[1], bounded)?,
            axis_steps(held.aim[0], bounded)?,
            axis_steps(held.aim[1], bounded)?,
        ];
        let changed = previous != Some(steps);
        let mut flags = 0;
        if changed {
            flags |= INPUT_CHANGED;
        }
        if held.attack {
            flags |= INPUT_ATTACK;
        }
        if record.input.beam.is_some() {
            flags |= INPUT_BEAM;
        }
        if !record.actions.as_slice().is_empty() {
            flags |= INPUT_ACTIONS;
        }
        out.push(flags);
        if changed {
            let base = previous.unwrap_or_default();
            for (step, base) in steps.iter().zip(base) {
                codec::put_varint(&mut out, zigzag(step - base));
            }
        }
        previous = Some(steps);
        if let Some(beam) = record.input.beam {
            for axis in beam.aim {
                codec::put_varint(&mut out, zigzag(axis_steps(axis, bounded)?));
            }
            let view = beam.view;
            codec::put_varint(&mut out, u64::from(view.sampled_fraction));
            codec::put_varint(&mut out, u64::from(view.viewed_fraction));
            codec::put_varint(&mut out, view.sampled_at.0);
            codec::put_varint(&mut out, view.viewed_at.0);
            codec::put_varint(&mut out, u64::from(view.reference));
        }
        if !record.actions.as_slice().is_empty() {
            let actions = engine_net::encode(&record.actions);
            if actions.len() > INPUT_ACTION_BYTES {
                return Err(invalid("input actions too large"));
            }
            codec::put_varint(&mut out, actions.len() as u64);
            out.extend_from_slice(&actions);
        }
    }
    Ok(out)
}
fn read_input_body(bytes: &[u8]) -> Result<InputPacket, CodecError> {
    let mut input = ByteReader::new(bytes);
    let ack = StreamAck {
        latest: input.varint_u32()?,
        received: input.varint()?,
    };
    let count = usize::from(input.u8()?);
    if count > MAX_INPUT_RECORDS {
        return Err(invalid("too many input records"));
    }
    let mut records = Vec::with_capacity(count);
    if count > 0 {
        let first_sequence = input.varint()?;
        let first_target = input.varint()?;
        let mut previous = [0_i64; 4];
        for index in 0..count {
            let sequence = CommandSeq(read_offset(&mut input, first_sequence)?);
            let target = TargetTick(read_offset(&mut input, first_target)?);
            let flags = input.u8()?;
            if flags & !(INPUT_CHANGED | INPUT_ATTACK | INPUT_BEAM | INPUT_ACTIONS) != 0
                || (index == 0 && flags & INPUT_CHANGED == 0)
            {
                return Err(invalid("invalid input record flags"));
            }
            if flags & INPUT_CHANGED != 0 {
                for step in &mut previous {
                    *step = step
                        .checked_add(unzigzag(input.varint()?))
                        .ok_or_else(|| invalid("input axis overflow"))?;
                }
            }
            let axis = |steps: i64| steps as f32 / INPUT_AXIS_STEPS;
            let held = dreamwake_sim::DreamInput {
                movement: [axis(previous[0]), axis(previous[1])],
                aim: [axis(previous[2]), axis(previous[3])],
                attack: flags & INPUT_ATTACK != 0,
                ..Default::default()
            };
            let beam = if flags & INPUT_BEAM != 0 {
                let aim = [
                    axis(unzigzag(input.varint()?)),
                    axis(unzigzag(input.varint()?)),
                ];
                let fraction =
                    |value: u64| u16::try_from(value).map_err(|_| invalid("invalid beam fraction"));
                Some(BeamAimSample {
                    aim,
                    view: CombatViewStamp {
                        sampled_fraction: fraction(input.varint()?)?,
                        viewed_fraction: fraction(input.varint()?)?,
                        sampled_at: ServerTick(input.varint()?),
                        viewed_at: ServerTick(input.varint()?),
                        reference: input.varint_u32()?,
                    },
                })
            } else {
                None
            };
            let actions = if flags & INPUT_ACTIONS != 0 {
                let length = usize::try_from(input.varint()?)
                    .ok()
                    .filter(|length| *length <= INPUT_ACTION_BYTES)
                    .ok_or_else(|| invalid("input actions too large"))?;
                engine_net::decode_with_limit(input.take(length)?, INPUT_ACTION_BYTES)
                    .map_err(|_| invalid("invalid input actions"))?
            } else {
                BoundedVec::default()
            };
            records.push(InputRecord {
                sequence,
                target,
                input: TickInput { held, beam },
                actions,
            });
        }
    }
    input.finish()?;
    let packet = InputPacket {
        ack,
        records: BoundedVec::new(records)?,
    };
    packet.validate()?;
    Ok(packet)
}

/// Input records before deflate; the client keeps each encoded frame within
/// one application frame.
pub const INPUT_PAYLOAD_BYTES: usize = 4096;
pub fn input_limit() -> FrameLimit {
    FrameLimit::new(INPUT_PAYLOAD_BYTES).expect("input frame allowance")
}
/// Encode without validating the records, so codec tests can confirm the
/// decoder rejects what a well-behaved client would never send.
/// Encode without validating the records, so codec tests can confirm the
/// decoder rejects what a well-behaved client would never send.
#[doc(hidden)]
pub fn encode_input_unvalidated(
    value: &InputPacket,
    epoch: ConnectionEpoch,
    sequence: u32,
    limit: FrameLimit,
) -> Result<Vec<u8>, CodecError> {
    let payload = input_body(value, false)?;
    if payload.len() > limit.payload_bytes() {
        return Err(CodecError::Oversized);
    }
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
pub fn encode_input(
    value: &InputPacket,
    epoch: ConnectionEpoch,
    sequence: u32,
    limit: FrameLimit,
) -> Result<Vec<u8>, CodecError> {
    value.validate()?;
    let payload = input_body(value, true)?;
    if payload.len() > limit.payload_bytes() {
        return Err(CodecError::Oversized);
    }
    codec::encode_frame_deflated(
        FrameHeader {
            lane: Lane::Input,
            connection: epoch,
            sequence,
        },
        &payload,
        limit,
    )
}
pub fn decode_input(
    bytes: &[u8],
    epoch: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<InputPacket, CodecError> {
    let (header, payload) = codec::decode_frame_inflated(bytes, epoch, limit)?;
    if header.lane != Lane::Input {
        return Err(CodecError::InvalidLane);
    }
    if payload.len() > limit.payload_bytes() {
        return Err(CodecError::Oversized);
    }
    read_input_body(&payload)
}

/// The owner's command progress, repeated in every state packet so the
/// newest one received is always current.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OwnerStatus {
    /// Every command targeting this tick or earlier has been consumed or
    /// replaced; the client stops sending them.
    pub finalized: ServerTick,
    pub menu_applied: Option<CommandSeq>,
    /// Every command up to this sequence has reached the server, whether or
    /// not it could still be used; the client stops repeating them.
    pub received: CommandSeq,
    pub arrival: ArrivalFeedback,
}

/// One entity's state, whole or as changes against the state the stream
/// named as its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateEntry {
    pub entity: EntityId,
    pub base: Option<u32>,
    pub body: Vec<u8>,
}

/// Server to client, unreliable. Every entry is the entity's state at `tick`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatePacket {
    /// The stream sequence, carried in the frame header.
    pub seq: u32,
    pub tick: ServerTick,
    pub status: OwnerStatus,
    pub entries: Vec<StateEntry>,
    pub removed: Vec<EntityId>,
}

fn put_entity(out: &mut Vec<u8>, entity: EntityId) {
    codec::put_varint(out, entity.index);
    codec::put_varint(out, u64::from(entity.generation));
}
fn read_entity(input: &mut ByteReader<'_>) -> Result<EntityId, CodecError> {
    let entity = EntityId {
        index: input.varint()?,
        generation: input.varint_u32()?,
    };
    if entity.index == 0 || entity.generation == 0 {
        return Err(invalid("invalid entity"));
    }
    Ok(entity)
}

impl StatePacket {
    fn body(&self) -> Result<Vec<u8>, CodecError> {
        if self.seq == 0
            || self.entries.len() > MAX_STATE_ENTRIES
            || self.removed.len() > MAX_STATE_REMOVALS
            || !self.status.arrival.validate()
        {
            return Err(invalid("invalid state packet"));
        }
        let mut out = Vec::with_capacity(64);
        codec::put_varint(&mut out, self.tick.0);
        codec::put_varint(&mut out, self.status.finalized.0);
        codec::put_varint(&mut out, self.status.menu_applied.map_or(0, |seq| seq.0));
        codec::put_varint(&mut out, self.status.received.0);
        let arrival = &self.status.arrival;
        codec::put_varint(&mut out, arrival.observed_commands);
        codec::put_varint(&mut out, arrival.late_commands);
        match arrival.sample {
            None => out.push(0),
            Some(sample) => {
                out.push(1);
                codec::put_varint(&mut out, sample.sequence.0);
                codec::put_varint(&mut out, sample.target.0);
                codec::put_varint(&mut out, sample.arrival_tick.0);
                out.push(sample.slack);
            }
        }
        codec::put_varint(&mut out, self.entries.len() as u64);
        for entry in &self.entries {
            put_entity(&mut out, entry.entity);
            // The base is named by its distance back, at most the stream window.
            let distance = match entry.base {
                None => 0,
                Some(base) if base < self.seq && self.seq - base <= STREAM_WINDOW => {
                    self.seq - base
                }
                Some(_) => return Err(invalid("state base outside the window")),
            };
            codec::put_varint(&mut out, u64::from(distance));
            codec::put_varint(&mut out, entry.body.len() as u64);
            out.extend_from_slice(&entry.body);
        }
        codec::put_varint(&mut out, self.removed.len() as u64);
        for entity in &self.removed {
            put_entity(&mut out, *entity);
        }
        Ok(out)
    }
    fn parse(seq: u32, bytes: &[u8]) -> Result<Self, CodecError> {
        if seq == 0 {
            return Err(invalid("invalid state sequence"));
        }
        let mut input = ByteReader::new(bytes);
        let tick = ServerTick(input.varint()?);
        let finalized = ServerTick(input.varint()?);
        let menu = input.varint()?;
        let received = CommandSeq(input.varint()?);
        let observed_commands = input.varint()?;
        let late_commands = input.varint()?;
        let sample = match input.u8()? {
            0 => None,
            1 => Some(ArrivalSample {
                sequence: CommandSeq(input.varint()?),
                target: TargetTick(input.varint()?),
                arrival_tick: ServerTick(input.varint()?),
                slack: input.u8()?,
            }),
            _ => return Err(invalid("invalid arrival sample")),
        };
        let arrival = ArrivalFeedback {
            sample,
            observed_commands,
            late_commands,
        };
        if !arrival.validate() {
            return Err(invalid("invalid arrival feedback"));
        }
        let count = usize::try_from(input.varint()?).map_err(|_| invalid("entry count"))?;
        if count > MAX_STATE_ENTRIES {
            return Err(invalid("too many state entries"));
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let entity = read_entity(&mut input)?;
            let distance = input.varint_u32()?;
            let base = match distance {
                0 => None,
                distance if distance <= STREAM_WINDOW && distance < seq => Some(seq - distance),
                _ => return Err(invalid("state base outside the window")),
            };
            let size = usize::try_from(input.varint()?).map_err(|_| invalid("entry size"))?;
            let body = input.take(size)?.to_vec();
            entries.push(StateEntry { entity, base, body });
        }
        let count = usize::try_from(input.varint()?).map_err(|_| invalid("removal count"))?;
        if count > MAX_STATE_REMOVALS {
            return Err(invalid("too many removals"));
        }
        let mut removed = Vec::with_capacity(count);
        for _ in 0..count {
            removed.push(read_entity(&mut input)?);
        }
        input.finish()?;
        Ok(Self {
            seq,
            tick,
            status: OwnerStatus {
                finalized,
                menu_applied: (menu != 0).then_some(CommandSeq(menu)),
                received,
                arrival,
            },
            entries,
            removed,
        })
    }
    /// Frame bytes this packet would occupy before compression.
    pub fn encoded_len(&self) -> Result<usize, CodecError> {
        Ok(codec::HEADER_BYTES + self.body()?.len())
    }
}
pub fn encode_state(
    value: &StatePacket,
    epoch: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<Vec<u8>, CodecError> {
    codec::encode_frame_deflated(
        FrameHeader {
            lane: Lane::State,
            connection: epoch,
            sequence: value.seq,
        },
        &value.body()?,
        limit,
    )
}
pub fn decode_state(
    bytes: &[u8],
    epoch: ConnectionEpoch,
    limit: FrameLimit,
) -> Result<StatePacket, CodecError> {
    let (header, payload) = codec::decode_frame_inflated(bytes, epoch, limit)?;
    if header.lane != Lane::State {
        return Err(CodecError::InvalidLane);
    }
    StatePacket::parse(header.sequence, &payload)
}

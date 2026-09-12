//! Bounded baseline envelopes for the complete declared prediction group.
//! Target identity is carried by the outer full state and group publication.
use super::*;
use engine_net::replication::baselines::*;

pub const MIN_MEMBERS: usize = 3;
pub const MEMBERS: usize = 4;
pub fn valid_member_count(count: usize) -> bool {
    (MIN_MEMBERS..=MEMBERS).contains(&count)
}
pub const PAYLOAD_BYTES: usize = 16_384;
pub const MEMBER_BYTES: usize = PAYLOAD_BYTES + 256;
pub const GROUP_BYTES: usize = 32 * 1024;
pub const GROUP_FRAGMENT_COUNT: usize = 32;
pub const GROUP_INCOMPLETE: usize = 8;
pub const GROUP_STAGING_BYTES: usize = GROUP_BYTES * GROUP_INCOMPLETE;
pub const GROUP_RESIDENT_BYTES: usize = MEMBERS * 160 * 1024;
pub const GROUP_PEAK_BYTES: usize = GROUP_RESIDENT_BYTES * 2 + GROUP_STAGING_BYTES;

pub fn limits() -> BaselineLimits {
    BaselineLimits::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodedBaseline {
    pub state: StateReceipt,
    pub baseline: BaselineReceipt,
}

pub fn context(state: &FullState<Vec<u8>>, publication: GroupPublication) -> BaselineContext {
    BaselineContext {
        scope: state.scope,
        schema: SchemaId(1),
        group: Some((publication.group, publication.revision)),
    }
}

pub fn target(
    state: &FullState<Vec<u8>>,
    publication: GroupPublication,
    generation: BaselineGeneration,
) -> BaselineReceipt {
    BaselineReceipt {
        context: context(state, publication),
        generation,
        snapshot: state.snapshot,
        version: state.version,
        end_tick: state.end_tick,
    }
}

pub fn proof(state: &FullState<Vec<u8>>, baseline: BaselineReceipt) -> DecodedBaseline {
    DecodedBaseline {
        state: state.receipt(),
        baseline,
    }
}

pub(super) fn valid_context(value: BaselineContext) -> bool {
    valid_scope(value.scope)
        && value.schema == SchemaId(1)
        && value
            .group
            .is_some_and(|(group, revision)| group == OWNER_GROUP && revision.0 != 0)
}

pub(super) fn valid_baseline(value: BaselineReceipt) -> bool {
    valid_context(value.context)
        && value.generation.0 != 0
        && value.snapshot.0 != 0
        && value.version.0 != 0
}

pub(super) fn valid_proof(value: &DecodedBaseline) -> bool {
    valid_receipt(&value.state)
        && valid_baseline(value.baseline)
        && value.state.scope == value.baseline.context.scope
        && value.state.snapshot == value.baseline.snapshot
        && value.state.version == value.baseline.version
}

#[derive(Serialize, Deserialize)]
struct Member {
    generation: BaselineGeneration,
    base: Option<BaselineReceipt>,
    bytes: BoundedVec<u8, { PAYLOAD_BYTES + 12 }>,
}
impl Validate for Member {
    fn validate(&self) -> Result<(), CodecError> {
        if self.generation.0 == 0 || self.base.is_some_and(|base| !valid_baseline(base)) {
            return Err(invalid("invalid baseline member"));
        }
        Ok(())
    }
}

pub fn encode_member(packet: &BaselinePacket) -> Result<Vec<u8>, CodecError> {
    if !valid_baseline(packet.target) {
        return Err(invalid("invalid baseline target"));
    }
    let base = match packet.encoding {
        BaselineEncoding::Full => None,
        BaselineEncoding::Delta { base } => Some(base),
    };
    validate_base(packet.target, base)?;
    codec::encode_payload(
        &Member {
            generation: packet.target.generation,
            base,
            bytes: BoundedVec::new(packet.bytes.clone()).map_err(|_| CodecError::Oversized)?,
        },
        MEMBER_BYTES,
    )
}

fn validate_base(target: BaselineReceipt, base: Option<BaselineReceipt>) -> Result<(), CodecError> {
    if base.is_some_and(|base| {
        !valid_baseline(base)
            || base.context != target.context
            || base.generation != target.generation
            || base.snapshot >= target.snapshot
    }) {
        return Err(invalid("baseline dependency differs from target context"));
    }
    Ok(())
}

pub fn decode_member(
    bytes: &[u8],
    state: &FullState<Vec<u8>>,
    publication: GroupPublication,
) -> Result<BaselinePacket, CodecError> {
    if publication.connection != state.scope.connection || publication.group != OWNER_GROUP {
        return Err(invalid("baseline group context mismatch"));
    }
    let member: Member = codec::decode_payload(bytes, MEMBER_BYTES)?;
    let target = target(state, publication, member.generation);
    if !valid_baseline(target) {
        return Err(invalid("invalid baseline target"));
    }
    validate_base(target, member.base)?;
    Ok(BaselinePacket {
        target,
        encoding: member
            .base
            .map_or(BaselineEncoding::Full, |base| BaselineEncoding::Delta {
                base,
            }),
        bytes: member.bytes.as_slice().to_vec(),
    })
}

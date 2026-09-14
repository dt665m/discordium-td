//! Bounded baseline envelopes for the complete declared prediction group.
//! Target identity is carried by the outer full state and group publication.
use super::*;
use engine_net::replication::baselines::*;

pub const MIN_MEMBERS: usize = 3;
pub const MEMBERS: usize = 4;
pub fn valid_member_count(count: usize) -> bool {
    (MIN_MEMBERS..=MEMBERS).contains(&count)
}
pub const PAYLOAD_BYTES: usize = 48 * 1024;
pub const MEMBER_BYTES: usize = PAYLOAD_BYTES + 256;
const _: () = assert!(MEMBER_BYTES <= u16::MAX as usize);
pub const GROUP_BYTES: usize = OWNER_GROUP_MESSAGE_BYTES - codec::HEADER_BYTES - 1;
pub const GROUP_INCOMPLETE: usize = 8;
pub const GROUP_STAGING_BYTES: usize = GROUP_BYTES * GROUP_INCOMPLETE;
pub const GROUP_RESIDENT_BYTES: usize = MEMBERS * 512 * 1024;
pub const GROUP_PEAK_BYTES: usize = GROUP_RESIDENT_BYTES * 2 + GROUP_STAGING_BYTES;

pub fn limits() -> BaselineLimits {
    BaselineLimits {
        payload_bytes: PAYLOAD_BYTES,
        packet_bytes: PAYLOAD_BYTES + 12,
        resident_bytes: 512 * 1024,
        peak_bytes: 640 * 1024,
        ..BaselineLimits::default()
    }
}

pub fn scope_limits() -> ScopeLimits {
    ScopeLimits {
        payload_bytes: PAYLOAD_BYTES,
        ..ScopeLimits::default()
    }
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
            || base.version > target.version
            || base.end_tick > target.end_tick
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_base_cannot_advance_beyond_target_version_or_tick() {
        let state = FullState {
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
            snapshot: SnapshotId(2),
            version: StateVersion(2),
            end_tick: ServerTick(11),
            payload: vec![],
        };
        let publication = GroupPublication {
            connection: ConnectionEpoch(1),
            group: OWNER_GROUP,
            revision: GroupRevision(1),
            snapshot: SnapshotId(2),
        };
        let target = target(&state, publication, BaselineGeneration(1));
        for bad_version in [false, true] {
            let mut base = BaselineReceipt {
                snapshot: SnapshotId(1),
                ..target
            };
            if bad_version {
                base.version = StateVersion(3);
            } else {
                base.end_tick = ServerTick(12);
            }
            assert!(
                encode_member(&BaselinePacket {
                    target,
                    encoding: BaselineEncoding::Delta { base },
                    bytes: vec![],
                })
                .is_err()
            );
            // Encode only the bounded wrapper to exercise the hostile wire path.
            let bytes = codec::encode_payload(
                &Member {
                    generation: BaselineGeneration(1),
                    base: Some(base),
                    bytes: BoundedVec::new(vec![]).unwrap(),
                },
                MEMBER_BYTES,
            )
            .unwrap();
            assert!(decode_member(&bytes, &state, publication).is_err());
        }
    }
}

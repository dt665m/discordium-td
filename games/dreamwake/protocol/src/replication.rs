//! Explicit positive payloads for Dreamwake scopes. Complete authority snapshots
//! are an offline/server checkpoint format and have no variant in this protocol.
use dreamwake_sim::replication::{
    DreamGlobalView, OwnerCheckpoint, OwnerExpectation, PublicReplica,
};
use engine_net::codec::BoundedVec;

const MAX_REPLICA_BYTES: usize = 16_384;
// Outer type byte + raw/compressed marker + fixed bincode collection length.
const MAX_DTO_BYTES: usize = MAX_REPLICA_BYTES - 10;
type DtoBytes = BoundedVec<u8, MAX_DTO_BYTES>;

#[derive(Debug, Clone, PartialEq)]
pub enum ReplicaPayload {
    Global(DreamGlobalView),
    Owner(OwnerCheckpoint),
    Actor(PublicReplica),
    Collision(dreamwake_sim::collision::CollisionManifest),
}

pub fn encode_replica(
    value: &ReplicaPayload,
) -> Result<Vec<u8>, dreamwake_sim::replication::ReplicationError> {
    encode_replica_with_compression(value, engine_net::CompressionPolicy::default())
}

/// Apply one outgoing policy consistently to public and private replica bodies.
/// Compression changes no content or decode authorization requirements.
pub fn encode_replica_with_compression(
    value: &ReplicaPayload,
    policy: engine_net::CompressionPolicy,
) -> Result<Vec<u8>, dreamwake_sim::replication::ReplicationError> {
    let (tag, bytes) = match value {
        ReplicaPayload::Global(value) => (0, value.encode()?),
        ReplicaPayload::Owner(value) => (1, value.encode()?),
        ReplicaPayload::Actor(value) => (2, value.encode_compact()?),
        ReplicaPayload::Collision(value) => (3, value.encode()?),
    };
    let dto = DtoBytes::new(bytes)
        .map_err(|_| dreamwake_sim::replication::ReplicationError::BudgetExceeded)?;
    // The shared codec uses lossless compression only when it reduces size.
    // No private/public schema or f32 recurrence changes with this encoding.
    let encoded = engine_net::encode_with_compression(&dto, policy);
    let mut framed = Vec::with_capacity(encoded.len() + 1);
    framed.push(tag);
    framed.extend_from_slice(&encoded);
    Ok(framed)
}

/// The connection's authenticated owner/epoch/scene expectation is mandatory for
/// private state. A self-described owner ID from the payload grants no access.
/// The authenticated session must pass exact protocol/content negotiation before
/// live decoding: public actors use the compact agreed schema without repeated
/// per-field metadata. Private owner validation remains independently required.
pub fn decode_replica(
    bytes: &[u8],
    owner: Option<OwnerExpectation>,
) -> Result<ReplicaPayload, dreamwake_sim::replication::ReplicationError> {
    use dreamwake_sim::replication::ReplicationError;
    if bytes.len() > MAX_REPLICA_BYTES {
        return Err(ReplicationError::BudgetExceeded);
    }
    let (tag, body) = bytes.split_first().ok_or(ReplicationError::InvalidState)?;
    if *tag > 3 {
        return Err(ReplicationError::InvalidState);
    }
    if *tag == 1 && owner.is_none() {
        return Err(ReplicationError::WrongOwner);
    }
    // Both expansion and the inner collection count are bounded before their
    // allocations. The DTO decoder then applies its own field/count validation.
    let dto: DtoBytes = engine_net::decode_with_limit(body, MAX_DTO_BYTES + 8)
        .map_err(|_| ReplicationError::InvalidState)?;
    let body = dto.as_slice();
    match tag {
        0 => DreamGlobalView::decode(body).map(ReplicaPayload::Global),
        1 => OwnerCheckpoint::decode(body, owner.ok_or(ReplicationError::WrongOwner)?)
            .map(ReplicaPayload::Owner),
        2 => PublicReplica::decode_compact(body).map(ReplicaPayload::Actor),
        3 => {
            dreamwake_sim::collision::CollisionManifest::decode(body).map(ReplicaPayload::Collision)
        }
        _ => Err(ReplicationError::InvalidState),
    }
}

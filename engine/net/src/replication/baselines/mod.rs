//! Opt-in bounded baseline protocol for one exact authorized representation.
//!
//! This layer does not grant interest or schedule transport. Call server transmit
//! only inside the graph/policy and scheduler admission callback. Close the cache
//! at revocation, scope exit, schema/representation or group-manifest change;
//! create a new cache for the new identity. Never reuse a cache across peers.
//! Group users must stage these caches and commit all members together, then emit
//! receipts, only after the existing atomic manifest/scene validation succeeds.
//! A per-member receipt before group commit violates the retention contract.
//!
//! Cache count is independently bounded by the caller's scope/group limits. Each
//! cache charges fixed Vec capacity plus payload capacities. Operations need at
//! most one decoded payload and one encoded packet beyond resident storage.
//! Codec implementations are trusted and must honor the supplied allocation cap;
//! the supplied BytePatch codec does so without general-purpose deserialization.
mod bytes;
mod cache;
#[cfg(test)]
mod tests;
use super::{Payload, ReplicationError, validate_scope};
use crate::types::*;
pub use bytes::BytePatch;
pub use cache::{ClientBaselines, ServerBaselines};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineContext {
    pub scope: ScopeIdentity,
    pub schema: SchemaId,
    /// Must change whenever the complete group member manifest changes.
    pub group: Option<(GroupId, GroupRevision)>,
}
impl BaselineContext {
    fn validate(self) -> Result<(), ReplicationError> {
        validate_scope(self.scope, self.scope.connection)?;
        if self.schema.0 == 0 || self.group.is_some_and(|(id, rev)| id.0 == 0 || rev.0 == 0) {
            return Err(ReplicationError::InvalidIdentity);
        }
        Ok(())
    }
}

/// Exact canonical state proof, including tick. It is not a transport receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineReceipt {
    pub context: BaselineContext,
    pub generation: BaselineGeneration,
    pub snapshot: SnapshotId,
    pub version: StateVersion,
    pub end_tick: ServerTick,
}
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineState<P> {
    pub receipt: BaselineReceipt,
    pub payload: P,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineEncoding {
    Full,
    Delta { base: BaselineReceipt },
}
/// Outer wire framing must bound bytes before allocating this value. Codec bytes
/// are not self-authorizing: the exact context is carried in this envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselinePacket {
    pub target: BaselineReceipt,
    pub encoding: BaselineEncoding,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineRetirement {
    pub context: BaselineContext,
    pub generation: BaselineGeneration,
    /// Cumulative snapshot fence, so no unbounded retired-ID set is necessary.
    pub through: SnapshotId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineReset {
    pub context: BaselineContext,
    pub generation: BaselineGeneration,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineRepair {
    pub context: BaselineContext,
    pub generation: BaselineGeneration,
}

#[derive(Debug, Clone, Copy)]
pub struct BaselineLimits {
    pub slots: usize,
    pub payload_bytes: usize,
    pub packet_bytes: usize,
    /// Includes Entry<P> vector capacity and owned payload capacities.
    pub resident_bytes: usize,
    /// Resident ceiling + one decoded payload + one encoded packet.
    pub peak_bytes: usize,
    pub repair_interval_ms: u64,
}
impl Default for BaselineLimits {
    fn default() -> Self {
        Self {
            slots: 8,
            payload_bytes: 16384,
            packet_bytes: 16384 + 12,
            resident_bytes: 160 * 1024,
            peak_bytes: 200 * 1024,
            repair_interval_ms: 1000,
        }
    }
}
/// Pure canonical codec. Returned payload/byte capacities must not exceed the
/// supplied limit; bound allocation before construction, not only after decode.
/// Clone for P must preserve or reduce retained capacity. P equality must compare
/// all canonical fields, never presentation tolerances or only visible fields.
pub trait BaselineCodec<P: Payload> {
    fn encode_full(&self, value: &P, limit: usize) -> Result<Vec<u8>, ReplicationError>;
    fn encode_delta(&self, base: &P, value: &P, limit: usize) -> Result<Vec<u8>, ReplicationError>;
    fn decode_full(&self, bytes: &[u8], limit: usize) -> Result<P, ReplicationError>;
    fn decode_delta(&self, base: &P, bytes: &[u8], limit: usize) -> Result<P, ReplicationError>;
    fn validate(&self, value: &P) -> bool;
}

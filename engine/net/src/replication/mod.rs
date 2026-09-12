//! Bounded, full-state scoped replication primitives, without sockets or gameplay.
//!
//! The game must positively project an authorized representation before offering
//! payloads. Selection is not transmission, and transmission is not decode. All
//! methods run on their owning connection thread; callers serialize publication
//! and policy barriers. The opt-in `baselines` layer adds decoded-retention
//! delta contracts; existing scope APIs continue to use complete states.
pub mod attachments;
pub mod audience;
pub mod baselines;
mod bytes;
mod client;
mod codec;
mod groups;
pub mod regions;
mod server;
use crate::types::*;
pub use bytes::*;
pub use client::*;
pub use codec::*;
pub use groups::*;
use serde::{Deserialize, Serialize};
pub use server::*;

/// Canonical, already validated representation storage. Implementations must
/// account for owned heap capacity, not only encoded length. The adapter must
/// bound decoding allocations before constructing this value.
pub trait Payload: Clone + PartialEq {
    fn retained_bytes(&self) -> usize;
}
impl Payload for Vec<u8> {
    fn retained_bytes(&self) -> usize {
        self.capacity()
    }
}
impl Payload for Box<[u8]> {
    fn retained_bytes(&self) -> usize {
        self.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsoleteReason {
    Scope,
    Publication,
    RetiredBaseline,
}
impl std::fmt::Display for ObsoleteReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Scope => "retired replication scope",
            Self::Publication => "superseded replication publication",
            Self::RetiredBaseline => "retired replication baseline",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationError {
    InvalidConfiguration,
    InvalidIdentity,
    Stale,
    /// A valid identity is provably behind a retained publication or fence.
    /// The packet remains rejected and must not produce a retention receipt.
    Obsolete(ObsoleteReason),
    /// A validated full seed for exactly the next generation must wait for its
    /// reliable reset. This neither publishes state nor promises retention.
    BaselineResetPending,
    Conflict,
    Unauthorized,
    Unknown,
    ResetRequired,
    Capacity,
    Expired,
    Incomplete,
    InvalidPayload,
}

impl std::fmt::Display for ReplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidConfiguration => "invalid replication limits",
            Self::InvalidIdentity => "invalid replication identity",
            Self::Stale => "stale replication incarnation or publication",
            Self::Obsolete(reason) => return reason.fmt(f),
            Self::BaselineResetPending => "next baseline generation awaits its reset",
            Self::Conflict => "conflicting replication identity or payload",
            Self::Unauthorized => "representation not authorized at the current barrier",
            Self::Unknown => "unknown replication proof or entity",
            Self::ResetRequired => "bounded replication fence exhausted; reset required",
            Self::Capacity => "replication capacity exceeded",
            Self::Expired => "replication assembly expired",
            Self::Incomplete => "replication group incomplete",
            Self::InvalidPayload => "invalid canonical replication payload",
        };
        f.write_str(message)
    }
}
impl std::error::Error for ReplicationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeLimits {
    /// Includes retired incarnation fences; never evict a fence silently.
    pub known_entities: usize,
    pub pending_transitions: usize,
    pub retained_payload_bytes: usize,
    pub payload_bytes: usize,
    pub sent_proofs_per_scope: usize,
}
impl Default for ScopeLimits {
    fn default() -> Self {
        Self {
            known_entities: 8192,
            pending_transitions: 1024,
            retained_payload_bytes: 4 * 1024 * 1024,
            payload_bytes: 16384,
            sent_proofs_per_scope: 8,
        }
    }
}
impl ScopeLimits {
    fn validate(self) -> Result<Self, ReplicationError> {
        if self.known_entities == 0
            || self.pending_transitions == 0
            || self.payload_bytes == 0
            || self.retained_payload_bytes < self.payload_bytes
            || self.sent_proofs_per_scope == 0
        {
            Err(ReplicationError::InvalidConfiguration)
        } else {
            Ok(self)
        }
    }
}

/// A complete canonical state under one exact incarnation. No partial/delta
/// payload may use this message kind. Public fields permit bounded wire decoding;
/// server send admission goes through ServerScopes, never an unrestricted list.
#[derive(Debug, Clone, PartialEq)]
pub struct FullState<P> {
    pub scope: ScopeIdentity,
    pub baseline_generation: BaselineGeneration,
    pub snapshot: SnapshotId,
    pub version: StateVersion,
    pub end_tick: ServerTick,
    pub payload: P,
}
impl<P> FullState<P> {
    pub fn receipt(&self) -> StateReceipt {
        StateReceipt {
            scope: self.scope,
            baseline_generation: self.baseline_generation,
            snapshot: self.snapshot,
            version: self.version,
        }
    }
    fn validate_identity(&self, connection: ConnectionEpoch) -> Result<(), ReplicationError> {
        validate_scope(self.scope, connection)?;
        if self.baseline_generation.0 == 0 || self.snapshot.0 == 0 || self.version.0 == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        Ok(())
    }
}

/// Decode/retention proof, distinct from a transport receipt and finalized input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateReceipt {
    pub scope: ScopeIdentity,
    pub baseline_generation: BaselineGeneration,
    pub snapshot: SnapshotId,
    pub version: StateVersion,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeExit {
    pub scope: ScopeIdentity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destroy {
    pub connection: ConnectionEpoch,
    pub entity: EntityId,
}

fn validate_scope(
    scope: ScopeIdentity,
    connection: ConnectionEpoch,
) -> Result<(), ReplicationError> {
    if scope.connection != connection {
        return Err(ReplicationError::Stale);
    }
    if connection.0 == 0
        || scope.entity.generation == 0
        || scope.scope.0 == 0
        || scope.representation.0 == 0
    {
        return Err(ReplicationError::InvalidIdentity);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

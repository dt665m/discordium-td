//! Positive, audience-selected replication projections. Full DreamSnapshot and
//! DreamCheckpoint remain authority/offline data and are never nested in these
//! payloads. CommittedReplication deliberately does not implement Serialize.
//!
//! The graph/server authenticates the peer, chooses eligible actor IDs and grants
//! source references before calling projections. This module does not grant
//! observer entitlement or turn spatial proximity into permission. Owner-only
//! restoration is isolated from public render replicas and from the AI world.
mod audience;
mod capture;
pub(crate) mod schema;
pub use schema::{owner_group_schema_maxima, schema_identities, schema_identity};
#[cfg(test)]
mod collision_tests;
#[cfg(test)]
mod crowding_tests;
mod owner;
#[cfg(test)]
mod platform_tests;
mod presentation;
mod public;
#[cfg(test)]
mod tests;
mod wire;
pub use capture::{ApprovedSources, CommittedReplication, ReplicationKey};
pub use owner::{
    OWNER_CHECKPOINT_SCHEMA, OwnerCheckpoint, OwnerCombatAction, OwnerExpectation,
    OwnerPredictionState,
};
pub use presentation::DreamPresentation;
pub use public::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicationError {
    InvalidIdentity,
    InvalidState,
    SchemaMismatch,
    WrongOwner,
    WrongEpoch,
    WrongScene,
    StaleRevision,
    MissingOwner,
    DuplicateReplica,
    BudgetExceeded,
    Codec(String),
}
impl std::fmt::Display for ReplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ReplicationError {}
fn codec(error: serde_json::Error) -> ReplicationError {
    ReplicationError::Codec(error.to_string())
}

//! Persistent replication interest over committed authoritative state.
//!
//! The sparse influence grid uses canonical XZ ground coordinates (+Y up), with
//! exact three-dimensional distance checks. Routing is only a source of candidates:
//! connection grants, hard visibility, scene and field policy still authorize every
//! result. Nothing here authenticates network requests or serializes gameplay state.
//!
//! The authority applies committed changes, prepares once per replication frame,
//! then gathers through an immutable borrow. Consumers must fence an `EligibleSet`
//! against the current world and policy revisions immediately before encoding.
mod canonical;
mod gather;
mod graph;
mod model;
#[cfg(test)]
mod tests;

pub use canonical::{CanonicalCacheError, CanonicalCacheStats, CanonicalPayloadCache};
pub use gather::{
    DependencyError, EligibleEntry, EligibleSet, GatherError, PredictionAdmission, PreparedGraph,
};
pub use graph::{Accounting, Change, Graph, GraphError};
pub use model::{
    Bounds, ConnectionAuthorizer, ConnectionView, DisclosurePolicy, EligibilityReasons,
    EntityRegistration, Limits, ObserverGrant, RepresentationGrant, Routes, SemanticId,
    SpatialRoute, Visibility,
};

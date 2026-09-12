use crate::types::{
    ConnectionId, EntityId, PolicyRevision, RepresentationRevision, SceneRevision, StateVersion,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub center: [f64; 3],
    /// Conservative world-space bounding sphere, used by broad and exact phases.
    pub radius: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpatialRoute {
    Static,
    Dynamic,
    /// The authority supplies committed pose changes when awake, explicit mutations
    /// when dormant. Delivery completion remains independently per connection.
    DormancyDriven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SemanticId(pub u64);

/// Multiple routes are permitted; removing one does not remove the others.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routes {
    pub spatial: Option<SpatialRoute>,
    pub global: bool,
    pub owner: Option<ConnectionId>,
    pub semantic: BTreeSet<SemanticId>,
}

/// Hard access restrictions, independent of candidate routing. A required or global
/// reason never overrides this check or the application field policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Owner(ConnectionId),
    Semantic(SemanticId),
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityRegistration {
    pub id: EntityId,
    pub bounds: Bounds,
    pub cull_radius: f64,
    pub routes: Routes,
    pub visibility: Visibility,
    pub dependencies: BTreeSet<EntityId>,
    pub state_version: StateVersion,
    pub scene_revision: SceneRevision,
    pub dormant: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ObserverGrant {
    pub position: [f64; 3],
    pub scene_revision: SceneRevision,
}

/// This is an authority-side proposed view, not a client subscription message.
/// Installation requires an explicit authorizer and bounded, finite coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionView {
    pub observers: Vec<ObserverGrant>,
    pub semantic_grants: BTreeSet<SemanticId>,
    pub ready_scenes: BTreeSet<SceneRevision>,
}

/// The game must verify authenticated ownership, camera envelope, scene readiness,
/// spectator timeline and semantic entitlements before approving a view. The graph
/// independently enforces resource limits. There is deliberately no allow-all default.
pub trait ConnectionAuthorizer {
    fn authorize(&self, connection: ConnectionId, view: &ConnectionView) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepresentationGrant {
    pub schema_id: u64,
    pub revision: RepresentationRevision,
    /// Schema-defined field selection. This is not a mask over arbitrary ECS fields.
    pub fields: u64,
    /// True only if this representation contains all state needed for prediction.
    pub prediction_allowed: bool,
}

/// Evaluate against immutable committed policy. The revision must change whenever
/// any decision can change. Scene/readiness checks are also enforced by the graph.
pub trait DisclosurePolicy {
    fn revision(&self) -> PolicyRevision;
    fn representation(
        &self,
        connection: ConnectionId,
        entity: &EntityRegistration,
    ) -> Option<RepresentationGrant>;
    fn permits_dependency(
        &self,
        connection: ConnectionId,
        source: EntityId,
        target: EntityId,
    ) -> bool;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EligibilityReasons(u8);
impl EligibilityReasons {
    pub const SPATIAL: Self = Self(1);
    pub const GLOBAL: Self = Self(2);
    pub const OWNER: Self = Self(4);
    pub const SEMANTIC: Self = Self(8);
    pub const REQUIRED: Self = Self(16);
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    pub(crate) fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
    pub(crate) fn without_spatial(self) -> Self {
        Self(self.0 & !Self::SPATIAL.0)
    }
    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Independent admission and work ceilings. Profiles must explicitly override these
/// values; parsing a second configuration file does not imply profile merging.
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub cell_size: f64,
    pub leave_margin: f64,
    /// Approved lookahead only; exact entry checks include this radius, while
    /// hard visibility, scene readiness and field policy still apply.
    pub prefetch: f64,
    pub max_coordinate: f64,
    pub max_entities: usize,
    pub max_cells_per_entity: usize,
    pub max_cells: usize,
    pub max_memberships: usize,
    pub max_route_memberships: usize,
    pub max_semantic_routes_per_entity: usize,
    pub max_connections: usize,
    pub max_observers: usize,
    pub max_semantic_grants: usize,
    pub max_ready_scenes: usize,
    pub max_candidates: usize,
    pub max_candidate_visits: usize,
    pub max_required: usize,
    pub max_dependency_edges: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            cell_size: 32.0,
            leave_margin: 10.0,
            prefetch: 8.0,
            max_coordinate: 1_000_000_000.0,
            max_entities: 100_000,
            max_cells_per_entity: 256,
            max_cells: 65_536,
            max_memberships: 2_000_000,
            max_route_memberships: 400_000,
            max_semantic_routes_per_entity: 8,
            max_connections: 1024,
            max_observers: 2,
            max_semantic_grants: 16,
            max_ready_scenes: 64,
            max_candidates: 4096,
            max_candidate_visits: 32_768,
            max_required: 128,
            max_dependency_edges: 16_384,
        }
    }
}

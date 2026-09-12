use super::{
    ConnectionAuthorizer, ConnectionView, EntityRegistration, Limits, PreparedGraph, SemanticId,
    SpatialRoute,
};
use crate::types::{ConnectionId, EntityId, PolicyRevision, ReplicationFrame, ServerTick};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) type Cell = (i64, i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    InvalidConfiguration,
    InvalidBounds,
    InvalidIdentity,
    GenerationConflict,
    RegressedStateVersion,
    NonMonotonicTick,
    NonMonotonicFrame,
    NonMonotonicPolicy,
    RevisionExhausted,
    UnauthorizedView,
    InvalidObserver,
    Limit(&'static str),
}
impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "replication graph: {self:?}")
    }
}
impl std::error::Error for GraphError {}

#[derive(Debug, Clone)]
pub enum Change {
    /// Includes spawn, pose/footprint changes, route/owner changes and dormancy.
    Upsert(EntityRegistration),
    Remove(EntityId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accounting {
    pub entities: usize,
    pub dynamic_entities: usize,
    pub cells: usize,
    pub memberships: usize,
    pub route_memberships: usize,
    pub connections: usize,
    pub observers: usize,
    pub prepare_calls: u64,
    pub world_revision: u64,
}

/// Authority-owned persistent index. No method accepts untrusted wire bytes. Changes
/// are validated in full before touching indices; failed changes leave all state
/// unchanged. Update work touches only old/new footprints and route lists.
pub struct Graph {
    pub(crate) limits: Limits,
    pub(crate) entities: BTreeMap<EntityId, EntityRegistration>,
    indices: BTreeMap<u64, EntityId>,
    dynamic: BTreeSet<EntityId>,
    pub(crate) cells: BTreeMap<Cell, BTreeSet<EntityId>>,
    footprints: BTreeMap<EntityId, BTreeSet<Cell>>,
    pub(crate) globals: BTreeSet<EntityId>,
    pub(crate) owners: BTreeMap<ConnectionId, BTreeSet<EntityId>>,
    pub(crate) semantics: BTreeMap<SemanticId, BTreeSet<EntityId>>,
    pub(crate) connections: BTreeMap<ConnectionId, ConnectionView>,
    memberships: usize,
    route_memberships: usize,
    observers: usize,
    pub(crate) world_revision: u64,
    last_tick: Option<ServerTick>,
    last_frame: Option<ReplicationFrame>,
    last_policy: Option<PolicyRevision>,
    prepare_calls: u64,
}
impl Graph {
    pub fn new(limits: Limits) -> Result<Self, GraphError> {
        if !limits.cell_size.is_finite()
            || limits.cell_size <= 0.0
            || !limits.max_coordinate.is_finite()
            || limits.max_coordinate <= 0.0
            || [limits.leave_margin, limits.prefetch]
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0)
            || limits.leave_margin + limits.prefetch > limits.max_coordinate
            || 2.0 * limits.max_coordinate / limits.cell_size >= (i64::MAX / 4) as f64
            || [
                limits.max_entities,
                limits.max_cells_per_entity,
                limits.max_cells,
                limits.max_memberships,
                limits.max_route_memberships,
                limits.max_semantic_routes_per_entity,
                limits.max_connections,
                limits.max_observers,
                limits.max_semantic_grants,
                limits.max_ready_scenes,
                limits.max_candidates,
                limits.max_candidate_visits,
                limits.max_required,
                limits.max_dependency_edges,
            ]
            .contains(&0)
            || limits.max_cells_per_entity > limits.max_memberships
            || limits.max_required > limits.max_candidates
            || limits.max_candidates > limits.max_candidate_visits
        {
            return Err(GraphError::InvalidConfiguration);
        }
        Ok(Self {
            limits,
            entities: BTreeMap::new(),
            indices: BTreeMap::new(),
            dynamic: BTreeSet::new(),
            cells: BTreeMap::new(),
            footprints: BTreeMap::new(),
            globals: BTreeSet::new(),
            owners: BTreeMap::new(),
            semantics: BTreeMap::new(),
            connections: BTreeMap::new(),
            memberships: 0,
            route_memberships: 0,
            observers: 0,
            world_revision: 0,
            last_tick: None,
            last_frame: None,
            last_policy: None,
            prepare_calls: 0,
        })
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    pub fn accounting(&self) -> Accounting {
        Accounting {
            entities: self.entities.len(),
            dynamic_entities: self.dynamic.len(),
            cells: self.cells.len(),
            memberships: self.memberships,
            route_memberships: self.route_memberships,
            connections: self.connections.len(),
            observers: self.observers,
            prepare_calls: self.prepare_calls,
            world_revision: self.world_revision,
        }
    }
    /// Shared pose-refresh candidates. The authority updates these once before
    /// prepare, or supplies an equivalent committed ECS change feed. Dormancy-driven
    /// actors enter this list on wake and leave it on dormant intent; delivery ACKs
    /// remain a separate per-connection replication responsibility.
    pub fn dynamic_entities(&self) -> impl ExactSizeIterator<Item = EntityId> + '_ {
        self.dynamic.iter().copied()
    }
    pub fn entity(&self, id: EntityId) -> Option<&EntityRegistration> {
        self.entities.get(&id)
    }

    fn validate_tick(&self, tick: ServerTick) -> Result<u64, GraphError> {
        if self.last_tick.is_some_and(|previous| tick < previous) {
            return Err(GraphError::NonMonotonicTick);
        }
        self.world_revision
            .checked_add(1)
            .ok_or(GraphError::RevisionExhausted)
    }
    fn committed(&mut self, tick: ServerTick, revision: u64) {
        self.last_tick = Some(tick);
        self.world_revision = revision;
    }
    pub(crate) fn valid_position(&self, position: [f64; 3]) -> bool {
        position
            .iter()
            .all(|v| v.is_finite() && v.abs() <= self.limits.max_coordinate)
    }
    pub(crate) fn cell(&self, position: [f64; 3]) -> Cell {
        (
            (position[0] / self.limits.cell_size).floor() as i64,
            (position[2] / self.limits.cell_size).floor() as i64,
        )
    }
    fn footprint_span(
        &self,
        entity: &EntityRegistration,
    ) -> Result<Option<(Cell, Cell)>, GraphError> {
        if !self.valid_position(entity.bounds.center)
            || [entity.bounds.radius, entity.cull_radius]
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(GraphError::InvalidBounds);
        }
        let radius = entity.bounds.radius
            + entity.cull_radius
            + self.limits.leave_margin
            + self.limits.prefetch;
        if !radius.is_finite() || radius > self.limits.max_coordinate {
            return Err(GraphError::InvalidBounds);
        }
        if entity.routes.spatial.is_none() {
            return Ok(None);
        }
        let center = entity.bounds.center;
        let low = self.cell([center[0] - radius, 0.0, center[2] - radius]);
        let high = self.cell([center[0] + radius, 0.0, center[2] + radius]);
        let count = (high.0 - low.0 + 1) as u128 * (high.1 - low.1 + 1) as u128;
        if count > self.limits.max_cells_per_entity as u128 {
            return Err(GraphError::Limit("cells per entity"));
        }
        Ok(Some((low, high)))
    }
    fn needs_refresh(entity: &EntityRegistration) -> bool {
        entity.routes.spatial == Some(SpatialRoute::Dynamic)
            || (entity.routes.spatial == Some(SpatialRoute::DormancyDriven) && !entity.dormant)
    }

    fn route_count(entity: &EntityRegistration) -> usize {
        usize::from(entity.routes.global)
            + usize::from(entity.routes.owner.is_some())
            + entity.routes.semantic.len()
    }

    pub fn apply(&mut self, tick: ServerTick, change: Change) -> Result<(), GraphError> {
        let revision = self.validate_tick(tick)?;
        match change {
            Change::Remove(id) => self.remove_indices(id),
            Change::Upsert(entity) => {
                if entity.id.generation == 0
                    || entity.state_version.0 == 0
                    || entity.dependencies.iter().any(|id| id.generation == 0)
                {
                    return Err(GraphError::InvalidIdentity);
                }
                if self
                    .indices
                    .get(&entity.id.index)
                    .is_some_and(|id| *id != entity.id)
                {
                    return Err(GraphError::GenerationConflict);
                }
                let old = self.entities.get(&entity.id);
                if old.is_some_and(|old| entity.state_version < old.state_version) {
                    return Err(GraphError::RegressedStateVersion);
                }
                if old.is_none() && self.entities.len() >= self.limits.max_entities {
                    return Err(GraphError::Limit("entities"));
                }
                if entity.dependencies.len() > self.limits.max_required {
                    return Err(GraphError::Limit("declared dependencies"));
                }
                if entity.routes.semantic.len() > self.limits.max_semantic_routes_per_entity {
                    return Err(GraphError::Limit("semantic routes"));
                }
                let same_routes = old.is_some_and(|old| old.routes == entity.routes);
                let same_geometry = old.is_some_and(|old| {
                    old.bounds == entity.bounds && old.cull_radius == entity.cull_radius
                });
                // Version-only changes need no new geometry validation. Changed
                // geometry is validated before comparing its influence rectangle;
                // exact positions still update even when no reverse list changes.
                let span = if same_routes && same_geometry {
                    None
                } else {
                    Some(self.footprint_span(&entity)?)
                };
                let same_footprint = same_routes
                    && (same_geometry || { self.footprint_span(old.unwrap())? == span.unwrap() });
                if same_footprint {
                    let was_dynamic = Self::needs_refresh(old.unwrap());
                    let is_dynamic = Self::needs_refresh(&entity);
                    if was_dynamic != is_dynamic {
                        if is_dynamic {
                            self.dynamic.insert(entity.id);
                        } else {
                            self.dynamic.remove(&entity.id);
                        }
                    }
                    let id = entity.id;
                    *self.entities.get_mut(&id).unwrap() = entity;
                    self.committed(tick, revision);
                    return Ok(());
                }
                let footprint = span.flatten().map_or_else(BTreeSet::new, |(low, high)| {
                    (low.0..=high.0)
                        .flat_map(|x| (low.1..=high.1).map(move |z| (x, z)))
                        .collect()
                });
                let previous = self.footprints.get(&entity.id);
                let old_count = previous.map_or(0, BTreeSet::len);
                let memberships = self.memberships - old_count + footprint.len();
                let allocated = footprint
                    .iter()
                    .filter(|cell| !self.cells.contains_key(cell))
                    .count();
                let freed = previous.map_or(0, |cells| {
                    cells
                        .iter()
                        .filter(|cell| {
                            !footprint.contains(cell)
                                && self.cells.get(cell).is_some_and(|ids| ids.len() == 1)
                        })
                        .count()
                });
                if memberships > self.limits.max_memberships {
                    return Err(GraphError::Limit("grid memberships"));
                }
                if self.cells.len() - freed + allocated > self.limits.max_cells {
                    return Err(GraphError::Limit("grid cells"));
                }
                let routes = self.route_memberships - old.map_or(0, Self::route_count)
                    + Self::route_count(&entity);
                if routes > self.limits.max_route_memberships {
                    return Err(GraphError::Limit("route memberships"));
                }
                // Everything which can fail has been checked. Commit only touched
                // reverse lists; no clone or scan of the full match is required.
                self.remove_indices(entity.id);
                for cell in &footprint {
                    self.cells.entry(*cell).or_default().insert(entity.id);
                }
                if entity.routes.global {
                    self.globals.insert(entity.id);
                }
                if let Some(owner) = entity.routes.owner {
                    self.owners.entry(owner).or_default().insert(entity.id);
                }
                for semantic in &entity.routes.semantic {
                    self.semantics
                        .entry(*semantic)
                        .or_default()
                        .insert(entity.id);
                }
                if Self::needs_refresh(&entity) {
                    self.dynamic.insert(entity.id);
                }
                self.indices.insert(entity.id.index, entity.id);
                self.footprints.insert(entity.id, footprint);
                self.entities.insert(entity.id, entity);
                self.memberships = memberships;
                self.route_memberships = routes;
            }
        }
        self.committed(tick, revision);
        Ok(())
    }

    fn remove_indices(&mut self, id: EntityId) {
        let Some(entity) = self.entities.remove(&id) else {
            return;
        };
        self.indices.remove(&id.index);
        self.dynamic.remove(&id);
        if let Some(cells) = self.footprints.remove(&id) {
            self.memberships -= cells.len();
            for cell in cells {
                remove_member(&mut self.cells, cell, id);
            }
        }
        self.route_memberships -= Self::route_count(&entity);
        self.globals.remove(&id);
        if let Some(owner) = entity.routes.owner {
            remove_member(&mut self.owners, owner, id);
        }
        for semantic in entity.routes.semantic {
            remove_member(&mut self.semantics, semantic, id);
        }
    }

    /// Install an already-authenticated, server-approved view at a committed barrier.
    /// A failed replacement retains the prior valid view. Revocation uses this same
    /// method (empty grants/observers are permitted) or `remove_connection`.
    pub fn set_connection(
        &mut self,
        tick: ServerTick,
        connection: ConnectionId,
        view: ConnectionView,
        authorizer: &impl ConnectionAuthorizer,
    ) -> Result<(), GraphError> {
        let revision = self.validate_tick(tick)?;
        if !self.connections.contains_key(&connection)
            && self.connections.len() >= self.limits.max_connections
        {
            return Err(GraphError::Limit("connections"));
        }
        if view.observers.len() > self.limits.max_observers
            || view.semantic_grants.len() > self.limits.max_semantic_grants
            || view.ready_scenes.len() > self.limits.max_ready_scenes
        {
            return Err(GraphError::Limit("connection grants"));
        }
        if view.observers.iter().any(|observer| {
            !self.valid_position(observer.position)
                || !view.ready_scenes.contains(&observer.scene_revision)
        }) {
            return Err(GraphError::InvalidObserver);
        }
        if !authorizer.authorize(connection, &view) {
            return Err(GraphError::UnauthorizedView);
        }
        let old_observers = self
            .connections
            .get(&connection)
            .map_or(0, |v| v.observers.len());
        self.observers = self.observers - old_observers + view.observers.len();
        self.connections.insert(connection, view);
        self.committed(tick, revision);
        Ok(())
    }
    pub fn remove_connection(
        &mut self,
        tick: ServerTick,
        connection: ConnectionId,
    ) -> Result<(), GraphError> {
        let revision = self.validate_tick(tick)?;
        if let Some(view) = self.connections.remove(&connection) {
            self.observers -= view.observers.len();
        }
        self.committed(tick, revision);
        Ok(())
    }

    /// Freeze the prepared revision using Rust's shared borrow. Dynamic pose feeds
    /// must be applied before this barrier, once globally, never per connection.
    pub fn prepare(
        &mut self,
        frame: ReplicationFrame,
        tick: ServerTick,
        policy_revision: PolicyRevision,
    ) -> Result<PreparedGraph<'_>, GraphError> {
        if self.last_frame.is_some_and(|previous| frame <= previous) {
            return Err(GraphError::NonMonotonicFrame);
        }
        if self.last_tick.is_some_and(|previous| tick < previous) {
            return Err(GraphError::NonMonotonicTick);
        }
        if self
            .last_policy
            .is_some_and(|previous| policy_revision < previous)
        {
            return Err(GraphError::NonMonotonicPolicy);
        }
        let calls = self
            .prepare_calls
            .checked_add(1)
            .ok_or(GraphError::RevisionExhausted)?;
        self.last_frame = Some(frame);
        self.last_tick = Some(tick);
        self.last_policy = Some(policy_revision);
        self.prepare_calls = calls;
        Ok(PreparedGraph {
            graph: self,
            frame,
            tick,
            policy_revision,
        })
    }
}

fn remove_member<K: Ord + Copy>(index: &mut BTreeMap<K, BTreeSet<EntityId>>, key: K, id: EntityId) {
    if let Some(ids) = index.get_mut(&key) {
        ids.remove(&id);
        if ids.is_empty() {
            index.remove(&key);
        }
    }
}

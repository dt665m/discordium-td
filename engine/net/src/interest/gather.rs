use super::{
    ConnectionView, DisclosurePolicy, EligibilityReasons, EntityRegistration, Graph,
    RepresentationGrant, Visibility,
};
use crate::types::{
    ConnectionId, EntityId, PolicyRevision, ReplicationFrame, SceneRevision, ServerTick,
    StateVersion,
};
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatherError {
    UnknownConnection,
    StalePolicy,
    CandidateLimit,
    CandidateVisitLimit,
    PreviousEligibilityLimit,
}
impl std::fmt::Display for GatherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "replication gather: {self:?}")
    }
}
impl std::error::Error for GatherError {}

/// A failed prediction group does not disclose dependency identities. Independently
/// permitted presentation entries remain available, but no partial required group does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyError {
    Missing,
    Denied,
    IncompleteRepresentation,
    EntityLimit,
    EdgeLimit,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredictionAdmission {
    NotRequested,
    Admitted { members: BTreeSet<EntityId> },
    Denied(DependencyError),
}

/// An authorization token for one representation, minted only by graph gathering.
/// It contains no gameplay payload and cannot be constructed from network bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibleEntry {
    entity: EntityId,
    representation: RepresentationGrant,
    reasons: EligibilityReasons,
    state_version: StateVersion,
    scene_revision: SceneRevision,
}
impl EligibleEntry {
    pub fn entity(&self) -> EntityId {
        self.entity
    }
    pub fn representation(&self) -> RepresentationGrant {
        self.representation
    }
    pub fn reasons(&self) -> EligibilityReasons {
        self.reasons
    }
    pub fn state_version(&self) -> StateVersion {
        self.state_version
    }
    pub fn scene_revision(&self) -> SceneRevision {
        self.scene_revision
    }
}

/// Opaque, connection-qualified authorization result. An encoder must validate
/// `is_current` at the current committed policy barrier, then honor each exact field
/// selection. Scope/baseline caches additionally qualify these grants by connection
/// epoch and representation identity; shared payload caches cannot ignore them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibleSet {
    connection: ConnectionId,
    world_revision: u64,
    policy_revision: PolicyRevision,
    frame: ReplicationFrame,
    tick: ServerTick,
    entries: BTreeMap<EntityId, EligibleEntry>,
    prediction: PredictionAdmission,
    candidate_visits: usize,
    candidates: usize,
}
impl EligibleSet {
    pub fn connection(&self) -> ConnectionId {
        self.connection
    }
    pub fn world_revision(&self) -> u64 {
        self.world_revision
    }
    pub fn policy_revision(&self) -> PolicyRevision {
        self.policy_revision
    }
    pub fn frame(&self) -> ReplicationFrame {
        self.frame
    }
    pub fn tick(&self) -> ServerTick {
        self.tick
    }
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &EligibleEntry> {
        self.entries.values()
    }
    pub fn get(&self, entity: EntityId) -> Option<&EligibleEntry> {
        self.entries.get(&entity)
    }
    pub fn prediction(&self) -> &PredictionAdmission {
        &self.prediction
    }
    pub fn candidate_visits(&self) -> usize {
        self.candidate_visits
    }
    pub fn candidate_count(&self) -> usize {
        self.candidates
    }
    pub fn is_current(&self, world_revision: u64, policy_revision: PolicyRevision) -> bool {
        self.world_revision == world_revision && self.policy_revision == policy_revision
    }
}

pub struct PreparedGraph<'a> {
    pub(crate) graph: &'a Graph,
    pub(crate) frame: ReplicationFrame,
    pub(crate) tick: ServerTick,
    pub(crate) policy_revision: PolicyRevision,
}
impl PreparedGraph<'_> {
    pub fn frame(&self) -> ReplicationFrame {
        self.frame
    }
    pub fn world_revision(&self) -> u64 {
        self.graph.world_revision
    }

    /// Gather bounded persistent lists, then close one atomic prediction group.
    /// Multiple roots here form ONE group; call-site grouping may not bypass global
    /// prediction budgets. `previous` is current eligible scope membership, not all
    /// historical/dormant/tombstone records retained by replication delivery.
    pub fn gather(
        &self,
        connection: ConnectionId,
        previous: &BTreeSet<EntityId>,
        prediction_roots: &BTreeSet<EntityId>,
        policy: &impl DisclosurePolicy,
    ) -> Result<EligibleSet, GatherError> {
        if policy.revision() != self.policy_revision {
            return Err(GatherError::StalePolicy);
        }
        let graph = self.graph;
        let view = graph
            .connections
            .get(&connection)
            .ok_or(GatherError::UnknownConnection)?;
        if previous.len() > graph.limits.max_candidates {
            return Err(GatherError::PreviousEligibilityLimit);
        }
        let mut candidates = BTreeMap::<EntityId, EligibilityReasons>::new();
        let mut visits = 0usize;
        let mut add = |id: EntityId, reason: EligibilityReasons| -> Result<(), GatherError> {
            if visits >= graph.limits.max_candidate_visits {
                return Err(GatherError::CandidateVisitLimit);
            }
            visits += 1;
            let full = candidates.len() >= graph.limits.max_candidates;
            match candidates.entry(id) {
                Entry::Occupied(mut entry) => entry.get_mut().insert(reason),
                Entry::Vacant(entry) => {
                    if full {
                        return Err(GatherError::CandidateLimit);
                    }
                    entry.insert(reason);
                }
            }
            Ok(())
        };
        for id in &graph.globals {
            add(*id, EligibilityReasons::GLOBAL)?;
        }
        if let Some(ids) = graph.owners.get(&connection) {
            for id in ids {
                add(*id, EligibilityReasons::OWNER)?;
            }
        }
        for semantic in &view.semantic_grants {
            if let Some(ids) = graph.semantics.get(semantic) {
                for id in ids {
                    add(*id, EligibilityReasons::SEMANTIC)?;
                }
            }
        }
        for observer in &view.observers {
            if let Some(ids) = graph.cells.get(&graph.cell(observer.position)) {
                for id in ids {
                    add(*id, EligibilityReasons::SPATIAL)?;
                }
            }
        }
        let candidate_count = candidates.len();
        // Candidate iteration is already ordered and unique. Let the standard
        // collection builder construct the result in bulk instead of searching
        // from its root for each successive insertion.
        let mut entries: BTreeMap<_, _> = candidates
            .into_iter()
            .filter_map(|(id, mut reasons)| {
                // Only reverse-indexed candidates are read; no per-connection world scan.
                let entity = &graph.entities[&id];
                let representation = permitted(connection, view, entity, policy)?;
                if reasons.contains(EligibilityReasons::SPATIAL)
                    && !self.spatially_eligible(entity, view, previous.contains(&id))
                {
                    reasons = reasons.without_spatial();
                }
                (!reasons.is_empty()).then(|| (id, entry(entity, representation, reasons)))
            })
            .collect();
        let prediction = if prediction_roots.is_empty() {
            PredictionAdmission::NotRequested
        } else {
            match self.required(connection, view, prediction_roots, policy) {
                Err(error) => PredictionAdmission::Denied(error),
                Ok(required) => {
                    let additional = required
                        .keys()
                        .filter(|id| !entries.contains_key(id))
                        .count();
                    if entries.len() + additional > graph.limits.max_candidates {
                        PredictionAdmission::Denied(DependencyError::EntityLimit)
                    } else {
                        let members = required.keys().copied().collect();
                        for (id, required_entry) in required {
                            entries
                                .entry(id)
                                .and_modify(|existing| {
                                    existing.reasons.insert(EligibilityReasons::REQUIRED);
                                })
                                .or_insert(required_entry);
                        }
                        PredictionAdmission::Admitted { members }
                    }
                }
            }
        };
        if policy.revision() != self.policy_revision {
            return Err(GatherError::StalePolicy);
        }
        Ok(EligibleSet {
            connection,
            world_revision: graph.world_revision,
            policy_revision: self.policy_revision,
            frame: self.frame,
            tick: self.tick,
            entries,
            prediction,
            candidate_visits: visits,
            candidates: candidate_count,
        })
    }

    fn spatially_eligible(
        &self,
        entity: &EntityRegistration,
        view: &ConnectionView,
        previous: bool,
    ) -> bool {
        let radius = entity.bounds.radius
            + entity.cull_radius
            + self.graph.limits.prefetch
            + if previous {
                self.graph.limits.leave_margin
            } else {
                0.0
            };
        view.observers.iter().any(|observer| {
            let p = entity.bounds.center;
            let q = observer.position;
            observer.scene_revision == entity.scene_revision
                && (p[0] - q[0]).hypot(p[1] - q[1]).hypot(p[2] - q[2]) <= radius
        })
    }

    fn required(
        &self,
        connection: ConnectionId,
        view: &ConnectionView,
        roots: &BTreeSet<EntityId>,
        policy: &impl DisclosurePolicy,
    ) -> Result<BTreeMap<EntityId, EligibleEntry>, DependencyError> {
        let limit = self.graph.limits.max_required;
        if roots.len() > limit {
            return Err(DependencyError::EntityLimit);
        }
        let mut pending = roots.clone();
        let mut visited = BTreeMap::new();
        let mut edges = 0usize;
        while let Some(id) = pending.pop_first() {
            let entity = self
                .graph
                .entities
                .get(&id)
                .ok_or(DependencyError::Missing)?;
            let representation =
                permitted(connection, view, entity, policy).ok_or(DependencyError::Denied)?;
            if !representation.prediction_allowed {
                return Err(DependencyError::IncompleteRepresentation);
            }
            visited.insert(
                id,
                entry(entity, representation, EligibilityReasons::REQUIRED),
            );
            for target in &entity.dependencies {
                if edges >= self.graph.limits.max_dependency_edges {
                    return Err(DependencyError::EdgeLimit);
                }
                edges += 1;
                if !policy.permits_dependency(connection, id, *target) {
                    return Err(DependencyError::Denied);
                }
                if !visited.contains_key(target) && !pending.contains(target) {
                    if visited.len() + pending.len() >= limit {
                        return Err(DependencyError::EntityLimit);
                    }
                    pending.insert(*target);
                }
            }
        }
        Ok(visited)
    }
}

fn permitted(
    connection: ConnectionId,
    view: &ConnectionView,
    entity: &EntityRegistration,
    policy: &impl DisclosurePolicy,
) -> Option<RepresentationGrant> {
    let visible = match entity.visibility {
        Visibility::Public => true,
        Visibility::Owner(owner) => owner == connection,
        Visibility::Semantic(semantic) => view.semantic_grants.contains(&semantic),
        Visibility::Never => false,
    };
    if !visible || !view.ready_scenes.contains(&entity.scene_revision) {
        return None;
    }
    policy
        .representation(connection, entity)
        .filter(|grant| grant.schema_id != 0 && grant.fields != 0)
}
fn entry(
    entity: &EntityRegistration,
    representation: RepresentationGrant,
    reasons: EligibilityReasons,
) -> EligibleEntry {
    EligibleEntry {
        entity: entity.id,
        representation,
        reasons,
        state_version: entity.state_version,
        scene_revision: entity.scene_revision,
    }
}

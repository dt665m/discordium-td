use super::*;
use crate::types::*;
use std::collections::{BTreeMap, BTreeSet};

fn test_limits() -> Limits {
    Limits {
        leave_margin: 2.0,
        prefetch: 0.0,
        ..Limits::default()
    }
}

fn id(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
fn actor(index: u64, center: [f64; 3]) -> EntityRegistration {
    EntityRegistration {
        id: id(index),
        bounds: Bounds {
            center,
            radius: 0.0,
        },
        cull_radius: 8.0,
        routes: Routes {
            spatial: Some(SpatialRoute::Dynamic),
            ..Default::default()
        },
        visibility: Visibility::Public,
        dependencies: BTreeSet::new(),
        state_version: StateVersion(1),
        scene_revision: SceneRevision(1),
        dormant: false,
    }
}
struct Approved;
impl ConnectionAuthorizer for Approved {
    fn authorize(&self, _: ConnectionId, _: &ConnectionView) -> bool {
        true
    }
}
struct Refused;
impl ConnectionAuthorizer for Refused {
    fn authorize(&self, _: ConnectionId, _: &ConnectionView) -> bool {
        false
    }
}
#[derive(Default)]
struct Policy {
    denied: BTreeSet<EntityId>,
    no_prediction: BTreeSet<EntityId>,
    denied_edges: BTreeSet<(EntityId, EntityId)>,
    revision: u64,
}
impl DisclosurePolicy for Policy {
    fn revision(&self) -> PolicyRevision {
        PolicyRevision(self.revision)
    }
    fn representation(
        &self,
        connection: ConnectionId,
        entity: &EntityRegistration,
    ) -> Option<RepresentationGrant> {
        (!self.denied.contains(&entity.id)).then_some(RepresentationGrant {
            schema_id: 1,
            revision: RepresentationRevision(1),
            fields: if connection == ConnectionId(1) {
                0b111
            } else {
                0b001
            },
            prediction_allowed: !self.no_prediction.contains(&entity.id),
        })
    }
    fn permits_dependency(&self, _: ConnectionId, source: EntityId, target: EntityId) -> bool {
        !self.denied_edges.contains(&(source, target))
    }
}
fn view(positions: &[[f64; 3]]) -> ConnectionView {
    ConnectionView {
        observers: positions
            .iter()
            .map(|position| ObserverGrant {
                position: *position,
                scene_revision: SceneRevision(1),
            })
            .collect(),
        semantic_grants: BTreeSet::new(),
        ready_scenes: BTreeSet::from([SceneRevision(1)]),
    }
}
fn connect(graph: &mut Graph, connection: u64, positions: &[[f64; 3]]) {
    graph
        .set_connection(
            ServerTick(0),
            ConnectionId(connection),
            view(positions),
            &Approved,
        )
        .unwrap();
}
fn put(graph: &mut Graph, entity: EntityRegistration) {
    graph.apply(ServerTick(0), Change::Upsert(entity)).unwrap();
}
fn gather(
    graph: &mut Graph,
    frame: u64,
    connection: u64,
    policy: &Policy,
    previous: &[u64],
    roots: &[u64],
) -> EligibleSet {
    graph
        .prepare(ReplicationFrame(frame), ServerTick(0), policy.revision())
        .unwrap()
        .gather(
            ConnectionId(connection),
            &previous.iter().copied().map(id).collect(),
            &roots.iter().copied().map(id).collect(),
            policy,
        )
        .unwrap()
}
fn ids(result: &EligibleSet) -> BTreeSet<EntityId> {
    result.entries().map(EligibleEntry::entity).collect()
}

#[test]
fn spatial_selectivity_owner_privacy_global_policy_and_projection() {
    let mut graph = Graph::new(test_limits()).unwrap();
    connect(&mut graph, 1, &[[0.0, 0.0, 0.0]]);
    connect(&mut graph, 2, &[[1000.0, 0.0, 0.0]]);
    put(&mut graph, actor(1, [0.0; 3]));
    put(&mut graph, actor(2, [1000.0, 0.0, 0.0]));
    let mut private = actor(3, [1000.0, 0.0, 0.0]);
    private.visibility = Visibility::Owner(ConnectionId(1));
    private.routes.owner = Some(ConnectionId(1));
    put(&mut graph, private);
    let mut global = actor(4, [500.0, 0.0, 0.0]);
    global.routes = Routes {
        global: true,
        ..Default::default()
    };
    put(&mut graph, global);
    let policy = Policy::default();
    let frozen = graph
        .prepare(ReplicationFrame(1), ServerTick(0), policy.revision())
        .unwrap();
    let a = frozen
        .gather(ConnectionId(1), &BTreeSet::new(), &BTreeSet::new(), &policy)
        .unwrap();
    let b = frozen
        .gather(ConnectionId(2), &BTreeSet::new(), &BTreeSet::new(), &policy)
        .unwrap();
    assert_eq!(ids(&a), BTreeSet::from([id(1), id(3), id(4)]));
    assert_eq!(ids(&b), BTreeSet::from([id(2), id(4)]));
    assert_eq!(a.get(id(4)).unwrap().representation().fields, 7);
    assert_eq!(b.get(id(4)).unwrap().representation().fields, 1);
    let denied = Policy {
        denied: BTreeSet::from([id(4)]),
        revision: 1,
        ..Default::default()
    };
    assert!(!ids(&gather(&mut graph, 2, 1, &denied, &[], &[])).contains(&id(4)));
    assert_eq!(graph.accounting().prepare_calls, 2);
}

#[test]
fn alternate_routes_deduplicate_and_reverse_indices_remove_only_old_reasons() {
    let mut graph = Graph::new(test_limits()).unwrap();
    let mut approved = view(&[[0.0; 3], [1.0, 0.0, 0.0]]);
    approved.semantic_grants.insert(SemanticId(7));
    graph
        .set_connection(ServerTick(0), ConnectionId(1), approved, &Approved)
        .unwrap();
    let mut e = actor(1, [0.0; 3]);
    e.routes.global = true;
    e.routes.owner = Some(ConnectionId(1));
    e.routes.semantic.insert(SemanticId(7));
    put(&mut graph, e.clone());
    let all = gather(&mut graph, 1, 1, &Policy::default(), &[], &[]);
    assert_eq!(all.entries().len(), 1);
    let reasons = all.get(id(1)).unwrap().reasons();
    for reason in [
        EligibilityReasons::GLOBAL,
        EligibilityReasons::OWNER,
        EligibilityReasons::SEMANTIC,
        EligibilityReasons::SPATIAL,
    ] {
        assert!(reasons.contains(reason));
    }
    e.routes = Routes {
        semantic: BTreeSet::from([SemanticId(7)]),
        ..Default::default()
    };
    e.bounds.center = [1000.0, 0.0, 0.0];
    put(&mut graph, e.clone());
    let semantic = gather(&mut graph, 2, 1, &Policy::default(), &[], &[]);
    assert_eq!(
        semantic.get(id(1)).unwrap().reasons(),
        EligibilityReasons::SEMANTIC
    );
    assert_eq!(graph.accounting().memberships, 0);
    assert!(graph.owners.is_empty());
    assert!(graph.globals.is_empty());
    e.routes = Routes::default();
    put(&mut graph, e);
    assert!(
        gather(&mut graph, 3, 1, &Policy::default(), &[], &[])
            .entries()
            .next()
            .is_none()
    );
    assert!(graph.semantics.is_empty());
    graph.apply(ServerTick(0), Change::Remove(id(1))).unwrap();
    assert_eq!(graph.accounting().entities, 0);
}

#[test]
fn footprint_changes_within_center_cell_and_radius_update() {
    let limits = Limits {
        cell_size: 10.0,
        leave_margin: 0.0,
        ..test_limits()
    };
    let mut graph = Graph::new(limits).unwrap();
    connect(&mut graph, 1, &[[10.1, 0.0, 0.0]]);
    let mut e = actor(1, [1.0, 0.0, 0.0]);
    e.cull_radius = 2.0;
    put(&mut graph, e.clone());
    assert!(ids(&gather(&mut graph, 1, 1, &Policy::default(), &[], &[])).is_empty());
    e.bounds.center[0] = 9.0;
    put(&mut graph, e.clone());
    assert_eq!(
        ids(&gather(&mut graph, 2, 1, &Policy::default(), &[], &[])),
        BTreeSet::from([id(1)])
    );
    e.bounds.center[0] = 1.0;
    e.bounds.radius = 8.0;
    put(&mut graph, e.clone());
    assert_eq!(
        ids(&gather(&mut graph, 3, 1, &Policy::default(), &[], &[])),
        BTreeSet::from([id(1)])
    );
    e.bounds.radius = 0.0;
    e.cull_radius = 0.0;
    put(&mut graph, e);
    assert!(ids(&gather(&mut graph, 4, 1, &Policy::default(), &[], &[])).is_empty());
}

#[test]
fn vertical_y_filter_and_negative_xz_boundaries() {
    let mut graph = Graph::new(test_limits()).unwrap();
    connect(&mut graph, 1, &[[-32.1, 0.0, -32.1]]);
    put(&mut graph, actor(1, [-31.9, 0.0, -31.9]));
    put(&mut graph, actor(2, [-31.9, 100.0, -31.9]));
    let result = gather(&mut graph, 1, 1, &Policy::default(), &[], &[]);
    assert_eq!(result.candidate_count(), 2);
    assert_eq!(ids(&result), BTreeSet::from([id(1)]));
}

#[test]
fn hysteresis_prefetch_never_override_hard_revocation_or_scene_readiness() {
    let mut graph = Graph::new(Limits {
        prefetch: 1.0,
        ..test_limits()
    })
    .unwrap();
    connect(&mut graph, 1, &[[0.0; 3]]);
    put(&mut graph, actor(1, [10.0, 0.0, 0.0]));
    let policy = Policy::default();
    assert!(ids(&gather(&mut graph, 1, 1, &policy, &[], &[])).is_empty());
    assert_eq!(
        ids(&gather(&mut graph, 2, 1, &policy, &[1], &[])),
        BTreeSet::from([id(1)])
    );
    let denied = Policy {
        denied: BTreeSet::from([id(1)]),
        revision: 1,
        ..Default::default()
    };
    assert!(ids(&gather(&mut graph, 3, 1, &denied, &[1], &[])).is_empty());
    let mut unready = actor(2, [0.0; 3]);
    unready.scene_revision = SceneRevision(2);
    unready.routes.global = true;
    put(&mut graph, unready);
    assert!(!ids(&gather(&mut graph, 4, 1, &denied, &[], &[])).contains(&id(2)));
}

#[test]
fn observer_and_connection_admission_are_bounded_and_transactional() {
    let mut graph = Graph::new(Limits {
        max_connections: 1,
        ..test_limits()
    })
    .unwrap();
    connect(&mut graph, 1, &[[0.0; 3]]);
    let before = graph.accounting();
    assert_eq!(
        graph.set_connection(
            ServerTick(0),
            ConnectionId(1),
            view(&[[1000.0, 0.0, 0.0]]),
            &Refused
        ),
        Err(GraphError::UnauthorizedView)
    );
    assert_eq!(graph.accounting(), before);
    assert_eq!(
        graph.connections[&ConnectionId(1)].observers[0].position,
        [0.0; 3]
    );
    assert!(matches!(
        graph.set_connection(ServerTick(0), ConnectionId(2), view(&[]), &Approved),
        Err(GraphError::Limit(_))
    ));
    assert!(matches!(
        graph.set_connection(
            ServerTick(0),
            ConnectionId(1),
            view(&[[0.0; 3]; 3]),
            &Approved
        ),
        Err(GraphError::Limit(_))
    ));
    assert_eq!(
        graph.set_connection(
            ServerTick(0),
            ConnectionId(1),
            view(&[[f64::NAN, 0.0, 0.0]]),
            &Approved
        ),
        Err(GraphError::InvalidObserver)
    );
    let mut not_ready = view(&[[0.0; 3]]);
    not_ready.ready_scenes.clear();
    assert_eq!(
        graph.set_connection(ServerTick(0), ConnectionId(1), not_ready, &Approved),
        Err(GraphError::InvalidObserver)
    );
    assert_eq!(graph.accounting(), before);
    graph
        .remove_connection(ServerTick(0), ConnectionId(1))
        .unwrap();
    assert_eq!(graph.accounting().observers, 0);
    let frozen = graph
        .prepare(ReplicationFrame(1), ServerTick(0), PolicyRevision(0))
        .unwrap();
    assert_eq!(
        frozen.gather(
            ConnectionId(1),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &Policy::default()
        ),
        Err(GatherError::UnknownConnection)
    );
}

#[test]
fn footprint_admission_is_transactional_for_nonfinite_and_oversized_changes() {
    let mut graph = Graph::new(test_limits()).unwrap();
    connect(&mut graph, 1, &[[0.0; 3]]);
    let original = actor(1, [0.0; 3]);
    put(&mut graph, original.clone());
    let before = graph.accounting();
    for radius in [f64::NAN, f64::INFINITY, -1.0, 100_000.0] {
        let mut invalid = original.clone();
        invalid.cull_radius = radius;
        assert!(graph.apply(ServerTick(0), Change::Upsert(invalid)).is_err());
        assert_eq!(graph.accounting(), before);
        assert_eq!(graph.entity(id(1)), Some(&original));
    }
    assert_eq!(
        ids(&gather(&mut graph, 1, 1, &Policy::default(), &[], &[])),
        BTreeSet::from([id(1)])
    );
}

#[test]
fn grid_cell_membership_and_route_caps_keep_previous_indices() {
    let limits = Limits {
        cell_size: 10.0,
        leave_margin: 0.0,
        max_cells: 1,
        max_cells_per_entity: 1,
        max_memberships: 1,
        max_route_memberships: 1,
        ..test_limits()
    };
    let mut graph = Graph::new(limits).unwrap();
    let mut e = actor(1, [5.0, 0.0, 5.0]);
    e.cull_radius = 0.0;
    put(&mut graph, e.clone());
    // A move can free the one old cell while allocating its replacement atomically.
    e.bounds.center = [15.0, 0.0, 15.0];
    put(&mut graph, e.clone());
    assert_eq!(graph.accounting().cells, 1);
    let before = graph.accounting();
    let mut second = e.clone();
    second.id = id(2);
    assert_eq!(
        graph.apply(ServerTick(0), Change::Upsert(second)),
        Err(GraphError::Limit("grid memberships"))
    );
    e.routes.global = true;
    e.routes.owner = Some(ConnectionId(1));
    assert_eq!(
        graph.apply(ServerTick(0), Change::Upsert(e)),
        Err(GraphError::Limit("route memberships"))
    );
    assert_eq!(graph.accounting(), before);
}

#[test]
fn monotonic_barriers_and_revision_fence_reject_stale_grants() {
    let mut graph = Graph::new(test_limits()).unwrap();
    connect(&mut graph, 1, &[[0.0; 3]]);
    put(&mut graph, actor(1, [0.0; 3]));
    let set = gather(&mut graph, 1, 1, &Policy::default(), &[], &[]);
    assert!(set.is_current(graph.accounting().world_revision, PolicyRevision(0)));
    assert!(matches!(
        graph.prepare(ReplicationFrame(1), ServerTick(0), PolicyRevision(0)),
        Err(GraphError::NonMonotonicFrame)
    ));
    graph.apply(ServerTick(1), Change::Remove(id(1))).unwrap();
    assert!(!set.is_current(graph.accounting().world_revision, PolicyRevision(0)));
    assert!(matches!(
        graph.apply(ServerTick(0), Change::Remove(id(1))),
        Err(GraphError::NonMonotonicTick)
    ));
    let frozen = graph
        .prepare(ReplicationFrame(2), ServerTick(1), PolicyRevision(1))
        .unwrap();
    assert_eq!(
        frozen.gather(
            ConnectionId(1),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &Policy::default()
        ),
        Err(GatherError::StalePolicy)
    );
    assert!(matches!(
        graph.prepare(ReplicationFrame(3), ServerTick(1), PolicyRevision(0)),
        Err(GraphError::NonMonotonicPolicy)
    ));
}

#[test]
fn closure_is_all_or_nothing_handles_cycles_and_requires_complete_projection() {
    let mut graph = Graph::new(test_limits()).unwrap();
    connect(&mut graph, 1, &[[0.0; 3]]);
    let mut a = actor(1, [0.0; 3]);
    a.dependencies.insert(id(2));
    let mut b = actor(2, [1000.0, 0.0, 0.0]);
    b.dependencies.insert(id(1));
    put(&mut graph, a);
    put(&mut graph, b);
    let allowed = gather(&mut graph, 1, 1, &Policy::default(), &[], &[1]);
    assert_eq!(ids(&allowed), BTreeSet::from([id(1), id(2)]));
    assert_eq!(
        allowed.prediction(),
        &PredictionAdmission::Admitted {
            members: BTreeSet::from([id(1), id(2)])
        }
    );
    for (frame, policy, error) in [
        (
            2,
            Policy {
                denied: BTreeSet::from([id(2)]),
                ..Default::default()
            },
            DependencyError::Denied,
        ),
        (
            3,
            Policy {
                no_prediction: BTreeSet::from([id(2)]),
                ..Default::default()
            },
            DependencyError::IncompleteRepresentation,
        ),
        (
            4,
            Policy {
                denied_edges: BTreeSet::from([(id(1), id(2))]),
                ..Default::default()
            },
            DependencyError::Denied,
        ),
    ] {
        let denied = gather(&mut graph, frame, 1, &policy, &[], &[1]);
        assert_eq!(ids(&denied), BTreeSet::from([id(1)]));
        assert_eq!(denied.prediction(), &PredictionAdmission::Denied(error));
        assert!(
            !denied
                .get(id(1))
                .unwrap()
                .reasons()
                .contains(EligibilityReasons::REQUIRED)
        );
    }
    graph.apply(ServerTick(0), Change::Remove(id(2))).unwrap();
    let missing = gather(&mut graph, 5, 1, &Policy::default(), &[], &[1]);
    assert_eq!(
        missing.prediction(),
        &PredictionAdmission::Denied(DependencyError::Missing)
    );
}

#[test]
fn denied_private_attachment_cannot_inherit_public_carrier_visibility() {
    let mut graph = Graph::new(test_limits()).unwrap();
    connect(&mut graph, 2, &[[0.0; 3]]);
    let mut carrier = actor(1, [0.0; 3]);
    carrier.dependencies.insert(id(2));
    let mut inventory = actor(2, [0.0; 3]);
    inventory.routes.global = true;
    inventory.visibility = Visibility::Owner(ConnectionId(1));
    put(&mut graph, carrier);
    put(&mut graph, inventory);
    let result = gather(&mut graph, 1, 2, &Policy::default(), &[], &[1]);
    assert_eq!(ids(&result), BTreeSet::from([id(1)]));
    assert_eq!(
        result.prediction(),
        &PredictionAdmission::Denied(DependencyError::Denied)
    );
}

#[test]
fn closure_entity_and_edge_limits_do_not_admit_partial_groups() {
    for (limits, expected) in [
        (
            Limits {
                max_required: 2,
                ..test_limits()
            },
            DependencyError::EntityLimit,
        ),
        (
            Limits {
                max_dependency_edges: 1,
                ..test_limits()
            },
            DependencyError::EdgeLimit,
        ),
    ] {
        let mut graph = Graph::new(limits).unwrap();
        connect(&mut graph, 1, &[[0.0; 3]]);
        for index in 1..=3 {
            let mut e = actor(index, [index as f64 * 100.0, 0.0, 0.0]);
            if index < 3 {
                e.dependencies.insert(id(index + 1));
            }
            put(&mut graph, e);
        }
        let result = gather(&mut graph, 1, 1, &Policy::default(), &[], &[1]);
        assert!(ids(&result).is_empty());
        assert_eq!(result.prediction(), &PredictionAdmission::Denied(expected));
    }
}

#[test]
fn dense_candidate_overload_is_explicit_not_silent_truncation() {
    let mut graph = Graph::new(Limits {
        max_candidates: 2,
        max_required: 2,
        ..test_limits()
    })
    .unwrap();
    connect(&mut graph, 1, &[[0.0; 3]]);
    for i in 0..3 {
        put(&mut graph, actor(i, [0.0; 3]));
    }
    let frozen = graph
        .prepare(ReplicationFrame(1), ServerTick(0), PolicyRevision(0))
        .unwrap();
    assert_eq!(
        frozen.gather(
            ConnectionId(1),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &Policy::default()
        ),
        Err(GatherError::CandidateLimit)
    );
}

#[test]
fn duplicate_reason_visit_work_is_independently_bounded() {
    let mut graph = Graph::new(Limits {
        max_candidates: 2,
        max_required: 2,
        max_candidate_visits: 2,
        ..test_limits()
    })
    .unwrap();
    connect(&mut graph, 1, &[[0.0; 3], [0.0; 3]]);
    let mut e = actor(1, [0.0; 3]);
    e.routes.global = true;
    put(&mut graph, e);
    let frozen = graph
        .prepare(ReplicationFrame(1), ServerTick(0), PolicyRevision(0))
        .unwrap();
    assert_eq!(
        frozen.gather(
            ConnectionId(1),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &Policy::default()
        ),
        Err(GatherError::CandidateVisitLimit)
    );
}

#[test]
fn distant_population_does_not_increase_per_connection_visits() {
    let mut graph = Graph::new(test_limits()).unwrap();
    connect(&mut graph, 1, &[[0.0; 3]]);
    put(&mut graph, actor(1, [0.0; 3]));
    let initial = gather(&mut graph, 1, 1, &Policy::default(), &[], &[]);
    for index in 2..=5000 {
        put(
            &mut graph,
            actor(index, [1000.0 + index as f64 * 40.0, 0.0, 1000.0]),
        );
    }
    let grown = gather(&mut graph, 2, 1, &Policy::default(), &[], &[]);
    assert_eq!(ids(&initial), ids(&grown));
    assert_eq!(initial.candidate_visits(), grown.candidate_visits());
    assert_eq!(grown.candidate_count(), 1);
}

/// Independent full-scan spatial oracle: no grid helpers, coverage functions, route
/// indices or gather permission helper are reused. Includes old-scope hysteresis,
/// randomized moves/removes/cull/extents changes, XZ negatives and vertical Y.
#[test]
fn randomized_persistent_changes_match_independent_oracle() {
    let mut seed = 0x37f0a29e_u64;
    fn random(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 32) as u32 as f64) / u32::MAX as f64
    }
    for _case in 0..12 {
        let limits = Limits {
            cell_size: 16.0,
            prefetch: 1.5,
            leave_margin: 3.0,
            ..test_limits()
        };
        let mut graph = Graph::new(limits).unwrap();
        let mut oracle = BTreeMap::new();
        let observers = [[-30.0, 0.0, -20.0], [30.0, 10.0, 40.0]];
        connect(&mut graph, 1, &observers);
        let mut previous = BTreeSet::new();
        for frame in 1..=30 {
            for _ in 0..25 {
                let index = (random(&mut seed) * 160.0) as u64;
                if random(&mut seed) < 0.15 {
                    graph
                        .apply(ServerTick(0), Change::Remove(id(index)))
                        .unwrap();
                    oracle.remove(&id(index));
                } else {
                    let mut e = actor(
                        index,
                        [
                            random(&mut seed) * 200.0 - 100.0,
                            random(&mut seed) * 60.0 - 30.0,
                            random(&mut seed) * 200.0 - 100.0,
                        ],
                    );
                    e.bounds.radius = random(&mut seed) * 4.0;
                    e.cull_radius = random(&mut seed) * 25.0;
                    e.routes.spatial = Some(match index % 3 {
                        0 => SpatialRoute::Static,
                        1 => SpatialRoute::Dynamic,
                        _ => SpatialRoute::DormancyDriven,
                    });
                    e.dormant = index % 2 == 0;
                    put(&mut graph, e.clone());
                    oracle.insert(e.id, e);
                }
            }
            let expected: BTreeSet<_> = oracle
                .values()
                .filter(|e| {
                    let radius = e.bounds.radius
                        + e.cull_radius
                        + 1.5
                        + if previous.contains(&e.id) { 3.0 } else { 0.0 };
                    observers.iter().any(|o| {
                        let distance_squared: f64 = (0..3)
                            .map(|axis| (e.bounds.center[axis] - o[axis]).powi(2))
                            .sum();
                        distance_squared <= radius * radius
                    })
                })
                .map(|e| e.id)
                .collect();
            let frozen = graph
                .prepare(ReplicationFrame(frame), ServerTick(0), PolicyRevision(0))
                .unwrap();
            let result = frozen
                .gather(
                    ConnectionId(1),
                    &previous,
                    &BTreeSet::new(),
                    &Policy::default(),
                )
                .unwrap();
            assert_eq!(ids(&result), expected, "randomized frame {frame}");
            previous = expected;
        }
    }
}

#[test]
fn identity_versions_and_entity_capacity_are_transactional() {
    let mut graph = Graph::new(Limits {
        max_entities: 1,
        ..test_limits()
    })
    .unwrap();
    let mut original = actor(7, [0.0; 3]);
    original.state_version = StateVersion(3);
    put(&mut graph, original.clone());
    let before = graph.accounting();
    let mut invalid = original.clone();
    invalid.state_version = StateVersion(2);
    assert_eq!(
        graph.apply(ServerTick(0), Change::Upsert(invalid)),
        Err(GraphError::RegressedStateVersion)
    );
    let mut next = original.clone();
    next.id.generation = 2;
    assert_eq!(
        graph.apply(ServerTick(0), Change::Upsert(next.clone())),
        Err(GraphError::GenerationConflict)
    );
    assert_eq!(
        graph.apply(ServerTick(0), Change::Upsert(actor(8, [0.0; 3]))),
        Err(GraphError::Limit("entities"))
    );
    assert_eq!(graph.accounting(), before);
    assert_eq!(graph.entity(id(7)), Some(&original));
    graph.apply(ServerTick(0), Change::Remove(id(7))).unwrap();
    put(&mut graph, next.clone());
    assert!(graph.entity(id(7)).is_none());
    assert_eq!(graph.entity(next.id), Some(&next));
    graph.apply(ServerTick(0), Change::Remove(id(7))).unwrap();
    assert_eq!(graph.entity(next.id), Some(&next));
}

#[test]
fn semantic_entitlement_revocation_overrides_global_and_required_reasons() {
    let mut graph = Graph::new(test_limits()).unwrap();
    let mut approved = view(&[[0.0; 3]]);
    approved.semantic_grants.insert(SemanticId(7));
    graph
        .set_connection(ServerTick(0), ConnectionId(1), approved, &Approved)
        .unwrap();
    let mut e = actor(1, [0.0; 3]);
    e.routes.global = true;
    e.visibility = Visibility::Semantic(SemanticId(7));
    put(&mut graph, e);
    let allowed = gather(&mut graph, 1, 1, &Policy::default(), &[], &[1]);
    assert_eq!(ids(&allowed), BTreeSet::from([id(1)]));
    graph
        .set_connection(ServerTick(0), ConnectionId(1), view(&[[0.0; 3]]), &Approved)
        .unwrap();
    assert!(!allowed.is_current(graph.accounting().world_revision, PolicyRevision(0)));
    let revoked = gather(&mut graph, 2, 1, &Policy::default(), &[1], &[1]);
    assert!(ids(&revoked).is_empty());
    assert_eq!(
        revoked.prediction(),
        &PredictionAdmission::Denied(DependencyError::Denied)
    );
}

#[test]
fn dependency_and_semantic_declaration_caps_precede_index_mutation() {
    let limits = Limits {
        max_required: 1,
        max_semantic_routes_per_entity: 1,
        max_semantic_grants: 1,
        max_ready_scenes: 1,
        ..test_limits()
    };
    let mut graph = Graph::new(limits).unwrap();
    let original = actor(1, [0.0; 3]);
    put(&mut graph, original.clone());
    let before = graph.accounting();
    let mut invalid = original.clone();
    invalid.dependencies = BTreeSet::from([id(2), id(3)]);
    assert_eq!(
        graph.apply(ServerTick(0), Change::Upsert(invalid)),
        Err(GraphError::Limit("declared dependencies"))
    );
    let mut invalid = original.clone();
    invalid.routes.semantic = BTreeSet::from([SemanticId(1), SemanticId(2)]);
    assert_eq!(
        graph.apply(ServerTick(0), Change::Upsert(invalid)),
        Err(GraphError::Limit("semantic routes"))
    );
    let mut invalid = view(&[]);
    invalid.semantic_grants = BTreeSet::from([SemanticId(1), SemanticId(2)]);
    assert_eq!(
        graph.set_connection(ServerTick(0), ConnectionId(1), invalid, &Approved),
        Err(GraphError::Limit("connection grants"))
    );
    let mut invalid = view(&[]);
    invalid.ready_scenes.insert(SceneRevision(2));
    assert_eq!(
        graph.set_connection(ServerTick(0), ConnectionId(1), invalid, &Approved),
        Err(GraphError::Limit("connection grants"))
    );
    assert_eq!(graph.accounting(), before);
    assert_eq!(graph.entity(id(1)), Some(&original));
}

#[test]
fn shared_dynamic_refresh_tracks_static_and_dormancy_transitions() {
    let mut graph = Graph::new(test_limits()).unwrap();
    let mut e = actor(1, [0.0; 3]);
    e.routes.spatial = Some(SpatialRoute::DormancyDriven);
    e.dormant = true;
    put(&mut graph, e.clone());
    assert_eq!(graph.dynamic_entities().len(), 0);
    let memberships = graph.accounting().memberships;
    e.dormant = false;
    e.state_version = StateVersion(2);
    put(&mut graph, e.clone());
    assert_eq!(graph.dynamic_entities().collect::<Vec<_>>(), vec![id(1)]);
    assert_eq!(graph.accounting().memberships, memberships);
    e.routes.spatial = Some(SpatialRoute::Static);
    put(&mut graph, e);
    assert_eq!(graph.dynamic_entities().len(), 0);
    graph.apply(ServerTick(0), Change::Remove(id(1))).unwrap();
    assert_eq!(graph.accounting().cells, 0);
}

#[test]
fn state_only_changes_refresh_authorization_and_dependencies_at_a_new_barrier() {
    let mut graph = Graph::new(test_limits()).unwrap();
    connect(&mut graph, 1, &[[0.0; 3]]);
    let mut source = actor(1, [0.0; 3]);
    source.routes.global = true;
    source.routes.owner = Some(ConnectionId(1));
    put(&mut graph, source.clone());
    put(&mut graph, actor(2, [1000.0, 0.0, 0.0]));
    let policy = Policy::default();
    let before = gather(&mut graph, 1, 1, &policy, &[], &[1]);
    let accounting = graph.accounting();
    source.state_version = StateVersion(2);
    source.dependencies.insert(id(2));
    put(&mut graph, source.clone());
    let after = gather(&mut graph, 2, 1, &policy, &[], &[1]);
    assert!(!before.is_current(after.world_revision(), policy.revision()));
    assert_eq!(after.get(id(1)).unwrap().state_version(), StateVersion(2));
    assert_eq!(ids(&after), BTreeSet::from([id(1), id(2)]));
    assert_eq!(graph.accounting().memberships, accounting.memberships);
    assert_eq!(
        graph.accounting().route_memberships,
        accounting.route_memberships
    );
    source.visibility = Visibility::Never;
    put(&mut graph, source.clone());
    let denied = gather(&mut graph, 3, 1, &policy, &[], &[1]);
    assert!(denied.entries().len() == 0);
    assert!(matches!(
        denied.prediction(),
        PredictionAdmission::Denied(_)
    ));
    source.visibility = Visibility::Public;
    source.scene_revision = SceneRevision(2);
    put(&mut graph, source);
    assert!(gather(&mut graph, 4, 1, &policy, &[], &[1]).entries().len() == 0);
    graph.apply(ServerTick(0), Change::Remove(id(1))).unwrap();
    assert_eq!(graph.accounting().route_memberships, 0);
}

#[test]
fn canonical_payload_cache_requires_current_exact_public_grants() {
    let mut graph = Graph::new(test_limits()).unwrap();
    put(&mut graph, actor(1, [0.0; 3]));
    for peer in 1..=3 {
        connect(&mut graph, peer, &[[0.0; 3]]);
    }
    connect(&mut graph, 4, &[[1000.0; 3]]);
    let policy = Policy::default();
    let prepared = graph
        .prepare(ReplicationFrame(1), ServerTick(0), policy.revision())
        .unwrap();
    let grants = |connection| {
        prepared
            .gather(
                ConnectionId(connection),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &policy,
            )
            .unwrap()
    };
    let first = grants(2);
    let same = grants(3);
    let different_fields = grants(1);
    let denied = grants(4);
    let mut cache = CanonicalPayloadCache::new(8, 4096);
    cache.begin_frame(
        first.frame(),
        first.tick(),
        first.world_revision(),
        first.policy_revision(),
    );
    assert_eq!(
        cache
            .encode(&first, id(1), || Ok::<_, ()>(vec![1]))
            .unwrap(),
        vec![1]
    );
    assert_eq!(
        cache
            .encode(&same, id(1), || -> Result<Vec<u8>, ()> {
                panic!("equivalent public payload encoded twice")
            })
            .unwrap(),
        vec![1]
    );
    assert_eq!(
        cache
            .encode(&different_fields, id(1), || Ok::<_, ()>(vec![7]))
            .unwrap(),
        vec![7]
    );
    assert_eq!(
        cache.encode(&denied, id(1), || -> Result<Vec<u8>, ()> {
            panic!("missing grant encoded")
        }),
        Err(CanonicalCacheError::MissingGrant)
    );
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().encodes, 2);
    assert_eq!(cache.stats().entries, 2);
    let mut changed = actor(1, [0.0; 3]);
    changed.state_version = StateVersion(2);
    put(&mut graph, changed);
    let next = gather(&mut graph, 2, 2, &policy, &[], &[]);
    cache.begin_frame(
        next.frame(),
        next.tick(),
        next.world_revision(),
        next.policy_revision(),
    );
    assert_eq!(
        cache.encode(&first, id(1), || -> Result<Vec<u8>, ()> {
            panic!("stale grant encoded")
        }),
        Err(CanonicalCacheError::StaleBarrier)
    );
    assert_eq!(
        cache.encode(&next, id(1), || Ok::<_, ()>(vec![2])).unwrap(),
        vec![2]
    );
    assert_eq!(cache.stats().hits, 0);
    assert_eq!(cache.stats().entries, 1);
    cache.clear();
    assert_eq!(cache.stats(), CanonicalCacheStats::default());
    assert_eq!(
        cache.encode(&next, id(1), || Ok::<_, ()>(vec![2])),
        Err(CanonicalCacheError::StaleBarrier)
    );
}

#[test]
fn canonical_payload_capacity_and_encode_failure_never_change_delivery() {
    let mut graph = Graph::new(test_limits()).unwrap();
    put(&mut graph, actor(1, [0.0; 3]));
    connect(&mut graph, 1, &[[0.0; 3]]);
    let eligible = gather(&mut graph, 1, 1, &Policy::default(), &[], &[]);
    for (entries, bytes) in [(0, 4096), (8, 0), (8, 64)] {
        let mut cache = CanonicalPayloadCache::new(entries, bytes);
        cache.begin_frame(
            eligible.frame(),
            eligible.tick(),
            eligible.world_revision(),
            eligible.policy_revision(),
        );
        for _ in 0..3 {
            assert_eq!(
                cache
                    .encode(&eligible, id(1), || Ok::<_, ()>(vec![9; 128]))
                    .unwrap(),
                vec![9; 128]
            );
        }
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().retained_bytes, 0);
        assert_eq!(cache.stats().uncached, 3);
    }
    let mut cache = CanonicalPayloadCache::new(8, 4096);
    cache.begin_frame(
        eligible.frame(),
        eligible.tick(),
        eligible.world_revision(),
        eligible.policy_revision(),
    );
    assert_eq!(
        cache.encode(&eligible, id(1), || Err::<Vec<u8>, _>("codec")),
        Err(CanonicalCacheError::Encode("codec"))
    );
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(
        cache
            .encode(&eligible, id(1), || Ok::<_, ()>(vec![3]))
            .unwrap(),
        vec![3]
    );
    assert!(cache.stats().retained_bytes <= 4096);
}

#[test]
fn unchanged_footprint_motion_updates_exact_distance_and_dynamic_state() {
    let mut graph = Graph::new(test_limits()).unwrap();
    let mut moving = actor(1, [7.9, 0.0, 0.0]);
    moving.routes.spatial = Some(SpatialRoute::DormancyDriven);
    put(&mut graph, moving.clone());
    connect(&mut graph, 1, &[[0.0; 3]]);
    let policy = Policy::default();
    let before = gather(&mut graph, 1, 1, &policy, &[], &[]);
    assert!(before.get(id(1)).is_some());
    let cells = graph.cells.clone();
    let memberships = graph.accounting().memberships;
    for (frame, position, dormant, visible) in [
        (2, [8.1, 0.0, 0.0], false, false),
        (3, [7.9, 0.0, 0.0], true, true),
        (4, [7.9, 3.0, 0.0], false, false),
    ] {
        moving.bounds.center = position;
        moving.state_version = StateVersion(frame);
        moving.dormant = dormant;
        put(&mut graph, moving.clone());
        assert_eq!(graph.cells, cells);
        assert_eq!(graph.accounting().memberships, memberships);
        assert_eq!(graph.dynamic_entities().count(), usize::from(!dormant));
        assert_eq!(graph.entity(id(1)), Some(&moving));
        let eligible = gather(&mut graph, frame, 1, &policy, &[], &[]);
        assert_eq!(eligible.get(id(1)).is_some(), visible);
        if visible {
            assert_eq!(
                eligible.get(id(1)).unwrap().state_version(),
                StateVersion(frame)
            );
        }
    }
    let previous = graph.accounting();
    let mut invalid = moving.clone();
    invalid.bounds.center[1] = f64::NAN;
    assert_eq!(
        graph.apply(ServerTick(0), Change::Upsert(invalid)),
        Err(GraphError::InvalidBounds)
    );
    assert_eq!(graph.accounting(), previous);
    assert_eq!(graph.entity(id(1)), Some(&moving));
}

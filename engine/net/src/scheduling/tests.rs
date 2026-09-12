use super::*;
use crate::{interest::*, types::*};

fn id(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
struct Policy;
impl DisclosurePolicy for Policy {
    fn revision(&self) -> PolicyRevision {
        PolicyRevision(1)
    }
    fn representation(
        &self,
        _: ConnectionId,
        _: &EntityRegistration,
    ) -> Option<RepresentationGrant> {
        Some(RepresentationGrant {
            schema_id: 1,
            revision: RepresentationRevision(1),
            fields: 1,
            prediction_allowed: true,
        })
    }
    fn permits_dependency(&self, _: ConnectionId, _: EntityId, _: EntityId) -> bool {
        true
    }
}
struct Approve;
impl ConnectionAuthorizer for Approve {
    fn authorize(&self, _: ConnectionId, _: &ConnectionView) -> bool {
        true
    }
}
fn eligible(tick: u64, count: u64, roots: &[u64]) -> EligibleSet {
    let mut graph = Graph::new(Limits::default()).unwrap();
    for i in 1..=count {
        graph
            .apply(
                ServerTick(tick),
                Change::Upsert(EntityRegistration {
                    id: id(i),
                    bounds: Bounds {
                        center: [0.0; 3],
                        radius: 0.0,
                    },
                    cull_radius: 1.0,
                    routes: Routes {
                        global: true,
                        ..Default::default()
                    },
                    visibility: Visibility::Public,
                    dependencies: BTreeSet::new(),
                    state_version: StateVersion(tick + 1),
                    scene_revision: SceneRevision(1),
                    dormant: false,
                }),
            )
            .unwrap();
    }
    graph
        .set_connection(
            ServerTick(tick),
            ConnectionId(1),
            ConnectionView {
                observers: vec![ObserverGrant {
                    position: [0.0; 3],
                    scene_revision: SceneRevision(1),
                }],
                semantic_grants: BTreeSet::new(),
                ready_scenes: BTreeSet::from([SceneRevision(1)]),
            },
            &Approve,
        )
        .unwrap();
    graph
        .prepare(
            ReplicationFrame(tick + 1),
            ServerTick(tick),
            PolicyRevision(1),
        )
        .unwrap()
        .gather(
            ConnectionId(1),
            &BTreeSet::new(),
            &roots.iter().map(|i| id(*i)).collect(),
            &Policy,
        )
        .unwrap()
}
fn cadence(priority: u8, critical: bool) -> Cadence {
    Cadence {
        period_ticks: 1,
        maximum_age_ticks: 8,
        priority,
        critical,
    }
}
fn scheduler(burst: usize, reserve: u16) -> Scheduler {
    Scheduler::new(
        ConnectionId(1),
        SchedulerLimits {
            bytes_per_second: burst as u64,
            burst_bytes: burst,
            critical_reserve: reserve,
            ..SchedulerLimits::default()
        },
    )
    .unwrap()
}
fn budget(bytes: usize) -> TransportBudget {
    TransportBudget {
        available_wire_bytes: bytes,
        maximum_atomic_wire_bytes: bytes,
    }
}
fn transmit(s: &mut Scheduler, plan: SchedulePlan<'_>, accepted: bool) -> TransmitMetrics {
    let revision = plan.eligible.world_revision();
    s.transmit(plan, revision, PolicyRevision(1), |_| accepted)
        .unwrap()
}

#[test]
fn no_unauthorized_or_overlapping_units_reach_send() {
    let e = eligible(0, 2, &[]);
    let mut s = scheduler(100, 4000);
    let unknown = [UnitRequest::entity(id(3), 10, cadence(0, false))];
    assert!(matches!(
        s.schedule(&e, &unknown, Duration::ZERO, budget(100)),
        Err(ScheduleError::Unauthorized)
    ));
    assert_eq!(s.known_units(), 0);
    let duplicate = [
        UnitRequest::entity(id(1), 10, cadence(0, false)),
        UnitRequest::entity(id(1), 10, cadence(1, true)),
    ];
    assert!(matches!(
        s.schedule(&e, &duplicate, Duration::ZERO, budget(100)),
        Err(ScheduleError::InvalidRequest)
    ));
    assert_eq!(s.known_units(), 0);
}

#[test]
fn required_closure_is_one_atomic_budget_unit() {
    let e = eligible(0, 2, &[1, 2]);
    let mut s = scheduler(100, 4000);
    let units = [UnitRequest::prediction_group(&e, GroupId(1), 110, cadence(0, true)).unwrap()];
    let plan = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    assert_eq!(plan.metrics().selected_units, 0);
    assert_eq!(plan.metrics().infeasible_units, 1);
    assert_eq!(transmit(&mut s, plan, true).accepted_units, 0);
    let denied = eligible(0, 2, &[]);
    assert!(matches!(
        UnitRequest::prediction_group(&denied, GroupId(1), 100, cadence(0, true)),
        Err(ScheduleError::Unauthorized)
    ));
    assert!(matches!(
        s.schedule(&denied, &units, Duration::ZERO, budget(100)),
        Err(ScheduleError::Unauthorized)
    ));
}

#[test]
fn critical_reserve_handles_whole_units_and_unused_share_is_borrowed() {
    let e = eligible(0, 3, &[]);
    let units = [
        UnitRequest::entity(id(1), 60, cadence(0, true)),
        UnitRequest::entity(id(2), 100, cadence(255, false)),
        UnitRequest::entity(id(3), 40, cadence(0, false)),
    ];
    let mut s = scheduler(100, 4000);
    let plan = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    assert_eq!(
        plan.selected().map(UnitRequest::id).collect::<Vec<_>>(),
        [UnitId::Entity(id(1)), UnitId::Entity(id(3))]
    );
    assert_eq!(transmit(&mut s, plan, true).accepted_wire_bytes, 100);
    let mut s = scheduler(100, 4000);
    let plan = s
        .schedule(&e, &units[1..2], Duration::ZERO, budget(100))
        .unwrap();
    assert_eq!(plan.metrics().selected_wire_bytes, 100);
}

#[test]
fn failed_send_does_not_spend_budget_or_advance_cadence() {
    let e = eligible(0, 1, &[]);
    let mut s = scheduler(100, 4000);
    let units = [UnitRequest::entity(id(1), 100, cadence(0, true))];
    let plan = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    assert_eq!(transmit(&mut s, plan, false).rejected_units, 1);
    assert_eq!(s.last_transmitted(UnitId::Entity(id(1))), None);
    let plan = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    assert_eq!(transmit(&mut s, plan, true).accepted_wire_bytes, 100);
    assert_eq!(
        s.last_transmitted(UnitId::Entity(id(1))),
        Some(ServerTick(0))
    );
    let plan = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    assert_eq!(plan.metrics().due_units, 0);
}

#[test]
fn revisions_fence_unsent_work_and_new_plans_invalidate_old_ones() {
    let e = eligible(0, 1, &[]);
    let mut s = scheduler(100, 4000);
    let units = [UnitRequest::entity(id(1), 100, cadence(0, true))];
    let plan = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    assert_eq!(
        s.transmit(plan, e.world_revision(), PolicyRevision(2), |_| panic!(
            "revoked encoder ran"
        )),
        Err(ScheduleError::Stale)
    );
    let old = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    let new = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    assert_eq!(
        s.transmit(old, e.world_revision(), PolicyRevision(1), |_| panic!(
            "old encoder ran"
        )),
        Err(ScheduleError::Stale)
    );
    assert_eq!(transmit(&mut s, new, true).accepted_units, 1);
}

#[test]
fn accepted_bytes_obey_rate_and_transport_allowance() {
    let mut s = scheduler(100, 4000);
    let units = [UnitRequest::entity(id(1), 50, cadence(0, true))];
    let e = eligible(0, 1, &[]);
    let plan = s.schedule(&e, &units, Duration::ZERO, budget(49)).unwrap();
    assert_eq!(plan.metrics().infeasible_units, 1);
    transmit(&mut s, plan, true);
    let plan = s.schedule(&e, &units, Duration::ZERO, budget(100)).unwrap();
    let mut bytes = transmit(&mut s, plan, true).accepted_wire_bytes;
    for tick in 1..=100 {
        let e = eligible(tick, 1, &[]);
        let plan = s
            .schedule(&e, &units, Duration::from_millis(tick * 10), budget(100))
            .unwrap();
        bytes += transmit(&mut s, plan, true).accepted_wire_bytes;
    }
    assert_eq!(bytes, 200); // Initial burst + one second of accrual.
}

#[test]
fn feasible_optional_state_has_bounded_age_despite_priority_gap() {
    let mut s = scheduler(100, 4000);
    let units = [
        UnitRequest::entity(id(1), 100, cadence(255, false)),
        UnitRequest::entity(id(2), 100, cadence(0, false)),
    ];
    let mut last_low = 0;
    for tick in 0..50 {
        let e = eligible(tick, 2, &[]);
        let plan = s
            .schedule(&e, &units, Duration::from_secs(tick), budget(100))
            .unwrap();
        if plan.selected().any(|u| u.id == UnitId::Entity(id(2))) {
            last_low = tick;
        }
        transmit(&mut s, plan, true);
        assert!(tick - last_low <= 8);
    }
}

#[test]
fn overload_reports_deadlines_instead_of_claiming_delivery() {
    let mut s = scheduler(100, 4000);
    let units = [UnitRequest::entity(id(1), 100, cadence(0, true))];
    for tick in 0..=8 {
        let e = eligible(tick, 1, &[]);
        let plan = s
            .schedule(&e, &units, Duration::from_secs(tick), budget(0))
            .unwrap();
        assert_eq!(
            plan.metrics().critical_deadline_misses,
            usize::from(tick == 8)
        );
        assert_eq!(plan.metrics().deferred_units, 1);
        transmit(&mut s, plan, true);
    }
}

#[test]
fn storage_and_time_are_bounded_transactionally() {
    let mut s = Scheduler::new(
        ConnectionId(1),
        SchedulerLimits {
            units: 1,
            ..SchedulerLimits::default()
        },
    )
    .unwrap();
    let e = eligible(2, 2, &[]);
    let units = [
        UnitRequest::entity(id(1), 10, cadence(0, false)),
        UnitRequest::entity(id(2), 10, cadence(0, false)),
    ];
    assert!(matches!(
        s.schedule(&e, &units, Duration::from_secs(2), budget(100)),
        Err(ScheduleError::Capacity)
    ));
    assert_eq!(s.known_units(), 0);
    let plan = s
        .schedule(&e, &units[..1], Duration::from_secs(2), budget(100))
        .unwrap();
    transmit(&mut s, plan, true);
    assert!(matches!(
        s.schedule(&e, &units[..1], Duration::from_secs(1), budget(100)),
        Err(ScheduleError::Stale)
    ));
    let plan = s
        .schedule(&e, &[], Duration::from_secs(2), budget(100))
        .unwrap();
    transmit(&mut s, plan, true);
    assert_eq!(s.known_units(), 0);
}

#[test]
fn transfer_credit_accounts_for_refill_without_spending_or_repeating_it() {
    let mut s = scheduler(100, 4000);
    let e = eligible(0, 1, &[]);
    let plan = s
        .schedule(
            &e,
            &[UnitRequest::entity(id(1), 100, cadence(1, true))],
            Duration::ZERO,
            budget(100),
        )
        .unwrap();
    assert_eq!(transmit(&mut s, plan, true).accepted_wire_bytes, 100);
    let later = Duration::from_millis(250);
    assert_eq!(s.available_wire_bytes(later), 25);
    assert_eq!(s.available_wire_bytes(later), 25);
    let e = eligible(1, 1, &[]);
    let plan = s
        .schedule(
            &e,
            &[UnitRequest::entity(id(1), 25, cadence(1, true))],
            later,
            budget(100),
        )
        .unwrap();
    assert_eq!(transmit(&mut s, plan, true).accepted_wire_bytes, 25);
    assert_eq!(s.available_wire_bytes(later), 0);
    assert_eq!(s.available_wire_bytes(Duration::from_secs(100)), 100);
}

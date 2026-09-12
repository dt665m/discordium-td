//! Synthetic current-host qualification, not a network or simulation benchmark.
//! Run with `cargo run --release -p engine_net --example graph_qualification -- OUTPUT_DIR`.
use engine_net::{interest::*, replication::*, scheduling::*, types::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Write,
    time::{Duration, Instant},
};

// Requested live heap bytes, distinct from allocator usable size and resident RSS.
struct CountingAllocator;
static LIVE_HEAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PEAK_HEAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn allocated(n: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    let live = LIVE_HEAP.fetch_add(n, Relaxed) + n;
    PEAK_HEAP.fetch_max(live, Relaxed);
}
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let p = unsafe { std::alloc::System.alloc(layout) };
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: std::alloc::Layout) {
        LIVE_HEAP.fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(p, layout) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { std::alloc::System.realloc(p, layout, new_size) };
        if !next.is_null() {
            LIVE_HEAP.fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
            allocated(new_size);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
fn live_heap() -> usize {
    LIVE_HEAP.load(std::sync::atomic::Ordering::Relaxed)
}

const POLICY: PolicyRevision = PolicyRevision(1);
const SCENE: SceneRevision = SceneRevision(1);
const PEERS: usize = 100;
fn id(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
fn origin(peer: usize) -> [f64; 3] {
    [
        -3500. + (peer % 10) as f64 * 256.,
        0.,
        -3500. + (peer / 10) as f64 * 256.,
    ]
}
struct Policy;
impl ConnectionAuthorizer for Policy {
    fn authorize(&self, _: ConnectionId, _: &ConnectionView) -> bool {
        true
    }
}
impl DisclosurePolicy for Policy {
    fn revision(&self) -> PolicyRevision {
        POLICY
    }
    fn representation(
        &self,
        c: ConnectionId,
        e: &EntityRegistration,
    ) -> Option<RepresentationGrant> {
        Some(RepresentationGrant {
            schema_id: 1,
            revision: RepresentationRevision(1),
            fields: if e.routes.owner == Some(c) { 3 } else { 1 },
            prediction_allowed: e.id.index > 100 || e.id.index == c.0,
        })
    }
    fn permits_dependency(&self, _: ConnectionId, _: EntityId, _: EntityId) -> bool {
        true
    }
}
fn view(peer: usize, teleport: bool, ready: bool) -> ConnectionView {
    ConnectionView {
        observers: if ready {
            vec![ObserverGrant {
                position: if teleport {
                    [2500., 0., 2500.]
                } else {
                    origin(peer)
                },
                scene_revision: SCENE,
            }]
        } else {
            Vec::new()
        },
        semantic_grants: BTreeSet::from([SemanticId((peer % 4) as u64 + 1)]),
        ready_scenes: if ready {
            BTreeSet::from([SCENE])
        } else {
            BTreeSet::new()
        },
    }
}
fn fixture() -> Vec<EntityRegistration> {
    let mut seed = 20260909u64;
    (1..=50_000u64)
        .map(|n| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let peer = ((n - 1) % 100) as usize;
            let mut p = origin(peer);
            if n > 5000 {
                p = if n <= 8000 {
                    [2500. + (n % 17) as f64, 0., 2500. + (n % 19) as f64]
                } else {
                    [
                        512. + (seed % 3300) as f64,
                        0.,
                        512. + ((seed >> 32) % 3300) as f64,
                    ]
                };
            } else if n > 100 {
                p[0] += ((n / 100) % 7) as f64 * 12.;
                p[2] += ((n / 700) % 7) as f64 * 12.;
            }
            if (1001..=1100).contains(&n) {
                p = origin((peer + 1) % 100);
            }
            if (101..=200).contains(&n) {
                p = origin((n - 101) as usize);
                p[0] += 256.;
            }
            let owner = if n <= 100 {
                Some(ConnectionId(n))
            } else if (1001..=1100).contains(&n) {
                Some(ConnectionId(n - 1000))
            } else {
                None
            };
            let team = (1103..=1202)
                .contains(&n)
                .then_some(SemanticId((n % 4) + 1));
            EntityRegistration {
                id: id(n),
                bounds: Bounds {
                    center: p,
                    radius: if n <= 100 { 1. } else { 2. },
                },
                cull_radius: if n <= 100 {
                    80.
                } else if n <= 1000 {
                    32.
                } else {
                    12.
                },
                routes: Routes {
                    spatial: Some(if n <= 1000 {
                        SpatialRoute::Dynamic
                    } else {
                        SpatialRoute::DormancyDriven
                    }),
                    global: n == 1101 || n == 1102,
                    owner,
                    semantic: team.into_iter().collect(),
                },
                visibility: if (1001..=1100).contains(&n) {
                    Visibility::Owner(owner.unwrap())
                } else if n == 1102 {
                    Visibility::Never
                } else if let Some(team) = team {
                    Visibility::Semantic(team)
                } else {
                    Visibility::Public
                },
                dependencies: if n <= 100 {
                    BTreeSet::from([id(n + 100)])
                } else {
                    BTreeSet::new()
                },
                state_version: StateVersion(1),
                scene_revision: SCENE,
                dormant: false,
            }
        })
        .collect()
}
// Deliberately independent: scans fixture data and directly evaluates policy,
// routing, distance and closure; does not call graph or DisclosurePolicy helpers.
fn oracle(
    actors: &[EntityRegistration],
    peer: usize,
    v: &ConnectionView,
    previous: &BTreeSet<EntityId>,
) -> BTreeMap<EntityId, u64> {
    let c = ConnectionId(peer as u64 + 1);
    let allowed = |e: &EntityRegistration| {
        v.ready_scenes.contains(&e.scene_revision)
            && match e.visibility {
                Visibility::Public => true,
                Visibility::Never => false,
                Visibility::Owner(o) => o == c,
                Visibility::Semantic(s) => v.semantic_grants.contains(&s),
            }
    };
    let mut out = BTreeMap::new();
    for e in actors {
        if !allowed(e) {
            continue;
        }
        let spatial = e.routes.spatial.is_some()
            && v.observers.iter().any(|o| {
                let d = ((e.bounds.center[0] - o.position[0]).powi(2)
                    + (e.bounds.center[1] - o.position[1]).powi(2)
                    + (e.bounds.center[2] - o.position[2]).powi(2))
                .sqrt();
                o.scene_revision == e.scene_revision
                    && d <= e.bounds.radius
                        + e.cull_radius
                        + 8.
                        + if previous.contains(&e.id) { 10. } else { 0. }
            });
        if spatial
            || e.routes.global
            || e.routes.owner == Some(c)
            || e.routes
                .semantic
                .iter()
                .any(|s| v.semantic_grants.contains(s))
        {
            out.insert(e.id, if e.routes.owner == Some(c) { 3 } else { 1 });
        }
    }
    if allowed(&actors[peer]) && allowed(&actors[peer + 100]) {
        out.insert(id(c.0), 3);
        out.insert(id(c.0 + 100), 1);
    }
    out
}
fn payload(e: &EntityRegistration, fields: u64) -> Vec<u8> {
    let mut p = Vec::with_capacity(if (5001..=8000).contains(&e.id.index) {
        2048
    } else {
        64
    });
    p.extend_from_slice(&e.id.index.to_le_bytes());
    p.extend_from_slice(&fields.to_le_bytes());
    p.extend_from_slice(&e.state_version.0.to_le_bytes());
    for x in e.bounds.center {
        p.extend_from_slice(&x.to_le_bytes());
    }
    if fields & 2 != 0 {
        p.extend_from_slice(&(e.id.index ^ 0xdead_beef).to_le_bytes());
    }
    if (5001..=8000).contains(&e.id.index) {
        p.resize(2048, 0);
    }
    p
}
fn validate(p: &[u8], peer: usize) -> bool {
    if p.len() < 48 {
        return false;
    }
    let n = u64::from_le_bytes(p[0..8].try_into().unwrap());
    let f = u64::from_le_bytes(p[8..16].try_into().unwrap());
    let own = n == peer as u64 + 1 || n == peer as u64 + 1001;
    f == if own { 3 } else { 1 }
        && p.len()
            == if (5001..=8000).contains(&n) {
                2048
            } else if own {
                56
            } else {
                48
            }
}
struct Peer {
    server: ServerScopes<Vec<u8>>,
    client: ClientScopes<Vec<u8>>,
    group: GroupAssembler<Vec<u8>>,
    scheduler: Scheduler,
    previous: BTreeSet<EntityId>,
    offered: BTreeSet<EntityId>,
    disconnected: bool,
}
impl Peer {
    fn new(p: usize) -> Self {
        Self {
            server: ServerScopes::new(
                ConnectionId(p as u64 + 1),
                ConnectionEpoch(1),
                0,
                POLICY,
                ScopeLimits::default(),
            )
            .unwrap(),
            client: ClientScopes::new(ConnectionEpoch(1), ScopeLimits::default()).unwrap(),
            group: GroupAssembler::new(ConnectionEpoch(1), GroupLimits::default()).unwrap(),
            scheduler: Scheduler::new(
                ConnectionId(p as u64 + 1),
                SchedulerLimits {
                    burst_bytes: 6000,
                    ..Default::default()
                },
            )
            .unwrap(),
            previous: BTreeSet::new(),
            offered: BTreeSet::new(),
            disconnected: false,
        }
    }
}
#[derive(Default)]
struct Metrics {
    gather_ns: u128,
    offer_encode_ns: u128,
    schedule_ns: u128,
    decode_ns: u128,
    visits: usize,
    eligible: usize,
    bytes: usize,
    sent: usize,
    overload: usize,
    capacity: usize,
    pending: usize,
    scope_bytes: usize,
    deferred: usize,
    age: u64,
    misses: usize,
    critical: usize,
    active: usize,
    resets: usize,
    oracle: usize,
    dormant_transmitted: usize,
    pending_new_versions: usize,
}
fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("output directory required");
    let mode = args.next();
    assert!(args.next().is_none(), "unexpected argument");
    if mode.as_deref() == Some("--maintenance-probe") {
        maintenance_probe(&dir, false);
        return;
    }
    if mode.as_deref() == Some("--mixed-maintenance-probe") {
        maintenance_probe(&dir, true);
        return;
    }
    if mode.as_deref() == Some("--canonical-probe") {
        canonical_probe(&dir);
        return;
    }
    assert!(mode.is_none(), "unknown mode");
    fs::create_dir_all(&dir).unwrap();
    let mut csv = File::create(format!("{dir}/frames.csv")).unwrap();
    writeln!(csv,"scenario,frame,maintenance_ns,prepare_ns,gather_ns,offer_encode_ns,schedule_ns,decode_ns,successful_candidate_visits,successful_eligible_entities,encoded_codec_bytes,transmitted_entities,gather_rejections,scope_capacity_events,pending_transitions,scope_payload_capacity_bytes,deferred_units,max_age_ticks,deadline_misses,critical_deadline_misses,active_peers,terminal_resets,oracle_checks,entities,cells,memberships,dormant_transmitted,pending_new_versions,shared_refreshed_actors").unwrap();
    let mut actors = fixture();
    let mut baseline = Vec::with_capacity(100);
    let heap_before_graph = live_heap();
    let mut graph = Graph::new(Limits::default()).unwrap();
    for e in &actors[..5000] {
        graph
            .apply(ServerTick(0), Change::Upsert(e.clone()))
            .unwrap();
    }
    for p in 0..PEERS {
        graph
            .set_connection(
                ServerTick(0),
                ConnectionId(p as u64 + 1),
                view(p, false, true),
                &Policy,
            )
            .unwrap();
    }
    let graph_5000_heap = live_heap() - heap_before_graph;
    {
        let prepared = graph
            .prepare(ReplicationFrame(1), ServerTick(0), POLICY)
            .unwrap();
        for p in 0..PEERS {
            let e = prepared
                .gather(
                    ConnectionId(p as u64 + 1),
                    &BTreeSet::new(),
                    &BTreeSet::from([id(p as u64 + 1)]),
                    &Policy,
                )
                .unwrap();
            assert_eq!(
                e.entries()
                    .map(|e| (e.entity(), e.representation().fields))
                    .collect::<BTreeMap<_, _>>(),
                oracle(&actors[..5000], p, &view(p, false, true), &BTreeSet::new())
            );
            baseline.push(e.candidate_visits());
        }
    }
    let heap_before_expansion = live_heap();
    for e in &actors[5000..] {
        graph
            .apply(ServerTick(0), Change::Upsert(e.clone()))
            .unwrap();
    }
    {
        let prepared = graph
            .prepare(ReplicationFrame(2), ServerTick(0), POLICY)
            .unwrap();
        for (p, b) in baseline.iter().enumerate() {
            let e = prepared
                .gather(
                    ConnectionId(p as u64 + 1),
                    &BTreeSet::new(),
                    &BTreeSet::from([id(p as u64 + 1)]),
                    &Policy,
                )
                .unwrap();
            assert_eq!(
                e.candidate_visits(),
                *b,
                "unrelated population changed local work"
            );
        }
    }
    let graph_50000_heap = graph_5000_heap + live_heap() - heap_before_expansion;
    fs::write(format!("{dir}/graph-memory.txt"),format!("graph_5000_requested_heap_bytes={graph_5000_heap}\ngraph_50000_requested_heap_bytes={graph_50000_heap}\nIncludes graph-owned entity copies, routes, connections and index allocations. Excludes preallocated fixture and baseline vector. Requested allocation sizes, not RSS or allocator overhead.\n")).unwrap();
    let mut peers: Vec<_> = (0..PEERS).map(Peer::new).collect();
    let mut tick = 0u64;
    let mut refresh: Vec<usize> = graph
        .dynamic_entities()
        .map(|e| e.index as usize - 1)
        .collect();
    let mut total_oracle = 100;
    for phase in [
        "sparse_distributed",
        "mostly_dormant",
        "mass_wake",
        "observer_teleport",
        "dense_hotspot",
    ] {
        eprintln!("running {phase}");
        let frames = if phase == "dense_hotspot" { 2 } else { 90 };
        for frame in 0..frames {
            tick += 1;
            let t = ServerTick(tick);
            let start = Instant::now();
            if frame == 0 && phase == "mostly_dormant" {
                for e in &mut actors[1000..48500] {
                    e.dormant = true;
                    e.state_version.0 += 1;
                    graph.apply(t, Change::Upsert(e.clone())).unwrap();
                }
            }
            if frame == 0 && phase == "mass_wake" {
                for e in &mut actors[1000..11000] {
                    e.dormant = false;
                    e.state_version.0 += 1;
                    graph.apply(t, Change::Upsert(e.clone())).unwrap();
                }
            }
            if phase == "dense_hotspot" && frame == 0 {
                for e in &mut actors {
                    e.bounds.center = [0., 0., 0.];
                    e.dormant = false;
                    e.state_version.0 += 1;
                    graph.apply(t, Change::Upsert(e.clone())).unwrap();
                }
            }
            if frame == 0 && (phase == "mostly_dormant" || phase == "mass_wake") {
                refresh = actors
                    .iter()
                    .enumerate()
                    .filter_map(|(i, e)| (!e.dormant).then_some(i))
                    .collect();
            }
            if phase != "dense_hotspot" {
                for index in &refresh {
                    let e = &mut actors[*index];
                    e.state_version.0 += 1;
                    graph.apply(t, Change::Upsert(e.clone())).unwrap();
                }
            }
            for p in 0..PEERS {
                let mut v = view(
                    p,
                    phase == "observer_teleport",
                    phase != "observer_teleport" || frame >= 6,
                );
                if phase == "dense_hotspot" {
                    v.observers[0].position = [0., 0., 0.];
                }
                graph
                    .set_connection(t, ConnectionId(p as u64 + 1), v, &Policy)
                    .unwrap();
            }
            let maintenance = start.elapsed().as_nanos();
            let account = graph.accounting();
            let start = Instant::now();
            let prepared = graph
                .prepare(ReplicationFrame(tick + 2), t, POLICY)
                .unwrap();
            let prepare = start.elapsed().as_nanos();
            let mut m = Metrics::default();
            for (p, peer) in peers.iter_mut().enumerate() {
                if peer.disconnected && phase != "dense_hotspot" {
                    continue;
                }
                let start = Instant::now();
                let result = prepared.gather(
                    ConnectionId(p as u64 + 1),
                    &peer.previous,
                    &BTreeSet::from([id(p as u64 + 1)]),
                    &Policy,
                );
                m.gather_ns += start.elapsed().as_nanos();
                let eligible = match result {
                    Ok(e) => e,
                    Err(GatherError::CandidateLimit | GatherError::CandidateVisitLimit) => {
                        m.overload += 1;
                        continue;
                    }
                    Err(e) => panic!("{e:?}"),
                };
                if [0, 49, 99].contains(&p)
                    && (frame == 0
                        || frame == frames - 1
                        || (phase == "observer_teleport" && frame == 6))
                {
                    let want = oracle(
                        &actors,
                        p,
                        &view(
                            p,
                            phase == "observer_teleport",
                            phase != "observer_teleport" || frame >= 6,
                        ),
                        &peer.previous,
                    );
                    assert_eq!(
                        eligible
                            .entries()
                            .map(|e| (e.entity(), e.representation().fields))
                            .collect::<BTreeMap<_, _>>(),
                        want,
                        "oracle {phase}/{frame}/{p}"
                    );
                    m.oracle += 1;
                }
                m.visits += eligible.candidate_visits();
                m.eligible += eligible.entries().len();
                peer.server
                    .advance_barrier(eligible.world_revision(), POLICY)
                    .unwrap();
                let current: BTreeSet<_> = eligible.entries().map(EligibleEntry::entity).collect();
                for gone in peer
                    .offered
                    .difference(&current)
                    .copied()
                    .collect::<Vec<_>>()
                {
                    let exit = peer.server.exit(gone).unwrap();
                    peer.server.mark_exit_sent(exit).unwrap();
                    peer.client.apply_exit(exit).unwrap();
                    peer.server.acknowledge_exit(exit).unwrap();
                    peer.offered.remove(&gone);
                }
                peer.previous = current;
                let group_members = match eligible.prediction() {
                    PredictionAdmission::Admitted { members } => members.clone(),
                    _ => BTreeSet::new(),
                };
                let start = Instant::now();
                let order = group_members
                    .iter()
                    .copied()
                    .chain(
                        eligible
                            .entries()
                            .map(EligibleEntry::entity)
                            .filter(|e| !group_members.contains(e)),
                    )
                    .collect::<Vec<_>>();
                for entity in &order {
                    let a = &actors[entity.index as usize - 1];
                    match peer.server.offer(&eligible, *entity, a.dormant, |grant| {
                        payload(a, grant.fields)
                    }) {
                        Ok(_) => {
                            peer.offered.insert(*entity);
                        }
                        Err(ReplicationError::Capacity) => {
                            m.capacity += 1;
                            break;
                        }
                        Err(e) => panic!("offer {phase}/{frame}/{p}: {e:?}"),
                    }
                }
                let cadence = Cadence {
                    period_ticks: 1,
                    maximum_age_ticks: 60,
                    priority: 0,
                    critical: false,
                };
                let mut packets = BTreeMap::<UnitId, Vec<u8>>::new();
                let mut requests = Vec::new();
                if !group_members.is_empty()
                    && group_members
                        .iter()
                        .all(|e| peer.server.pending(*e).is_some())
                {
                    let members = group_members
                        .iter()
                        .map(|e| peer.server.pending(*e).unwrap().clone())
                        .collect::<Vec<_>>();
                    let chunk = GroupChunk {
                        publication: GroupPublication {
                            connection: ConnectionEpoch(1),
                            group: GroupId(1),
                            revision: GroupRevision(if phase == "observer_teleport" {
                                2
                            } else {
                                1
                            }),
                            snapshot: SnapshotId(tick),
                        },
                        end_tick: t,
                        manifest: members.iter().map(|m| m.scope).collect(),
                        index: 0,
                        count: 1,
                        members,
                    };
                    let encoded =
                        encode_group(&chunk, GroupLimits::default(), |p| Ok(p.clone())).unwrap();
                    requests.push(
                        UnitRequest::prediction_group(
                            &eligible,
                            GroupId(1),
                            encoded.len(),
                            Cadence {
                                critical: true,
                                priority: 255,
                                ..cadence
                            },
                        )
                        .unwrap(),
                    );
                    packets.insert(UnitId::PredictionGroup(GroupId(1)), encoded);
                }
                for entity in &order {
                    if group_members.contains(entity) {
                        continue;
                    }
                    if let Some(state) = peer.server.pending(*entity) {
                        let encoded = encode_full_state(state, 16384).unwrap();
                        requests.push(UnitRequest::entity(*entity, encoded.len(), cadence));
                        packets.insert(UnitId::Entity(*entity), encoded);
                    }
                }
                m.offer_encode_ns += start.elapsed().as_nanos();
                let start = Instant::now();
                let plan = peer
                    .scheduler
                    .schedule(
                        &eligible,
                        &requests,
                        Duration::from_nanos(tick * 1_000_000_000 / 30),
                        TransportBudget {
                            available_wire_bytes: 6000,
                            maximum_atomic_wire_bytes: 6000,
                        },
                    )
                    .unwrap();
                let sm = plan.metrics();
                m.deferred += sm.deferred_units;
                m.age = m.age.max(sm.maximum_update_age_ticks);
                m.misses += sm.deadline_misses;
                m.critical += sm.critical_deadline_misses;
                m.schedule_ns += start.elapsed().as_nanos();
                let start = Instant::now();
                let sent = peer
                    .scheduler
                    .transmit(plan, eligible.world_revision(), POLICY, |unit| {
                        let bytes = &packets[&unit.id()];
                        match unit.id() {
                            UnitId::Entity(e) => {
                                let state =
                                    decode_full_state(bytes, ConnectionEpoch(1), 16384).unwrap();
                                let receipt = state.receipt();
                                peer.server.mark_sent(receipt).unwrap();
                                if !(p == 0
                                    && e == id(1101)
                                    && phase == "mostly_dormant"
                                    && frame < 4)
                                {
                                    let ack =
                                        peer.client.apply_full(state, |v| validate(v, p)).unwrap();
                                    peer.server.acknowledge(ack).unwrap();
                                }
                                m.sent += 1;
                                m.dormant_transmitted +=
                                    usize::from(actors[e.index as usize - 1].dormant);
                            }
                            UnitId::PredictionGroup(_) => {
                                let chunk =
                                    decode_group(bytes, GroupLimits::default(), |p| Ok(p.to_vec()))
                                        .unwrap();
                                let count = chunk.members.len();
                                for s in &chunk.members {
                                    peer.server.mark_sent(s.receipt()).unwrap();
                                }
                                let GroupProgress::Published(receipts) = peer
                                    .group
                                    .receive(chunk, tick * 1000 / 30, &mut peer.client, |v| {
                                        validate(v, p)
                                    })
                                    .unwrap()
                                else {
                                    panic!("partial group")
                                };
                                assert_eq!(receipts.len(), count);
                                for ack in receipts {
                                    peer.server.acknowledge(ack).unwrap();
                                }
                                m.sent += count;
                            }
                        }
                        true
                    })
                    .unwrap();
                m.bytes += sent.accepted_wire_bytes;
                m.decode_ns += start.elapsed().as_nanos();
                if phase == "mass_wake" {
                    m.pending_new_versions += peer
                        .offered
                        .iter()
                        .filter(|e| {
                            (1001..=11000).contains(&e.index) && peer.server.pending(**e).is_some()
                        })
                        .count();
                }
                m.pending += peer.server.pending_transitions();
                m.scope_bytes += peer.server.retained_bytes() + peer.client.retained_bytes();
                assert!(peer.server.pending_transitions() <= 1024);
                assert!(peer.server.retained_bytes() <= 4 * 1024 * 1024);
                let active = !group_members.is_empty()
                    && group_members
                        .iter()
                        .all(|e| peer.server.decoded_current(*e).is_some());
                if phase == "observer_teleport" && frame < 6 {
                    assert!(!active)
                }
                m.active += usize::from(active);
                if phase == "observer_teleport"
                    && frame == 66
                    && (peer.server.pending_transitions() > 0
                        || peer.offered.len() < peer.previous.len())
                {
                    peer.server.reset(ConnectionEpoch(2)).unwrap();
                    peer.client.reset(ConnectionEpoch(2)).unwrap();
                    peer.group.reset(ConnectionEpoch(2)).unwrap();
                    peer.disconnected = true;
                    m.resets += 1;
                }
            }
            total_oracle += m.oracle;
            if phase == "mostly_dormant" && frame == 1 {
                assert!(
                    peers[0].server.pending(id(1101)).is_some(),
                    "loss peer must retain dirty version"
                );
                assert!(
                    peers[1].server.decoded_current(id(1101)).is_some(),
                    "other peer must independently finish dormancy"
                );
            }
            if phase == "mostly_dormant" && frame > 10 {
                assert_eq!(
                    m.dormant_transmitted, 0,
                    "unchanged decoded dormant states must not broadcast"
                );
            }
            if phase == "dense_hotspot" && frame == 0 {
                let mut counts = String::new();
                for p in [0, 49, 99] {
                    let mut v = view(p, false, true);
                    v.observers[0].position = [0., 0., 0.];
                    let result = oracle(&actors, p, &v, &BTreeSet::new());
                    counts.push_str(&format!("peer {} brute_force_eligible {} graph_result bounded_rejection candidate_count unknown cap 4096 visit_cap 32768\n",p+1,result.len()));
                }
                fs::write(format!("{dir}/dense-oracle.txt"), counts).unwrap();
            }
            if phase == "dense_hotspot" {
                assert_eq!(m.overload, PEERS);
                assert_eq!(m.sent, 0);
            }
            writeln!(csv,"{phase},{frame},{maintenance},{prepare},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",m.gather_ns,m.offer_encode_ns,m.schedule_ns,m.decode_ns,m.visits,m.eligible,m.bytes,m.sent,m.overload,m.capacity,m.pending,m.scope_bytes,m.deferred,m.age,m.misses,m.critical,m.active,m.resets,m.oracle,account.entities,account.cells,account.memberships,m.dormant_transmitted,m.pending_new_versions,if phase=="dense_hotspot" {0} else {refresh.len()}).unwrap();
        }
    }
    fs::write(format!("{dir}/qualification.txt"),format!("Independent oracle comparisons: {total_oracle}\nSparse 5000/50000 candidate-visit equality: all 100 peers passed\nDense hotspot: all 100 peers bounded gather rejection; no partial group transmitted\nTransport basis: actual full/group codec bytes, synthetic immediate sink; excludes transport framing, encryption, UDP/IP, loss, assets, client prediction and simulation\nScope bytes: actual payload allocation capacities only; metadata excluded\nTotal harness final requested heap: {} bytes; peak: {} bytes\nHost timing: release wall-clock phases, one unpinned worker; no reference hardware or 24-hour soak claim\n", live_heap(), PEAK_HEAP.load(std::sync::atomic::Ordering::Relaxed))).unwrap();
}

/// Short unchanged-population probe: five frames per update pattern, 50,000
/// awake actors and 100 authorized views. No scheduler/transport verdict follows
/// from these timings. The full qualification path remains independently runnable.
fn maintenance_probe(dir: &str, mixed: bool) {
    fs::create_dir_all(dir).unwrap();
    let mut csv = File::create(format!(
        "{dir}/{}.csv",
        if mixed {
            "mixed-maintenance"
        } else {
            "maintenance"
        }
    ))
    .unwrap();
    writeln!(csv, "pattern,frame,actors,peers,updates,changed_footprints,maintenance_ns,prepare_ns,gather_ns,candidate_visits,cells,memberships").unwrap();
    let mut actors = fixture();
    // Keep this probe's small displacement strictly away from cell boundaries.
    // The full qualification workload retains its original fixture positions.
    for actor in &mut actors {
        actor.bounds.center[0] += 0.25;
        actor.bounds.center[2] += 0.25;
    }
    let mut graph = Graph::new(Limits::default()).unwrap();
    for actor in &actors {
        graph
            .apply(ServerTick(0), Change::Upsert(actor.clone()))
            .unwrap();
    }
    for peer in 0..PEERS {
        graph
            .set_connection(
                ServerTick(0),
                ConnectionId(peer as u64 + 1),
                view(peer, false, true),
                &Policy,
            )
            .unwrap();
    }
    assert_eq!(graph.dynamic_entities().count(), 50_000);
    // Preserve all 49,000 prop registrations but mutate only players/dynamics.
    let updated_actors = if mixed { 1000 } else { actors.len() };
    let mut tick = 0;
    for pattern in ["version_only", "small_motion", "cell_motion"] {
        for frame in 0..5 {
            tick += 1;
            let started = Instant::now();
            let mut changed_footprints = 0;
            for actor in &mut actors[..updated_actors] {
                let old_span = influence_span(actor, graph.limits());
                actor.state_version.0 += 1;
                actor.bounds.center[0] += match pattern {
                    "small_motion" => 0.001,
                    "cell_motion" => graph.limits().cell_size,
                    _ => 0.0,
                };
                changed_footprints +=
                    usize::from(old_span != influence_span(actor, graph.limits()));
                graph
                    .apply(ServerTick(tick), Change::Upsert(actor.clone()))
                    .unwrap();
            }
            for peer in 0..PEERS {
                graph
                    .set_connection(
                        ServerTick(tick),
                        ConnectionId(peer as u64 + 1),
                        view(peer, false, true),
                        &Policy,
                    )
                    .unwrap();
            }
            let maintenance = started.elapsed().as_nanos();
            assert_eq!(
                changed_footprints,
                if pattern == "cell_motion" {
                    updated_actors
                } else {
                    0
                }
            );
            let accounting = graph.accounting();
            let started = Instant::now();
            let prepared = graph
                .prepare(ReplicationFrame(tick), ServerTick(tick), POLICY)
                .unwrap();
            let prepare = started.elapsed().as_nanos();
            let mut gather = 0;
            let mut visits = 0;
            for peer in 0..PEERS {
                let started = Instant::now();
                let eligible = prepared
                    .gather(
                        ConnectionId(peer as u64 + 1),
                        &BTreeSet::new(),
                        &BTreeSet::from([id(peer as u64 + 1)]),
                        &Policy,
                    )
                    .unwrap();
                gather += started.elapsed().as_nanos();
                visits += eligible.candidate_visits();
                if frame == 4 && [0, 49, 99].contains(&peer) {
                    assert_eq!(
                        eligible
                            .entries()
                            .map(|entry| (entry.entity(), entry.representation().fields))
                            .collect::<BTreeMap<_, _>>(),
                        oracle(&actors, peer, &view(peer, false, true), &BTreeSet::new())
                    );
                }
            }
            writeln!(
                csv,
                "{pattern},{frame},{},{PEERS},{},{changed_footprints},{maintenance},{prepare},{gather},{visits},{},{}",
                actors.len(),
                updated_actors,
                accounting.cells,
                accounting.memberships
            )
            .unwrap();
        }
    }
}

fn canonical_probe(dir: &str) {
    fs::create_dir_all(dir).unwrap();
    let mut csv = File::create(format!("{dir}/canonical.csv")).unwrap();
    writeln!(csv, "scenario,frame,actors,peers,updates,maintenance_ns,gather_ns,uncached_payload_ns,cached_payload_ns,public_requests,public_encodes,public_hits,private_encodes,retained_bytes,uncached_bytes,cached_bytes,candidate_visits,eligible_entities").unwrap();
    for scenario in [
        "sparse_distributed",
        "dense_shared",
        "mostly_dormant",
        "mass_wake",
    ] {
        let mut actors = fixture();
        if scenario == "dense_shared" {
            for (index, actor) in actors.iter_mut().enumerate() {
                actor.bounds.center = if index < 1000 {
                    [0.0, 0.0, 0.0]
                } else {
                    [
                        6000.0 + (index % 100) as f64,
                        0.0,
                        6000.0 + (index / 100) as f64,
                    ]
                };
            }
        }
        if matches!(scenario, "mostly_dormant" | "mass_wake") {
            for actor in &mut actors[1000..48500] {
                actor.dormant = true;
            }
        }
        let mut graph = Graph::new(Limits::default()).unwrap();
        for actor in &actors {
            graph
                .apply(ServerTick(0), Change::Upsert(actor.clone()))
                .unwrap();
        }
        for peer in 0..PEERS {
            let mut observer = view(peer, false, true);
            if scenario == "dense_shared" {
                observer.observers[0].position = [0.0; 3];
            }
            graph
                .set_connection(
                    ServerTick(0),
                    ConnectionId(peer as u64 + 1),
                    observer,
                    &Policy,
                )
                .unwrap();
        }
        let mut cache = CanonicalPayloadCache::new(50_000, 2 * 1024 * 1024);
        for frame in 1..=5 {
            let tick = ServerTick(frame);
            let started = Instant::now();
            let mut updates = 0;
            for actor in &mut actors {
                if scenario == "mass_wake" && frame == 1 && (1001..=11000).contains(&actor.id.index)
                {
                    actor.dormant = false;
                }
                if !actor.dormant {
                    updates += 1;
                    actor.state_version.0 += 1;
                    graph.apply(tick, Change::Upsert(actor.clone())).unwrap();
                }
            }
            let maintenance = started.elapsed().as_nanos();
            let prepared = graph
                .prepare(ReplicationFrame(frame), tick, POLICY)
                .unwrap();
            cache.begin_frame(prepared.frame(), tick, prepared.world_revision(), POLICY);
            let (mut gather_ns, mut uncached_ns, mut cached_ns) = (0, 0, 0);
            let (
                mut public_requests,
                mut private_encodes,
                mut reference_bytes,
                mut cached_bytes,
                mut visits,
                mut count,
            ) = (0, 0, 0, 0, 0, 0);
            for peer in 0..PEERS {
                let started = Instant::now();
                let eligible = prepared
                    .gather(
                        ConnectionId(peer as u64 + 1),
                        &BTreeSet::new(),
                        &BTreeSet::from([id(peer as u64 + 1)]),
                        &Policy,
                    )
                    .unwrap();
                gather_ns += started.elapsed().as_nanos();
                visits += eligible.candidate_visits();
                count += eligible.entries().len();
                for entry in eligible.entries() {
                    let actor = &actors[entry.entity().index as usize - 1];
                    let fields = entry.representation().fields;
                    let started = Instant::now();
                    let reference = payload(actor, fields);
                    uncached_ns += started.elapsed().as_nanos();
                    let started = Instant::now();
                    let actual = if actor.visibility == Visibility::Public && fields == 1 {
                        public_requests += 1;
                        cache
                            .encode(&eligible, entry.entity(), || {
                                Ok::<_, ()>(payload(actor, fields))
                            })
                            .unwrap()
                    } else {
                        private_encodes += 1;
                        payload(actor, fields)
                    };
                    cached_ns += started.elapsed().as_nanos();
                    assert_eq!(actual, reference);
                    reference_bytes += reference.len();
                    cached_bytes += actual.len();
                }
            }
            let stats = cache.stats();
            assert_eq!(public_requests, stats.hits + stats.encodes);
            assert!(stats.retained_bytes <= 2 * 1024 * 1024);
            writeln!(csv, "{scenario},{frame},{},{PEERS},{updates},{maintenance},{gather_ns},{uncached_ns},{cached_ns},{public_requests},{},{},{private_encodes},{},{reference_bytes},{cached_bytes},{visits},{count}", actors.len(), stats.encodes, stats.hits, stats.retained_bytes).unwrap();
        }
    }
}

fn influence_span(actor: &EntityRegistration, limits: &Limits) -> [i64; 4] {
    let radius = actor.bounds.radius + actor.cull_radius + limits.leave_margin + limits.prefetch;
    [
        ((actor.bounds.center[0] - radius) / limits.cell_size).floor() as i64,
        ((actor.bounds.center[0] + radius) / limits.cell_size).floor() as i64,
        ((actor.bounds.center[2] - radius) / limits.cell_size).floor() as i64,
        ((actor.bounds.center[2] + radius) / limits.cell_size).floor() as i64,
    ]
}

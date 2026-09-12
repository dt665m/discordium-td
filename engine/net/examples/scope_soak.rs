//! Real-wall-clock persistent resource soak of production generic net components.
//! Synthetic delivery does not qualify game, UDP, or browser workloads.
#[path = "scope_soak/model.rs"]
mod model;
#[path = "scope_soak/retirement.rs"]
mod retirement;
use engine_net::{
    commands::OwnerStream, interest::*, prediction::*, replication::*, scheduling::*, types::*,
};
use model::{Input, Model};
use retirement::Retirement;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
    io::Write,
    time::{Duration, Instant},
};
const PEERS: usize = 100;
const ENTITIES: usize = 50_000;
const EPOCH: ConnectionEpoch = ConnectionEpoch(1);
const POLICY: PolicyRevision = PolicyRevision(1);
const SCENE: SceneRevision = SceneRevision(1);
const QUEUE_PACKETS: usize = 512;
const QUEUE_BYTES: usize = 256 * 1024;
const GROUP: GroupId = GroupId(1);
fn byte_limits() -> ByteLimits {
    ByteLimits {
        fragment_payload_bytes: 256,
        group_bytes: 16384,
        fragments: 64,
        incomplete: 8,
        known_groups: 8,
        staging_bytes: 128 * 1024,
        max_age_ms: 2000,
    }
}
fn id(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
fn position(peer: usize) -> [f64; 3] {
    [(peer % 10) as f64 * 256., 0., (peer / 10) as f64 * 256.]
}
fn owner(peer: usize) -> EntityId {
    id(peer as u64 + 1)
}
fn dependency(peer: usize) -> EntityId {
    id(peer as u64 + 101)
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
        connection: ConnectionId,
        entity: &EntityRegistration,
    ) -> Option<RepresentationGrant> {
        Some(RepresentationGrant {
            schema_id: 1,
            revision: RepresentationRevision(1),
            fields: if entity.routes.owner == Some(connection) {
                3
            } else {
                1
            },
            prediction_allowed: true,
        })
    }
    fn permits_dependency(&self, _: ConnectionId, _: EntityId, _: EntityId) -> bool {
        true
    }
}
fn view(peer: usize, away: bool) -> ConnectionView {
    let mut p = position(peer);
    if away {
        p[0] += 112.;
        p[2] += 112.;
    }
    ConnectionView {
        observers: vec![ObserverGrant {
            position: p,
            scene_revision: SCENE,
        }],
        semantic_grants: BTreeSet::new(),
        ready_scenes: BTreeSet::from([SCENE]),
    }
}
fn fixture() -> Vec<EntityRegistration> {
    (1..=ENTITIES as u64)
        .map(|n| {
            let peer = ((n - 1) % PEERS as u64) as usize;
            // Unrelated population shares distant cells without overflowing grid capacity.
            let p = if n <= 5000 {
                position(peer)
            } else {
                [
                    8192. + (n % 100) as f64 * 16.,
                    0.,
                    8192. + (n / 100) as f64 * 16.,
                ]
            };
            EntityRegistration {
                id: id(n),
                bounds: Bounds {
                    center: p,
                    radius: 1.,
                },
                cull_radius: 48.,
                routes: Routes {
                    spatial: Some(SpatialRoute::DormancyDriven),
                    owner: (n <= 200).then_some(ConnectionId(peer as u64 + 1)),
                    ..Default::default()
                },
                visibility: if n <= 200 {
                    Visibility::Owner(ConnectionId(peer as u64 + 1))
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
                dormant: n > 5000,
            }
        })
        .collect()
}
fn payload(actor: &EntityRegistration, tick: u64) -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes[..8].copy_from_slice(&tick.to_le_bytes());
    bytes[8..16].copy_from_slice(&actor.id.index.to_le_bytes());
    bytes[16..20].copy_from_slice(&actor.id.generation.to_le_bytes());
    bytes
}
struct Packet {
    due: u64,
    group: bool,
    bytes: Vec<u8>,
}
struct Peer {
    server: ServerScopes<Vec<u8>>,
    client: ClientScopes<Vec<u8>>,
    groups: GroupAssembler<Vec<u8>>,
    byte_groups: ByteAssembler,
    scheduler: Scheduler,
    predictor: PredictionManager<Model>,
    model: Model,
    binding: PredictionBinding,
    dependencies: DependencyFrame<Vec<u8>>,
    previous: BTreeSet<EntityId>,
    queue: VecDeque<Packet>,
    queue_bytes: usize,
    retirement: Retirement,
}
impl Peer {
    fn new(peer: usize) -> Self {
        let connection = ConnectionId(peer as u64 + 1);
        Self {
            server: ServerScopes::new(connection, EPOCH, 0, POLICY, ScopeLimits::default())
                .unwrap(),
            client: ClientScopes::new(EPOCH, ScopeLimits::default()).unwrap(),
            groups: GroupAssembler::new(EPOCH, GroupLimits::default()).unwrap(),
            byte_groups: ByteAssembler::new(EPOCH, byte_limits()).unwrap(),
            scheduler: Scheduler::new(connection, SchedulerLimits::default()).unwrap(),
            predictor: PredictionManager::new(EPOCH, PredictionLimits::default()).unwrap(),
            model: Model {
                dependency: dependency(peer),
            },
            binding: PredictionBinding {
                owner: OwnerStream {
                    connection,
                    epoch: EPOCH,
                    stream: CommandStream(1),
                    owner: owner(peer),
                    ownership: OwnershipEpoch(1),
                },
                scene_revision: SCENE,
                teleport_segment: 0,
            },
            dependencies: DependencyFrame::default(),
            previous: BTreeSet::new(),
            queue: VecDeque::with_capacity(QUEUE_PACKETS),
            queue_bytes: 0,
            retirement: Retirement::new(owner(peer)),
        }
    }
    fn predict_through(&mut self, tick: u64) {
        while let Some(predicted) = self.predictor.predicted_tick(GROUP) {
            if predicted.0 >= tick {
                break;
            }
            self.predictor
                .predict(
                    GROUP,
                    Input {
                        owner: self.binding.owner,
                        tick: predicted.0 + 1,
                    },
                    self.dependencies.clone(),
                    &self.model,
                )
                .unwrap();
            assert!(self.predictor.recovery(GROUP).is_none());
        }
    }
    fn deliver(&mut self, tick: u64, metrics: &mut Metrics) {
        // Inspect only the bounded queue once; reverse insertion order creates real reorder.
        let count = self.queue.len();
        for _ in 0..count {
            let packet = self.queue.pop_front().unwrap();
            if packet.due > tick {
                self.queue.push_back(packet);
                continue;
            }
            self.queue_bytes -= packet.bytes.capacity();
            if packet.group {
                let fragment = ByteFragment::decode(&packet.bytes, byte_limits()).unwrap();
                let complete = match self.byte_groups.receive(fragment, tick * 1000 / 30) {
                    Ok(Some(complete)) => complete,
                    Ok(None) => {
                        metrics.partial += 1;
                        continue;
                    }
                    Err(ReplicationError::Stale | ReplicationError::Expired) => {
                        metrics.stale += 1;
                        continue;
                    }
                    error => panic!("group fragment: {error:?}"),
                };
                match complete.decode_and_publish(
                    &mut self.groups,
                    &mut self.client,
                    tick * 1000 / 30,
                    |bytes| decode_group(bytes, GroupLimits::default(), |p| Ok(p.to_vec())),
                    |p| p.len() == 64,
                ) {
                    Ok(GroupProgress::Published(receipts)) => {
                        for receipt in receipts {
                            acknowledge(&mut self.server, receipt, metrics);
                        }
                        let proof = self.groups.published_group(GROUP, &self.client).unwrap();
                        self.dependencies = self
                            .model
                            .decode_group(&proof, self.binding)
                            .unwrap()
                            .dependencies;
                        if self.predictor.group_count() == 0 {
                            self.predictor
                                .initialize(&proof, self.binding, &self.model)
                                .unwrap();
                        } else {
                            // Periodically withhold reconciliation while retaining 256 ticks of real manager history.
                            if !(30..300).contains(&(tick % 600)) {
                                self.predictor
                                    .reconcile(&proof, self.binding, proof.end_tick(), &self.model)
                                    .unwrap();
                            }
                        }
                        self.predict_through(tick + 2);
                        metrics.groups += 1;
                    }
                    Ok(GroupProgress::Staged) => metrics.partial += 1,
                    Err(ReplicationError::Stale | ReplicationError::Expired) => metrics.stale += 1,
                    other => panic!("group receive: {other:?}"),
                }
            } else {
                let state = decode_full_state(&packet.bytes, EPOCH, 16384).unwrap();
                match self.client.apply_full(state, |p| p.len() == 64) {
                    Ok(receipt) => acknowledge(&mut self.server, receipt, metrics),
                    Err(ReplicationError::Stale) => metrics.stale += 1,
                    other => panic!("state receive: {other:?}"),
                }
            }
        }
        self.groups.expire(tick * 1000 / 30).unwrap();
        self.byte_groups.expire(tick * 1000 / 30).unwrap();
    }
}
fn acknowledge(server: &mut ServerScopes<Vec<u8>>, receipt: StateReceipt, metrics: &mut Metrics) {
    match server.acknowledge(receipt) {
        Ok(()) => metrics.decoded += 1,
        Err(ReplicationError::Stale | ReplicationError::Unknown) => metrics.stale += 1,
        error => panic!("decode receipt: {error:?}"),
    }
}
#[derive(Default)]
struct Metrics {
    sent: u64,
    dropped: u64,
    reordered: u64,
    stale: u64,
    decoded: u64,
    groups: u64,
    partial: u64,
    exits: u64,
    destroyed: u64,
    dormant_offers: u64,
    deferred: u64,
    peak_queue: usize,
    peak_queue_bytes: usize,
    peak_scope_bytes: usize,
    peak_prediction_bytes: usize,
    peak_retirement_bytes: usize,
    peak_known: usize,
    peak_scheduler: usize,
    max_tick_micros: u128,
    late_ticks: u64,
    full_history_windows: u64,
    retired_history_checks: u64,
}
fn report(
    file: &mut File,
    phase: &str,
    elapsed: Duration,
    tick: u64,
    graph: &Graph,
    peers: &[Peer],
    m: &Metrics,
) {
    let account = graph.accounting();
    let retirement_count: u64 = peers.iter().map(|p| p.retirement.retired).sum();
    let baseline_loss: u64 = peers.iter().map(|p| p.retirement.lost).sum();
    let retained: usize = peers
        .iter()
        .map(|p| {
            p.server.retained_bytes()
                + p.client.retained_bytes()
                + p.predictor.retained_bytes()
                + p.retirement.bytes()
                + p.groups.staged_bytes()
                + p.byte_groups.staged_bytes()
                + p.queue_bytes
        })
        .sum();
    let baseline_states: usize = peers.iter().map(|p| p.retirement.states()).sum();
    let events: usize = peers.iter().map(|p| p.retirement.events()).sum();
    writeln!(file, "{{\"phase\":\"{phase}\",\"elapsed_seconds\":{:.6},\"ticks\":{tick},\"entities\":{},\"connections\":{},\"observers\":{},\"cells\":{},\"memberships\":{},\"world_revision\":{},\"sent\":{},\"dropped\":{},\"reordered\":{},\"stale_rejected\":{},\"decoded\":{},\"groups_published\":{},\"partial_groups\":{},\"scope_exits\":{},\"destroyed_generations\":{},\"dormant_offers\":{},\"deferred_units\":{},\"peak_queue_packets_per_peer\":{},\"peak_queue_bytes_per_peer\":{},\"peak_scope_bytes_per_peer\":{},\"peak_prediction_bytes_per_peer\":{},\"peak_retirement_bytes_per_peer\":{},\"peak_known_entities_per_peer\":{},\"peak_scheduler_units_per_peer\":{},\"retained_payload_and_accounted_history_bytes\":{retained},\"baseline_retirements\":{retirement_count},\"baseline_packet_or_receipt_loss\":{baseline_loss},\"baseline_retained_states\":{baseline_states},\"event_records\":{events},\"full_history_windows\":{},\"retired_history_checks\":{},\"max_tick_micros\":{},\"late_ticks\":{},\"production_game_or_transport_soak\":false}}",
        elapsed.as_secs_f64(), account.entities, account.connections, account.observers, account.cells, account.memberships,
        account.world_revision, m.sent, m.dropped, m.reordered, m.stale, m.decoded, m.groups, m.partial,
        m.exits, m.destroyed, m.dormant_offers, m.deferred, m.peak_queue, m.peak_queue_bytes,
        m.peak_scope_bytes, m.peak_prediction_bytes, m.peak_retirement_bytes, m.peak_known, m.peak_scheduler,
        m.full_history_windows, m.retired_history_checks, m.max_tick_micros, m.late_ticks).unwrap();
    file.flush().unwrap();
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        4,
        "usage: scope_soak SECONDS REPORT_SECONDS METRICS_JSONL"
    );
    let seconds: u64 = args[1].parse().unwrap();
    let report_seconds: u64 = args[2].parse().unwrap();
    assert!((1..=604800).contains(&seconds) && (1..=3600).contains(&report_seconds));
    assert!(
        seconds.div_ceil(report_seconds) <= 1024,
        "at most 1024 interval reports"
    );
    let mut file = File::create(&args[3]).unwrap();
    let mut actors = fixture();
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
                view(peer, false),
                &Policy,
            )
            .unwrap();
    }
    let mut peers: Vec<_> = (0..PEERS).map(Peer::new).collect();
    let mut metrics = Metrics::default();
    let start = Instant::now();
    let mut next = start;
    let interval = Duration::from_nanos(1_000_000_000 / 30);
    let mut next_report = Duration::from_secs(report_seconds);
    let mut tick = 0u64;
    report(
        &mut file,
        "start",
        start.elapsed(),
        tick,
        &graph,
        &peers,
        &metrics,
    );
    while start.elapsed() < Duration::from_secs(seconds) {
        let work_start = Instant::now();
        tick = tick.checked_add(1).unwrap();
        let now = ServerTick(tick);
        let second = tick / 30;
        // Always keep owner/dependency groups alive. Only spatial public scopes churn.
        if tick % 30 == 1 {
            for p in 0..PEERS {
                let slot = p + 200;
                let old = actors[slot].id;
                for peer in &mut peers {
                    if peer.previous.remove(&old) {
                        let death = peer.server.destroy(old).unwrap();
                        peer.server.mark_destroy_sent(death).unwrap();
                        peer.client.apply_destroy(death).unwrap();
                        peer.server.acknowledge_destroy(death).unwrap();
                        metrics.destroyed += 1;
                    }
                }
                graph.apply(now, Change::Remove(old)).unwrap();
                actors[slot].id.generation = actors[slot].id.generation.checked_add(1).unwrap();
                actors[slot].state_version = StateVersion(tick + 1);
                graph
                    .apply(now, Change::Upsert(actors[slot].clone()))
                    .unwrap();
            }
            for p in 0..PEERS {
                graph
                    .set_connection(
                        now,
                        ConnectionId(p as u64 + 1),
                        view(p, second % 20 >= 15),
                        &Policy,
                    )
                    .unwrap();
            }
        }
        for (index, actor) in actors[..5000].iter_mut().enumerate() {
            let dormant = index >= 200 && second % 30 >= 10 && second % 30 < 20;
            // Explicit dormant mutations only on entry; owner group has fresh end-tick each step.
            if !dormant || !actor.dormant {
                actor.dormant = dormant;
                actor.state_version = StateVersion(tick + 1);
                graph.apply(now, Change::Upsert(actor.clone())).unwrap();
            }
        }
        {
            let prepared = graph.prepare(ReplicationFrame(tick), now, POLICY).unwrap();
            for (p, peer) in peers.iter_mut().enumerate() {
                peer.predictor.begin_frame(ReplicationFrame(tick)).unwrap();
                peer.predict_through(tick + 2);
                peer.deliver(tick, &mut metrics);
                let eligible = prepared
                    .gather(
                        ConnectionId(p as u64 + 1),
                        &peer.previous,
                        &BTreeSet::from([owner(p)]),
                        &Policy,
                    )
                    .unwrap();
                assert!(eligible.entries().len() <= 50);
                peer.server
                    .advance_barrier(eligible.world_revision(), POLICY)
                    .unwrap();
                let current: BTreeSet<_> = eligible.entries().map(EligibleEntry::entity).collect();
                for gone in peer
                    .previous
                    .difference(&current)
                    .copied()
                    .collect::<Vec<_>>()
                {
                    let exit = peer.server.exit(gone).unwrap();
                    peer.server.mark_exit_sent(exit).unwrap();
                    peer.client.apply_exit(exit).unwrap();
                    peer.server.acknowledge_exit(exit).unwrap();
                    metrics.exits += 1;
                }
                let PredictionAdmission::Admitted { members } = eligible.prediction() else {
                    panic!("owner dependency lost");
                };
                assert_eq!(members, &BTreeSet::from([owner(p), dependency(p)]));
                for entry in eligible.entries() {
                    let actor = &actors[entry.entity().index as usize - 1];
                    let unchanged_dormant = actor.dormant
                        && peer
                            .server
                            .decoded_current(actor.id)
                            .is_some_and(|r| r.version == actor.state_version);
                    peer.server
                        .offer(&eligible, actor.id, actor.dormant, |_| {
                            payload(
                                actor,
                                if actor.id.index <= 200 {
                                    tick
                                } else {
                                    actor.state_version.0
                                },
                            )
                        })
                        .unwrap();
                    if unchanged_dormant {
                        assert!(peer.server.pending(actor.id).is_none());
                        metrics.dormant_offers += 1;
                    }
                }
                peer.previous = current;
                let group_states: Vec<_> = members
                    .iter()
                    .map(|e| peer.server.pending(*e).unwrap().clone())
                    .collect();
                let chunk = GroupChunk {
                    publication: GroupPublication {
                        connection: EPOCH,
                        group: GROUP,
                        revision: GroupRevision(1),
                        snapshot: SnapshotId(tick),
                    },
                    end_tick: now,
                    manifest: group_states.iter().map(|s| s.scope).collect(),
                    index: 0,
                    count: 1,
                    members: group_states,
                };
                let group_bytes =
                    encode_group(&chunk, GroupLimits::default(), |p| Ok(p.clone())).unwrap();
                let encoded: Vec<Vec<u8>> =
                    fragment_group(chunk.publication, &group_bytes, byte_limits())
                        .unwrap()
                        .iter()
                        .map(|fragment| fragment.encode(byte_limits()).unwrap())
                        .collect();
                let cadence = Cadence {
                    period_ticks: 1,
                    maximum_age_ticks: 60,
                    priority: 0,
                    critical: false,
                };
                let mut requests = vec![
                    UnitRequest::prediction_group(
                        &eligible,
                        GROUP,
                        encoded.iter().map(Vec::len).sum(),
                        Cadence {
                            priority: 255,
                            critical: true,
                            ..cadence
                        },
                    )
                    .unwrap(),
                ];
                let mut packets = BTreeMap::from([(UnitId::PredictionGroup(GROUP), encoded)]);
                for entity in peer.previous.iter().filter(|e| !members.contains(e)) {
                    if let Some(state) = peer.server.pending(*entity) {
                        let bytes = encode_full_state(state, 16384).unwrap();
                        requests.push(UnitRequest::entity(*entity, bytes.len(), cadence));
                        packets.insert(UnitId::Entity(*entity), vec![bytes]);
                    }
                }
                let plan = peer
                    .scheduler
                    .schedule(
                        &eligible,
                        &requests,
                        start.elapsed(),
                        TransportBudget {
                            available_wire_bytes: 8192,
                            maximum_atomic_wire_bytes: 8192,
                        },
                    )
                    .unwrap();
                metrics.deferred += plan.metrics().deferred_units as u64;
                peer.scheduler
                    .transmit(plan, eligible.world_revision(), POLICY, |unit| {
                        let batch = &packets[&unit.id()];
                        if peer.queue.len() + batch.len() > QUEUE_PACKETS
                            || peer.queue_bytes + batch.iter().map(Vec::len).sum::<usize>()
                                > QUEUE_BYTES
                        {
                            return false;
                        }
                        let group = matches!(unit.id(), UnitId::PredictionGroup(_));
                        if group {
                            for state in &chunk.members {
                                peer.server.mark_sent(state.receipt()).unwrap();
                            }
                        } else if let UnitId::Entity(entity) = unit.id() {
                            peer.server
                                .mark_sent(peer.server.pending(entity).unwrap().receipt())
                                .unwrap();
                        }
                        for (part, bytes) in batch.iter().enumerate() {
                            metrics.sent += 1;
                            // Each part has independent bounded loss/delay; only complete groups publish.
                            let salt = match unit.id() {
                                UnitId::Entity(entity) => entity.index,
                                UnitId::PredictionGroup(_) => part as u64,
                            };
                            if (tick + p as u64 + salt) % 7 == 0 {
                                metrics.dropped += 1;
                                continue;
                            }
                            let delay = (tick + p as u64 + salt) % 3;
                            let packet = Packet {
                                due: tick + delay,
                                group,
                                bytes: bytes.clone(),
                            };
                            peer.queue_bytes += packet.bytes.capacity();
                            if delay == 0 {
                                peer.queue.push_front(packet);
                                metrics.reordered += 1;
                            } else {
                                peer.queue.push_back(packet);
                            }
                        }
                        true
                    })
                    .unwrap();
                // A second delivery permits same-frame messages while respecting the same prediction work budget.
                peer.deliver(tick, &mut metrics);
                peer.predict_through(tick + 2);
                if let Some(predicted) = peer.predictor.predicted_tick(GROUP) {
                    if predicted.0 > 257 {
                        assert!(
                            peer.predictor
                                .history_state(GROUP, ServerTick(predicted.0 - 257))
                                .is_none()
                        );
                        metrics.retired_history_checks += 1;
                        if peer
                            .predictor
                            .history_state(GROUP, ServerTick(predicted.0 - 256))
                            .is_some()
                        {
                            metrics.full_history_windows += 1;
                        }
                    }
                }
                peer.retirement.step(tick);
                assert!(peer.server.known_entities() <= 50 && peer.client.known_entities() <= 50);
                assert!(
                    peer.server.pending_transitions() <= ScopeLimits::default().pending_transitions
                );
                assert!(
                    peer.server.retained_bytes() <= ScopeLimits::default().retained_payload_bytes
                );
                assert!(
                    peer.client.retained_bytes() <= ScopeLimits::default().retained_payload_bytes
                );
                assert!(
                    peer.predictor.retained_bytes() <= PredictionLimits::default().history_bytes
                );
                assert!(peer.groups.incomplete() <= GroupLimits::default().incomplete);
                assert!(peer.byte_groups.incomplete() <= byte_limits().incomplete);
                assert!(peer.byte_groups.staged_bytes() <= byte_limits().staging_bytes);
                assert!(peer.scheduler.known_units() <= SchedulerLimits::default().units);
                metrics.peak_known = metrics
                    .peak_known
                    .max(peer.server.known_entities())
                    .max(peer.client.known_entities());
                metrics.peak_scheduler = metrics.peak_scheduler.max(peer.scheduler.known_units());
                metrics.peak_queue = metrics.peak_queue.max(peer.queue.len());
                metrics.peak_queue_bytes = metrics.peak_queue_bytes.max(peer.queue_bytes);
                metrics.peak_scope_bytes = metrics
                    .peak_scope_bytes
                    .max(peer.server.retained_bytes() + peer.client.retained_bytes());
                metrics.peak_prediction_bytes = metrics
                    .peak_prediction_bytes
                    .max(peer.predictor.retained_bytes());
                metrics.peak_retirement_bytes =
                    metrics.peak_retirement_bytes.max(peer.retirement.bytes());
            }
        }
        assert_eq!(graph.accounting().entities, ENTITIES);
        assert_eq!(graph.accounting().connections, PEERS);
        assert_eq!(graph.accounting().observers, PEERS);
        metrics.max_tick_micros = metrics
            .max_tick_micros
            .max(work_start.elapsed().as_micros());
        let elapsed = start.elapsed();
        if elapsed >= next_report {
            report(
                &mut file, "interval", elapsed, tick, &graph, &peers, &metrics,
            );
            next_report += Duration::from_secs(report_seconds);
        }
        next += interval;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        } else {
            metrics.late_ticks += 1;
            next = Instant::now();
        }
    }
    report(
        &mut file,
        "final",
        start.elapsed(),
        tick,
        &graph,
        &peers,
        &metrics,
    );
}

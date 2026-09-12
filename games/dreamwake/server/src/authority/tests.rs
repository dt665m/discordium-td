use super::*;
use engine_net::clock::TickClock;
use renet::RenetClient;
#[test]
fn fast_receive_polling_keeps_upstream_renet_sends_at_tick_rate() {
    let mut server = RenetClient::new(wire::connection_config());
    let mut client = RenetClient::new(wire::connection_config());
    server.set_connected();
    client.set_connected();
    client.send_message(wire::ACTION_CHANNEL, b"reliable action".to_vec());
    let mut clock = TickClock::new(dreamwake_sim::TICK_HZ, wire::TICKS_PER_SNAPSHOT);
    let mut to_server = VecDeque::<(Duration, Vec<u8>)>::new();
    let mut to_client = VecDeque::<(Duration, Vec<u8>)>::new();
    let mut flushes = 0;
    let mut sent_packets = 0;
    let mut snapshots = 0;
    let mut actions = 0;
    for millis in 1..=3_000 {
        let now = Duration::from_millis(millis);
        server.update(Duration::from_millis(1));
        client.update(Duration::from_millis(1));
        while to_server.front().is_some_and(|(at, _)| *at <= now) {
            server.process_packet(&to_server.pop_front().unwrap().1);
        }
        while to_client.front().is_some_and(|(at, _)| *at <= now) {
            client.process_packet(&to_client.pop_front().unwrap().1);
        }
        while server.receive_message(wire::INPUT_CHANNEL).is_some() {}
        while server.receive_message(wire::ACTION_CHANNEL).is_some() {
            actions += 1;
        }
        while client.receive_message(wire::STATE_CHANNEL).is_some() {
            snapshots += 1;
        }
        let tick = clock.advance(now, || {});
        if tick.snapshot_due {
            server.send_message(wire::STATE_CHANNEL, vec![1]);
        }
        if tick.send_due {
            flushes += 1;
            client.send_message(wire::INPUT_CHANNEL, vec![1]);
            for packet in client.get_packets_to_send() {
                to_server.push_back((now + Duration::from_millis(50), packet));
            }
            for packet in server.get_packets_to_send() {
                sent_packets += 1;
                to_client.push_back((now + Duration::from_millis(50), packet));
            }
        }
    }
    assert_eq!(flushes, 180, "60 Hz sends despite 1 kHz receive polling");
    assert!(
        sent_packets <= 240,
        "at most 60 ACKs + 20 snapshots per second"
    );
    assert!(snapshots >= 58, "20 Hz snapshots arrive across 100 ms RTT");
    assert_eq!(actions, 1, "reliable action delivered exactly once");
    assert_eq!(server.packet_loss(), 0.0);
    assert_eq!(client.packet_loss(), 0.0);
}
#[test]
fn send_tick_continues_without_publications_and_coalesces_catchup() {
    let mut clock = TickClock::new(dreamwake_sim::TICK_HZ, wire::TICKS_PER_SNAPSHOT);
    let first = clock.advance(Duration::ZERO, || {});
    assert!(!first.send_due);
    assert!(!first.snapshot_due);
    assert!(!clock.advance(Duration::from_millis(1), || {}).send_due);
    let resumed = Duration::from_secs(600);
    assert!(clock.advance(resumed, || {}).send_due);
    assert!(!clock.advance(resumed, || {}).send_due);
    let next = clock.advance(resumed + Duration::from_nanos(16_666_667), || {});
    assert!(next.send_due);
    assert!(next.snapshot_due);
    assert!(next.health.overloaded);
}
#[test]
fn catchup_retains_debt_and_coalesces_publication_until_the_next_flush() {
    let mut clock = TickClock::new(dreamwake_sim::TICK_HZ, wire::TICKS_PER_SNAPSHOT);
    let mut steps = 0;
    assert!(!clock.advance(Duration::ZERO, || steps += 1).snapshot_due);
    let resumed = Duration::from_secs(600);
    assert!(clock.advance(resumed, || steps += 1).snapshot_due);
    assert_eq!(
        steps, 4,
        "S[0] has no step; each catch-up burst commits at most four ticks"
    );
    assert!(!clock.advance(resumed, || steps += 1).snapshot_due);
    assert!(
        clock
            .advance(resumed + Duration::from_nanos(16_666_667), || steps += 1)
            .snapshot_due
    );
    assert_eq!(
        steps, 12,
        "the next publication includes every committed catch-up step"
    );
    assert_eq!(clock.health(resumed).debt_ticks, 36_001 - 12);
}
#[test]
fn publication_clock_keeps_servicing_paused_or_intro_state() {
    let mut clock = TickClock::new(dreamwake_sim::TICK_HZ, wire::TICKS_PER_SNAPSHOT);
    let mut authority = DreamAuthority::new(8192, false, 8);
    assert!(authority.admit(41));
    let mut publications = 0;
    for tick in 1..=6 {
        let now = engine_net::types::TickRate::new(dreamwake_sim::TICK_HZ)
            .unwrap()
            .deadline(engine_net::types::ServerTick(tick))
            .unwrap();
        publications += usize::from(clock.advance(now, || authority.step(now)).snapshot_due);
    }
    assert_eq!(
        publications,
        (6 / wire::TICKS_PER_SNAPSHOT) as usize,
        "publication follows server steps, not the paused gameplay tick"
    );
}

use dreamwake_protocol::replication::{ReplicaPayload, decode_replica};
use dreamwake_sim::replication::{OwnerExpectation, OwnerPredictionState};
use dreamwake_sim::{DreamPresentation, EnemyKind, RewardKind, UpgradeKind};
use engine_net::replication::*;
use engine_server::{ServerDriver, SharedNet};
use renet_cross::{
    BootstrapConfig, BootstrapService, MixedTransportBuilder, MonotonicClientIdAllocator,
    NativeConnectOptions, PacketProfile, SecureSessionAuthPolicy, ServerAuthentication,
    SessionCreateRequest, UdpNetcodeClientTransport,
};
use std::sync::{Arc, Mutex};
type TestBootstrap = BootstrapService<
    MonotonicClientIdAllocator,
    SecureSessionAuthPolicy<crate::admission::GuestAdmission>,
>;

struct TestClient {
    baselines: BTreeMap<
        EntityId,
        (
            engine_net::replication::baselines::BaselineContext,
            engine_net::replication::baselines::ClientBaselines<Vec<u8>>,
        ),
    >,
    owner_proofs: Vec<live::baselines::DecodedBaseline>,
    collision_cache: Option<(Vec<u8>, [u8; 32])>,
    id: u64,
    renet: RenetClient,
    transport: UdpNetcodeClientTransport,
    welcome: Option<live::Welcome>,
    bytes: Option<ByteAssembler>,
    groups: Option<GroupAssembler<Vec<u8>>>,
    scopes: Option<ClientScopes<Vec<u8>>>,
    known: BTreeSet<EntityId>,
    active: bool,
    sequence: u32,
    commands: u64,
    menu_sequence: u64,
    held: DreamInput,
    edge: Option<DreamAction>,
    history: VecDeque<engine_net::commands::Command<live::TickInput, DreamAction>>,
    view: Option<DreamPresentation>,
    finalized: ServerTick,
    finalized_sequences: BTreeSet<CommandSeq>,
    active_proof: Option<SnapshotId>,
    decoded_groups: BTreeSet<SnapshotId>,
    notices: Vec<String>,
    outcomes: BTreeMap<ActionKey, live::ActionOutcome>,
    max_packet: usize,
    drop_states: bool,
    send_proofs: bool,
    receive_application: bool,
}
impl TestClient {
    fn control(&mut self, value: live::ClientControl) {
        let Some(welcome) = &self.welcome else {
            return;
        };
        self.sequence += 1;
        let bytes = live::encode_control(
            &value,
            welcome.stream.epoch,
            self.sequence,
            welcome.frame_limit().unwrap(),
        )
        .unwrap();
        self.renet.send_message(wire::CONTROL_CHANNEL, bytes);
    }
    fn expected(&self) -> OwnerExpectation {
        let w = self.welcome.as_ref().unwrap();
        OwnerExpectation {
            owner: w.player.get(),
            match_epoch: w.match_epoch,
            ownership_revision: w.stream.ownership.0,
            scene_revision: u64::from(w.content.scene_revision.0),
            minimum_revision: 0,
        }
    }
    fn receive(&mut self, now: Duration) {
        if !self.receive_application {
            return;
        }
        let mut changed = false;
        let mut finalized_ack = None;
        let mut outcomes_ack = None;
        while let Some(bytes) = self.renet.receive_message(wire::CONTROL_CHANNEL) {
            if live::is_welcome(&bytes) {
                let welcome = live::decode_welcome(&bytes).unwrap();
                assert!(welcome.compatible(ConnectionId(self.id)));
                if self
                    .welcome
                    .as_ref()
                    .is_some_and(|old| old.stream.epoch == welcome.stream.epoch)
                {
                    continue;
                }
                self.bytes = Some(
                    ByteAssembler::new(
                        welcome.stream.epoch,
                        live::byte_limits(welcome.frame_limit().unwrap()).unwrap(),
                    )
                    .unwrap(),
                );
                self.groups =
                    Some(GroupAssembler::new(welcome.stream.epoch, live::group_limits()).unwrap());
                self.scopes =
                    Some(ClientScopes::new(welcome.stream.epoch, ScopeLimits::default()).unwrap());
                let content = welcome.content;
                self.welcome = Some(welcome);
                self.active = false;
                self.sequence = 0;
                self.commands = 0;
                self.menu_sequence = 0;
                self.held = DreamInput::default();
                self.edge = None;
                self.history.clear();
                self.known.clear();
                self.baselines.clear();
                self.owner_proofs.clear();
                self.collision_cache = None;
                self.view = None;
                self.decoded_groups.clear();
                self.active_proof = None;
                self.control(live::ClientControl::Ready { content });
                continue;
            }
            let Some(welcome) = &self.welcome else {
                continue;
            };
            let control: live::ServerControl =
                live::decode_control(&bytes, welcome.stream.epoch, welcome.frame_limit().unwrap())
                    .unwrap();
            match control {
                live::ServerControl::BaselineRetired { fences } => {
                    for fence in fences.as_slice() {
                        if let Some((context, cache)) =
                            self.baselines.get_mut(&fence.context.scope.entity)
                        {
                            if *context == fence.context {
                                let _ = cache.acknowledge_retirement(*fence);
                            }
                        }
                    }
                }
                live::ServerControl::BaselineReset { resets } => {
                    for reset in resets.as_slice() {
                        if let Some((context, cache)) =
                            self.baselines.get_mut(&reset.context.scope.entity)
                        {
                            if *context == reset.context {
                                cache.apply_reset(*reset).unwrap();
                            }
                        }
                    }
                }
                live::ServerControl::Finalized {
                    through, receipts, ..
                } => {
                    self.finalized = through;
                    finalized_ack = Some(through);
                    for receipt in receipts.as_slice() {
                        if let Some(sequence) = receipt.sequence {
                            self.finalized_sequences.insert(sequence);
                        }
                    }
                }
                live::ServerControl::Active { owner_snapshot, .. } => {
                    assert!(self.decoded_groups.contains(&owner_snapshot));
                    self.active = true;
                    self.active_proof = Some(owner_snapshot);
                }
                live::ServerControl::Exit(exit) => {
                    changed = true;
                    self.known.remove(&exit.scope.entity);
                    self.scopes.as_mut().unwrap().apply_exit(exit).unwrap();
                    self.control(live::ClientControl::Exited(exit));
                }
                live::ServerControl::Destroy(destroy) => {
                    changed = true;
                    self.known.remove(&destroy.entity);
                    self.scopes
                        .as_mut()
                        .unwrap()
                        .apply_destroy(destroy)
                        .unwrap();
                    self.control(live::ClientControl::Destroyed(destroy));
                }
                live::ServerControl::Notice { utf8 } => self
                    .notices
                    .push(String::from_utf8(utf8.into_vec()).unwrap()),
                live::ServerControl::ActionOutcomes { batch, records } => {
                    outcomes_ack = Some(batch);
                    for value in records.into_vec() {
                        if let Some(previous) = self.outcomes.insert(value.key, value.clone()) {
                            assert_eq!(previous, value);
                        }
                    }
                }
                live::ServerControl::ClockReply { .. }
                | live::ServerControl::OutcomeUnavailable { .. } => {}
            }
        }
        if let Some(through) = finalized_ack {
            self.control(live::ClientControl::FinalizedAck { through });
        }
        if let Some(through) = outcomes_ack {
            self.control(live::ClientControl::OutcomesAck { through });
        }
        while let Some(bytes) = self.renet.receive_message(wire::STATE_CHANNEL) {
            self.max_packet = self.max_packet.max(bytes.len());
            if self.drop_states {
                continue;
            }
            let Some(welcome) = self.welcome.clone() else {
                continue;
            };
            let expected = self.expected();
            let mut receipts = Vec::new();
            let state = match live::decode_state(
                &bytes,
                welcome.stream.epoch,
                welcome.frame_limit().unwrap(),
            ) {
                Ok(state) => state,
                Err(engine_net::codec::CodecError::WrongEpoch) => continue,
                Err(error) => panic!("state decode: {error:?}"),
            };
            match state {
                live::StateFrame::Full(state) => {
                    assert!(matches!(
                        decode_replica(&state.payload, None).unwrap(),
                        ReplicaPayload::Actor(_)
                    ));
                    let entity = state.scope.entity;
                    match self
                        .scopes
                        .as_mut()
                        .unwrap()
                        .apply_full(state, |payload| decode_replica(payload, None).is_ok())
                    {
                        Ok(receipt) => {
                            self.known.insert(entity);
                            receipts.push(receipt);
                            changed = true;
                        }
                        Err(ReplicationError::Obsolete(_)) => continue,
                        Err(error) => panic!("public Full: {error:?}"),
                    }
                }
                live::StateFrame::Fragment(fragment) => {
                    let complete = match self
                        .bytes
                        .as_mut()
                        .unwrap()
                        .receive(fragment, nanos(now) / 1_000_000)
                    {
                        Ok(value) => value,
                        Err(ReplicationError::Obsolete(_)) => continue,
                        Err(error) => panic!("fragment: {error:?}"),
                    };
                    if let Some(complete) = complete {
                        let publication = complete.publication();
                        let mut staged_baselines = self.baselines.clone();
                        let mut baseline_proofs = Vec::new();
                        let mut retirements = Vec::new();
                        let progress = complete.decode_and_publish(
                            self.groups.as_mut().unwrap(),
                            self.scopes.as_mut().unwrap(),
                            nanos(now) / 1_000_000,
                            |bytes| {
                                let mut group =
                                    decode_group(bytes, live::group_limits(), |payload| {
                                        Ok(payload.to_vec())
                                    })?;
                                for state in &mut group.members {
                                    use engine_net::replication::baselines::*;
                                    let packet = live::baselines::decode_member(
                                        &state.payload,
                                        state,
                                        publication,
                                    )
                                    .map_err(|_| ReplicationError::InvalidPayload)?;
                                    let context = packet.target.context;
                                    if staged_baselines
                                        .get(&state.scope.entity)
                                        .is_none_or(|(old, _)| *old != context)
                                    {
                                        staged_baselines.insert(
                                            state.scope.entity,
                                            (
                                                context,
                                                ClientBaselines::new(
                                                    context,
                                                    packet.target.generation,
                                                    live::baselines::limits(),
                                                )?,
                                            ),
                                        );
                                    }
                                    let (_, cache) =
                                        staged_baselines.get_mut(&state.scope.entity).unwrap();
                                    let previous =
                                        cache.current().map(|state| state.receipt.snapshot);
                                    let receipt = cache.receive(&packet, &BytePatch, |bytes| {
                                        decode_replica(bytes, Some(expected)).is_ok()
                                    })?;
                                    state.payload = cache.state(receipt)?.payload.clone();
                                    baseline_proofs.push(live::baselines::proof(state, receipt));
                                    if let Some(previous) =
                                        previous.filter(|previous| *previous < receipt.snapshot)
                                    {
                                        if let Ok(request) = cache.propose_retirement(previous) {
                                            retirements.push(request);
                                        }
                                    }
                                }
                                Ok(group)
                            },
                            |payload| decode_replica(payload, Some(expected)).is_ok(),
                        );
                        let progress = match progress {
                            Ok(value) => value,
                            Err(
                                ReplicationError::Obsolete(_)
                                | ReplicationError::BaselineResetPending,
                            ) => continue,
                            Err(error) => panic!(
                                "group decode: {error:?}; incoming={publication:?}; current={:?}",
                                self.groups.as_ref().unwrap().publication(publication.group)
                            ),
                        };
                        match progress {
                            GroupProgress::Published(values) => {
                                changed = true;
                                assert!(live::baselines::valid_member_count(values.len()));
                                self.baselines = staged_baselines;
                                self.owner_proofs = baseline_proofs.clone();
                                if self.send_proofs {
                                    self.control(live::ClientControl::OwnerDecoded {
                                        proofs: BoundedVec::new(baseline_proofs).unwrap(),
                                    });
                                    if !retirements.is_empty() {
                                        self.control(live::ClientControl::BaselineRetire {
                                            requests: BoundedVec::new(retirements).unwrap(),
                                        });
                                    }
                                }
                                self.decoded_groups.insert(publication.snapshot);
                            }
                            GroupProgress::Staged => panic!("canonical whole group remains staged"),
                        }
                    }
                }
            }
            if self.send_proofs && !receipts.is_empty() {
                self.control(live::ClientControl::Decoded {
                    receipts: BoundedVec::new(receipts).unwrap(),
                });
            }
        }
        if changed {
            self.refresh();
        }
    }
    fn refresh(&mut self) {
        let Some(welcome) = &self.welcome else {
            return;
        };
        let expected = self.expected();
        let scopes = self.scopes.as_ref().unwrap();
        let (Some(global), Some(owner), Some(collision)) = (
            scopes.state(welcome.global_entity),
            scopes.state(welcome.owner_entity),
            scopes.state(welcome.collision_entity),
        ) else {
            return;
        };
        assert_eq!(global.end_tick, owner.end_tick);
        assert_eq!(owner.end_tick, collision.end_tick);
        if self
            .collision_cache
            .as_ref()
            .is_none_or(|(bytes, _)| *bytes != collision.payload)
        {
            let ReplicaPayload::Collision(manifest) =
                decode_replica(&collision.payload, None).unwrap()
            else {
                panic!("missing collision dependency")
            };
            self.collision_cache = Some((collision.payload.clone(), manifest.identity()));
        }
        let collision_identity = self.collision_cache.as_ref().unwrap().1;
        assert_eq!(collision_identity, welcome.content.scene);
        let ReplicaPayload::Global(global) = decode_replica(&global.payload, None).unwrap() else {
            panic!()
        };
        let ReplicaPayload::Owner(owner) = decode_replica(&owner.payload, Some(expected)).unwrap()
        else {
            panic!()
        };
        assert_eq!(owner.collision_identity(), collision_identity);
        let owner = OwnerPredictionState::from_checkpoint(&owner, expected).unwrap();
        let actors: Vec<_> = self
            .known
            .iter()
            .filter_map(|id| scopes.state(*id))
            .filter_map(
                |state| match decode_replica(&state.payload, None).unwrap() {
                    ReplicaPayload::Actor(value) => Some(value),
                    _ => None,
                },
            )
            .collect();
        self.view = Some(DreamPresentation::from_replicas(&global, &owner, &actors).unwrap());
    }
    fn send_input(&mut self, target: TargetTick) {
        if !self.active {
            return;
        }
        let welcome = self.welcome.as_ref().unwrap();
        self.commands += 1;
        let actions = self
            .edge
            .take()
            .map(|action| engine_net::commands::ActionEdge { slot: 0, action })
            .into_iter()
            .collect();
        self.history.push_back(engine_net::commands::Command {
            owner: welcome.stream,
            sequence: CommandSeq(self.commands),
            target,
            input: self.held.into(),
            actions: BoundedVec::new(actions).unwrap(),
        });
        while self.history.len() > 3 {
            self.history.pop_front();
        }
        let bundle = live::CommandBundle {
            records: BoundedVec::new(self.history.iter().cloned().collect()).unwrap(),
        };
        self.sequence += 1;
        let bytes = live::encode_commands(
            &bundle,
            welcome.stream.epoch,
            self.sequence,
            welcome.frame_limit().unwrap(),
        )
        .unwrap();
        self.renet.send_message(wire::INPUT_CHANNEL, bytes);
    }
}
struct Harness {
    driver: ServerDriver<DreamAuthority>,
    bootstrap: Arc<TestBootstrap>,
    clients: Vec<TestClient>,
    now: Duration,
    frame: u64,
}
impl Harness {
    fn new() -> Self {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Warn)
            .try_init();
        let private_key = renet_cross::generate_random_bytes();
        let profile = PacketProfile::ipv6_1200();
        let transport = MixedTransportBuilder::new(wire::PROTOCOL_ID)
            .udp_bind("127.0.0.1:0".parse().unwrap())
            .webrtc_bind("127.0.0.1:0".parse().unwrap())
            .authentication(ServerAuthentication::Secure { private_key })
            .build()
            .unwrap();
        let address = transport.udp().addresses()[0];
        let bootstrap = Arc::new(BootstrapService::new(
            BootstrapConfig {
                session_ttl: Duration::from_secs(3600),
                public_udp_addr: address,
                public_webrtc_addr: address,
                public_http_base: "http://127.0.0.1".into(),
            },
            MonotonicClientIdAllocator::new(41),
            SecureSessionAuthPolicy::new(crate::admission::GuestAdmission::default(), private_key)
                .with_packet_profile(profile)
                .unwrap(),
        ));
        let shared = SharedNet {
            health: Default::default(),
            max_clients: 8,
            packet_config: profile.packet_config().unwrap(),
            transport: Arc::new(Mutex::new(transport)),
            bootstrap: bootstrap.clone(),
        };
        let mut authority = DreamAuthority::new(29, false, 8);
        authority.observe_transport(shared.clone());
        let driver = ServerDriver::new(
            shared,
            super::connection_config(),
            dreamwake_sim::TICK_HZ,
            wire::TICKS_PER_SNAPSHOT,
            authority,
        );
        Self {
            driver,
            bootstrap,
            clients: Vec::new(),
            now: Duration::ZERO,
            frame: 0,
        }
    }
    fn join(&mut self) -> usize {
        self.join_player(41 + self.clients.len() as u64)
    }
    fn join_player(&mut self, player: u64) -> usize {
        let request = SessionCreateRequest {
            protocol_id: Some(wire::PROTOCOL_ID),
            service: wire::SESSION_SERVICE.into(),
            match_id: wire::SESSION_MATCH.into(),
            require_secure: true,
            packet_profile: PacketProfile::ipv6_1200(),
            credential: format!("dw-player-v1:{player}:{}", "42".repeat(32)),
            ..Default::default()
        };
        let session = self
            .bootstrap
            .create_session_with_request(&request)
            .unwrap();
        let index = self.clients.len();
        let (renet, transport, id) = renet_cross::connect_from_session(
            session,
            wire::PROTOCOL_ID,
            NativeConnectOptions {
                session_request: request,
                require_secure: true,
                connection_config: wire::connection_config(),
                packet_config: PacketProfile::ipv6_1200().packet_config().unwrap(),
                ..Default::default()
            },
        )
        .unwrap();
        self.clients.push(TestClient {
            baselines: BTreeMap::new(),
            owner_proofs: Vec::new(),
            collision_cache: None,
            id,
            renet,
            transport,
            welcome: None,
            bytes: None,
            groups: None,
            scopes: None,
            known: BTreeSet::new(),
            active: false,
            sequence: 0,
            commands: 0,
            menu_sequence: 0,
            held: DreamInput::default(),
            edge: None,
            history: VecDeque::new(),
            view: None,
            finalized: ServerTick(0),
            finalized_sequences: BTreeSet::new(),
            active_proof: None,
            decoded_groups: BTreeSet::new(),
            notices: Vec::new(),
            outcomes: Default::default(),
            max_packet: 0,
            drop_states: false,
            send_proofs: true,
            receive_application: true,
        });
        index
    }
    fn pump(&mut self, steps: usize) {
        for _ in 0..steps {
            self.frame += 1;
            let now = TickRate::new(dreamwake_sim::TICK_HZ)
                .unwrap()
                .deadline(ServerTick(self.frame))
                .unwrap();
            let delta = now.saturating_sub(self.now);
            self.now = now;
            for client in &mut self.clients {
                client.send_input(TargetTick(self.driver.authority.tick.0 + 2));
                client.renet.update(delta);
                if client.transport.update(delta, &mut client.renet).is_ok() {
                    client.receive(now);
                    let _ = client.transport.send_packets(&mut client.renet);
                }
            }
            self.driver.poll(now).unwrap();
        }
    }
    fn action(&mut self, index: usize, _sequence: u32, action: DreamAction) {
        let client = &mut self.clients[index];
        if matches!(action, DreamAction::Cast { .. } | DreamAction::Dash { .. }) {
            client.edge = Some(action);
        } else {
            client.menu_sequence += 1;
            client.control(live::ClientControl::Menu {
                sequence: CommandSeq(client.menu_sequence),
                action,
            });
        }
    }
    fn input(&mut self, index: usize, movement: [f32; 2]) {
        self.clients[index].held = DreamInput {
            movement,
            aim: movement,
            ..Default::default()
        };
    }
    fn latest(&self, index: usize) -> DreamPresentation {
        self.clients[index]
            .view
            .clone()
            .expect("decoded positive presentation")
    }
}

#[test]
fn stalled_udp_input_backlog_is_bounded_and_drains_without_disconnect() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    assert!(h.clients[0].active);
    let client = &mut h.clients[0];
    let welcome = client.welcome.as_ref().unwrap();
    let bundle = live::CommandBundle {
        records: BoundedVec::new(client.history.iter().cloned().collect()).unwrap(),
    };
    assert_eq!(bundle.records.as_slice().len(), 3);
    // Thirty legal redundant bundles arrive after a stalled receive loop: 90
    // records exceed a single tick's admission budget of 64.
    for _ in 0..30 {
        client.sequence += 1;
        let bytes = live::encode_commands(
            &bundle,
            welcome.stream.epoch,
            client.sequence,
            welcome.frame_limit().unwrap(),
        )
        .unwrap();
        client.renet.send_message(wire::INPUT_CHANNEL, bytes);
    }
    client.transport.send_packets(&mut client.renet).unwrap();
    for _ in 0..10 {
        h.driver.poll(h.now).unwrap();
    }
    assert_eq!(h.driver.authority.peers[&41].input_messages, 8);
    for _ in 0..12 {
        h.pump(1);
        assert!(h.driver.authority.peers[&41].input_messages <= 8);
    }
    assert!(h.driver.authority.peers[&41].active);
    assert!(h.clients[0].renet.disconnect_reason().is_none());
    assert!(h.driver.authority.peers[&41].input_messages < 8);
}

#[test]
fn legal_control_burst_drains_across_ticks_without_disconnect() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    for stamp in 0..70 {
        h.clients[0].control(live::ClientControl::ClockProbe {
            client_send_nanos: stamp,
        });
    }
    let client = &mut h.clients[0];
    client.transport.send_packets(&mut client.renet).unwrap();
    for _ in 0..10 {
        h.driver.poll(h.now).unwrap();
    }
    assert_eq!(
        h.driver.authority.peers[&41].messages,
        MAX_MESSAGES_PER_TICK
    );
    for _ in 0..6 {
        h.pump(1);
        assert!(h.driver.authority.peers[&41].messages <= MAX_MESSAGES_PER_TICK);
    }
    assert!(h.clients[0].renet.disconnect_reason().is_none());
    assert!(h.driver.authority.peers[&41].active);
}

#[test]
fn production_udp_uses_two_stage_activation_and_distinct_command_finalization() {
    let mut h = Harness::new();
    h.join();
    h.join();
    h.pump(36);
    assert!(
        h.clients.iter().all(|client| client.active),
        "peers={} clients={:?}",
        h.driver.authority.peers.len(),
        h.clients
            .iter()
            .map(|c| (
                c.id,
                c.welcome.is_some(),
                c.decoded_groups.len(),
                c.renet.disconnect_reason()
            ))
            .collect::<Vec<_>>()
    );
    assert!(h.driver.authority.peers.values().all(|peer| peer.active));
    assert!(
        h.clients
            .iter()
            .all(|client| client.decoded_groups.len() >= 2)
    );
    assert_eq!(h.latest(0).hero.id, 41);
    assert_eq!(h.latest(1).hero.id, 42);
    h.action(0, 1, DreamAction::Start { lucid: false });
    h.pump(12);
    let first = h.latest(0).hero.position;
    let second = h.latest(1).hero.position;
    h.input(0, [1.0, 0.0]);
    h.input(1, [-1.0, 0.0]);
    h.pump(12);
    assert!(h.latest(0).hero.position[0] > first[0]);
    assert!(h.latest(1).hero.position[0] < second[0]);
    assert!(
        h.clients
            .iter()
            .all(|client| !client.finalized_sequences.is_empty())
    );
    assert!(
        h.clients
            .iter()
            .all(|client| client.max_packet <= usize::from(APPLICATION_FRAME_BYTES))
    );
}
#[test]
fn initial_expiry_retires_stream_and_recovers_within_original_deadline() {
    let mut h = Harness::new();
    h.join();
    h.clients[0].send_proofs = false;
    h.pump(45);
    let original = h.driver.authority.peers[&41].welcome.stream;
    let deadline = h.driver.authority.peers[&41].initial_deadline;
    h.pump(90);
    let peer = &h.driver.authority.peers[&41];
    assert_eq!(peer.resyncs, 1);
    assert_ne!(peer.welcome.stream.epoch, original.epoch);
    assert_eq!(peer.initial_deadline, deadline);
    assert!(!peer.active && !peer.activation_committed);
    assert!(!h.driver.authority.sim.player_is_active(41));
    h.clients[0].send_proofs = true;
    h.pump(36);
    assert!(h.driver.authority.peers[&41].active);
    assert!(h.driver.authority.sim.player_is_active(41));
    assert!(h.now < deadline);
}

#[test]
fn initial_audit_expiry_recovers_after_six_second_application_startup_stall() {
    let mut h = Harness::new();
    h.join();
    // Keep transport receipt ACKs flowing while rendering blocks the application
    // from consuming Welcome, sending Ready, and acknowledging Finalized.
    h.clients[0].receive_application = false;
    h.pump(12);
    let original = h.driver.authority.peers[&41].welcome.stream;
    let deadline = h.driver.authority.peers[&41].initial_deadline;
    h.pump(348);
    let peer = &h.driver.authority.peers[&41];
    assert!(!peer.ready && !peer.active && !peer.activation_committed);
    assert_eq!(peer.resyncs, 1);
    assert_ne!(peer.welcome.stream.epoch, original.epoch);
    assert_eq!(peer.initial_deadline, deadline);
    assert_eq!(peer.resync_attempts.len(), 1);
    h.clients[0].receive_application = true;
    h.pump(60);
    assert!(h.driver.authority.peers[&41].active);
    assert!(h.driver.authority.sim.player_is_active(41));
    assert!(h.now < deadline);
}

#[test]
fn initial_audit_recovery_cannot_extend_admission_deadline() {
    let mut h = Harness::new();
    h.join();
    h.clients[0].receive_application = false;
    h.pump(12);
    let deadline = h.driver.authority.peers[&41].initial_deadline;
    h.pump(348);
    assert_eq!(h.driver.authority.peers[&41].initial_deadline, deadline);
    h.pump(300);
    assert!(!h.driver.authority.peers.contains_key(&41));
    assert!(!h.driver.authority.sim.player_is_active(41));
}

#[test]
fn committed_owner_audit_expiry_does_not_use_initial_recovery() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    let peer = h.driver.authority.peers.get_mut(&41).unwrap();
    assert!(peer.active && peer.activation_committed);
    let original = peer.welcome.stream;
    // Isolate the control-audit stall from independent state-transfer deadlines.
    peer.ready = false;
    peer.frozen = None;
    h.clients[0].active = false;
    h.clients[0].receive_application = false;
    h.pump(360);
    let peer = &h.driver.authority.peers[&41];
    assert_eq!(peer.welcome.stream, original);
    assert_eq!(peer.resyncs, 0);
    let through = peer.finalized_sent_through;
    h.driver
        .authority
        .receive_control(
            41,
            live::ClientControl::FinalizedAck { through },
            &mut h.driver.server,
        )
        .unwrap();
    super::publish::states(&mut h.driver.server, &mut h.driver.authority);
    assert!(!h.driver.authority.peers.contains_key(&41));
}

#[test]
fn initial_recovery_keeps_absolute_deadline() {
    let mut h = Harness::new();
    h.join();
    h.clients[0].send_proofs = false;
    h.pump(45);
    let peer = h.driver.authority.peers.get_mut(&41).unwrap();
    peer.initial_deadline = h.now + Duration::from_secs(2);
    let deadline = peer.initial_deadline;
    h.pump(90);
    assert_eq!(h.driver.authority.peers[&41].resyncs, 1);
    assert_eq!(h.driver.authority.peers[&41].initial_deadline, deadline);
    h.pump(40);
    assert!(h.driver.authority.peers.is_empty());
}

#[test]
fn recovery_limit_is_a_bounded_sliding_window_not_a_lifetime_disconnect_counter() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    let now = h.driver.authority.now;
    let player = h.driver.authority.peers[&41].welcome.player;
    for expected in 1..=3 {
        h.driver.authority.resync(41).unwrap();
        assert_eq!(h.driver.authority.peers[&41].resyncs, expected);
        assert_eq!(
            h.driver.authority.peers[&41].resync_attempts.len(),
            expected as usize
        );
    }
    let retained_stream = h.driver.authority.peers[&41].welcome.stream;
    assert!(
        h.driver
            .authority
            .resync(41)
            .unwrap_err()
            .contains("Repeated resync limit")
    );
    assert_eq!(
        h.driver.authority.peers[&41].welcome.stream,
        retained_stream
    );
    assert!(h.driver.authority.sim.player_is_active(player.get()));
    h.driver.authority.now = now + Duration::from_secs(30);
    h.driver.authority.resync(41).unwrap();
    let peer = &h.driver.authority.peers[&41];
    assert_eq!(peer.resyncs, 4);
    assert_eq!(peer.resync_attempts.len(), 1);
    assert_ne!(peer.welcome.stream, retained_stream);
    assert!(h.driver.authority.sim.player_is_active(player.get()));
}

#[test]
fn withheld_decode_proof_never_activates_or_accepts_commands() {
    let mut h = Harness::new();
    h.join();
    h.clients[0].send_proofs = false;
    h.pump(45);
    assert!(!h.clients[0].active);
    assert!(!h.driver.authority.peers[&41].active);
    assert!(!h.driver.authority.sim.player_is_active(41));
    assert!(
        h.driver.authority.peers[&41]
            .inbox
            .highest_received_sequence()
            .is_none()
    );
    let original = h.driver.authority.peers[&41]
        .frozen
        .as_ref()
        .unwrap()
        .members[0]
        .receipt();
    h.pump(30);
    assert_eq!(
        h.driver.authority.peers[&41]
            .frozen
            .as_ref()
            .unwrap()
            .members[0]
            .receipt(),
        original
    );
    h.pump(360);
    assert_eq!(h.driver.authority.peers[&41].resyncs, 3);
    let deadline = h.driver.authority.peers[&41].initial_deadline;
    h.pump(90);
    assert!(h.now < deadline);
    assert!(h.driver.authority.peers.is_empty());
}
#[test]
fn transient_state_loss_recovers_without_unbounded_transfer_restart() {
    let mut h = Harness::new();
    h.join();
    h.join();
    h.pump(36);
    h.action(0, 1, DreamAction::Start { lucid: false });
    h.pump(12);
    let before = h.latest(1).stamp.server_tick;
    h.clients[1].drop_states = true;
    h.pump(45);
    assert_eq!(h.latest(1).stamp.server_tick, before);
    assert!(h.clients[1].finalized.0 > before);
    assert!(h.driver.authority.peers.contains_key(&42));
    h.clients[1].drop_states = false;
    h.pump(24);
    assert!(h.latest(1).stamp.server_tick > before);
    assert_eq!(h.latest(1).phase, h.latest(0).phase);
    h.clients[1].renet.disconnect();
    h.pump(12);
    assert!(!h.driver.authority.peers.contains_key(&42));
    let rejoined = h.join();
    h.pump(36);
    assert!(h.clients[rejoined].active);
    assert_eq!(h.latest(rejoined).hero.id, 43);
}
#[derive(Default)]
struct Pilot {
    action_sequence: u32,
    input_sequence: u32,
    next_menu: u64,
    last_cast: [u64; 4],
    last_dash: u64,
}
fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn normalized(v: [f32; 2]) -> [f32; 2] {
    let length = v[0].hypot(v[1]).max(0.001);
    [v[0] / length, v[1] / length]
}

impl Pilot {
    fn act(&mut self, h: &mut Harness, index: usize, action: DreamAction) {
        self.action_sequence += 1;
        h.action(index, self.action_sequence, action);
    }
    /// Decisions read only the last received public snapshot. All changes go
    /// back over the real reliable/unreliable client channels.
    fn drive(&mut self, h: &mut Harness, index: usize, snapshot: &DreamPresentation) {
        if snapshot.phase != RunPhase::Combat {
            if h.frame < self.next_menu {
                return;
            }
            self.next_menu = h.frame + 12;
            match snapshot.phase {
                RunPhase::Intro | RunPhase::Transition if !snapshot.ready => {
                    self.act(
                        h,
                        index,
                        if snapshot.phase == RunPhase::Intro
                            && h.driver.authority.peers.keys().next().copied()
                                == Some(snapshot.hero.id)
                        {
                            DreamAction::Start { lucid: false }
                        } else {
                            DreamAction::Continue
                        },
                    );
                }
                RunPhase::Reward | RunPhase::Rest if !snapshot.rewards.is_empty() => {
                    let choice = snapshot
                        .rewards
                        .iter()
                        .position(|r| {
                            matches!(
                                r.kind,
                                RewardKind::Upgrade(
                                    UpgradeKind::Ability
                                        | UpgradeKind::Health
                                        | UpgradeKind::Attack
                                        | UpgradeKind::Recovery
                                )
                            )
                        })
                        .unwrap_or(snapshot.rewards.len() - 1);
                    self.act(
                        h,
                        index,
                        DreamAction::Choose {
                            choice: choice as u8,
                            slot: 0,
                        },
                    );
                }
                RunPhase::Rest if !snapshot.ready => self.act(h, index, DreamAction::Continue),
                _ => {}
            }
            return;
        }
        if snapshot.hero.hp <= 0.0 {
            return;
        }
        let Some(target) = snapshot.enemies.iter().min_by(|a, b| {
            distance(a.position, snapshot.hero.position)
                .total_cmp(&distance(b.position, snapshot.hero.position))
        }) else {
            return;
        };
        let dir = normalized([
            target.position[0] - snapshot.hero.position[0],
            target.position[1] - snapshot.hero.position[1],
        ]);
        let reach = distance(target.position, snapshot.hero.position);
        let mut movement = if reach > 2.6 { dir } else { [0.0; 2] };
        let danger = snapshot
            .enemies
            .iter()
            .filter(|e| e.windup > 0.0 && e.kind != EnemyKind::Ranged)
            .find(|e| distance(e.target, snapshot.hero.position) < e.warn_radius + 1.3);
        if let Some(danger) = danger {
            let away = [
                snapshot.hero.position[0] - danger.target[0],
                snapshot.hero.position[1] - danger.target[1],
            ];
            movement = if away[0].hypot(away[1]) > 0.2 {
                normalized(away)
            } else {
                [-dir[1], dir[0]]
            };
        }
        self.input_sequence += 1;
        h.clients[index].held = DreamInput {
            movement,
            aim: dir,
            attack: true,
            ..Default::default()
        };
        if danger.is_some_and(|e| e.windup < 0.35)
            && snapshot.hero.dash_cooldown <= 0.0
            && h.frame > self.last_dash + 6
        {
            self.last_dash = h.frame;
            self.act(
                h,
                index,
                DreamAction::Dash {
                    direction: movement,
                },
            );
            return;
        }
        let cast_conditions = [reach < 4.8, true, reach < 5.0, snapshot.hero.shield < 10.0];
        for slot in [3, 1, 0, 2] {
            if cast_conditions[slot]
                && snapshot.hero.memories[slot].cooldown <= 0.0
                && h.frame > self.last_cast[slot] + 6
            {
                self.last_cast[slot] = h.frame;
                self.act(
                    h,
                    index,
                    DreamAction::Cast {
                        slot: slot as u8,
                        aim: dir,
                    },
                );
                break;
            }
        }
    }
}

#[test]
fn complete_cooperative_run_over_live_udp_reaches_every_realm_and_boss_victory() {
    let mut h = Harness::new();
    h.join();
    h.join();
    h.pump(36);
    let mut pilots = [Pilot::default(), Pilot::default()];
    let mut views = vec![h.latest(0), h.latest(1)];
    let mut realms = BTreeSet::new();
    let mut rooms = BTreeSet::new();
    let mut boss_phases = BTreeSet::new();
    for iteration in 0..90_000 {
        for index in 0..2 {
            pilots[index].drive(&mut h, index, &views[index]);
        }
        h.pump(1);
        for (index, view) in views.iter_mut().enumerate() {
            *view = h.latest(index);
        }
        let view = &views[0];
        if iteration % 600 == 0 {
            eprintln!(
                "Cooperative UDP tick={} room={} phase={:?} elapsed={} heroes={:?} enemies={:?}",
                h.driver.authority.tick.0,
                view.room,
                view.phase,
                view.elapsed,
                view.heroes
                    .iter()
                    .map(|hero| (hero.id, hero.position, hero.hp))
                    .collect::<Vec<_>>(),
                view.enemies
                    .iter()
                    .map(|enemy| (enemy.id, enemy.position, enemy.hp))
                    .take(3)
                    .collect::<Vec<_>>()
            );
        }
        realms.insert(view.realm);
        rooms.insert(view.room);
        for boss in views
            .iter()
            .flat_map(|view| view.enemies.iter())
            .filter(|e| e.kind == EnemyKind::Boss)
        {
            boss_phases.insert(boss.phase);
        }
        if matches!(view.phase, RunPhase::Victory | RunPhase::Defeat) {
            break;
        }
        assert_eq!(
            h.driver.authority.peers.len(),
            2,
            "both live peers retained; room={} phase={:?} tick={}",
            view.room,
            view.phase,
            h.driver.authority.tick.0
        );
    }
    let end = &views[0];
    assert_eq!(
        end.phase,
        RunPhase::Victory,
        "room={} elapsed={} hp={:?}",
        end.room,
        end.elapsed,
        end.heroes.iter().map(|v| v.hp).collect::<Vec<_>>()
    );
    assert_eq!(realms, BTreeSet::from([0, 1, 2]));
    assert_eq!(rooms.len(), 10);
    assert_eq!(boss_phases, BTreeSet::from([1, 2, 3]));
    assert!(
        h.clients
            .iter()
            .all(|client| client.max_packet <= usize::from(APPLICATION_FRAME_BYTES))
    );
    h.pump(12);
    assert_eq!(h.latest(0).phase, RunPhase::Victory);
    assert_eq!(h.latest(1).phase, RunPhase::Victory);
    assert_eq!(
        h.latest(0).heroes.iter().map(|v| v.id).collect::<Vec<_>>(),
        h.latest(1).heroes.iter().map(|v| v.id).collect::<Vec<_>>()
    );
}

#[test]
fn party_menu_rules_and_tick_action_identity_survive_live_protocol() {
    let mut h = Harness::new();
    h.join();
    h.join();
    h.pump(36);
    h.action(1, 1, DreamAction::Start { lucid: false });
    h.pump(12);
    h.action(1, 2, DreamAction::Restart);
    h.action(1, 3, DreamAction::Pause { paused: true });
    h.action(1, 4, DreamAction::Swap { a: 0, b: 1 });
    h.pump(12);
    assert_eq!(h.driver.authority.epoch, 1);
    assert!(!h.driver.authority.sim.is_paused());
    assert!(
        h.clients[1]
            .notices
            .iter()
            .any(|value| value.contains("Finish the shared run"))
    );
    assert!(
        h.clients[1]
            .notices
            .iter()
            .any(|value| value.contains("shared dream"))
    );
    let sequence = h.clients[0].commands + 1;
    h.action(
        0,
        77,
        DreamAction::Cast {
            slot: 2,
            aim: [0.0, -1.0],
        },
    );
    h.pump(3);
    assert!(
        h.driver
            .authority
            .sim
            .snapshot_for(41)
            .presentations
            .iter()
            .any(|effect| effect.id.owner == 41 && u64::from(effect.id.action_seq) == sequence)
    );
    h.pump(6);
    assert!(
        h.clients[0]
            .finalized_sequences
            .contains(&CommandSeq(sequence))
    );
}
#[test]
fn invalid_owner_stream_and_action_sequence_exhaustion_cannot_enter_authority_inbox() {
    let mut h = Harness::new();
    h.join();
    h.join();
    h.pump(36);
    let make = |owner, sequence| live::CommandBundle {
        records: BoundedVec::new(vec![engine_net::commands::Command {
            owner,
            sequence: CommandSeq(sequence),
            target: TargetTick(h.driver.authority.tick.0 + 2),
            input: DreamInput::default().into(),
            actions: BoundedVec::default(),
        }])
        .unwrap(),
    };
    let wrong = make(h.clients[1].welcome.as_ref().unwrap().stream, 100);
    let exhausted = make(
        h.clients[0].welcome.as_ref().unwrap().stream,
        u64::from(u32::MAX) + 1,
    );
    let before = h.driver.authority.peers[&41].inbox.pending_len();
    assert!(h.driver.authority.receive_bundle(41, wrong).is_err());
    assert!(h.driver.authority.receive_bundle(41, exhausted).is_err());
    assert_eq!(h.driver.authority.peers[&41].inbox.pending_len(), before);
}
#[test]
fn snapshots_follow_completed_ticks_despite_late_wakeups() {
    let mut clock = TickClock::new(dreamwake_sim::TICK_HZ, wire::TICKS_PER_SNAPSHOT);
    let mut authority = DreamAuthority::new(8192, false, 8);
    assert!(authority.admit(41));
    // Isolate scheduling with an already active offline actor; live readiness is
    // exercised separately through the actual UDP driver above.
    authority.pending_admissions.clear();
    authority.sim.add_player(41);
    authority.begin_run();
    let mut publications = Vec::new();
    for millis in (0..=1000).step_by(10) {
        let now = Duration::from_millis(millis);
        if clock.advance(now, || authority.step(now)).snapshot_due {
            publications.push(authority.sim.gameplay_tick());
        }
        assert!(
            !clock
                .advance(now, || panic!("no new simulation tick is due"))
                .send_due
        );
    }
    assert_eq!(
        publications,
        (1..=wire::SNAPSHOT_HZ)
            .map(|n| n * wire::TICKS_PER_SNAPSHOT)
            .collect::<Vec<_>>()
    );
}

#[test]
fn owner_baselines_bound_missing_proofs_and_resume_after_one_reset() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    assert!(h.clients[0].active);
    h.clients[0].send_proofs = false;
    h.pump(9);
    let owner = h.clients[0].welcome.as_ref().unwrap().owner_entity;
    let old = h.clients[0]
        .scopes
        .as_ref()
        .unwrap()
        .state(owner)
        .unwrap()
        .receipt();
    let initial = h.latest(0).stamp.server_tick;
    // A delayed proof for a retained sent version is allowed as historical
    // evidence, but cannot acknowledge the newer desired full state.
    h.pump(6);
    h.clients[0].control(live::ClientControl::Decoded {
        receipts: BoundedVec::new(vec![old]).unwrap(),
    });
    h.pump(3);
    let scopes = h.driver.authority.replication.scopes_mut(41).unwrap();
    assert_ne!(scopes.pending(owner).unwrap().receipt(), old);
    assert!(scopes.decoded_current(owner).is_none());
    let retired = h.clients[0]
        .scopes
        .as_ref()
        .unwrap()
        .state(owner)
        .unwrap()
        .receipt();
    assert_ne!(retired, old);
    let old_proofs = h.clients[0].owner_proofs.clone();
    let topology = h.driver.authority.peers[&41].group_revision;
    // Missing feedback fills bounded retention and negotiates exactly one reset.
    // Its full repair point remains retransmittable while finalization advances.
    h.pump(150);
    assert!(h.driver.authority.peers[&41].active);
    assert!(h.latest(0).stamp.server_tick > initial);
    assert!(h.latest(0).stamp.server_tick < initial + 120);
    let reset = h.driver.authority.peers[&41]
        .baselines
        .reset_pending
        .clone()
        .unwrap();
    assert_eq!(reset.len(), OWNER_GROUP_MEMBERS);
    let repair_tick = h.latest(0).stamp.server_tick;
    h.pump(30);
    assert_eq!(h.latest(0).stamp.server_tick, repair_tick);
    assert_eq!(
        h.driver.authority.peers[&41]
            .baselines
            .reset_pending
            .as_ref()
            .unwrap(),
        &reset
    );
    assert!(h.driver.authority.tick.0 - h.clients[0].finalized.0 <= 3);
    h.clients[0].control(live::ClientControl::OwnerDecoded {
        proofs: BoundedVec::new(old_proofs).unwrap(),
    });
    h.pump(3);
    assert_eq!(
        h.driver.authority.peers[&41]
            .baselines
            .reset_pending
            .as_ref()
            .unwrap(),
        &reset
    );
    let scopes = h.driver.authority.replication.scopes_mut(41).unwrap();
    assert!(scopes.decoded_current(owner).is_none());
    assert_eq!(h.driver.authority.peers[&41].group_revision, topology);
    h.clients[0].send_proofs = true;
    h.pump(12);
    assert!(h.clients[0].active);
    assert!(h.latest(0).stamp.server_tick > initial + 150);
}

#[test]
fn lost_owner_groups_recover_from_retained_full_reset_without_generation_churn() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    let before = h.latest(0).stamp.server_tick;
    h.clients[0].drop_states = true;
    h.pump(150);
    assert_eq!(h.latest(0).stamp.server_tick, before);
    assert!(h.driver.authority.peers[&41].active);
    let newest = h.driver.authority.peers[&41]
        .frozen
        .as_ref()
        .unwrap()
        .members[0]
        .end_tick;
    assert!(newest.0 > before);
    assert!(newest.0 < before + 120);
    assert!(
        h.driver.authority.peers[&41]
            .baselines
            .reset_pending
            .is_some()
    );
    h.clients[0].drop_states = false;
    h.pump(12);
    assert!(h.latest(0).stamp.server_tick >= newest.0);
    let w = h.clients[0].welcome.as_ref().unwrap();
    let scopes = h.clients[0].scopes.as_ref().unwrap();
    assert_eq!(
        scopes.state(w.owner_entity).unwrap().end_tick,
        scopes.state(w.global_entity).unwrap().end_tick
    );
}

#[test]
fn arrival_feedback_counts_first_effective_observations_without_duplicate_inflation() {
    let mut authority = DreamAuthority::new(71, false, 8);
    assert!(authority.admit(41));
    authority.peers.get_mut(&41).unwrap().active = true;
    let owner = authority.peers[&41].welcome.stream;
    let rules = Rules(owner);
    let command = |sequence, tick| engine_net::commands::Command {
        owner,
        sequence: CommandSeq(sequence),
        target: TargetTick(tick),
        input: DreamInput::default().into(),
        actions: BoundedVec::default(),
    };
    let sample = |sequence, target, arrival_tick, slack| live::ArrivalSample {
        sequence: CommandSeq(sequence),
        target: TargetTick(target),
        arrival_tick: ServerTick(arrival_tick),
        slack,
    };
    let receive = |authority: &mut DreamAuthority, records| {
        authority
            .receive_bundle(
                41,
                live::CommandBundle {
                    records: BoundedVec::new(records).unwrap(),
                },
            )
            .unwrap();
    };
    for tick in 1..=300 {
        let peer = authority.peers.get_mut(&41).unwrap();
        let prepared = peer.inbox.prepare_next(&rules).unwrap();
        peer.inbox.commit(prepared.tick()).unwrap();
        authority.tick = ServerTick(tick);
        receive(&mut authority, vec![command(tick, tick)]);
        assert_eq!(
            authority.peers[&41].arrival,
            live::ArrivalFeedback {
                sample: Some(sample(tick, tick, tick, 0)),
                observed_commands: tick,
                late_commands: tick,
            }
        );
        authority.peers.get_mut(&41).unwrap().arrival.sample = None;
        receive(&mut authority, vec![command(tick, tick)]);
        assert_eq!(
            authority.peers[&41].arrival,
            live::ArrivalFeedback {
                sample: None,
                observed_commands: tick,
                late_commands: tick,
            }
        );
    }
    receive(&mut authority, vec![command(301, 302), command(300, 300)]);
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: Some(sample(301, 302, 300, 2)),
            observed_commands: 301,
            late_commands: 300,
        }
    );
    assert_eq!(authority.peers[&41].inbox.pending_len(), 1);
    // A newer command with more slack does not displace the least-slack sample.
    receive(&mut authority, vec![command(304, 304)]);
    assert_eq!(
        authority.peers[&41].arrival.sample,
        Some(sample(301, 302, 300, 2))
    );
    assert_eq!(authority.peers[&41].arrival.observed_commands, 302);

    // Equal slack at a later committed tick selects the newest command identity,
    // even though a larger sequence was previously received with more margin.
    let peer = authority.peers.get_mut(&41).unwrap();
    let prepared = peer.inbox.prepare_next(&rules).unwrap();
    peer.inbox.commit(prepared.tick()).unwrap();
    authority.tick = ServerTick(301);
    receive(&mut authority, vec![command(303, 303)]);
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: Some(sample(303, 303, 301, 2)),
            observed_commands: 303,
            late_commands: 300,
        }
    );
    authority.peers.get_mut(&41).unwrap().arrival.sample = None;
    receive(&mut authority, vec![command(302, 300), command(306, 305)]);
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: Some(sample(302, 300, 301, 0)),
            observed_commands: 305,
            late_commands: 301,
        }
    );
    // The same newest-identity tie rule applies to two distinct missed deadlines.
    receive(&mut authority, vec![command(307, 299)]);
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: Some(sample(307, 299, 301, 0)),
            observed_commands: 306,
            late_commands: 302,
        }
    );
    assert!(authority.peers[&41].arrival.validate());
    authority.peers.get_mut(&41).unwrap().arrival.sample = None;
    receive(
        &mut authority,
        vec![command(302, 300), command(304, 304), command(307, 299)],
    );
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: None,
            observed_commands: 306,
            late_commands: 302,
        }
    );

    // Future is not effective feedback. Retrying identical input contributes
    // exactly once when committed progress opens the unchanged twelve-tick window.
    receive(&mut authority, vec![command(305, 314)]);
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: None,
            observed_commands: 306,
            late_commands: 302,
        }
    );
    let peer = authority.peers.get_mut(&41).unwrap();
    let prepared = peer.inbox.prepare_next(&rules).unwrap();
    peer.inbox.commit(prepared.tick()).unwrap();
    authority.tick = ServerTick(302);
    receive(&mut authority, vec![command(305, 314)]);
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: Some(sample(305, 314, 302, 12)),
            observed_commands: 307,
            late_commands: 302,
        }
    );
    authority.peers.get_mut(&41).unwrap().arrival.sample = None;
    receive(&mut authority, vec![command(305, 314)]);
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: None,
            observed_commands: 307,
            late_commands: 302,
        }
    );
}

#[test]
fn stable_player_has_one_controller_and_reclaim_changes_both_epochs() {
    let mut authority = DreamAuthority::new(19, false, 8);
    let player = PlayerId::new(9007199254740993).unwrap();
    assert!(authority.admit_player(41, player));
    authority.step(Duration::from_millis(17));
    authority.sim.set_player_active(player.get(), true);
    let first = authority.peers[&41].welcome.clone();
    assert_eq!(first.player, player);
    assert_ne!(first.client.0, player.get());
    assert!(!authority.admit_player(42, player));
    authority.disconnect(42);
    assert!(authority.sim.player_is_active(player.get()));
    assert_eq!(authority.peers[&41].welcome, first);
    authority.disconnect(41);
    assert!(!authority.sim.player_is_active(player.get()));
    let before = authority.sim.snapshot_for(player.get()).hero;
    assert!(authority.admit_player(43, player));
    authority.step(Duration::from_millis(34));
    let second = authority.peers[&43].welcome.clone();
    assert!(second.stream.epoch > first.stream.epoch);
    assert!(second.stream.ownership > first.stream.ownership);
    assert_eq!(second.player, first.player);
    assert_eq!(authority.sim.snapshot_for(player.get()).hero.hp, before.hp);
    authority.peers.get_mut(&43).unwrap().active = true;
    let old = engine_net::commands::Command {
        owner: first.stream,
        sequence: CommandSeq(1),
        target: TargetTick(authority.tick.0 + 2),
        input: DreamInput::default().into(),
        actions: BoundedVec::new(vec![]).unwrap(),
    };
    let bundle = live::CommandBundle {
        records: BoundedVec::new(vec![old]).unwrap(),
    };
    assert!(
        authority.receive_bundle(43, bundle).is_err(),
        "old owner stream cannot control reclaimed player"
    );
}

#[test]
fn authenticated_udp_rejoin_preserves_player_and_rejects_a_second_controller() {
    let mut h = Harness::new();
    let original = h.join_player(700);
    h.join_player(800);
    h.pump(36);
    assert!(h.clients[original].active);
    assert_eq!(h.latest(original).hero.id, 700);
    let first = h.clients[original].welcome.clone().unwrap();
    let second = h.join_player(700);
    h.pump(20);
    assert!(!h.clients[second].active);
    assert!(h.clients[second].renet.is_disconnected());
    assert!(h.driver.authority.peers[&41].active);
    h.action(original, 1, DreamAction::Start { lucid: false });
    h.pump(120);
    h.clients[original].renet.disconnect();
    h.pump(12);
    assert!(!h.driver.authority.sim.player_is_active(700));
    let retained = h.driver.authority.sim.snapshot_for(700).hero;
    let reconnect = h.join_player(700);
    h.pump(36);
    assert!(h.clients[reconnect].active);
    let welcome = h.clients[reconnect].welcome.as_ref().unwrap();
    assert_ne!(welcome.client, first.client);
    assert!(welcome.stream.epoch > first.stream.epoch);
    assert!(welcome.stream.ownership > first.stream.ownership);
    let resumed = h.latest(reconnect).hero;
    // Resume never recreates starter stats or copies another party member.
    assert_eq!(resumed.id, retained.id);
    assert!(resumed.hp <= retained.hp);
    assert_eq!(resumed.level, retained.level);
    assert_eq!(resumed.shards, retained.shards);
}

#[test]
fn retained_participant_capacity_is_separate_from_live_capacity_and_resets_with_run() {
    let mut authority = DreamAuthority::new(3, false, 1);
    for player in 1..=dreamwake_sim::MAX_HEROES as u64 {
        let connection = 10_000 + player;
        assert!(authority.admit_player(connection, PlayerId::new(player).unwrap()));
        assert!(authority.sim.add_pending_player(player));
        authority.disconnect(connection);
    }
    assert!(authority.peers.is_empty());
    assert_eq!(authority.ownership.len(), dreamwake_sim::MAX_HEROES);
    let next_connection = authority.next_connection;
    assert!(!authority.admit_player(30_000, PlayerId::new(20_000).unwrap()));
    assert_eq!(authority.next_connection, next_connection);
    assert!(!authority.ownership.contains_key(&20_000));
    assert!(!authority.peers.contains_key(&30_000));
    assert!(!authority.pending_admissions.contains(&30_000));
    assert!(
        authority.admit_player(30_001, PlayerId::new(1).unwrap()),
        "retained player can reclaim a full roster"
    );
    assert_eq!(
        authority.peers[&30_001].welcome.stream.ownership,
        OwnershipEpoch(2)
    );
    assert!(authority.sim.add_pending_player(1));
    authority.disconnect(30_001);
    authority.restart(false, false);
    assert!(authority.admit_player(30_002, PlayerId::new(20_000).unwrap()));
}

#[test]
fn game_connection_limit_cannot_exceed_retained_hero_capacity() {
    let authority = DreamAuthority::new(3, false, wire::MAX_PLAYERS);
    assert_eq!(authority.max_clients, dreamwake_sim::MAX_HEROES);
    let default = DreamAuthority::new(3, false, wire::DEFAULT_MAX_PLAYERS);
    assert_eq!(default.max_clients, wire::DEFAULT_MAX_PLAYERS);
}

#[test]
fn finalized_receipts_are_sent_once_on_the_reliable_lane() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    assert!(h.clients[0].active);
    let peer = &h.driver.authority.peers[&41];
    let before = (
        peer.egress.finalized_receipts,
        peer.egress.finalized_bytes,
        peer.egress.finalized_messages,
    );
    h.pump(120);
    let peer = &h.driver.authority.peers[&41];
    assert_eq!(peer.egress.finalized_receipts - before.0, 120);
    assert_eq!(peer.egress.finalized_messages - before.2, 40);
    assert_eq!(peer.finalized_sent_through, h.driver.authority.tick);
    assert!(h.driver.authority.tick.0 - h.clients[0].finalized.0 <= 3);
    let repeated: Vec<_> = peer
        .inbox
        .receipts()
        .skip(peer.inbox.receipts().len() - 32)
        .copied()
        .collect();
    let old = live::encode_control(
        &live::ServerControl::Finalized {
            through: h.driver.authority.tick,
            receipts: BoundedVec::new(repeated).unwrap(),
            menu_applied: peer.menu_applied,
            arrival: live::ArrivalFeedback::default(),
        },
        peer.welcome.stream.epoch,
        1,
        peer.limit(),
    )
    .unwrap()
    .len();
    let new_bytes = peer.egress.finalized_bytes - before.1;
    assert!(new_bytes * 4 < old as u64 * 40);
    eprintln!(
        "Finalized measured: old_32_receipt_frame_bytes={old} old_20hz_bytes_per_second={} new_120tick_bytes={new_bytes} new_20hz_bytes_per_second={} receipt_count=120 publication_count=40",
        old * 20,
        new_bytes / 2
    );
}

#[test]
fn finalized_queue_coalesces_unsent_ticks_and_recovers_after_ack_stall() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    assert!(h.clients[0].active);
    // Keep simulation/publication running while this client neither reads nor ACKs.
    for _ in 0..90 {
        h.frame += 1;
        h.now = TickRate::new(dreamwake_sim::TICK_HZ)
            .unwrap()
            .deadline(ServerTick(h.frame))
            .unwrap();
        h.driver.poll(h.now).unwrap();
    }
    let queue = h
        .driver
        .server
        .channel_send_queue(41, wire::CONTROL_CHANNEL)
        .unwrap();
    // Four coalesced Finalized messages plus one reliable baseline-reset fence.
    assert!(queue.messages <= 5, "coalesced Finalized queue: {queue:?}");
    assert!(queue.payload_bytes < 1100);
    assert!(h.driver.authority.peers[&41].egress.finalized_deferred > 0);
    // Receive-only recovery isolates server finalization from client command-clock behavior.
    h.clients[0].active = false;
    h.pump(60);
    assert!(h.driver.authority.peers.contains_key(&41));
    assert!(h.driver.authority.tick.0 - h.clients[0].finalized.0 <= 3);
    assert_eq!(
        h.driver.authority.peers[&41].finalized_sent_through,
        h.driver.authority.tick
    );
}

#[test]
fn production_channel_priority_keeps_control_before_saturating_state() {
    let mut server = renet::RenetServer::new(super::connection_config());
    server.add_connection(41);
    server.send_message(41, wire::STATE_CHANNEL, vec![1; 1000]);
    server.send_message(41, wire::CONTROL_CHANNEL, vec![2; 100]);
    let packets = server.get_packets_to_send(41).unwrap();
    let mut client = RenetClient::new(wire::connection_config());
    client.set_connected();
    client.process_packet(&packets[0]);
    assert_eq!(
        client
            .receive_message(wire::CONTROL_CHANNEL)
            .unwrap()
            .as_ref(),
        &[2; 100]
    );
    assert!(client.receive_message(wire::STATE_CHANNEL).is_none());
    let queue = server
        .channel_send_queue(41, wire::CONTROL_CHANNEL)
        .unwrap();
    assert_eq!((queue.messages, queue.payload_bytes), (1, 100));
    for packet in client.get_packets_to_send() {
        server.process_packet_from(&packet, 41).unwrap();
    }
    assert_eq!(
        server
            .channel_send_queue(41, wire::CONTROL_CHANNEL)
            .unwrap(),
        renet::SendQueueStats::default()
    );
}

#[test]
fn collision_decode_proof_is_required_before_owner_activation() {
    let mut h = Harness::new();
    h.join();
    h.clients[0].send_proofs = false;
    h.pump(18);
    let peer = &h.driver.authority.peers[&41];
    let group = peer.frozen.as_ref().unwrap();
    assert_eq!(group.members.len(), OWNER_GROUP_MEMBERS);
    let encoded_group_bytes: usize = group
        .fragments
        .iter()
        .enumerate()
        .map(|(index, fragment)| {
            live::encode_state(
                &live::StateFrame::Fragment(fragment.clone()),
                peer.welcome.stream.epoch,
                index as u32 + 1,
                peer.limit(),
            )
            .unwrap()
            .len()
        })
        .sum();
    eprintln!(
        "N043 initial owner group: members={} fragments={} application_bytes={} full_group_bytes_per_second={} payload_bytes={:?}",
        group.members.len(),
        group.fragments.len(),
        encoded_group_bytes,
        encoded_group_bytes * wire::SNAPSHOT_HZ as usize,
        group
            .members
            .iter()
            .map(|state| (state.scope.entity, state.payload.len()))
            .collect::<Vec<_>>()
    );
    assert!(
        group
            .members
            .iter()
            .all(|state| state.end_tick == group.members[0].end_tick)
    );
    let collision = peer.welcome.collision_entity;
    let without_collision: Vec<_> = group
        .members
        .iter()
        .filter(|state| state.scope.entity != collision)
        .map(|state| state.receipt())
        .collect();
    assert_eq!(without_collision.len(), 2);
    let collision_receipt = group
        .members
        .iter()
        .find(|state| state.scope.entity == collision)
        .unwrap()
        .receipt();
    h.clients[0].control(live::ClientControl::Decoded {
        receipts: BoundedVec::new(without_collision).unwrap(),
    });
    h.pump(9);
    assert!(!h.driver.authority.peers[&41].activate_pending);
    assert!(!h.driver.authority.peers[&41].active);
    assert!(!h.driver.authority.sim.player_is_active(41));
    assert_eq!(collision_receipt.scope.entity, collision);
    let proofs = h.clients[0].owner_proofs.clone();
    h.clients[0].control(live::ClientControl::OwnerDecoded {
        proofs: BoundedVec::new(proofs).unwrap(),
    });
    h.clients[0].send_proofs = true;
    h.pump(18);
    assert!(h.clients[0].active);
    assert!(h.driver.authority.sim.player_is_active(41));
    let counters = &h.driver.authority.peers[&41].egress;
    let before = (counters.state_bytes, counters.control_bytes);
    let transport = h.driver.authority.transport_diagnostics.clone().unwrap();
    let native_before = transport
        .peer_egress_stats(41)
        .unwrap()
        .native_udp_ip
        .unwrap();
    h.pump(120);
    let counters = &h.driver.authority.peers[&41].egress;
    let native_after = transport
        .peer_egress_stats(41)
        .unwrap()
        .native_udp_ip
        .unwrap();
    eprintln!(
        "N043 sustained 120 ticks: state_bytes_per_second={} control_bytes_per_second={} native_udp_ip_bytes_per_second={} pacing_drops={}",
        (counters.state_bytes - before.0) / 2,
        (counters.control_bytes - before.1) / 2,
        (native_after.native_udp_ip_bytes - native_before.native_udp_ip_bytes) / 2,
        native_after.pacing_drops - native_before.pacing_drops,
    );
    assert!(h.clients[0].active);
    assert!(h.driver.authority.tick.0 - h.clients[0].finalized.0 <= 3);
}

#[test]
fn wrong_collision_ready_identity_revokes_the_session() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    assert!(h.clients[0].active);
    let mut content = h.clients[0].welcome.as_ref().unwrap().content;
    content.scene[0] ^= 1;
    h.clients[0].control(live::ClientControl::Ready { content });
    h.pump(6);
    assert!(!h.driver.authority.peers.contains_key(&41));
}

#[test]
fn paced_owner_transfer_spans_flushes_without_partial_sent_proofs() {
    let mut h = Harness::new();
    let shared = h.driver.authority.transport_diagnostics.clone().unwrap();
    shared
        .transport
        .lock()
        .unwrap()
        .udp_mut()
        .set_egress_limit(Some(
            renet_cross::EgressConfig::new(
                renet_cross::EgressBasis::NativeUdpIp,
                60_000,
                2048,
                512,
            )
            .unwrap(),
        ))
        .unwrap();
    h.join();
    let mut staged = false;
    let mut identity = None;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(25));
        h.pump(1);
        if let Some(peer) = h.driver.authority.peers.get(&41)
            && let Some(group) = &peer.frozen
            && !peer.active
            && group.next_fragment != 0
            && !group.sent_once
        {
            staged = true;
            let receipt = group.members[0].receipt();
            if let Some((active_state, previous)) = identity {
                if active_state == group.active_state {
                    assert_eq!(receipt, previous);
                }
            }
            identity = Some((group.active_state, receipt));
            let proofs: Vec<_> = group
                .members
                .iter()
                .zip(&group.baseline_packets)
                .map(|(state, packet)| live::baselines::proof(state, packet.target))
                .collect();
            assert!(!peer.baselines.validate_proofs(&proofs).unwrap());
            assert!(!h.clients[0].active);
        }
        if h.clients[0].active {
            break;
        }
    }
    assert!(staged, "full owner group must exceed this data burst");
    assert!(
        h.clients[0].active,
        "continuous input ACKs must not starve staged bootstrap"
    );
    assert_eq!(h.driver.authority.peers[&41].resyncs, 0);
    let stats = shared.peer_egress_stats(41).unwrap().native_udp_ip.unwrap();
    assert_eq!(stats.cap_drops, 0);
    assert_eq!(stats.pacing_drops, 0);
}

#[test]
fn paced_owner_and_moving_enemies_keep_actual_remote_cadence() {
    let mut h = Harness::new();
    let shared = h.driver.authority.transport_diagnostics.clone().unwrap();
    shared
        .transport
        .lock()
        .unwrap()
        .udp_mut()
        .set_egress_limit(Some(
            renet_cross::EgressConfig::new(
                renet_cross::EgressBasis::NativeUdpIp,
                crate::DEFAULT_EGRESS_BYTES_PER_SECOND,
                crate::DEFAULT_EGRESS_BURST_BYTES,
                2400,
            )
            .unwrap(),
        ))
        .unwrap();
    h.join();
    let setup = std::time::Instant::now();
    for frame in 1..=120 {
        let due = setup + Duration::from_secs_f64(f64::from(frame) / 60.0);
        if let Some(wait) = due.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(wait);
        }
        h.pump(1);
        if frame == 60 {
            h.action(0, 1, DreamAction::Start { lucid: false });
        }
    }
    let before = shared.peer_egress_stats(41).unwrap().native_udp_ip.unwrap();
    let started = std::time::Instant::now();
    let mut latest = BTreeMap::new();
    let mut gaps = Vec::new();
    let mut moving_updates = 0;
    let mut moving_gaps = Vec::new();
    let mut near_gaps = Vec::new();
    let mut owner_age = 0;
    for frame in 0..480 {
        // The actual native limiter uses wall time; simulated pumping alone
        // would not test its wire allowance or split-prefix pressure.
        let due = started + Duration::from_secs_f64(f64::from(frame + 1) / 60.0);
        if let Some(wait) = due.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(wait);
        }
        h.pump(1);
        assert!(h.clients[0].active);
        if frame < 60 {
            continue;
        }
        let owner_position = h
            .driver
            .authority
            .sim
            .snapshot()
            .heroes
            .iter()
            .find(|hero| hero.id == 41)
            .unwrap()
            .position;
        let client = &h.clients[0];
        let scopes = client.scopes.as_ref().unwrap();
        let owner = scopes
            .state(client.welcome.as_ref().unwrap().owner_entity)
            .unwrap();
        owner_age = owner_age.max(h.driver.authority.tick.0 - owner.end_tick.0);
        for entity in &client.known {
            let Some(state) = scopes.state(*entity) else {
                continue;
            };
            let decoded = decode_replica(&state.payload, None).unwrap();
            let ReplicaPayload::Actor(dreamwake_sim::replication::PublicReplica::Enemy(enemy)) =
                decoded
            else {
                continue;
            };
            if let Some((previous, position)) =
                latest.insert(state.scope, (state.end_tick, enemy.position))
            {
                if previous != state.end_tick {
                    gaps.push(state.end_tick.0 - previous.0);
                    let gap = state.end_tick.0 - previous.0;
                    if position != enemy.position {
                        moving_gaps.push(gap);
                    }
                    let dx = enemy.position[0] - owner_position[0];
                    let dz = enemy.position[1] - owner_position[1];
                    if dx * dx + dz * dz <= 32.0 * 32.0 {
                        near_gaps.push(gap);
                    }
                    moving_updates += usize::from(position != enemy.position);
                }
            }
        }
    }
    let after = shared.peer_egress_stats(41).unwrap().native_udp_ip.unwrap();
    let elapsed = started.elapsed().as_secs_f64();
    eprintln!(
        "paced enemies updates={} moving={} maximum_gap={:?} owner_age={} native_bytes={} elapsed={elapsed}",
        gaps.len(),
        moving_updates,
        gaps.iter().max(),
        owner_age,
        after.native_udp_ip_bytes - before.native_udp_ip_bytes
    );
    assert!(
        moving_updates >= 12,
        "exercise sustained moving-enemy publications"
    );
    gaps.sort_unstable();
    eprintln!(
        "enemy gap distribution p50={} p95={} max={}",
        gaps[gaps.len() / 2],
        gaps[(gaps.len() - 1) * 95 / 100],
        gaps.last().unwrap()
    );
    for (name, values) in [("moving", &mut moving_gaps), ("near", &mut near_gaps)] {
        assert!(!values.is_empty(), "exercise {name} actor publications");
        values.sort_unstable();
        assert!(
            values[(values.len() - 1) * 95 / 100] <= 6,
            "{name} actors must normally update inside the 100 ms presentation buffer"
        );
        eprintln!(
            "{name} enemy gap p50={} p95={} max={}",
            values[values.len() / 2],
            values[(values.len() - 1) * 95 / 100],
            values.last().unwrap()
        );
    }
    assert!(
        gaps.iter().all(|gap| *gap <= 12),
        "remote updates exceeded 12 ticks: {gaps:?}"
    );
    assert!(
        owner_age <= 12,
        "owner completion must also remain fresh: {owner_age}"
    );
    assert_eq!(after.cap_drops - before.cap_drops, 0);
    assert_eq!(after.pacing_drops - before.pacing_drops, 0);
    assert!(
        (after.native_udp_ip_bytes - before.native_udp_ip_bytes) as f64
            <= crate::DEFAULT_EGRESS_BYTES_PER_SECOND as f64 * elapsed
                + crate::DEFAULT_EGRESS_BURST_BYTES as f64
    );
}

#[test]
fn reliable_action_outcome_arrives_alongside_continuing_finalization() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    h.action(0, 1, DreamAction::Start { lucid: false });
    h.pump(12);
    let before = h.clients[0].finalized;
    h.action(
        0,
        2,
        DreamAction::Dash {
            direction: [1.0, 0.0],
        },
    );
    h.pump(24);
    assert!(h.clients[0].finalized > before);
    assert!(
        h.clients[0]
            .outcomes
            .values()
            .any(|outcome| outcome.data.status == live::TerminalStatus::Accepted)
    );
    let outcome = h.clients[0].outcomes.values().next().unwrap().clone();
    h.clients[0].control(live::ClientControl::OutcomeLookup {
        keys: BoundedVec::new(vec![outcome.key]).unwrap(),
    });
    h.pump(18);
    assert_eq!(h.clients[0].outcomes.get(&outcome.key), Some(&outcome));
}

#[test]
fn lifecycle_fences_enqueue_once_during_ack_stall_and_allow_fresh_welcome() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    assert!(h.clients[0].active);
    let original = h.driver.authority.peers[&41].welcome.stream;
    let welcome = h.driver.authority.peers[&41].welcome.clone();
    let authority = &mut h.driver.authority;
    let scopes = authority.replication.scopes_mut(41).unwrap();
    let exit = scopes.exit(welcome.global_entity).unwrap();
    let destroy = scopes.destroy(welcome.owner_entity).unwrap();
    let peer = authority.peers.get_mut(&41).unwrap();
    let before = peer.egress.control_messages;
    // Thirty 20 Hz publication attempts with all client reads/ACKs withheld.
    for _ in 0..30 {
        publish::lifecycle(&mut h.driver.server, 41, peer, scopes).unwrap();
    }
    assert_eq!(peer.egress.control_messages - before, 2);
    assert_eq!(scopes.pending_exits().collect::<Vec<_>>(), vec![exit]);
    assert_eq!(scopes.pending_destroys().collect::<Vec<_>>(), vec![destroy]);
    let queue = h
        .driver
        .server
        .channel_send_queue(41, wire::CONTROL_CHANNEL)
        .unwrap();
    assert!(queue.messages <= 7, "lifecycle queue: {queue:?}");
    h.driver.authority.resync(41).unwrap();
    h.clients[0].active = false;
    h.pump(60);
    let peer = &h.driver.authority.peers[&41];
    assert_ne!(peer.welcome.stream.epoch, original.epoch);
    assert!(peer.active);
    assert!(h.clients[0].active);
}

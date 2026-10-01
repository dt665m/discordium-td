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

use dreamwake_sim::replication::{
    OwnerExpectation, OwnerPredictionState, PublicReplica, Replica, ReplicaState,
};
use dreamwake_sim::{DreamPresentation, EnemyKind, RewardKind, UpgradeKind};
use engine_net::replication::stream::{Base, ClientStream};
use engine_server::{ServerDriver, SharedNet};
use renet_cross::{
    BootstrapConfig, BootstrapService, MixedTransportBuilder, MonotonicClientIdAllocator,
    NativeConnectOptions, SecureSessionAuthPolicy, ServerAuthentication, SessionCreateRequest,
    UdpNetcodeClientTransport,
};
use std::sync::{Arc, Mutex};
type TestBootstrap = BootstrapService<
    MonotonicClientIdAllocator,
    SecureSessionAuthPolicy<crate::admission::GuestAdmission>,
>;

/// A minimal client speaking the live protocol over real UDP: it decodes the
/// state stream, acknowledges it in every input packet and resends its
/// unconsumed commands, without prediction.
struct TestClient {
    id: u64,
    renet: RenetClient,
    transport: UdpNetcodeClientTransport,
    welcome: Option<live::Welcome>,
    stream: ClientStream<EntityId, ReplicaState>,
    replicas: BTreeMap<EntityId, (ServerTick, Replica)>,
    status: live::OwnerStatus,
    status_seq: u32,
    sequence: u32,
    commands: u64,
    menu_sequence: u64,
    held: DreamInput,
    edge: Option<DreamAction>,
    unconsumed: VecDeque<live::InputRecord>,
    view: Option<DreamPresentation>,
    notices: Vec<String>,
    outcomes: BTreeMap<ActionKey, live::ActionOutcome>,
    max_packet: usize,
    state_bytes: usize,
    drop_states: bool,
    send_acks: bool,
    receive_application: bool,
}
impl TestClient {
    fn new(id: u64, renet: RenetClient, transport: UdpNetcodeClientTransport) -> Self {
        Self {
            id,
            renet,
            transport,
            welcome: None,
            stream: ClientStream::new(4096),
            replicas: BTreeMap::new(),
            status: live::OwnerStatus::default(),
            status_seq: 0,
            sequence: 0,
            commands: 0,
            menu_sequence: 0,
            held: DreamInput::default(),
            edge: None,
            unconsumed: VecDeque::new(),
            view: None,
            notices: Vec::new(),
            outcomes: BTreeMap::new(),
            max_packet: 0,
            state_bytes: 0,
            drop_states: false,
            send_acks: true,
            receive_application: true,
        }
    }
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
    /// The owner's hero participates in the run.
    fn active(&self) -> bool {
        self.owner().is_some_and(|owner| owner.is_active())
    }
    fn owner(&self) -> Option<&dreamwake_sim::replication::OwnerCheckpoint> {
        let owner = self.welcome.as_ref()?.owner_entity;
        match self.replicas.get(&owner)? {
            (_, Replica::Owner(owner)) => Some(owner),
            _ => None,
        }
    }
    fn receive(&mut self, _now: Duration) {
        if !self.receive_application {
            return;
        }
        let mut changed = false;
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
                let content = welcome.content;
                self.welcome = Some(welcome);
                self.stream = ClientStream::new(4096);
                self.replicas.clear();
                self.status = live::OwnerStatus::default();
                self.status_seq = 0;
                self.sequence = 0;
                self.commands = 0;
                self.menu_sequence = 0;
                self.held = DreamInput::default();
                self.edge = None;
                self.unconsumed.clear();
                self.view = None;
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
        if let Some(through) = outcomes_ack {
            self.control(live::ClientControl::OutcomesAck { through });
        }
        while let Some(bytes) = self.renet.receive_message(wire::STATE_CHANNEL) {
            if self.drop_states {
                continue;
            }
            self.max_packet = self.max_packet.max(bytes.len());
            self.state_bytes += bytes.len();
            let Some(welcome) = self.welcome.clone() else {
                continue;
            };
            let limit = engine_net::codec::FrameLimit::new(live::MAX_STATE_FRAME_BYTES).unwrap();
            let packet = match live::decode_state(&bytes, welcome.stream.epoch, limit) {
                Ok(packet) => packet,
                Err(engine_net::codec::CodecError::WrongEpoch) => continue,
                Err(error) => panic!("state decode: {error:?}"),
            };
            if !self.stream.accepts(packet.seq) {
                continue;
            }
            let expected = self.expected();
            let mut decoded = Vec::new();
            let mut restored = Vec::new();
            for entry in &packet.entries {
                let base = match self
                    .stream
                    .base(entry.entity, entry.base, packet.seq)
                    .unwrap()
                {
                    Base::Superseded => continue,
                    Base::Whole => None,
                    Base::Delta(base) => Some(base),
                };
                let state = ReplicaState::decode(&entry.body, base).unwrap();
                let owner = (entry.entity == welcome.owner_entity).then_some(expected);
                let replica = state.restore(owner).unwrap();
                assert_eq!(
                    matches!(replica, Replica::Owner(_)),
                    entry.entity == welcome.owner_entity
                );
                decoded.push((entry.entity, state));
                restored.push((entry.entity, replica));
            }
            let changes = self
                .stream
                .accept(packet.seq, decoded, &packet.removed)
                .unwrap();
            for (entity, replica) in restored {
                if changes.updated.contains(&entity) {
                    self.replicas.insert(entity, (packet.tick, replica));
                }
            }
            for entity in changes.removed {
                self.replicas.remove(&entity);
            }
            if packet.seq > self.status_seq {
                self.status_seq = packet.seq;
                self.status = packet.status;
                self.unconsumed
                    .retain(|record| record.target.0 > packet.status.finalized.0);
            }
            changed = true;
        }
        if changed {
            self.refresh();
        }
    }
    fn refresh(&mut self) {
        let Some(welcome) = &self.welcome else {
            return;
        };
        let Some((_, Replica::Global(global))) = self.replicas.get(&welcome.global_entity) else {
            return;
        };
        let Some(owner) = self.owner() else {
            return;
        };
        assert_eq!(owner.collision_identity(), welcome.content.scene);
        let owner = OwnerPredictionState::from_checkpoint(owner, self.expected()).unwrap();
        let actors: Vec<_> = self
            .replicas
            .values()
            .filter_map(|(_, replica)| match replica {
                Replica::Actor(actor) => Some(actor.clone()),
                _ => None,
            })
            .collect();
        self.view = Some(DreamPresentation::from_replicas(global, &owner, &actors).unwrap());
    }
    /// One input packet per tick: the newest command and the oldest ones the
    /// server has not consumed, with the stream acknowledgement.
    fn send_input(&mut self, target: TargetTick) {
        let Some(welcome) = &self.welcome else {
            return;
        };
        if self.active() {
            self.commands += 1;
            let actions = self
                .edge
                .take()
                .map(|action| engine_net::commands::ActionEdge { slot: 0, action })
                .into_iter()
                .collect();
            self.unconsumed.push_back(live::InputRecord {
                sequence: CommandSeq(self.commands),
                target,
                input: self.held.into(),
                actions: BoundedVec::new(actions).unwrap(),
            });
            while self.unconsumed.len() > 64 {
                self.unconsumed.pop_front();
            }
        }
        let records: Vec<_> = self
            .unconsumed
            .iter()
            .rev()
            .take(3)
            .chain(self.unconsumed.iter().take(3))
            .cloned()
            .fold(Vec::new(), |mut records, record| {
                if !records.contains(&record) {
                    records.push(record);
                }
                records
            });
        let packet = live::InputPacket {
            ack: if self.send_acks {
                self.stream.ack()
            } else {
                Default::default()
            },
            records: BoundedVec::new(records).unwrap(),
        };
        self.sequence += 1;
        let bytes = live::encode_input(
            &packet,
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
            SecureSessionAuthPolicy::new(crate::admission::GuestAdmission::default(), private_key),
        ));
        let shared = SharedNet {
            health: Default::default(),
            max_clients: 8,
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
                ..Default::default()
            },
        )
        .unwrap();
        self.clients.push(TestClient::new(id, renet, transport));
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
    assert!(h.clients[0].active());
    let client = &mut h.clients[0];
    let welcome = client.welcome.clone().unwrap();
    let packet = live::InputPacket {
        ack: client.stream.ack(),
        records: BoundedVec::new(client.unconsumed.iter().take(3).cloned().collect()).unwrap(),
    };
    // Thirty legal redundant packets arrive after a stalled receive loop.
    for _ in 0..30 {
        client.sequence += 1;
        let bytes = live::encode_input(
            &packet,
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
fn activation_follows_the_client_acknowledging_its_own_hero() {
    let mut h = Harness::new();
    h.join();
    h.join();
    h.clients[0].send_acks = false;
    h.pump(36);
    // Without acknowledgements the joining client receives its hero and the
    // match summary again and again, and nothing else, and never activates.
    let first = &h.clients[0];
    assert!(first.owner().is_some());
    assert!(!first.active());
    assert!(!h.driver.authority.peers[&41].active);
    assert!(
        first
            .replicas
            .values()
            .all(|(_, replica)| !matches!(replica, Replica::Actor(_))),
        "remote actors wait for activation"
    );
    assert!(h.clients[1].active());
    assert!(
        h.clients[1]
            .replicas
            .values()
            .any(|(_, replica)| matches!(replica, Replica::Actor(_))),
        "an active client sees the world"
    );
    h.clients[0].send_acks = true;
    h.pump(24);
    assert!(h.driver.authority.peers[&41].active);
    assert!(h.clients[0].active());
    assert!(h.clients[0].renet.disconnect_reason().is_none());
}

#[test]
fn lost_state_packets_recover_without_application_retransmission() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    h.action(0, 1, DreamAction::Start { lucid: false });
    h.pump(60);
    let before = h.latest(0).stamp.server_tick;
    // A second of total downstream loss: every packet is gone, nothing is
    // acknowledged, and the server keeps publishing current state.
    h.clients[0].drop_states = true;
    h.pump(60);
    assert_eq!(h.latest(0).stamp.server_tick, before);
    assert!(h.driver.authority.peers[&41].active);
    h.clients[0].drop_states = false;
    h.pump(6);
    let after = h.latest(0);
    assert!(after.stamp.server_tick >= h.driver.authority.tick.0 - 6);
    // Everything the server holds for this client is what the client holds.
    let client = &h.clients[0];
    let server = &h.driver.authority.peers[&41].stream;
    let held: BTreeSet<_> = client.replicas.keys().copied().collect();
    let sent: BTreeSet<_> = server.keys().collect();
    assert_eq!(held, sent);
    assert!(h.clients[0].renet.disconnect_reason().is_none());
}

#[test]
fn unchanged_actors_cost_nothing_once_acknowledged() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    assert!(h.clients[0].active());
    // The intro has no moving enemies: after the first whole states, each
    // publication carries the owner and match summary changes only.
    let start = h.clients[0].state_bytes;
    h.pump(120);
    let per_second = (h.clients[0].state_bytes - start) / 2;
    let peer = &h.driver.authority.peers[&41];
    eprintln!(
        "idle intro state bytes/s={per_second} packets={} entries={}",
        peer.egress.state_packets, peer.egress.state_entries
    );
    assert!(
        per_second < 4_000,
        "{per_second} bytes per second while idle"
    );
}

#[test]
fn moving_world_stays_within_tens_of_kilobytes_per_second() {
    let mut h = Harness::new();
    h.join();
    h.join();
    h.pump(36);
    h.action(0, 1, DreamAction::Start { lucid: false });
    let mut pilots = [Pilot::default(), Pilot::default()];
    for _ in 0..120 {
        for index in 0..2 {
            if let Some(view) = h.clients[index].view.clone() {
                pilots[index].drive(&mut h, index, &view);
            }
        }
        h.pump(1);
    }
    let start: Vec<_> = h.clients.iter().map(|client| client.state_bytes).collect();
    for _ in 0..300 {
        for index in 0..2 {
            if let Some(view) = h.clients[index].view.clone() {
                pilots[index].drive(&mut h, index, &view);
            }
        }
        h.pump(1);
    }
    for (client, start) in h.clients.iter().zip(start) {
        let per_second = (client.state_bytes - start) / 5;
        eprintln!("combat state bytes/s={per_second}");
        assert!(
            per_second < 30_000,
            "{per_second} bytes per second in combat"
        );
    }
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
            .unconsumed
            .iter()
            .all(|record| record.sequence.0 > sequence),
        "consumed commands are no longer resent"
    );
}

#[test]
fn action_sequence_exhaustion_cannot_enter_authority_inbox() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    let exhausted = engine_net::commands::Command {
        owner: h.clients[0].welcome.as_ref().unwrap().stream,
        sequence: CommandSeq(u64::from(u32::MAX) + 1),
        target: TargetTick(h.driver.authority.tick.0 + 2),
        input: DreamInput::default().into(),
        actions: BoundedVec::default(),
    };
    let before = h.driver.authority.peers[&41].inbox.pending_len();
    assert!(
        h.driver
            .authority
            .receive_commands(41, vec![exhausted])
            .is_err()
    );
    assert_eq!(h.driver.authority.peers[&41].inbox.pending_len(), before);
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
    let mut moving_gaps = Vec::new();
    let mut owner_age = 0;
    for frame in 0..480 {
        // The native limiter uses wall time, so pace the pump to it.
        let due = started + Duration::from_secs_f64(f64::from(frame + 1) / 60.0);
        if let Some(wait) = due.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(wait);
        }
        h.pump(1);
        assert!(h.clients[0].active());
        if frame < 60 {
            continue;
        }
        let client = &h.clients[0];
        let owner = client.welcome.as_ref().unwrap().owner_entity;
        owner_age = owner_age.max(h.driver.authority.tick.0 - client.replicas[&owner].0.0);
        for (entity, (tick, replica)) in &client.replicas {
            let Replica::Actor(PublicReplica::Enemy(enemy)) = replica else {
                continue;
            };
            if let Some((previous, position)) = latest.insert(*entity, (*tick, enemy.position))
                && previous != *tick
                && position != enemy.position
            {
                moving_gaps.push(tick.0 - previous.0);
            }
        }
    }
    let after = shared.peer_egress_stats(41).unwrap().native_udp_ip.unwrap();
    let elapsed = started.elapsed().as_secs_f64();
    assert!(moving_gaps.len() >= 12, "exercise moving enemies");
    moving_gaps.sort_unstable();
    eprintln!(
        "moving enemy gap p50={} p95={} max={} owner_age={owner_age} native_bytes={}",
        moving_gaps[moving_gaps.len() / 2],
        moving_gaps[(moving_gaps.len() - 1) * 95 / 100],
        moving_gaps.last().unwrap(),
        after.native_udp_ip_bytes - before.native_udp_ip_bytes
    );
    assert!(
        moving_gaps[(moving_gaps.len() - 1) * 95 / 100] <= 6,
        "moving actors update inside the 100 ms presentation buffer"
    );
    assert!(owner_age <= 6, "owner state stays fresh: {owner_age}");
    assert_eq!(after.pacing_drops - before.pacing_drops, 0);
    assert!(
        (after.native_udp_ip_bytes - before.native_udp_ip_bytes) as f64
            <= crate::DEFAULT_EGRESS_BYTES_PER_SECOND as f64 * elapsed
                + crate::DEFAULT_EGRESS_BURST_BYTES as f64
    );
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
            .all(|client| client.max_packet <= live::MAX_STATE_FRAME_BYTES)
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
        authority.receive_commands(41, records).unwrap();
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
    // exactly once when committed progress opens the unchanged window.
    let window = engine_net::commands::CommandLimits::default().future_ticks;
    let future_target = 302 + u64::from(window);
    // Reported slack is capped at what a lead can actually use.
    let reported = engine_net::types::MAX_COMMAND_LEAD_TICKS.min(window);
    receive(&mut authority, vec![command(305, future_target)]);
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
    receive(&mut authority, vec![command(305, future_target)]);
    assert_eq!(
        authority.peers[&41].arrival,
        live::ArrivalFeedback {
            sample: Some(sample(305, future_target, 302, reported)),
            observed_commands: 307,
            late_commands: 302,
        }
    );
    authority.peers.get_mut(&41).unwrap().arrival.sample = None;
    receive(&mut authority, vec![command(305, future_target)]);
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
    // Input framed for the old connection epoch never decodes on the new one.
    let old = live::encode_input(
        &live::InputPacket {
            ack: Default::default(),
            records: BoundedVec::new(vec![live::InputRecord {
                sequence: CommandSeq(1),
                target: TargetTick(authority.tick.0 + 2),
                input: DreamInput::default().into(),
                actions: BoundedVec::default(),
            }])
            .unwrap(),
        },
        first.stream.epoch,
        1,
        authority.peers[&43].limit(),
    )
    .unwrap();
    assert_eq!(
        live::decode_input(&old, second.stream.epoch, authority.peers[&43].limit()),
        Err(engine_net::codec::CodecError::WrongEpoch),
        "old owner stream cannot control reclaimed player"
    );
}

#[test]
fn authenticated_udp_rejoin_preserves_player_and_rejects_a_second_controller() {
    let mut h = Harness::new();
    let original = h.join_player(700);
    h.join_player(800);
    h.pump(36);
    assert!(h.clients[original].active());
    assert_eq!(h.latest(original).hero.id, 700);
    let first = h.clients[original].welcome.clone().unwrap();
    let second = h.join_player(700);
    h.pump(20);
    assert!(!h.clients[second].active());
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
    assert!(h.clients[reconnect].active());
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
fn stalled_bootstrap_peer_is_displaced_by_a_fresh_stream_for_the_same_player() {
    let mut h = Harness::new();
    let stalled = h.join_player(700);
    // Withholding acknowledgements reproduces a client whose join never
    // completes, so it holds no participation, health or readiness.
    h.clients[stalled].send_acks = false;
    h.pump(30);
    let first = h.clients[stalled].welcome.clone().unwrap();
    let first_connection = first.client.0;
    assert!(!h.clients[stalled].active());
    assert!(!h.driver.authority.peers[&first_connection].active);

    // The replacement is admitted instead of refused, so a stalled join
    // cannot strand its own profile.
    let fresh = h.join_player(700);
    h.pump(45);
    assert!(
        !h.driver.authority.peers.contains_key(&first_connection),
        "the never-activated peer is displaced"
    );
    assert!(h.clients[fresh].active(), "the fresh stream activates");
    let welcome = h.clients[fresh].welcome.as_ref().unwrap();
    assert_eq!(welcome.player, first.player);
    assert!(welcome.stream.epoch > first.stream.epoch);
    assert!(welcome.stream.ownership > first.stream.ownership);
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
    assert_eq!(
        wire::CONTROL_MEMORY_BYTES - server.channel_available_memory(41, wire::CONTROL_CHANNEL),
        100
    );
    for packet in client.get_packets_to_send() {
        server.process_packet_from(&packet, 41).unwrap();
    }
    assert_eq!(
        server.channel_available_memory(41, wire::CONTROL_CHANNEL),
        wire::CONTROL_MEMORY_BYTES
    );
}

#[test]
fn reliable_action_outcome_arrives_alongside_continuing_finalization() {
    let mut h = Harness::new();
    h.join();
    h.pump(36);
    h.action(0, 1, DreamAction::Start { lucid: false });
    h.pump(12);
    let before = h.clients[0].status.finalized;
    h.action(
        0,
        2,
        DreamAction::Dash {
            direction: [1.0, 0.0],
        },
    );
    h.pump(24);
    assert!(h.clients[0].status.finalized > before);
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

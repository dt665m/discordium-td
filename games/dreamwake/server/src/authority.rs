//! Dedicated Dreamwake authority over the repository's shared UDP/WebRTC transport.
//! The authenticated transport identity is the only source of player ownership.
use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

use dreamwake_protocol::{self as wire, DreamAction, DreamClientMessage, DreamServerMessage};
use dreamwake_sim::{DreamInput, DreamSimulation, RunPhase};
use renet::RenetServer;
#[cfg(test)]
use renet::ServerEvent;

use engine_server::SharedNet;

#[cfg(test)]
const TICK: Duration = Duration::from_nanos(1_000_000_000 / dreamwake_sim::TICK_HZ as u64);
const INPUT_TIMEOUT: Duration = Duration::from_millis(200);
const MAX_PENDING_ACTIONS: usize = 32;
const MAX_MESSAGES_PER_TICK: usize = 64;

#[cfg(test)]
use engine_net::clock::TickClock;

type Peer = engine_net::session::PeerInbox<DreamInput, DreamAction>;

struct DreamAuthority {
    sim: DreamSimulation,
    peers: BTreeMap<u64, Peer>,
    epoch: u32,
    revision: u64,
    max_clients: usize,
    seed: u64,
    lucid: bool,
    notices: VecDeque<(u64, &'static str)>,
}

impl DreamAuthority {
    fn new(seed: u64, lucid: bool, max_clients: usize) -> Self {
        let mut sim = DreamSimulation::new(seed, lucid);
        sim.remove_player(1);
        Self {
            sim,
            peers: BTreeMap::new(),
            epoch: 1,
            revision: 0,
            max_clients,
            seed,
            lucid,
            notices: VecDeque::new(),
        }
    }

    fn admit(&mut self, id: u64) -> bool {
        if self.peers.contains_key(&id) {
            return true;
        }
        if self.peers.len() >= self.max_clients || !self.sim.add_player(id) {
            return false;
        }
        self.peers.insert(id, Peer::new(self.epoch));
        // A second player cannot inherit a paused single-player session.
        if self.peers.len() > 1 {
            self.sim.set_paused(false);
        }
        true
    }

    fn disconnect(&mut self, id: u64) {
        self.peers.remove(&id);
        self.sim.remove_player(id);
        self.notices.retain(|(recipient, _)| *recipient != id);
        if self.peers.is_empty() {
            self.sim.set_paused(false);
        }
    }

    /// Reject non-finite input before it reaches the shared simulation. Diagonals
    /// and oversized axes are normalized once at this authority boundary.
    fn direction(value: [f32; 2]) -> Option<[f32; 2]> {
        if value.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let length = f64::from(value[0]).hypot(f64::from(value[1]));
        let scale = length.max(1.0);
        Some([
            (f64::from(value[0]) / scale) as f32,
            (f64::from(value[1]) / scale) as f32,
        ])
    }

    fn receive(
        &mut self,
        id: u64,
        channel: u8,
        message: DreamClientMessage,
        now: Duration,
    ) -> Result<(), &'static str> {
        let Some(peer) = self.peers.get_mut(&id) else {
            return Err("No admitted player session");
        };
        peer.consume_message(MAX_MESSAGES_PER_TICK)
            .map_err(|_| "Input rate exceeded")?;
        match message {
            DreamClientMessage::Input {
                epoch,
                seq,
                mut input,
            } => {
                if channel != wire::INPUT_CHANNEL {
                    return Err("Input on invalid channel");
                }
                if !peer.accepts_input(epoch, seq) {
                    return Ok(());
                }
                input.movement = Self::direction(input.movement).ok_or("Invalid movement")?;
                input.aim = Self::direction(input.aim).ok_or("Invalid aim")?;
                // One-shot actions have their own reliable sequence and cannot be
                // injected repeatedly through held movement packets.
                input.dash = false;
                input.casts = [false; 4];
                input.action_sequences = [0; 5];
                peer.receive_input(epoch, seq, input, now);
            }
            DreamClientMessage::Action {
                epoch,
                seq,
                mut action,
            } => {
                if channel != wire::ACTION_CHANNEL {
                    return Err("Action on invalid channel");
                }
                if !peer.accepts_action(epoch, seq) {
                    return Ok(());
                }
                match &mut action {
                    DreamAction::Cast { slot, aim } => {
                        if *slot >= 4 {
                            return Err("Invalid Memory slot");
                        }
                        *aim = Self::direction(*aim).ok_or("Invalid cast aim")?;
                    }
                    DreamAction::Dash { direction } => {
                        *direction = Self::direction(*direction).ok_or("Invalid dash direction")?;
                    }
                    _ => {}
                }
                peer.queue_action(epoch, seq, action, MAX_PENDING_ACTIONS)
                    .map_err(|_| "Action queue exceeded")?;
            }
        }
        Ok(())
    }

    fn notice(&mut self, id: u64, text: &'static str) {
        // A rejected menu operation must never turn into unbounded output.
        if self.notices.len() < self.max_clients * 8 {
            self.notices.push_back((id, text));
        }
    }

    fn restart(&mut self, lucid: bool, new_seed: bool) {
        if new_seed {
            self.seed = self
                .seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
        }
        self.lucid = lucid;
        self.sim.restart_party(self.seed, lucid);
        self.epoch = self.epoch.wrapping_add(1).max(1);
        for peer in self.peers.values_mut() {
            peer.reset(self.epoch);
        }
        self.notices.clear();
    }

    // Starting is a shared action, not an all-player readiness vote.
    fn begin_run(&mut self) {
        if self.sim.phase() == RunPhase::Intro {
            for id in self.peers.keys().copied() {
                self.sim.continue_run_for(id);
            }
        }
    }

    fn step(&mut self, now: Duration) {
        for peer in self.peers.values_mut() {
            peer.begin_tick();
        }
        let mut inputs: BTreeMap<_, _> = self
            .peers
            .iter()
            .map(|(&id, peer)| {
                let mut input = peer.input;
                if !peer.input_is_fresh(now, INPUT_TIMEOUT) {
                    input.movement = [0.0; 2];
                    input.attack = false;
                }
                (id, input)
            })
            .collect();
        let actions: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(&id, peer)| peer.next_action().map(|(seq, action)| (id, seq, action)))
            .collect();
        let original_epoch = self.epoch;
        for (id, seq, action) in actions {
            if original_epoch != self.epoch {
                break;
            }
            let mut accepted = true;
            match action {
                DreamAction::Cast { slot, aim } => {
                    if let Some(input) = inputs.get_mut(&id) {
                        input.aim = aim;
                        input.casts[slot as usize] = true;
                        input.action_sequences[slot as usize + 1] = seq;
                    }
                }
                DreamAction::Dash { direction } => {
                    if let Some(input) = inputs.get_mut(&id) {
                        input.movement = direction;
                        input.dash = true;
                        input.action_sequences[0] = seq;
                    }
                }
                DreamAction::Start { lucid } => {
                    if matches!(
                        self.sim.phase(),
                        RunPhase::Intro | RunPhase::Victory | RunPhase::Defeat
                    ) {
                        if self.lucid != lucid || self.sim.phase() != RunPhase::Intro {
                            self.restart(lucid, false);
                            inputs.clear();
                        }
                        self.begin_run();
                    } else {
                        self.notice(id, "This dream has already begun.");
                    }
                }
                DreamAction::Choose { choice, slot } => {
                    accepted = self
                        .sim
                        .choose_reward_for(id, choice as usize, slot as usize);
                }
                DreamAction::Continue => {
                    if self.sim.phase() == RunPhase::Intro {
                        self.begin_run();
                    } else {
                        accepted = self.sim.continue_run_for(id);
                    }
                }
                DreamAction::Swap { a, b } => {
                    let solo_paused = self.sim.is_paused() && self.peers.len() == 1;
                    if self.sim.phase() == RunPhase::Combat && !solo_paused {
                        self.notice(id, "Rearrange Memories between encounters.");
                    } else {
                        accepted = self.sim.swap_memories_for(id, a as usize, b as usize);
                    }
                }
                DreamAction::BuyMemoryUpgrade { slot } => {
                    accepted = self.sim.buy_memory_upgrade_for(id, slot as usize);
                }
                DreamAction::Restart => {
                    if self.peers.len() == 1
                        || matches!(self.sim.phase(), RunPhase::Victory | RunPhase::Defeat)
                    {
                        self.restart(self.lucid, true);
                        self.begin_run();
                        inputs.clear();
                    } else {
                        self.notice(id, "Finish the shared run before starting another.");
                    }
                }
                DreamAction::Pause { paused } => {
                    if self.peers.len() == 1 {
                        self.sim.set_paused(paused);
                    } else {
                        self.notice(
                            id,
                            "The shared dream keeps moving while party members use menus.",
                        );
                    }
                }
            }
            if !accepted {
                self.notice(id, "That choice is not available in the current encounter.");
            }
            if original_epoch == self.epoch {
                if let Some(peer) = self.peers.get_mut(&id) {
                    peer.acknowledge_action(seq);
                }
            }
        }
        let inputs: Vec<_> = inputs.into_iter().collect();
        self.sim.step_multiplayer(&inputs);
        for peer in self.peers.values_mut() {
            peer.acknowledge_input();
        }
    }

    fn state(&self, id: u64) -> Option<DreamServerMessage> {
        let peer = self.peers.get(&id)?;
        Some(DreamServerMessage::State {
            epoch: self.epoch,
            revision: self.revision,
            client_id: id,
            host_id: 0, // Reserved wire slot; there is no party leader.
            ack_input: peer.applied_input,
            ack_action: peer.applied_action,
            snapshot: Box::new(self.sim.snapshot_for(id)),
        })
    }

    /// Capture and sort the shared ECS world once per publication. Only the local
    /// hero view and command acknowledgements differ between recipients.
    fn for_each_state(&self, mut send: impl FnMut(u64, &DreamServerMessage)) {
        let Some(mut state) = self.peers.keys().next().and_then(|id| self.state(*id)) else {
            return;
        };
        for (&id, peer) in &self.peers {
            let DreamServerMessage::State {
                client_id,
                ack_input,
                ack_action,
                snapshot,
                ..
            } = &mut state
            else {
                unreachable!("authority state always contains a world snapshot");
            };
            if !snapshot.select_player(id) {
                continue;
            }
            *client_id = id;
            *ack_input = peer.applied_input;
            *ack_action = peer.applied_action;
            send(id, &state);
        }
    }
}

fn receive_messages(server: &mut RenetServer, authority: &mut DreamAuthority, now: Duration) {
    let ids: Vec<_> = authority.peers.keys().copied().collect();
    for id in ids {
        for channel in [wire::INPUT_CHANNEL, wire::ACTION_CHANNEL] {
            for _ in 0..MAX_MESSAGES_PER_TICK {
                let Some(bytes) = server.receive_message(id, channel) else {
                    break;
                };
                let result =
                    wire::decode_with_limit::<DreamClientMessage>(&bytes, wire::MAX_INPUT_BYTES)
                        .map_err(|_| "Malformed Dreamwake packet")
                        .and_then(|message| authority.receive(id, channel, message, now));
                if let Err(reason) = result {
                    log::warn!("disconnecting Dreamwake client {id}: {reason}");
                    server.disconnect(id);
                    break;
                }
            }
        }
    }
}

fn send_states(server: &mut RenetServer, authority: &mut DreamAuthority) {
    authority.revision = authority.revision.wrapping_add(1);
    authority.for_each_state(|id, state| {
        let bytes = wire::encode(state);
        if server.can_send_message(id, wire::STATE_CHANNEL, bytes.len()) {
            server.send_message(id, wire::STATE_CHANNEL, bytes);
        }
    });
    while let Some((id, text)) = authority.notices.pop_front() {
        let bytes = wire::encode(&DreamServerMessage::Notice { text: text.into() });
        if server.can_send_message(id, wire::CONTROL_CHANNEL, bytes.len()) {
            server.send_message(id, wire::CONTROL_CHANNEL, bytes);
        } else {
            server.disconnect(id);
        }
    }
}

impl engine_server::runtime::Authority for DreamAuthority {
    fn admit(&mut self, id: u64, server: &mut RenetServer) -> bool {
        if !DreamAuthority::admit(self, id) {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        if let Some(state) = self.state(id) {
            server.send_message(id, wire::STATE_CHANNEL, wire::encode(&state));
        }
        true
    }
    fn disconnect(&mut self, id: u64) {
        DreamAuthority::disconnect(self, id);
    }
    fn receive(&mut self, server: &mut RenetServer, elapsed: Duration) {
        receive_messages(server, self, elapsed);
    }
    fn step(&mut self, elapsed: Duration) {
        DreamAuthority::step(self, elapsed);
    }
    fn publish(&mut self, server: &mut RenetServer) {
        send_states(server, self);
    }
}
/// Installs Dreamwake admission, commands and snapshots on the reusable server
/// runtime. Its sole authority owns the same shared simulation used by prediction.
pub struct DreamwakeServerPlugin {
    pub shared: SharedNet,
    pub seed: u64,
    pub lucid: bool,
}

impl bevy::prelude::Plugin for DreamwakeServerPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        let authority = DreamAuthority::new(self.seed, self.lucid, self.shared.max_clients);
        app.world_mut()
            .insert_non_send(engine_server::runtime::ServerDriver::new(
                self.shared.clone(),
                wire::connection_config(),
                dreamwake_sim::TICK_HZ,
                wire::TICKS_PER_SNAPSHOT,
                authority,
            ));
        app.add_plugins(engine_server::runtime::ServerPlugin::<DreamAuthority>::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn production_plugin_connects_two_udp_players_and_acknowledges_applied_commands() {
        use engine_server::runtime::{ServerDriver, ServerElapsed, ServerFailure};
        use renet_cross::MixedTransportBuilder;
        use std::sync::{Arc, Mutex};
        let transport = MixedTransportBuilder::new(wire::PROTOCOL_ID)
            .udp_bind("127.0.0.1:0".parse().unwrap())
            .webrtc_bind("127.0.0.1:0".parse().unwrap())
            .build()
            .unwrap();
        let address = transport.udp().addresses()[0];
        let bootstrap = Arc::new(BootstrapService::new(
            BootstrapConfig {
                session_ttl: Duration::from_secs(120),
                public_udp_addr: address,
                public_webrtc_addr: address,
                public_http_base: "http://127.0.0.1".into(),
            },
            MonotonicClientIdAllocator::new(41),
            UnsecureDevAuthPolicy,
        ));
        let shared = SharedNet {
            max_clients: 8,
            transport: Arc::new(Mutex::new(transport)),
            bootstrap: bootstrap.clone(),
        };
        let mut app = crate::build_app(shared, 29, false);
        let mut clients: Vec<_> = (0..2)
            .map(|_| {
                let id = bootstrap.create_session().unwrap().client_id;
                TestClient {
                    id,
                    renet: RenetClient::new(wire::connection_config()),
                    transport: UdpNetcodeClientTransport::new(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap(),
                        ClientAuthentication::Unsecure {
                            protocol_id: wire::PROTOCOL_ID,
                            client_id: id,
                            server_addr: address,
                            user_data: None,
                        },
                        UdpSocket::bind("127.0.0.1:0").unwrap(),
                    )
                    .unwrap(),
                }
            })
            .collect();
        let mut states = [None, None];
        for frame in 0..90 {
            for client in &mut clients {
                client.renet.update(TICK);
                client.transport.update(TICK, &mut client.renet).unwrap();
                if frame == 30 {
                    client.renet.send_message(
                        wire::ACTION_CHANNEL,
                        wire::encode(&DreamClientMessage::Action {
                            epoch: 1,
                            seq: 1,
                            action: DreamAction::Start { lucid: false },
                        }),
                    );
                    client.renet.send_message(
                        wire::INPUT_CHANNEL,
                        wire::encode(&DreamClientMessage::Input {
                            epoch: 1,
                            seq: 7,
                            input: DreamInput {
                                movement: [1.0, 0.0],
                                ..Default::default()
                            },
                        }),
                    );
                }
                client.transport.send_packets(&mut client.renet).unwrap();
            }
            app.world_mut().resource_mut::<ServerElapsed>().0 = Some(TICK * frame);
            app.update();
            assert!(app.world().resource::<ServerFailure>().error().is_none());
            for (index, client) in clients.iter_mut().enumerate() {
                while let Some(bytes) = client.renet.receive_message(wire::STATE_CHANNEL) {
                    states[index] = Some(wire::decode::<DreamServerMessage>(&bytes).unwrap());
                }
            }
        }
        assert_eq!(
            app.world()
                .get_non_send::<ServerDriver<DreamAuthority>>()
                .unwrap()
                .authority
                .peers
                .len(),
            2
        );
        let mut worlds = Vec::new();
        for (index, state) in states.into_iter().enumerate() {
            let DreamServerMessage::State {
                client_id,
                ack_input,
                ack_action,
                snapshot,
                ..
            } = state.unwrap()
            else {
                panic!()
            };
            assert_eq!(client_id, clients[index].id);
            assert_eq!(snapshot.hero.id, client_id);
            assert_eq!(snapshot.phase, RunPhase::Combat);
            assert_eq!((ack_input, ack_action), (Some(7), Some(1)));
            worlds.push((snapshot.tick, snapshot.heroes, snapshot.enemies));
        }
        assert_eq!(worlds[0], worlds[1]);
        app.world_mut().write_message(bevy::app::AppExit::Success);
        app.update();
        assert!(
            app.world()
                .get_non_send::<ServerDriver<DreamAuthority>>()
                .unwrap()
                .authority
                .peers
                .is_empty()
        );
    }

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
        for millis in 0..3_000 {
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
        assert!(first.send_due);
        assert!(!first.snapshot_due);
        assert!(!clock.advance(Duration::from_millis(1), || {}).send_due);
        let resumed = Duration::from_secs(600);
        assert!(clock.advance(resumed, || {}).send_due);
        assert!(!clock.advance(resumed, || {}).send_due);
        let next = clock.advance(resumed + TICK, || {});
        assert!(next.send_due);
        assert!(!next.snapshot_due);
    }

    #[test]
    fn snapshots_follow_completed_ticks_despite_late_wakeups() {
        let mut clock = TickClock::new(dreamwake_sim::TICK_HZ, wire::TICKS_PER_SNAPSHOT);
        let mut authority = DreamAuthority::new(8192, false, 8);
        assert!(authority.admit(41));
        authority.begin_run();
        let mut publications = Vec::new();
        // Poll at 10 ms intervals, deliberately not aligned with 60 Hz ticks.
        for millis in (0..1_000).step_by(10) {
            let now = Duration::from_millis(millis);
            if clock.advance(now, || authority.step(now)).snapshot_due {
                let DreamServerMessage::State { snapshot, .. } = authority.state(41).unwrap()
                else {
                    unreachable!();
                };
                publications.push(snapshot.tick);
            }
            // Servicing transport again between ticks cannot publish another state.
            assert!(
                !clock
                    .advance(now, || panic!("no new simulation tick is due"))
                    .send_due
            );
        }
        assert_eq!(publications, (1..=20).map(|n| n * 3).collect::<Vec<_>>());
    }

    #[test]
    fn catchup_publishes_latest_state_once_and_does_not_count_skipped_wall_time() {
        let mut clock = TickClock::new(dreamwake_sim::TICK_HZ, wire::TICKS_PER_SNAPSHOT);
        let mut steps = 0;
        assert!(!clock.advance(Duration::ZERO, || steps += 1).snapshot_due);
        let resumed = Duration::from_secs(600);
        assert!(clock.advance(resumed, || steps += 1).snapshot_due);
        assert_eq!(
            steps, 7,
            "one initial step plus the bounded six catch-up steps"
        );
        assert!(!clock.advance(resumed, || steps += 1).snapshot_due);
        assert!(!clock.advance(resumed + TICK, || steps += 1).snapshot_due);
        assert!(
            clock
                .advance(resumed + TICK * 2, || steps += 1)
                .snapshot_due
        );
        assert_eq!(steps, 9, "the next publication follows three actual steps");
    }

    #[test]
    fn publication_clock_keeps_servicing_paused_or_intro_state() {
        let mut clock = TickClock::new(dreamwake_sim::TICK_HZ, wire::TICKS_PER_SNAPSHOT);
        let mut authority = DreamAuthority::new(8192, false, 8);
        assert!(authority.admit(41));
        let mut publications = 0;
        for tick in 0..6 {
            let now = TICK * tick;
            publications += usize::from(clock.advance(now, || authority.step(now)).snapshot_due);
        }
        assert_eq!(
            publications, 2,
            "publication follows server steps, not the paused gameplay tick"
        );
    }
    use dreamwake_sim::{DreamSnapshot, EnemyKind, RewardKind, UpgradeKind};
    use renet::RenetClient;
    use renet_cross::{
        BootstrapConfig, BootstrapService, ClientAuthentication, MonotonicClientIdAllocator,
        ServerAuthentication, ServerConfig, UdpNetcodeClientTransport, UdpNetcodeServerTransport,
        UnsecureDevAuthPolicy,
    };
    use std::net::UdpSocket;

    struct TestClient {
        id: u64,
        renet: RenetClient,
        transport: UdpNetcodeClientTransport,
    }
    /// Real UDP, netcode admission, Renet fragmentation, compressed snapshots and
    /// the production authority router. A synthetic clock makes this deterministic.
    struct Harness {
        server: RenetServer,
        transport: UdpNetcodeServerTransport,
        bootstrap: BootstrapService,
        authority: DreamAuthority,
        clients: Vec<TestClient>,
        now: Duration,
        frame: u64,
    }
    impl Harness {
        fn new() -> Self {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            let address = socket.local_addr().unwrap();
            let transport = UdpNetcodeServerTransport::new(
                ServerConfig {
                    current_time: Duration::ZERO,
                    max_clients: 16,
                    protocol_id: wire::PROTOCOL_ID,
                    public_addresses: vec![address],
                    authentication: ServerAuthentication::Unsecure,
                },
                socket,
            )
            .unwrap();
            let bootstrap = BootstrapService::new(
                BootstrapConfig {
                    session_ttl: Duration::from_secs(120),
                    public_udp_addr: address,
                    public_webrtc_addr: address,
                    public_http_base: "http://127.0.0.1".into(),
                },
                MonotonicClientIdAllocator::new(41),
                UnsecureDevAuthPolicy,
            );
            Self {
                server: RenetServer::new(wire::connection_config()),
                transport,
                bootstrap,
                authority: DreamAuthority::new(29, false, 8),
                clients: Vec::new(),
                now: Duration::ZERO,
                frame: 0,
            }
        }
        fn join(&mut self) -> usize {
            let session = self.bootstrap.create_session().unwrap();
            self.connect(session.client_id)
        }
        fn connect(&mut self, id: u64) -> usize {
            let index = self.clients.len();
            let transport = UdpNetcodeClientTransport::new(
                self.now,
                ClientAuthentication::Unsecure {
                    protocol_id: wire::PROTOCOL_ID,
                    client_id: id,
                    server_addr: self.transport.addresses()[0],
                    user_data: None,
                },
                UdpSocket::bind("127.0.0.1:0").unwrap(),
            )
            .unwrap();
            self.clients.push(TestClient {
                id,
                renet: RenetClient::new(wire::connection_config()),
                transport,
            });
            index
        }
        fn pump(&mut self, steps: usize) {
            for _ in 0..steps {
                self.now += TICK;
                self.frame += 1;
                for client in &mut self.clients {
                    client.renet.update(TICK);
                    if client.transport.update(TICK, &mut client.renet).is_ok() {
                        let _ = client.transport.send_packets(&mut client.renet);
                    }
                }
                self.server.update(TICK);
                self.transport.update(TICK, &mut self.server).unwrap();
                while let Some(event) = self.server.get_event() {
                    match event {
                        ServerEvent::ClientConnected { client_id } => {
                            if self.bootstrap.on_client_connected(client_id).is_err()
                                || !self.authority.admit(client_id)
                            {
                                self.bootstrap.on_client_disconnected(client_id);
                                self.server.disconnect(client_id);
                            }
                        }
                        ServerEvent::ClientDisconnected { client_id, .. } => {
                            self.bootstrap.on_client_disconnected(client_id);
                            self.authority.disconnect(client_id);
                        }
                    }
                }
                receive_messages(&mut self.server, &mut self.authority, self.now);
                self.authority.step(self.now);
                if self.frame.is_multiple_of(3) {
                    send_states(&mut self.server, &mut self.authority);
                }
                self.transport.send_packets(&mut self.server);
            }
        }
        fn action(&mut self, index: usize, seq: u32, action: DreamAction) {
            self.clients[index].renet.send_message(
                wire::ACTION_CHANNEL,
                wire::encode(&DreamClientMessage::Action {
                    epoch: self.authority.epoch,
                    seq,
                    action,
                }),
            );
        }
        fn input(&mut self, index: usize, seq: u32, movement: [f32; 2]) {
            self.clients[index].renet.send_message(
                wire::INPUT_CHANNEL,
                wire::encode(&DreamClientMessage::Input {
                    epoch: self.authority.epoch,
                    seq,
                    input: DreamInput {
                        movement,
                        aim: movement,
                        ..Default::default()
                    },
                }),
            );
        }
        fn latest(&mut self, index: usize) -> DreamServerMessage {
            self.drain_latest(index)
                .expect("client must receive a full authoritative snapshot")
        }
        fn drain_latest(&mut self, index: usize) -> Option<DreamServerMessage> {
            let mut latest = None;
            while let Some(bytes) = self.clients[index]
                .renet
                .receive_message(wire::STATE_CHANNEL)
            {
                latest = Some(wire::decode(&bytes).unwrap());
            }
            latest
        }
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

    #[test]
    fn shared_capture_keeps_eight_recipient_states_and_acknowledgements_exact() {
        let mut authority = DreamAuthority::new(29, false, 8);
        let mut published = Vec::new();
        authority.for_each_state(|id, state| published.push((id, wire::encode(state))));
        assert!(published.is_empty());
        for id in 41..49 {
            assert!(authority.admit(id));
            let peer = authority.peers.get_mut(&id).unwrap();
            peer.applied_input = Some(id as u32 * 5);
            peer.applied_action = Some(id as u32 * 3);
            if id % 2 == 0 {
                assert!(authority.sim.continue_run_for(id));
            }
        }
        authority.revision = 123;
        authority.for_each_state(|id, state| published.push((id, wire::encode(state))));
        assert_eq!(published.len(), 8);
        for (id, bytes) in &published {
            assert_eq!(
                wire::decode::<DreamServerMessage>(bytes).unwrap(),
                authority.state(*id).unwrap()
            );
        }
        // Departure removes the recipient and its saved actor before the next
        // publication; remaining players keep their own readiness and ACKs.
        authority.disconnect(41);
        published.clear();
        authority.for_each_state(|id, state| published.push((id, wire::encode(state))));
        assert_eq!(published.len(), 7);
        assert!(!published.iter().any(|(id, _)| *id == 41));
        for (id, bytes) in published {
            assert_eq!(
                wire::decode::<DreamServerMessage>(&bytes).unwrap(),
                authority.state(id).unwrap()
            );
        }
    }

    impl Pilot {
        fn act(&mut self, h: &mut Harness, index: usize, action: DreamAction) {
            self.action_sequence += 1;
            h.action(index, self.action_sequence, action);
        }
        /// Decisions read only the last received public snapshot. All changes go
        /// back over the real reliable/unreliable client channels.
        fn drive(&mut self, h: &mut Harness, index: usize, snapshot: &DreamSnapshot) {
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
                                && h.authority.peers.keys().next().copied()
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
            h.clients[index].renet.send_message(
                wire::INPUT_CHANNEL,
                wire::encode(&DreamClientMessage::Input {
                    epoch: h.authority.epoch,
                    seq: self.input_sequence,
                    input: DreamInput {
                        movement,
                        aim: dir,
                        attack: true,
                        ..Default::default()
                    },
                }),
            );
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
    fn complete_cooperative_run_over_udp_reaches_every_realm_and_boss_victory() {
        use std::collections::BTreeSet;
        let mut h = Harness::new();
        h.join();
        h.join();
        h.pump(24);
        let mut pilots = [Pilot::default(), Pilot::default()];
        let mut views: Vec<_> = (0..2)
            .map(|index| match h.latest(index) {
                DreamServerMessage::State { snapshot, .. } => snapshot,
                _ => panic!(),
            })
            .collect();
        let mut realms = BTreeSet::new();
        let mut rooms = BTreeSet::new();
        let mut boss_phases = BTreeSet::new();
        let mut max_state_bytes = 0;
        for _ in 0..90_000 {
            for index in 0..2 {
                pilots[index].drive(&mut h, index, &views[index]);
            }
            h.pump(1);
            for (index, view) in views.iter_mut().enumerate() {
                if let Some(DreamServerMessage::State { snapshot, .. }) = h.drain_latest(index) {
                    max_state_bytes = max_state_bytes.max(wire::encode(&snapshot).len());
                    *view = snapshot;
                }
            }
            let snapshot = &views[0];
            realms.insert(snapshot.realm);
            rooms.insert(snapshot.room);
            for boss in snapshot
                .enemies
                .iter()
                .filter(|e| e.kind == EnemyKind::Boss)
            {
                boss_phases.insert(boss.phase);
            }
            if matches!(snapshot.phase, RunPhase::Victory | RunPhase::Defeat) {
                break;
            }
        }
        let end = &views[0];
        println!(
            "co-op UDP seed={} phase={:?} room={} elapsed={:.1}s kills={} hero_hp={:?} realms={:?} boss_phases={:?} largest_compressed_snapshot={}bytes",
            end.seed,
            end.phase,
            end.room,
            end.elapsed,
            end.kills,
            end.heroes.iter().map(|v| v.hp).collect::<Vec<_>>(),
            realms,
            boss_phases,
            max_state_bytes
        );
        assert_eq!(
            end.phase,
            RunPhase::Victory,
            "both Travelers must win through ordinary input/reward messages"
        );
        assert_eq!(realms, BTreeSet::from([0, 1, 2]));
        assert_eq!(rooms.len(), 10);
        assert_eq!(boss_phases, BTreeSet::from([1, 2, 3]));
        assert!(
            max_state_bytes < 64 * 1024,
            "full snapshots should fit comfortably in bounded Renet channels"
        );
        h.pump(6);
        let DreamServerMessage::State {
            snapshot: first, ..
        } = h.latest(0)
        else {
            panic!()
        };
        let DreamServerMessage::State {
            snapshot: second, ..
        } = h.latest(1)
        else {
            panic!()
        };
        assert_eq!(
            (first.phase, second.phase),
            (RunPhase::Victory, RunPhase::Victory)
        );
        assert_eq!(first.heroes, second.heroes);
        assert_eq!(first.enemies, second.enemies);
    }

    #[test]
    fn delayed_lost_states_recover_and_real_disconnect_rejoin_preserves_the_party() {
        use renet_cross::conditioner::{ConditionerConfig, ConditionerHandle};
        let mut h = Harness::new();
        h.join();
        h.join();
        h.pump(24);
        h.action(0, 1, DreamAction::Start { lucid: false });
        h.action(1, 1, DreamAction::Continue);
        h.pump(6);
        let old = h.latest(1);
        let DreamServerMessage::State {
            revision: old_revision,
            snapshot: old_snapshot,
            ..
        } = &old
        else {
            panic!()
        };
        let existing_before = h.authority.sim.snapshot_for(41).hero;
        let condition = ConditionerHandle::new(ConditionerConfig {
            enabled: true,
            latency: Duration::from_millis(35),
            jitter: Duration::from_millis(10),
            packet_loss: 0.30,
            seed: 392,
            ..Default::default()
        })
        .unwrap();
        h.clients[1].transport.set_conditioner(condition.clone());
        // The transport conditioner uses wall time; only this focused impairment
        // segment waits. The full run above uses the accelerated clock.
        for tick in 0..90 {
            h.input(1, tick + 10, [1.0, 0.0]);
            h.pump(1);
            thread::sleep(Duration::from_millis(3));
        }
        let stats = condition.stats();
        assert!(stats.incoming.simulated_loss_drops > 0);
        assert!(stats.outgoing.simulated_loss_drops > 0);
        assert!(
            h.authority.peers.contains_key(&42),
            "transient loss must not remove the Traveler"
        );
        condition.configure(ConditionerConfig::default()).unwrap();
        h.input(1, 200, [0.0, 0.0]);
        h.pump(12);
        let DreamServerMessage::State {
            revision: new_revision,
            snapshot: recovered,
            ..
        } = h.latest(1)
        else {
            panic!()
        };
        let DreamServerMessage::State {
            snapshot: survivor, ..
        } = h.latest(0)
        else {
            panic!()
        };
        assert!(new_revision > *old_revision);
        assert!(recovered.tick > old_snapshot.tick);
        assert_eq!(recovered.enemies, survivor.enemies);
        assert_eq!(recovered.heroes, survivor.heroes);
        // A delayed older state is identifiable even when the simulation is
        // paused or waiting for rewards: its stream revision never advances.
        let mut accepted_revision = new_revision;
        if let DreamServerMessage::State { revision, .. } = old {
            if revision > accepted_revision {
                accepted_revision = revision;
            }
        }
        assert_eq!(accepted_revision, new_revision);

        h.clients[1].renet.disconnect();
        h.pump(6);
        assert_eq!(h.authority.peers.len(), 1);
        assert!(
            !h.authority
                .sim
                .snapshot_for(41)
                .heroes
                .iter()
                .any(|v| v.id == 42)
        );
        assert_eq!(h.authority.peers.keys().next().copied(), Some(41));
        let rejoined = h.join();
        h.pump(18);
        let DreamServerMessage::State {
            client_id,
            snapshot: joined,
            ..
        } = h.latest(rejoined)
        else {
            panic!()
        };
        let DreamServerMessage::State {
            snapshot: existing, ..
        } = h.latest(0)
        else {
            panic!()
        };
        assert_eq!(client_id, 43);
        assert_eq!(joined.hero.id, 43);
        assert_eq!(joined.phase, RunPhase::Combat);
        assert!(joined.tick > recovered.tick);
        assert_eq!(joined.enemies, existing.enemies);
        assert_eq!(joined.heroes, existing.heroes);
        assert_eq!(existing.hero.id, existing_before.id);
        assert_eq!(existing.hero.position, existing_before.position);
        assert_eq!(existing.hero.max_hp, existing_before.max_hp);
        assert_eq!(existing.hero.memories, existing_before.memories);
        println!(
            "state impairment recovered: incoming_drops={} outgoing_drops={} old_tick={} recovered_tick={} rejoin_tick={} old_client=42 new_client=43",
            stats.incoming.simulated_loss_drops,
            stats.outgoing.simulated_loss_drops,
            old_snapshot.tick,
            recovered.tick,
            joined.tick
        );
    }

    #[test]
    fn solo_paused_build_swap_is_allowed_and_reliable_action_backlog_is_bounded() {
        let mut a = DreamAuthority::new(29, false, 8);
        assert!(a.admit(41));
        let action = |seq, action| DreamClientMessage::Action {
            epoch: 1,
            seq,
            action,
        };
        for (seq, command) in [
            (1, DreamAction::Start { lucid: false }),
            (2, DreamAction::Pause { paused: true }),
        ] {
            a.receive(
                41,
                wire::ACTION_CHANNEL,
                action(seq, command),
                Duration::ZERO,
            )
            .unwrap();
            a.step(Duration::ZERO);
        }
        let before = a.sim.snapshot_for(41);
        assert_eq!(before.phase, RunPhase::Combat);
        assert!(before.paused);
        a.receive(
            41,
            wire::ACTION_CHANNEL,
            action(3, DreamAction::Swap { a: 0, b: 1 }),
            Duration::ZERO,
        )
        .unwrap();
        a.step(Duration::ZERO);
        assert_eq!(
            a.sim.snapshot_for(41).hero.memories[0].kind,
            before.hero.memories[1].kind
        );
        for seq in 4..4 + MAX_PENDING_ACTIONS as u32 {
            a.receive(
                41,
                wire::ACTION_CHANNEL,
                action(
                    seq,
                    DreamAction::Cast {
                        slot: 1,
                        aim: [0.0, -1.0],
                    },
                ),
                Duration::ZERO,
            )
            .unwrap();
        }
        assert_eq!(a.peers[&41].actions.len(), MAX_PENDING_ACTIONS);
        assert!(
            a.receive(
                41,
                wire::ACTION_CHANNEL,
                action(
                    99,
                    DreamAction::Cast {
                        slot: 1,
                        aim: [0.0, -1.0]
                    }
                ),
                Duration::ZERO
            )
            .is_err()
        );
        assert_eq!(a.peers[&41].actions.len(), MAX_PENDING_ACTIONS);
        assert_eq!(a.peers[&41].applied_action, Some(3));
        a.step(Duration::ZERO);
        assert_eq!(a.peers[&41].applied_action, Some(4));
        assert_eq!(a.peers[&41].actions.len(), MAX_PENDING_ACTIONS - 1);
        // A second admission unpauses the shared world and closes combat swaps.
        assert!(a.admit(42));
        assert!(!a.sim.snapshot_for(41).paused);
    }

    #[test]
    fn reliable_cast_and_dash_keep_command_identity_across_network_delay() {
        let mut a = DreamAuthority::new(29, false, 8);
        assert!(a.admit(41));
        let message = |seq, action| DreamClientMessage::Action {
            epoch: 1,
            seq,
            action,
        };
        a.receive(
            41,
            wire::ACTION_CHANNEL,
            message(1, DreamAction::Start { lucid: false }),
            Duration::ZERO,
        )
        .unwrap();
        a.step(Duration::ZERO);
        // The action sequence is deliberately unrelated to the simulation tick.
        for tick in 1..20 {
            a.step(TICK * tick);
        }
        a.receive(
            41,
            wire::ACTION_CHANNEL,
            message(
                77,
                DreamAction::Cast {
                    slot: 2,
                    aim: [0.0, -1.0],
                },
            ),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(a.peers[&41].applied_action, Some(1));
        a.step(Duration::from_secs(1));
        assert_eq!(a.peers[&41].applied_action, Some(77));
        assert!(
            a.sim
                .snapshot_for(41)
                .presentations
                .iter()
                .any(|p| p.id.owner == 41 && p.id.action_seq == 77)
        );
        a.receive(
            41,
            wire::ACTION_CHANNEL,
            message(
                77,
                DreamAction::Cast {
                    slot: 2,
                    aim: [0.0, -1.0],
                },
            ),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(
            a.peers[&41].actions.is_empty(),
            "a retransmit cannot allocate another cast"
        );
        a.receive(
            41,
            wire::ACTION_CHANNEL,
            message(
                78,
                DreamAction::Dash {
                    direction: [1.0, 0.0],
                },
            ),
            Duration::from_secs(1),
        )
        .unwrap();
        a.step(Duration::from_secs(1));
        assert_eq!(a.peers[&41].applied_action, Some(78));
        assert!(
            a.sim
                .snapshot_for(41)
                .presentations
                .iter()
                .any(|p| p.id.owner == 41 && p.id.action_seq == 78)
        );
    }

    #[test]
    fn two_udp_clients_share_world_and_keep_distinct_player_ownership() {
        let mut h = Harness::new();
        h.join();
        h.join();
        h.pump(24);
        assert_eq!(h.authority.peers.len(), 2);
        assert_eq!(h.authority.peers.keys().next().copied(), Some(41));
        assert_eq!(
            h.authority
                .sim
                .snapshot_for(41)
                .heroes
                .iter()
                .map(|v| v.id)
                .collect::<Vec<_>>(),
            [41, 42]
        );
        h.action(0, 1, DreamAction::Start { lucid: false });
        h.action(1, 1, DreamAction::Continue);
        h.pump(5);
        assert_eq!(h.authority.sim.snapshot_for(41).phase, RunPhase::Combat);
        let before_a = h.authority.sim.snapshot_for(41).hero.position;
        let before_b = h.authority.sim.snapshot_for(42).hero.position;
        h.input(0, 5, [1.0, 0.0]);
        h.input(1, 8, [-1.0, 0.0]);
        h.pump(6);
        let DreamServerMessage::State {
            client_id: a,
            ack_input: ack_a,
            snapshot: first,
            ..
        } = h.latest(0)
        else {
            panic!()
        };
        let DreamServerMessage::State {
            client_id: b,
            ack_input: ack_b,
            snapshot: second,
            ..
        } = h.latest(1)
        else {
            panic!()
        };
        assert_eq!((a, b), (41, 42));
        assert_eq!((first.hero.id, second.hero.id), (41, 42));
        assert_eq!((ack_a, ack_b), (Some(5), Some(8)));
        assert_eq!(first.tick, second.tick);
        assert_eq!(first.enemies, second.enemies);
        assert_eq!(first.heroes, second.heroes);
        assert!(first.hero.position[0] > before_a[0]);
        assert!(second.hero.position[0] < before_b[0]);
        // Real late admission joins the same encounter and retains existing actors.
        let tick = first.tick;
        h.join();
        h.pump(16);
        let DreamServerMessage::State { snapshot: late, .. } = h.latest(2) else {
            panic!()
        };
        assert_eq!(late.hero.id, 43);
        assert_eq!(late.heroes.len(), 3);
        assert_eq!(late.phase, RunPhase::Combat);
        assert!(late.tick > tick);
    }

    #[test]
    fn any_traveler_starts_without_ready_votes_and_restarts_a_finished_run() {
        let mut a = DreamAuthority::new(29, false, 8);
        assert!(a.admit(41));
        assert!(a.admit(42));
        let action = |epoch, seq, action| DreamClientMessage::Action { epoch, seq, action };
        a.receive(
            42,
            wire::ACTION_CHANNEL,
            action(a.epoch, 1, DreamAction::Start { lucid: false }),
            Duration::ZERO,
        )
        .unwrap();
        a.step(Duration::from_millis(20));
        assert_eq!(
            a.sim.phase(),
            RunPhase::Combat,
            "the first player never readied"
        );
        let before = a.sim.snapshot_for(41);
        assert!(a.admit(43));
        assert_eq!(a.sim.phase(), RunPhase::Combat);
        assert_eq!(a.sim.snapshot_for(41).hero, before.hero);
        a.disconnect(41);
        assert_eq!(a.sim.phase(), RunPhase::Combat);
        for _ in 0..3600 {
            a.sim.step_multiplayer(&[]);
            if a.sim.phase() == RunPhase::Defeat {
                break;
            }
        }
        assert_eq!(a.sim.phase(), RunPhase::Defeat);
        let old_epoch = a.epoch;
        a.receive(
            43,
            wire::ACTION_CHANNEL,
            action(old_epoch, 1, DreamAction::Restart),
            Duration::from_millis(40),
        )
        .unwrap();
        a.step(Duration::from_millis(60));
        assert_eq!(a.epoch, old_epoch + 1);
        assert_eq!(a.sim.phase(), RunPhase::Combat);
        assert_eq!(a.sim.snapshot_for(43).heroes.len(), 2);
        // Another old-epoch click cannot restart the newly running party.
        a.receive(
            42,
            wire::ACTION_CHANNEL,
            action(old_epoch, 2, DreamAction::Restart),
            Duration::from_millis(80),
        )
        .unwrap();
        a.step(Duration::from_millis(100));
        assert_eq!(a.epoch, old_epoch + 1);
    }

    #[test]
    fn no_player_can_reset_or_pause_an_active_party_run() {
        let mut h = Harness::new();
        h.join();
        h.join();
        h.pump(24);
        h.action(1, 1, DreamAction::Start { lucid: false });
        h.pump(6);
        let before = h.authority.sim.snapshot_for(41);
        h.action(1, 2, DreamAction::Restart);
        h.action(1, 3, DreamAction::Pause { paused: true });
        h.action(1, 4, DreamAction::Swap { a: 0, b: 1 });
        h.pump(6);
        let after_host = h.authority.sim.snapshot_for(41);
        let after_guest = h.authority.sim.snapshot_for(42);
        assert_eq!(h.authority.epoch, 1);
        assert_eq!(after_host.seed, before.seed);
        assert!(!after_host.paused);
        assert_eq!(after_host.hero.memories, before.hero.memories);
        assert_eq!(
            after_guest.hero.memories[0].kind,
            before.hero.memories[0].kind
        );
        let mut notices = Vec::new();
        while let Some(bytes) = h.clients[1].renet.receive_message(wire::CONTROL_CHANNEL) {
            if let DreamServerMessage::Notice { text } = wire::decode(&bytes).unwrap() {
                notices.push(text);
            }
        }
        assert!(notices.iter().any(|v| v.contains("Finish the shared run")));
        assert!(notices.iter().any(|v| v.contains("shared dream")));
    }

    #[test]
    fn acknowledgments_follow_application_and_stale_or_invalid_inputs_never_apply() {
        let mut a = DreamAuthority::new(29, false, 8);
        assert!(a.admit(41));
        let message = |seq, input| DreamClientMessage::Input {
            epoch: 1,
            seq,
            input,
        };
        a.receive(
            41,
            wire::ACTION_CHANNEL,
            DreamClientMessage::Action {
                epoch: 1,
                seq: 2,
                action: DreamAction::Start { lucid: false },
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(a.peers[&41].applied_action, None);
        a.step(Duration::ZERO);
        assert_eq!(a.peers[&41].applied_action, Some(2));
        let input = DreamInput {
            movement: [1.0, 0.0],
            aim: [1.0, 0.0],
            dash: true,
            casts: [true; 4],
            action_sequences: [1234; 5],
            ..Default::default()
        };
        a.receive(41, wire::INPUT_CHANNEL, message(10, input), Duration::ZERO)
            .unwrap();
        assert_eq!(a.peers[&41].applied_input, None);
        assert!(!a.peers[&41].input.dash);
        assert_eq!(a.peers[&41].input.casts, [false; 4]);
        assert_eq!(a.peers[&41].input.action_sequences, [0; 5]);
        a.step(Duration::from_millis(10));
        assert_eq!(a.peers[&41].applied_input, Some(10));
        let position = a.sim.snapshot_for(41).hero.position;
        a.receive(
            41,
            wire::INPUT_CHANNEL,
            message(
                9,
                DreamInput {
                    movement: [-1.0, 0.0],
                    ..Default::default()
                },
            ),
            Duration::from_millis(50),
        )
        .unwrap();
        assert_eq!(a.peers[&41].input.movement, [1.0, 0.0]);
        a.step(Duration::from_millis(250));
        assert_eq!(
            a.sim.snapshot_for(41).hero.position,
            position,
            "missing packets stop held movement after 200 ms"
        );
        assert!(
            a.receive(
                41,
                wire::INPUT_CHANNEL,
                message(
                    11,
                    DreamInput {
                        movement: [f32::NAN, 0.0],
                        ..Default::default()
                    }
                ),
                Duration::from_millis(260)
            )
            .is_err()
        );
        a.receive(
            41,
            wire::ACTION_CHANNEL,
            DreamClientMessage::Action {
                epoch: 1,
                seq: 3,
                action: DreamAction::Restart,
            },
            Duration::from_millis(280),
        )
        .unwrap();
        a.step(Duration::from_millis(280));
        assert_eq!(a.epoch, 2);
        assert_eq!(a.peers[&41].applied_action, None);
        a.receive(
            41,
            wire::INPUT_CHANNEL,
            message(90, input),
            Duration::from_millis(300),
        )
        .unwrap();
        assert_eq!(
            a.peers[&41].received_input, None,
            "old-epoch packets cannot move a restarted hero"
        );
    }

    #[test]
    fn eight_udp_clients_start_and_receive_the_same_live_encounter() {
        let mut h = Harness::new();
        for _ in 0..8 {
            h.join();
        }
        h.pump(24);
        assert_eq!(h.authority.peers.len(), 8);
        let host = h.authority.peers.keys().next().copied().unwrap();
        for index in 0..8 {
            h.action(
                index,
                1,
                if h.clients[index].id == host {
                    DreamAction::Start { lucid: false }
                } else {
                    DreamAction::Continue
                },
            );
        }
        h.pump(5);
        assert_eq!(h.authority.sim.snapshot_for(host).phase, RunPhase::Combat);
        for index in 0..8 {
            h.input(index, 4, [if index % 2 == 0 { 1.0 } else { -1.0 }, 0.0]);
            h.action(
                index,
                2,
                DreamAction::Cast {
                    slot: 1,
                    aim: [0.0, -1.0],
                },
            );
        }
        h.pump(6);
        let mut shared = None;
        for index in 0..8 {
            let DreamServerMessage::State {
                client_id,
                ack_input,
                ack_action,
                snapshot,
                ..
            } = h.latest(index)
            else {
                panic!()
            };
            assert_eq!(client_id, h.clients[index].id);
            assert_eq!(snapshot.hero.id, client_id);
            assert_eq!(snapshot.heroes.len(), 8);
            assert_eq!((ack_input, ack_action), (Some(4), Some(2)));
            let world = (
                snapshot.tick,
                snapshot.heroes,
                snapshot.enemies,
                snapshot.projectiles,
            );
            if let Some(previous) = &shared {
                assert_eq!(previous, &world);
            } else {
                shared = Some(world);
            }
        }
    }

    #[test]
    fn bootstrap_and_configured_limits_reject_unissued_or_ninth_player() {
        let mut h = Harness::new();
        for _ in 0..9 {
            h.join();
        }
        h.connect(900);
        h.pump(36);
        assert_eq!(h.authority.peers.len(), 8);
        assert!(!h.authority.peers.contains_key(&900));
        assert_eq!(h.authority.sim.snapshot_for(41).heroes.len(), 8);
        let previous = h.authority.peers.keys().next().copied().unwrap();
        h.authority.disconnect(previous);
        assert_eq!(h.authority.peers.len(), 7);
        assert!(
            !h.authority
                .sim
                .snapshot_for(h.authority.peers.keys().next().copied().unwrap())
                .heroes
                .iter()
                .any(|v| v.id == previous)
        );
        assert!(h.clients.iter().any(|client| client.id == 900));
    }
}

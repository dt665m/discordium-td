use std::{
    collections::{HashSet, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::{http_api, simulation::Simulation};
use clap::Parser;
use game_shared::{FIXED_DT_SECONDS, PROTOCOL_ID, ReliableServerMessage, WorldDelta, encode};
use renet::{ConnectionConfig, DefaultChannel, RenetServer, ServerEvent};
use renet_cross::{
    BootstrapConfig, BootstrapService, MixedServerTransport, MixedTransportBuilder,
    MonotonicClientIdAllocator, ServerAuthentication, UnsecureDevAuthPolicy,
};

#[derive(Debug, Clone, Parser)]
#[command(name = "game_server")]
pub struct ServerArgs {
    #[arg(long, env = "TD_HTTP_BIND", default_value = "0.0.0.0:8080")]
    http_bind: SocketAddr,
    #[arg(long, env = "TD_UDP_BIND", default_value = "0.0.0.0:5000")]
    udp_bind: SocketAddr,
    #[arg(long, env = "TD_WEBRTC_BIND", default_value = "0.0.0.0:5001")]
    webrtc_bind: SocketAddr,
    #[arg(long, env = "TD_PUBLIC_UDP_ADDR", default_value = "127.0.0.1:5000")]
    public_udp_addr: SocketAddr,
    #[arg(long, env = "TD_PUBLIC_WEBRTC_ADDR", default_value = "127.0.0.1:5001")]
    public_webrtc_addr: SocketAddr,
    #[arg(
        long,
        env = "TD_PUBLIC_HTTP_BASE",
        default_value = "http://127.0.0.1:8080"
    )]
    public_http_base: String,
}

pub fn run(args: ServerArgs) -> Result<(), Box<dyn std::error::Error>> {
    log::info!(
        "starting server: http_bind={} udp_bind={} webrtc_bind={} public_http_base={} public_udp_addr={} public_webrtc_addr={}",
        args.http_bind,
        args.udp_bind,
        args.webrtc_bind,
        args.public_http_base,
        args.public_udp_addr,
        args.public_webrtc_addr
    );

    let server = RenetServer::new(ConnectionConfig::default());
    let shared_transport = Arc::new(Mutex::new(
        MixedTransportBuilder::new(PROTOCOL_ID)
            .udp_bind(args.udp_bind)
            .webrtc_bind(args.webrtc_bind)
            .public_udp_addr(args.public_udp_addr)
            .public_webrtc_addr(args.public_webrtc_addr)
            .max_clients(256)
            .authentication(ServerAuthentication::Unsecure)
            .build()?,
    ));

    let bootstrap = Arc::new(BootstrapService::new(
        BootstrapConfig {
            session_ttl: Duration::from_secs(120),
            public_udp_addr: args.public_udp_addr,
            public_webrtc_addr: args.public_webrtc_addr,
            public_http_base: args.public_http_base,
        },
        MonotonicClientIdAllocator::new(1),
        UnsecureDevAuthPolicy,
    ));

    let _http_thread = http_api::spawn_http_server_thread(
        Arc::clone(&bootstrap),
        Arc::clone(&shared_transport),
        args.http_bind,
        args.public_webrtc_addr,
    );

    run_game_loop(server, shared_transport, bootstrap)
}

fn run_game_loop(
    mut server: RenetServer,
    shared_transport: Arc<Mutex<MixedServerTransport>>,
    bootstrap: Arc<BootstrapService<MonotonicClientIdAllocator, UnsecureDevAuthPolicy>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut sim = Simulation::new();
    let mut pending_join_snapshots = VecDeque::new();

    let tick_dt = Duration::from_secs_f32(FIXED_DT_SECONDS);
    let mut previous = Instant::now();
    let mut accumulator = Duration::ZERO;

    loop {
        let now = Instant::now();
        let frame_dt = now.saturating_duration_since(previous);
        previous = now;

        server.update(frame_dt);

        {
            let mut transport = lock_transport(&shared_transport)?;
            if let Err(err) = transport.update(frame_dt, &mut server) {
                log::warn!("transport update error: {err}");
            }
        }

        while let Some(event) = server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    match bootstrap.on_client_connected(client_id) {
                        Ok(()) => {
                            sim.add_player(client_id);
                            pending_join_snapshots.push_back(client_id);
                            log::info!("client connected: {client_id}");
                        }
                        Err(err) => {
                            log::warn!("disconnecting unauthorized client {client_id}: {err}");
                            server.disconnect(client_id);
                        }
                    }
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    bootstrap.on_client_disconnected(client_id);
                    sim.remove_player(client_id);
                    pending_join_snapshots.retain(|id| *id != client_id);
                    log::info!("client disconnected: {client_id} ({reason})");
                }
            }
        }

        receive_client_commands(&mut server, &mut sim);

        accumulator += frame_dt;
        while accumulator >= tick_dt {
            send_pending_joins(&mut server, &sim, &mut pending_join_snapshots);

            let tick_output = sim.step();
            for event in tick_output.reliable_events {
                let payload = encode(&ReliableServerMessage::Event(event));
                server.broadcast_message(DefaultChannel::ReliableOrdered, payload);
            }

            broadcast_world_deltas(&mut server, &sim);
            accumulator -= tick_dt;

            if sim.tick().is_multiple_of(120) {
                log::debug!(
                    "authoritative tick={} clients={}",
                    sim.tick(),
                    server.clients_id().len()
                );
            }
        }

        {
            let mut transport = lock_transport(&shared_transport)?;
            transport.send_packets(&mut server);
        }

        thread::sleep(Duration::from_millis(1));
    }
}

fn lock_transport(
    shared_transport: &Arc<Mutex<MixedServerTransport>>,
) -> Result<std::sync::MutexGuard<'_, MixedServerTransport>, Box<dyn std::error::Error>> {
    shared_transport
        .lock()
        .map_err(|_| std::io::Error::other("transport mutex poisoned").into())
}

fn receive_client_commands(server: &mut RenetServer, sim: &mut Simulation) {
    let clients = server.clients_id();
    for client_id in clients {
        while let Some(bytes) = server.receive_message(client_id, DefaultChannel::ReliableOrdered) {
            if let Err(err) = decode_and_queue_command(client_id, &bytes, sim) {
                log::debug!("dropping invalid reliable command from client {client_id}: {err}");
            }
        }

        while let Some(bytes) = server.receive_message(client_id, DefaultChannel::Unreliable) {
            if let Err(err) = decode_and_queue_command(client_id, &bytes, sim) {
                log::debug!("dropping invalid unreliable command from client {client_id}: {err}");
            }
        }
    }
}

fn decode_and_queue_command(
    client_id: u64,
    bytes: &[u8],
    sim: &mut Simulation,
) -> Result<(), String> {
    let command = game_shared::decode(bytes).map_err(|err| err.to_string())?;
    sim.queue_command(client_id, command);
    Ok(())
}

fn send_pending_joins(
    server: &mut RenetServer,
    sim: &Simulation,
    pending_joins: &mut VecDeque<u64>,
) {
    let connected: HashSet<u64> = server.clients_id().into_iter().collect();
    while let Some(client_id) = pending_joins.pop_front() {
        if !connected.contains(&client_id) {
            continue;
        }

        let join = sim.join_snapshot(client_id);
        let payload = encode(&ReliableServerMessage::JoinSnapshot(join));
        server.send_message(client_id, DefaultChannel::ReliableOrdered, payload);
    }
}

fn broadcast_world_deltas(server: &mut RenetServer, sim: &Simulation) {
    let client_ids = server.clients_id();
    for client_id in client_ids {
        let delta: WorldDelta = sim.world_delta_for(client_id);
        server.send_message(client_id, DefaultChannel::Unreliable, encode(&delta));
    }
}

use std::{
    collections::{HashSet, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::{
    http_api::{self, HttpTlsConfig},
    simulation::Simulation,
};
use axum::http::HeaderValue;
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
    #[arg(long, env = "TD_HTTP_BIND")]
    http_bind: Option<SocketAddr>,
    #[arg(long, env = "TD_HTTP_TLS_CERT")]
    http_tls_cert: Option<PathBuf>,
    #[arg(long, env = "TD_HTTP_TLS_KEY")]
    http_tls_key: Option<PathBuf>,
    #[arg(long, env = "TD_UDP_BIND", default_value = "0.0.0.0:5000")]
    udp_bind: SocketAddr,
    #[arg(long, env = "TD_WEBRTC_BIND", default_value = "0.0.0.0:5001")]
    webrtc_bind: SocketAddr,
    #[arg(long, env = "TD_PUBLIC_UDP_ADDR", default_value = "127.0.0.1:5000")]
    public_udp_addr: SocketAddr,
    #[arg(long, env = "TD_PUBLIC_WEBRTC_ADDR", default_value = "127.0.0.1:5001")]
    public_webrtc_addr: SocketAddr,
    #[arg(long, env = "TD_PUBLIC_HTTP_BASE")]
    public_http_base: Option<String>,
    #[arg(long, env = "TD_CORS_ALLOWED_ORIGINS")]
    cors_allowed_origins: Option<String>,
}

fn default_http_bind(tls_enabled: bool) -> SocketAddr {
    if tls_enabled {
        SocketAddr::from(([0, 0, 0, 0], 443))
    } else {
        SocketAddr::from(([0, 0, 0, 0], 8080))
    }
}

fn default_public_http_base(http_bind: SocketAddr, tls_enabled: bool) -> String {
    let scheme = if tls_enabled { "https" } else { "http" };
    let default_host = "127.0.0.1";
    let default_port = if tls_enabled { 443 } else { 80 };

    if http_bind.port() == default_port {
        format!("{scheme}://{default_host}")
    } else {
        format!("{scheme}://{default_host}:{}", http_bind.port())
    }
}

fn parse_cors_allowed_origins(
    raw: Option<String>,
) -> Result<Option<Vec<HeaderValue>>, std::io::Error> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let origins: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .collect();

    if origins.is_empty() || (origins.len() == 1 && origins[0] == "*") {
        return Ok(None);
    }

    if origins.contains(&"*") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "TD_CORS_ALLOWED_ORIGINS cannot mix '*' with explicit origins",
        ));
    }

    let mut values = Vec::with_capacity(origins.len());
    for origin in origins {
        let value = HeaderValue::from_str(origin).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid CORS origin '{origin}': {err}"),
            )
        })?;
        values.push(value);
    }

    Ok(Some(values))
}

fn cors_origins_label(origins: Option<&[HeaderValue]>) -> String {
    let Some(origins) = origins else {
        return "*".to_owned();
    };
    origins
        .iter()
        .map(|origin| origin.to_str().unwrap_or("<non-utf8-origin>"))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn run(args: ServerArgs) -> Result<(), Box<dyn std::error::Error>> {
    let ServerArgs {
        http_bind,
        http_tls_cert,
        http_tls_key,
        udp_bind,
        webrtc_bind,
        public_udp_addr,
        public_webrtc_addr,
        public_http_base,
        cors_allowed_origins,
    } = args;

    let http_tls = match (http_tls_cert, http_tls_key) {
        (Some(cert_path), Some(key_path)) => Some(HttpTlsConfig {
            cert_path,
            key_path,
        }),
        (None, None) => None,
        (Some(_), None) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "TD_HTTP_TLS_CERT was set but TD_HTTP_TLS_KEY is missing",
            )
            .into());
        }
        (None, Some(_)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "TD_HTTP_TLS_KEY was set but TD_HTTP_TLS_CERT is missing",
            )
            .into());
        }
    };
    let tls_enabled = http_tls.is_some();
    let http_bind = http_bind.unwrap_or_else(|| default_http_bind(tls_enabled));
    let mut public_http_base =
        public_http_base.unwrap_or_else(|| default_public_http_base(http_bind, tls_enabled));
    if tls_enabled && public_http_base.starts_with("http://") {
        public_http_base = format!("https://{}", public_http_base.trim_start_matches("http://"));
    }
    let cors_allowed_origins = parse_cors_allowed_origins(cors_allowed_origins)?;
    let cors_origins = cors_origins_label(cors_allowed_origins.as_deref());

    log::info!(
        "starting server: http_bind={} http_tls={} udp_bind={} webrtc_bind={} public_http_base={} public_udp_addr={} public_webrtc_addr={} cors_allowed_origins={}",
        http_bind,
        http_tls.is_some(),
        udp_bind,
        webrtc_bind,
        public_http_base,
        public_udp_addr,
        public_webrtc_addr,
        cors_origins
    );

    let server = RenetServer::new(ConnectionConfig::default());
    let shared_transport = Arc::new(Mutex::new(
        MixedTransportBuilder::new(PROTOCOL_ID)
            .udp_bind(udp_bind)
            .webrtc_bind(webrtc_bind)
            .public_udp_addr(public_udp_addr)
            .public_webrtc_addr(public_webrtc_addr)
            .max_clients(8)
            .authentication(ServerAuthentication::Unsecure)
            .build()?,
    ));

    let bootstrap = Arc::new(BootstrapService::new(
        BootstrapConfig {
            session_ttl: Duration::from_secs(120),
            public_udp_addr,
            public_webrtc_addr,
            public_http_base,
        },
        MonotonicClientIdAllocator::new(1),
        UnsecureDevAuthPolicy,
    ));

    let _http_thread = http_api::spawn_http_server_thread(
        Arc::clone(&bootstrap),
        Arc::clone(&shared_transport),
        http_bind,
        public_webrtc_addr,
        http_tls,
        cors_allowed_origins,
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

            if sim.has_players() {
                let tick_output = sim.step();
                for event in tick_output.reliable_events {
                    let payload = encode(&ReliableServerMessage::Event(event));
                    server.broadcast_message(DefaultChannel::ReliableOrdered, payload);
                }

                broadcast_world_deltas(&mut server, &sim);

                if sim.tick().is_multiple_of(120) {
                    log::debug!(
                        "authoritative tick={} clients={}",
                        sim.tick(),
                        server.clients_id().len()
                    );
                }
            }

            accumulator -= tick_dt;
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

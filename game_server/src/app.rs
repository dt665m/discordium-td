use std::{
    any::Any,
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;

use crate::{
    debug_bridge::ServerDebugBridgeHandle,
    debug_context::{DebugContext, unix_ms},
    debug_recorder::DebugRecorderHandle,
    http_api::{self, HttpTlsConfig},
    net::{NetRuntime, SharedNet},
};
use axum::http::HeaderValue;
use clap::Parser;
use game_shared::{
    ClientAck, ClientCommand, ClientMoveBundle, PROTOCOL_ID, ReliableServerMessage, SERVER_TICK_HZ,
    ServerDebugClientFrame, ServerDebugFrame, ServerWorldMessage, WorldDelta, WorldPatch, encode,
};
use game_sim::Simulation;
use renet::{ConnectionConfig, DefaultChannel, RenetServer, ServerEvent};
use renet_cross::{
    BootstrapConfig, BootstrapService, MixedTransportBuilder, MonotonicClientIdAllocator,
    ServerAuthentication, UnsecureDevAuthPolicy,
};

#[derive(Debug, Clone, Parser)]
#[command(name = "game_server")]
pub struct ServerArgs {
    #[command(flatten)]
    pub network_conditioner: crate::conditioner::NetworkConditionerArgs,
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
    #[arg(long, default_value_t = false)]
    pub ui: bool,
    #[arg(long, default_value_t = false)]
    pub debug_bridge: bool,
    #[arg(long, default_value_t = false)]
    pub debug_recorder: bool,
    #[arg(long, env = "TD_DEBUG_LOG_DIR")]
    pub debug_log_dir: Option<PathBuf>,
}

#[allow(dead_code)]
enum ServerMode {
    Headless,
    Ui,
}

#[derive(Resource, Default)]
struct ClientNetStates(HashMap<u64, ClientNetState>);

#[derive(Resource, Default)]
struct PendingJoinSnapshots(VecDeque<u64>);

#[derive(Resource, Clone, Default)]
struct DebugBridgeResource(Option<ServerDebugBridgeHandle>);

#[derive(Resource, Clone, Default)]
pub(crate) struct DebugRecorderResource(pub(crate) Option<DebugRecorderHandle>);

const DEBUG_BRIDGE_HISTORY_LIMIT: usize = 240;
const DEFAULT_DEBUG_LOG_DIR: &str = "target/debug-recorder";

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
    let mode = if args.ui {
        #[cfg(not(feature = "ui"))]
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "--ui requires the 'ui' feature: cargo run --features ui",
            )
            .into());
        }
        #[cfg(feature = "ui")]
        ServerMode::Ui
    } else {
        ServerMode::Headless
    };

    let ServerArgs {
        network_conditioner,
        http_bind,
        http_tls_cert,
        http_tls_key,
        udp_bind,
        webrtc_bind,
        public_udp_addr,
        public_webrtc_addr,
        public_http_base,
        cors_allowed_origins,
        ui: _,
        debug_bridge,
        debug_recorder,
        debug_log_dir,
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
    let debug_context = DebugContext::new();
    log::info!("server identity: {:?}", debug_context.identity);
    let debug_bridge_enabled = debug_bridge || debug_recorder;
    let debug_bridge =
        debug_bridge_enabled.then(|| ServerDebugBridgeHandle::new(DEBUG_BRIDGE_HISTORY_LIMIT));
    let debug_recorder = if debug_recorder {
        let log_dir = debug_log_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_DEBUG_LOG_DIR));
        Some(DebugRecorderHandle::new(log_dir, debug_context.clone())?)
    } else {
        None
    };

    log::info!(
        "starting server: http_bind={} http_tls={} udp_bind={} webrtc_bind={} public_http_base={} public_udp_addr={} public_webrtc_addr={} cors_allowed_origins={} debug_bridge={} debug_recorder={}",
        http_bind,
        http_tls.is_some(),
        udp_bind,
        webrtc_bind,
        public_http_base,
        public_udp_addr,
        public_webrtc_addr,
        cors_origins,
        debug_bridge.is_some(),
        debug_recorder.is_some()
    );

    let conditioner_config = network_conditioner.config();
    log::info!("server network conditioner startup: {conditioner_config:?}");
    let server = Arc::new(Mutex::new(RenetServer::new(ConnectionConfig::default())));
    let conditioner = renet_cross::server_conditioner::ServerConditionerHandle::new(
        renet_cross::server_conditioner::ServerConditionerConfig {
            packets: conditioner_config,
            max_peers: 32,
            ..Default::default()
        },
    )?;
    let shared_transport = Arc::new(Mutex::new(
        MixedTransportBuilder::new(PROTOCOL_ID)
            .transport_config(renet_cross::ServerTransportConfig {
                conditioner: Some(conditioner.clone()),
            })
            .udp_bind(udp_bind)
            .webrtc_bind(webrtc_bind)
            .public_udp_addr(public_udp_addr)
            .public_webrtc_addr(public_webrtc_addr)
            .max_clients(8)
            .authentication(ServerAuthentication::Unsecure)
            .build()?,
    ));

    if let Some(recorder) = &debug_recorder {
        recorder.event(
            "server_conditioner_startup",
            None,
            None,
            format!("{:?}", crate::conditioner::snapshot(&conditioner)),
        );
    }
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
        debug_bridge.clone(),
        debug_recorder.clone(),
    );

    let shared_net = SharedNet {
        server,
        transport: shared_transport,
        bootstrap,
    };

    build_and_run_app(
        mode,
        shared_net,
        debug_bridge,
        debug_recorder,
        debug_context,
        conditioner,
    );
    Ok(())
}

fn build_and_run_app(
    mode: ServerMode,
    shared_net: SharedNet,
    debug_bridge: Option<ServerDebugBridgeHandle>,
    debug_recorder: Option<DebugRecorderHandle>,
    debug_context: DebugContext,
    conditioner: renet_cross::server_conditioner::ServerConditionerHandle,
) {
    let mut app = App::new();

    match mode {
        ServerMode::Headless => {
            app.add_plugins(
                MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(1))),
            );
        }
        ServerMode::Ui => {
            #[cfg(feature = "ui")]
            {
                app.add_plugins(
                    DefaultPlugins
                        .set(WindowPlugin {
                            primary_window: Some(Window {
                                title: "Discordium TD Server".into(),
                                resolution: (1200, 900).into(),
                                ..default()
                            }),
                            ..default()
                        })
                        .disable::<bevy::log::LogPlugin>(),
                );
                app.add_systems(
                    Startup,
                    (crate::ui::setup_ui_scene, crate::ui::setup_conditioner_ui),
                )
                .add_systems(
                    Update,
                    (
                        crate::ui::conditioner_controls,
                        crate::ui::update_network_panel,
                    )
                        .chain(),
                );
            }
        }
    }

    app.insert_resource(Time::<Fixed>::from_hz(SERVER_TICK_HZ as f64))
        .insert_resource(crate::conditioner::ServerConditioner(conditioner))
        .insert_resource(NetRuntime { shared: shared_net })
        .insert_resource(Simulation::new())
        .insert_resource(crate::state_history::StateHistory::default())
        .insert_resource(ClientNetStates::default())
        .insert_resource(PendingJoinSnapshots::default())
        .insert_resource(DebugBridgeResource(debug_bridge))
        .insert_resource(DebugRecorderResource(debug_recorder))
        .insert_resource(debug_context)
        .add_systems(Update, network_transport_update)
        .add_systems(FixedUpdate, fixed_server_tick)
        .add_systems(PostUpdate, flush_transport_packets)
        .run();
}

// ---------------------------------------------------------------------------
// Bevy systems
// ---------------------------------------------------------------------------

fn network_transport_update(
    recorder: Res<DebugRecorderResource>,
    time: Res<Time<Real>>,
    net: Res<NetRuntime>,
    mut sim: ResMut<Simulation>,
    mut client_states: ResMut<ClientNetStates>,
    mut pending_joins: ResMut<PendingJoinSnapshots>,
) {
    let frame_dt = time.delta();
    let shared = &net.shared;
    let bootstrap = Arc::clone(&shared.bootstrap);

    shared.with_server_and_transport(|server, transport| {
        server.update(frame_dt);

        let update_result = catch_unwind(AssertUnwindSafe(|| transport.update(frame_dt, server)));
        match update_result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                log::warn!("transport update error: {err}");
                if let Some(r) = &recorder.0 {
                    r.event("transport_error", None, Some(sim.tick()), err.to_string());
                }
            }
            Err(payload) => {
                log::error!(
                    "transport update panicked: {}; disconnecting all clients",
                    panic_payload_to_string(payload.as_ref())
                );
                transport.disconnect_all(server);
            }
        }

        while let Some(event) = server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    match bootstrap.on_client_connected(client_id) {
                        Ok(()) => {
                            sim.add_player(client_id);
                            pending_joins.0.push_back(client_id);
                            client_states.0.insert(
                                client_id,
                                ClientNetState {
                                    last_acked_tick: None,
                                    confirmed_baseline: None,
                                    sent_history: VecDeque::new(),
                                },
                            );
                            log::info!("client connected: {client_id}");
                            if let Some(r) = &recorder.0 {
                                r.event(
                                    "client_connected",
                                    Some(client_id),
                                    Some(sim.tick()),
                                    String::new(),
                                );
                            }
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
                    pending_joins.0.retain(|id| *id != client_id);
                    client_states.0.remove(&client_id);
                    log::info!("client disconnected: {client_id} ({reason})");
                    if let Some(r) = &recorder.0 {
                        r.event(
                            "client_disconnected",
                            Some(client_id),
                            Some(sim.tick()),
                            reason.to_string(),
                        );
                    }
                }
            }
        }
    });
}

fn fixed_server_tick(
    conditioner: Res<crate::conditioner::ServerConditioner>,
    context: Res<DebugContext>,
    net: Res<NetRuntime>,
    mut sim: ResMut<Simulation>,
    mut history: ResMut<crate::state_history::StateHistory>,
    mut client_states: ResMut<ClientNetStates>,
    mut pending_joins: ResMut<PendingJoinSnapshots>,
    debug_bridge: Res<DebugBridgeResource>,
    debug_recorder: Res<DebugRecorderResource>,
) {
    net.shared.with_server_and_transport(|server, transport| {
        receive_client_commands(server, &mut sim, &mut client_states.0);
        send_pending_joins(server, &sim, &mut pending_joins.0);

        if sim.has_players() {
            let step_started = std::time::Instant::now();
            let current = sim.world_delta_for(0);
            history.record(&current);
            for (client, action) in sim.ready_actions() {
                if !matches!(
                    action.command,
                    game_shared::ClientCommand::CastAbility {
                        ability: game_shared::AbilityId::ArcBurst,
                        ..
                    }
                ) {
                    continue;
                }
                let rtt = server.network_info(client).map(|n| n.rtt).unwrap_or(0.0);
                let decision =
                    crate::state_history::validate_view_tick(sim.tick(), action.view_tick, rtt)
                        .and_then(|tick| {
                            let origin = current
                                .heroes
                                .iter()
                                .find(|h| h.client_id == client)
                                .ok_or(crate::state_history::RewindFallback::CasterDiscontinuity)?
                                .pos;
                            history.validated_targets(
                                action.match_epoch,
                                tick,
                                sim.tick(),
                                client,
                                origin,
                            )
                        });
                let detail = match decision {
                    Ok(targets) => {
                        let count = targets.len();
                        sim.set_historical_targets(client, action.command.seq(), targets);
                        format!(
                            "seq={} view_tick={:?} historical_candidates={count}",
                            action.command.seq(),
                            action.view_tick
                        )
                    }
                    Err(reason) => format!(
                        "seq={} view_tick={:?} current_world_fallback={reason:?}",
                        action.command.seq(),
                        action.view_tick
                    ),
                };
                log::debug!("lag compensation: {detail}");
                if let Some(recorder) = &debug_recorder.0 {
                    recorder.event(
                        "lag_compensation_query",
                        Some(client),
                        Some(sim.tick()),
                        detail,
                    );
                }
            }
            let tick_output = sim.step();
            history.record(&sim.world_delta_for(0));
            let step_duration_ms = step_started.elapsed().as_secs_f64() * 1000.0;
            for event in tick_output.reliable_events {
                let payload = encode(&ReliableServerMessage::Event(event));
                server.broadcast_message(DefaultChannel::ReliableOrdered, payload);
            }

            broadcast_world_deltas(server, &sim, &mut client_states.0);
            if debug_bridge.0.is_some() || debug_recorder.0.is_some() {
                let frame = build_server_debug_frame(
                    server,
                    transport,
                    &sim,
                    &client_states.0,
                    &context,
                    step_duration_ms,
                    &conditioner.0,
                );
                if let Some(debug_bridge) = &debug_bridge.0 {
                    debug_bridge.push_frame(frame.clone());
                }
                if let Some(debug_recorder) = &debug_recorder.0 {
                    debug_recorder.record_server_frame(frame);
                }
            }

            if sim.tick().is_multiple_of(120) {
                log::debug!(
                    "authoritative tick={} clients={}",
                    sim.tick(),
                    server.clients_id().len()
                );
            }
        }
    });
}

fn flush_transport_packets(net: Res<NetRuntime>) {
    net.shared.with_server_and_transport(|server, transport| {
        let send_result = catch_unwind(AssertUnwindSafe(|| transport.send_packets(server)));
        if let Err(payload) = send_result {
            log::error!(
                "transport send_packets panicked: {}; disconnecting all clients",
                panic_payload_to_string(payload.as_ref())
            );
            transport.disconnect_all(server);
        }
    });
}

// ---------------------------------------------------------------------------
// Free functions (unchanged internals)
// ---------------------------------------------------------------------------

/// Per-client net state for delta compression.
struct ClientNetState {
    /// The tick the client last confirmed receiving.
    last_acked_tick: Option<u32>,
    /// The snapshot that the client confirmed receiving (set when ack matches sent_history).
    /// Only this snapshot is used as a baseline for patches — never an unconfirmed one.
    confirmed_baseline: Option<WorldDelta>,
    /// Ring buffer of recently sent snapshots so we can look up the one matching an ack.
    sent_history: VecDeque<(u32, WorldDelta)>,
}

impl ClientNetState {
    fn acknowledge(&mut self, tick: u32) {
        if self
            .last_acked_tick
            .is_some_and(|old| !game_shared::is_newer_input_seq(tick, old))
        {
            return;
        }
        // Unknown/future ACKs must not poison the watermark or baseline.
        if let Some(pos) = self.sent_history.iter().position(|(t, _)| *t == tick) {
            self.last_acked_tick = Some(tick);
            self.confirmed_baseline = Some(self.sent_history[pos].1.clone());
            self.sent_history.drain(..pos);
        }
    }
}

fn build_server_debug_frame(
    server: &RenetServer,
    transport: &renet_cross::MixedServerTransport,
    sim: &Simulation,
    client_net_states: &HashMap<u64, ClientNetState>,
    context: &DebugContext,
    step_duration_ms: f64,
    conditioner: &renet_cross::server_conditioner::ServerConditionerHandle,
) -> ServerDebugFrame {
    let mut client_ids = server.clients_id();
    client_ids.sort_unstable();

    let clients = client_ids
        .into_iter()
        .map(|client_id| {
            let state = client_net_states.get(&client_id);
            let mut world = sim.world_delta_for(client_id);
            sort_world_delta(&mut world);
            ServerDebugClientFrame {
                network: server.network_info(client_id).ok().map(|n| {
                    game_shared::DebugNetworkHealth {
                        browser_drops: None,
                        transport: if transport.udp().client_addr(client_id).is_some() {
                            "udp"
                        } else {
                            "webrtc"
                        }
                        .into(),
                        rtt_ms: n.rtt * 1000.0,
                        packet_loss: n.packet_loss,
                        sent_bytes_per_second: n.bytes_sent_per_second as f64,
                        received_bytes_per_second: n.bytes_received_per_second as f64,
                    }
                }),
                client_id,
                last_acked_tick: state.and_then(|state| state.last_acked_tick),
                confirmed_baseline_tick: state
                    .and_then(|state| state.confirmed_baseline.as_ref())
                    .map(|baseline| baseline.tick),
                sent_history_len: state.map_or(0, |state| state.sent_history.len()),
                world,
            }
        })
        .collect();

    ServerDebugFrame {
        conditioner: Some(crate::conditioner::snapshot(conditioner)),
        schema_version: game_shared::DEBUG_SCHEMA_VERSION,
        identity: context.identity.clone(),
        capture_unix_ms: unix_ms(),
        capture_elapsed_ms: context.elapsed_ms(),
        step_duration_ms,
        tick: sim.tick(),
        clients,
    }
}

fn sort_world_delta(world: &mut WorldDelta) {
    world.heroes.sort_by_key(|hero| hero.client_id);
    world.enemies.sort_by_key(|enemy| enemy.id);
    world.towers.sort_by_key(|tower| tower.id);
    world.objectives.sort_by_key(|objective| objective.lane);
}

fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_owned();
    }

    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }

    "<non-string panic payload>".to_owned()
}

fn receive_client_commands(
    server: &mut RenetServer,
    sim: &mut Simulation,
    client_net_states: &mut HashMap<u64, ClientNetState>,
) {
    let clients = server.clients_id();
    for client_id in clients {
        // Reliable channel: action commands (attacks, abilities, builds, etc.)
        for _ in 0..16 {
            let Some(bytes) = server.receive_message(client_id, DefaultChannel::ReliableOrdered)
            else {
                break;
            };
            if bytes.len() > 64 {
                server.disconnect(client_id);
                break;
            }
            match game_shared::decode::<game_shared::ClientAction>(&bytes) {
                Ok(action) if !matches!(action.command, ClientCommand::Move { .. }) => {
                    sim.queue_action(client_id, action);
                }
                Ok(_) => {
                    server.disconnect(client_id);
                    break;
                }
                Err(err) => {
                    log::debug!("dropping invalid reliable command from client {client_id}: {err}");
                }
            }
        }

        // Unreliable channel: move bundles and client acks
        for _ in 0..32 {
            let Some(bytes) = server.receive_message(client_id, DefaultChannel::Unreliable) else {
                break;
            };
            if bytes.len() > 2048 {
                continue;
            }
            // Try to decode as ClientMoveBundle first
            if let Ok(bundle) = game_shared::decode::<ClientMoveBundle>(&bytes) {
                if bundle.match_epoch != sim.sim_meta().match_epoch
                    || bundle.moves.len() > 16
                    || bundle.actions.len() > 64
                {
                    continue;
                }
                for action in bundle.actions {
                    sim.queue_action(client_id, action);
                }
                for (seq, dir) in bundle.moves {
                    sim.queue_command(client_id, ClientCommand::Move { seq, dir });
                }
                continue;
            }
            // Try to decode as ClientAck
            if let Ok(ack) = game_shared::decode::<ClientAck>(&bytes) {
                if let Some(state) = client_net_states.get_mut(&client_id) {
                    state.acknowledge(ack.tick);
                }
                continue;
            }
            log::debug!("dropping unrecognized unreliable message from client {client_id}");
        }
    }
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

/// Max tick age for a baseline to be usable for delta compression.
const DELTA_BASELINE_MAX_AGE: u32 = 60;

fn broadcast_world_deltas(
    server: &mut RenetServer,
    sim: &Simulation,
    client_net_states: &mut HashMap<u64, ClientNetState>,
) {
    let client_ids = server.clients_id();
    for client_id in client_ids {
        let delta: WorldDelta = sim.world_delta_for(client_id);

        let state = client_net_states
            .entry(client_id)
            .or_insert_with(|| ClientNetState {
                last_acked_tick: None,
                confirmed_baseline: None,
                sent_history: VecDeque::new(),
            });

        // Only build a patch against a baseline the client has confirmed receiving.
        let msg = snapshot_message(&delta, state.confirmed_baseline.as_ref());

        server.send_message(client_id, DefaultChannel::Unreliable, encode(&msg));

        // Store in sent_history so we can promote to confirmed_baseline when acked.
        state.sent_history.push_back((delta.tick, delta));
        while state.sent_history.len() > 64 {
            state.sent_history.pop_front();
        }
    }
}

fn snapshot_message(delta: &WorldDelta, baseline: Option<&WorldDelta>) -> ServerWorldMessage {
    if let Some(baseline) = baseline {
        let age = delta.tick.wrapping_sub(baseline.tick);
        if age > 0 && age <= DELTA_BASELINE_MAX_AGE {
            return ServerWorldMessage::Patch(build_world_patch(delta, baseline));
        }
    }
    ServerWorldMessage::Full(delta.clone())
}

fn build_world_patch(current: &WorldDelta, baseline: &WorldDelta) -> WorldPatch {
    let mut hero_patches = Vec::new();
    let mut enemy_patches = Vec::new();
    let mut tower_patches = Vec::new();
    let mut removed_ids = Vec::new();
    let mut removed_hero_ids = Vec::new();

    // Heroes: find changed and new
    for hero in &current.heroes {
        let changed = baseline
            .heroes
            .iter()
            .find(|b| b.client_id == hero.client_id)
            .is_none_or(|b| b != hero);
        if changed {
            hero_patches.push(hero.clone());
        }
    }

    // Enemies: find changed and new
    for enemy in &current.enemies {
        let changed = baseline
            .enemies
            .iter()
            .find(|b| b.id == enemy.id)
            .is_none_or(|b| b != enemy);
        if changed {
            enemy_patches.push(*enemy);
        }
    }

    // Towers: find changed and new
    for tower in &current.towers {
        let changed = baseline
            .towers
            .iter()
            .find(|b| b.id == tower.id)
            .is_none_or(|b| b != tower);
        if changed {
            tower_patches.push(*tower);
        }
    }

    // Find removed entities (in baseline but not in current)
    for hero in &baseline.heroes {
        if !current.heroes.iter().any(|c| c.client_id == hero.client_id) {
            removed_hero_ids.push(hero.client_id);
        }
    }
    for enemy in &baseline.enemies {
        if !current.enemies.iter().any(|c| c.id == enemy.id) {
            removed_ids.push(enemy.id);
        }
    }
    for tower in &baseline.towers {
        if !current.towers.iter().any(|c| c.id == tower.id) {
            removed_ids.push(tower.id);
        }
    }

    WorldPatch {
        presentations: current.presentations.clone(),
        tick: current.tick,
        baseline_tick: baseline.tick,
        phase: current.phase,
        match_restart_ticks_remaining: current.match_restart_ticks_remaining,
        wave: current.wave,
        team_life: current.team_life,
        objectives: current.objectives.clone(),
        your_last_input_seq: current.your_last_input_seq,
        hero_patches,
        enemy_patches,
        tower_patches,
        removed_ids,
        removed_hero_ids,
        sim_meta: current.sim_meta,
    }
}

#[cfg(test)]
mod protocol_regressions {
    use super::*;

    #[test]
    fn unknown_ack_cannot_poison_baseline_and_old_ack_cannot_regress_it() {
        let mut delta = Simulation::new().world_delta_for(1);
        delta.tick = 10;
        let mut state = ClientNetState {
            last_acked_tick: None,
            confirmed_baseline: None,
            sent_history: VecDeque::from([(10, delta.clone())]),
        };
        state.acknowledge(1000);
        assert_eq!(state.last_acked_tick, None);
        state.acknowledge(10);
        state.acknowledge(9);
        assert_eq!(state.last_acked_tick, Some(10));
        assert_eq!(state.confirmed_baseline.unwrap(), delta);
    }

    #[test]
    fn baseline_expiry_falls_back_to_full_including_tick_wrap() {
        let mut baseline = Simulation::new().world_delta_for(1);
        baseline.tick = u32::MAX - 20;
        let mut delta = baseline.clone();
        delta.tick = baseline.tick.wrapping_add(60);
        assert!(matches!(
            snapshot_message(&delta, Some(&baseline)),
            ServerWorldMessage::Patch(_)
        ));
        delta.tick = baseline.tick.wrapping_add(61);
        assert!(matches!(
            snapshot_message(&delta, Some(&baseline)),
            ServerWorldMessage::Full(_)
        ));
        assert!(matches!(
            snapshot_message(&delta, None),
            ServerWorldMessage::Full(_)
        ));
    }

    #[test]
    fn hero_removal_is_separate_from_world_entity_namespace() {
        let mut sim = Simulation::new();
        sim.add_player(1_000_000);
        let before = sim.world_delta_for(1_000_000);
        sim.remove_player(1_000_000);
        let patch = build_world_patch(&sim.world_delta_for(1_000_000), &before);
        assert_eq!(patch.removed_hero_ids, vec![1_000_000]);
        assert!(patch.removed_ids.is_empty());
    }
}

#[cfg(test)]
mod presentation_replication_tests {
    use super::*;
    #[test]
    fn patches_carry_creation_and_expiry_of_simulation_presentations() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let baseline = sim.world_delta_for(1);
        sim.queue_command(
            1,
            game_shared::ClientCommand::CastAbility {
                seq: 1,
                ability: game_shared::AbilityId::ArcBurst,
            },
        );
        sim.step();
        let active = sim.world_delta_for(1);
        let patch = build_world_patch(&active, &baseline);
        assert_eq!(patch.presentations, active.presentations);
        assert_eq!(patch.presentations.len(), 1);
        for _ in 0..20 {
            sim.step();
        }
        let expired = sim.world_delta_for(1);
        assert!(
            build_world_patch(&expired, &active)
                .presentations
                .is_empty()
        );
    }
}

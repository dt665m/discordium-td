#[cfg(test)]
use crate::network::{Ingress, NetworkMetrics};
#[cfg(test)]
use crate::network::{egress::broadcast_world_deltas, worker::PacketSendClock};
#[cfg(test)]
use crate::replication::{ClientNetState, ReplicationHistory};
#[cfg(test)]
use game_shared::{
    ClientAck, ClientMoveBundle, ReliableServerMessage, ServerWorldMessage, WorldDelta,
    build_world_patch, encode,
};
#[cfg(test)]
use std::collections::{HashMap, VecDeque};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;

use crate::{
    debug_bridge::ServerDebugBridgeHandle,
    debug_context::DebugContext,
    debug_recorder::DebugRecorderHandle,
    http_api::{self, HttpTlsConfig},
    net::SharedNet,
    network::{
        ServerNetworkPlugin,
        schedule::{PendingJoinSnapshots, fixed_server_tick, receive_client_commands},
    },
};
use axum::http::HeaderValue;
use clap::Parser;
use game_shared::{PROTOCOL_ID, SERVER_TICK_HZ};
#[cfg(test)]
use game_sim::Simulation;
#[cfg(test)]
use game_sim::world as sim;
#[cfg(test)]
use renet::ConnectionConfig;
#[cfg(test)]
use renet::{DefaultChannel, RenetServer};
use renet_cross::{
    BootstrapConfig, BootstrapService, MixedTransportBuilder, MonotonicClientIdAllocator,
    ServerAuthentication, UnsecureDevAuthPolicy,
};

#[derive(Debug, Clone, Parser)]
#[command(name = "game_server")]
pub struct ServerArgs {
    /// Run the server-authoritative Dreamwake cooperative roguelite.
    #[arg(long, default_value_t = false)]
    pub dreamwake: bool,
    /// Shared encounter/reward seed for a Dreamwake party.
    #[arg(long, default_value_t = 8192)]
    pub dream_seed: u64,
    /// Start Dreamwake with the optional Lucid challenge enabled.
    #[arg(long, default_value_t = false)]
    pub dream_lucid: bool,
    #[command(flatten)]
    pub network_conditioner: crate::conditioner::NetworkConditionerArgs,
    /// Admission limit; benchmark the chosen player/entity workload before raising it.
    #[arg(long, env = "TD_MAX_CLIENTS", default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=1024))]
    pub max_clients: u16,
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
        dreamwake,
        dream_seed,
        dream_lucid,
        network_conditioner,
        max_clients,
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

    let protocol_id = if dreamwake {
        game_dream_net::PROTOCOL_ID
    } else {
        PROTOCOL_ID
    };
    let conditioner_config = network_conditioner.config();
    log::info!("server network conditioner startup: {conditioner_config:?}");
    let conditioner = renet_cross::server_conditioner::ServerConditionerHandle::new(
        renet_cross::server_conditioner::ServerConditionerConfig {
            packets: conditioner_config,
            max_peers: (usize::from(max_clients) * 2).max(32),
            ..Default::default()
        },
    )?;
    let shared_transport = Arc::new(Mutex::new(
        MixedTransportBuilder::new(protocol_id)
            .transport_config(renet_cross::ServerTransportConfig {
                conditioner: Some(conditioner.clone()),
            })
            .udp_bind(udp_bind)
            .webrtc_bind(webrtc_bind)
            .public_udp_addr(public_udp_addr)
            .public_webrtc_addr(public_webrtc_addr)
            .max_clients(usize::from(max_clients))
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
        max_clients: usize::from(max_clients),
        transport: shared_transport,
        bootstrap,
    };

    if dreamwake {
        return crate::dreamwake::run_dreamwake(shared_net, dream_seed, dream_lucid)
            .map_err(|error| std::io::Error::other(error).into());
    }
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
            // These frame schedules only coordinate time, messages and export.
            // Dispatching their tiny systems across the pool at 1 kHz adds work
            // without parallel gameplay. Simulation queries keep their own pool.
            let labels = app
                .world()
                .resource::<bevy::app::MainScheduleOrder>()
                .labels
                .clone();
            for label in labels {
                app.edit_schedule(label, |schedule| {
                    schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
                });
            }
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

    #[cfg(feature = "ui")]
    app.insert_resource(crate::conditioner::ServerConditioner(conditioner.clone()));

    app.add_plugins(game_sim::SimulationPlugins)
        .add_plugins(ServerNetworkPlugin {
            shared: shared_net,
            context: debug_context.clone(),
            bridge: debug_bridge.clone(),
            recorder: debug_recorder.clone(),
            conditioner: conditioner.clone(),
        })
        .insert_resource(Time::<Fixed>::from_hz(SERVER_TICK_HZ as f64))
        .insert_resource(crate::state_history::StateHistory::default())
        .insert_resource(PendingJoinSnapshots::default())
        .insert_resource(DebugRecorderResource(debug_recorder))
        .insert_resource(debug_context)
        .add_systems(PreUpdate, receive_client_commands)
        .add_systems(FixedUpdate, fixed_server_tick)
        .run();
}

#[cfg(test)]
mod protocol_regressions {
    use super::*;

    #[test]
    fn preupdate_stages_ordered_ingress_before_shared_ecs_tick() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, game_sim::SimulationPlugins));
        let (worker, output) = crate::network::test_worker(vec![
            Ingress::Connected(1),
            Ingress::Disconnected(1),
            Ingress::Connected(2),
            Ingress::Moves(
                2,
                ClientMoveBundle {
                    match_epoch: sim::sim_meta(app.world()).match_epoch,
                    moves: vec![(7, [1.0, 0.0])],
                    actions: Vec::new(),
                },
            ),
        ]);
        app.init_resource::<crate::network::PendingOutput>()
            .add_systems(PostUpdate, crate::network::inbox::publish_output)
            .insert_resource(worker)
            .init_resource::<NetworkMetrics>()
            .init_resource::<PendingJoinSnapshots>()
            .init_resource::<DebugRecorderResource>()
            .init_resource::<crate::state_history::StateHistory>()
            .add_systems(PreUpdate, receive_client_commands)
            .add_systems(FixedUpdate, fixed_server_tick);
        // The soft per-pass deadline may split connection setup over frames.
        for _ in 0..4 {
            app.world_mut().run_schedule(PreUpdate);
        }
        assert_eq!(sim::world_delta(app.world()).tick, 0);
        assert_eq!(sim::world_delta(app.world()).heroes[0].last_move_seq, None);
        app.world_mut().run_schedule(FixedUpdate);
        assert!(output.try_recv().is_err());
        app.world_mut().run_schedule(PostUpdate);
        let batch = output.try_recv().unwrap();
        assert_eq!(batch.messages.len(), 1);
        let crate::network::Outbound::ToClient(2, ReliableServerMessage::JoinSnapshot(join)) =
            &batch.messages[0]
        else {
            panic!()
        };
        assert_eq!(join.world.tick, 0);
        let snapshot = batch.world.unwrap();
        assert_eq!(snapshot.tick, 1);
        assert_eq!(snapshot.heroes.len(), 1);
        assert_eq!(snapshot.heroes[0].client_id, 2);
        assert_eq!(snapshot.heroes[0].last_move_seq, Some(7));
        assert_eq!(snapshot, sim::world_delta(app.world()));
    }

    #[test]
    fn staging_bursts_leave_remainder_for_later_frames() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, game_sim::SimulationPlugins));
        let total = crate::network::ingress::MAX_INGRESS_PER_PASS + 10;
        let (worker, _output) =
            crate::network::test_worker((0..total).map(|_| Ingress::Metrics(Vec::new())).collect());
        app.insert_resource(worker)
            .init_resource::<NetworkMetrics>()
            .add_systems(PreUpdate, receive_client_commands);
        app.world_mut().run_schedule(PreUpdate);
        let metrics = app.world().resource::<NetworkMetrics>();
        assert!(metrics.ingress_messages > 0);
        assert!(metrics.ingress_messages <= crate::network::ingress::MAX_INGRESS_PER_PASS as u64);
        assert_eq!(metrics.ingress_deferred_passes, 1);
        assert_eq!(metrics.ingress_peak_pending, total);
        // No fixed tick ran; additional staging never gives the worker a new quota.
        assert_eq!(app.world().resource::<NetworkMetrics>().tick, 0);
        for _ in 0..total {
            app.world_mut().run_schedule(PreUpdate);
        }
        assert_eq!(
            app.world().resource::<NetworkMetrics>().ingress_messages,
            total as u64
        );
    }

    #[test]
    fn unsent_snapshot_cannot_be_acknowledged_as_baseline() {
        let mut config = ConnectionConfig::default();
        for channel in &mut config.server_channels_config {
            channel.max_memory_usage_bytes = 1;
        }
        let mut server = RenetServer::new(config);
        server.add_connection(1);
        let mut simulation = Simulation::new();
        simulation.add_player(1);
        let snapshot = simulation.world_delta();
        let mut clients = HashMap::new();
        let mut history = ReplicationHistory::default();
        broadcast_world_deltas(&mut server, &snapshot, &mut clients, &mut history);
        let client = clients.get_mut(&1).unwrap();
        assert!(client.outstanding.is_empty());
        client.acknowledge(snapshot.tick);
        assert!(client.last_acked_tick.is_none());
    }

    #[test]
    fn admission_limit_is_configurable_and_rejects_transport_overflow() {
        assert_eq!(
            ServerArgs::try_parse_from(["server"]).unwrap().max_clients,
            8
        );
        assert_eq!(
            ServerArgs::try_parse_from(["server", "--max-clients", "64"])
                .unwrap()
                .max_clients,
            64
        );
        for invalid in ["0", "1025"] {
            assert!(ServerArgs::try_parse_from(["server", "--max-clients", invalid]).is_err());
        }
    }

    #[test]
    fn packet_generation_is_bounded_under_fast_app_polling() {
        let mut last = PacketSendClock::default();
        let sends = (0..1000)
            .filter(|ms| last.should_flush(Duration::from_millis(*ms), false))
            .count();
        assert!(sends <= 31 && sends >= 29);
        assert!(last.should_flush(Duration::from_secs(3), false));
        assert!(!last.should_flush(Duration::from_secs(3), false));
    }
    #[test]
    fn lossless_baseline_codec_scales_with_units_and_survives_structural_changes() {
        let mut sim = Simulation::new();
        for id in 1..=8 {
            sim.add_player(id);
        }
        for _ in 0..=game_shared::WAVE_PREP_TICKS {
            sim.step();
        }
        let seed = sim.world_delta();
        for count in [84, 256, 1024] {
            let mut world = seed.clone();
            world.enemies = (0..count)
                .map(|i| {
                    let mut enemy = seed.enemies[0];
                    enemy.id += i;
                    enemy.pos = [(i % 28) as f32 - 14.0, (i / 28) as f32 % 20.0 - 10.0];
                    enemy
                })
                .collect();
            let mut total = 0;
            let mut maximum = 0;
            let started = std::time::Instant::now();
            for step in 1..=120 {
                // A nine-tick baseline represents about 300ms of round-trip age.
                let baseline = world.clone();
                world.tick += 9;
                for (index, enemy) in world.enemies.iter_mut().enumerate() {
                    let angle = (index as f32 * 0.137 + step as f32 * 0.017).sin();
                    enemy.vel = [angle, angle.cos()];
                    enemy.pos[0] += enemy.vel[0] * 0.3;
                    enemy.pos[1] += enemy.vel[1] * 0.3;
                    enemy.facing.dir = enemy.vel;
                    enemy.repath_cooldown = step % 12;
                }
                for hero in &mut world.heroes {
                    hero.last_move_seq = Some(step);
                    hero.mana = 50.0 + step as f32 * 0.1;
                }
                if step % 15 == 0 {
                    world.enemies.remove(0);
                }
                let patch = ServerWorldMessage::Patch(build_world_patch(&world, &baseline));
                let encoded = encode(&patch);
                let full = encode(&ServerWorldMessage::Full(world.clone()));
                let size = encoded.len().min(full.len());
                total += size;
                maximum = maximum.max(size);
                let ServerWorldMessage::Patch(decoded) = game_shared::decode(&encoded).unwrap()
                else {
                    panic!()
                };
                assert_eq!(
                    game_shared::apply_world_patch(&baseline, &decoded).unwrap(),
                    world
                );
            }
            eprintln!(
                "replication scaling: units={count} baseline_age=9 ticks avg_bytes={} max_bytes={maximum} encode_decode_us_per_update={}",
                total / 120,
                started.elapsed().as_micros() / 120
            );
            if count == 84 {
                assert!(
                    maximum < 4000,
                    "crowded-wave update exceeded measured four-slice budget"
                );
            }
        }
    }

    #[test]
    fn crowded_snapshot_delivery_survives_five_percent_datagram_loss() {
        let mut sim = Simulation::new();
        for id in 1..=8 {
            sim.add_player(id);
        }
        for _ in 0..=game_shared::WAVE_PREP_TICKS {
            sim.step();
        }
        let mut world = sim.world_delta();
        let enemy = world.enemies[0];
        world.enemies = (0..84)
            .map(|i| {
                let mut enemy = enemy;
                enemy.id += i;
                enemy.pos = [(i % 14) as f32 - 7.0, (i / 14) as f32 - 3.0];
                enemy
            })
            .collect();
        let mut server = RenetServer::new(ConnectionConfig::default());
        server.add_connection(1);
        let mut client = renet::RenetClient::new(ConnectionConfig::default());
        client.set_connected();
        let mut packet_count = 0;
        let mut delivered = 0;
        let mut wire_bytes = 0;
        let mut state = ClientNetState::default();
        let mut history = ReplicationHistory::default();
        let mut client_history = VecDeque::<WorldDelta>::new();
        let mut reverse_packets = 0;
        let ticks = 600;
        for tick in 0..ticks {
            let dt = Duration::from_secs_f64(1.0 / 30.0);
            server.update(dt);
            client.update(dt);
            world.tick += 1;
            for enemy in &mut world.enemies {
                enemy.pos[0] += 0.01;
                enemy.vel = [0.3, 0.0];
            }
            history.record(&world);
            let packet = history.packet_for(state.last_acked_tick);
            let payload = packet.payload.clone();
            state.sent(packet);
            wire_bytes += payload.len();
            server.send_message(1, DefaultChannel::Unreliable, payload);
            for packet in server.get_packets_to_send(1).unwrap() {
                packet_count += 1;
                if packet_count % 20 != 0 {
                    client.process_packet(&packet);
                }
            }
            while let Some(bytes) = client.receive_message(DefaultChannel::Unreliable) {
                let rebuilt = match game_shared::decode::<ServerWorldMessage>(&bytes).unwrap() {
                    ServerWorldMessage::Full(world) => world,
                    ServerWorldMessage::Patch(patch) => {
                        let baseline = client_history
                            .iter()
                            .find(|w| w.tick == patch.baseline_tick)
                            .expect("server must use a client-confirmed complete version");
                        game_shared::apply_world_patch(baseline, &patch).unwrap()
                    }
                };
                assert_eq!(
                    rebuilt, world,
                    "lossless restoration includes all simulation state"
                );
                client.send_message(
                    DefaultChannel::Unreliable,
                    encode(&ClientAck { tick: rebuilt.tick }),
                );
                client_history.push_back(rebuilt);
                while client_history.len() > 64 {
                    client_history.pop_front();
                }
                delivered += 1;
            }
            for packet in client.get_packets_to_send() {
                reverse_packets += 1;
                if reverse_packets % 20 != 0 {
                    server.process_packet_from(&packet, 1).unwrap();
                }
            }
            while let Some(bytes) = server.receive_message(1, DefaultChannel::Unreliable) {
                state.acknowledge(
                    game_shared::decode_with_limit::<ClientAck>(&bytes, 64)
                        .unwrap()
                        .tick,
                );
            }
            assert!(client.is_connected(), "tick {tick}");
        }
        eprintln!(
            "crowded snapshot wire: bytes/tick={} packets/tick={:.1} delivered={delivered}/{ticks}",
            wire_bytes / ticks,
            packet_count as f64 / ticks as f64
        );
        assert!(
            delivered >= ticks * 4 / 5,
            "snapshot fragmentation amplified 5% packet loss: {delivered}/{ticks} snapshots survived"
        );
    }

    #[test]
    fn hero_removal_is_separate_from_world_entity_namespace() {
        let mut sim = Simulation::new();
        sim.add_player(1_000_000);
        let before = sim.world_delta();
        sim.remove_player(1_000_000);
        let patch = build_world_patch(&sim.world_delta(), &before);
        let restored = game_shared::apply_world_patch(&before, &patch).unwrap();
        assert!(restored.heroes.is_empty());
        assert_eq!(restored.enemies, before.enemies);
    }
}

#[cfg(test)]
mod presentation_replication_tests {
    use super::*;
    #[test]
    fn patches_carry_creation_and_expiry_of_simulation_presentations() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let baseline = sim.world_delta();
        sim.queue_command(
            1,
            game_shared::ClientCommand::CastAbility {
                seq: 1,
                ability: game_shared::AbilityId::ArcBurst,
            },
        );
        sim.step();
        let active = sim.world_delta();
        let patch = build_world_patch(&active, &baseline);
        let restored = game_shared::apply_world_patch(&baseline, &patch).unwrap();
        assert_eq!(restored, active);
        assert_eq!(restored.presentations.len(), 1);
        for _ in 0..20 {
            sim.step();
        }
        let expired = sim.world_delta();
        assert!(
            game_shared::apply_world_patch(&active, &build_world_patch(&expired, &active))
                .unwrap()
                .presentations
                .is_empty()
        );
    }
}

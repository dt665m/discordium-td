//! Dreamwake's game authority, composed with the engine's server services.
mod action_trace;
mod admission;
pub use action_trace::{ActionTrace, AuthorityActionTrace, AuthorityTraceBatch};
mod authority;
pub mod plugins;
mod replication;
pub use plugins::server::DreamwakeServerPlugin;

/// Per-connection transport ceiling for Dreamwake.
/// The engine default remains independent of this game's serialized state.
pub const DEFAULT_EGRESS_BYTES_PER_SECOND: u64 = 160_000;
pub const DEFAULT_EGRESS_BURST_BYTES: u64 = 16_000;

/// Compose the headless runner with the game's authority plugin. Native hosts
/// can run this app on their server thread; tests may supply `ServerElapsed`.
pub fn build_app(shared: engine_server::SharedNet, seed: u64, lucid: bool) -> bevy::prelude::App {
    build_app_with_replication_distance(shared, seed, lucid, 80.0)
}

/// Set the game's base spatial disclosure distance. The graph still applies
/// exact bounds, hysteresis, ownership and field policy to every gathered actor.
pub fn build_app_with_replication_distance(
    shared: engine_server::SharedNet,
    seed: u64,
    lucid: bool,
    replication_distance: f32,
) -> bevy::prelude::App {
    build_app_with_replication_policy(
        shared,
        seed,
        lucid,
        replication_distance,
        engine_net::CompressionPolicy::default(),
    )
}

fn build_app_with_replication_policy(
    shared: engine_server::SharedNet,
    seed: u64,
    lucid: bool,
    replication_distance: f32,
    compression: engine_net::CompressionPolicy,
) -> bevy::prelude::App {
    use bevy::{app::ScheduleRunnerPlugin, prelude::*};
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(
        std::time::Duration::from_millis(1),
    )))
    .add_plugins(DreamwakeServerPlugin {
        shared,
        seed,
        lucid,
        replication_distance,
        compression,
    });
    app
}
/// Authorize a future replacement of an active Traveler's controller. Only a
/// subsequent authenticated connection for that same Traveler can fill this
/// bounded intent. Missing/expired replacements leave current control intact.
/// Call from trusted server code; this is deliberately absent from client RPCs.
pub fn authorize_controller_handoff(
    world: &mut bevy::prelude::World,
    current_connection: u64,
    effective_tick: u64,
) -> Result<(), String> {
    let mut driver = world
        .get_non_send_mut::<engine_server::ServerDriver<authority::DreamAuthority>>()
        .ok_or("Dreamwake server is not running")?;
    driver.authority.authorize_handoff(
        current_connection,
        engine_net::types::ServerTick(effective_tick),
    )
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum CompressionMode {
    Auto,
    Off,
}
impl CompressionMode {
    fn policy(self, minimum_bytes: usize) -> engine_net::CompressionPolicy {
        engine_net::CompressionPolicy {
            mode: match self {
                Self::Auto => engine_net::CompressionMode::Auto,
                Self::Off => engine_net::CompressionMode::Off,
            },
            minimum_bytes,
        }
    }
}

#[derive(Debug, Clone, clap::Parser)]
#[command(name = "game_server")]
pub struct ServerArgs {
    #[arg(long, hide = true)]
    pub dreamwake: bool,
    #[arg(long, alias = "dream-seed", default_value_t = 8192)]
    pub seed: u64,
    #[arg(long, alias = "dream-lucid")]
    pub lucid: bool,
    /// Base spatial actor distance in world meters (deployment configuration).
    #[arg(long, default_value = "80", value_parser = parse_replication_distance)]
    pub replication_distance: f32,
    /// Durable registration of numeric Traveler IDs and reconnect proofs.
    #[arg(long, default_value_os_t = default_identity_store())]
    pub identity_store: std::path::PathBuf,
    /// Opt-in local shot/action diagnostics. Creates a new private file, at most 8 MiB.
    #[arg(long)]
    pub action_trace: Option<std::path::PathBuf>,
    /// Compress replica bodies only when the complete encoded result is smaller.
    #[arg(
        long,
        env = "DREAMWAKE_COMPRESSION",
        value_enum,
        default_value = "auto"
    )]
    pub compression: CompressionMode,
    /// Minimum serialized replica-body bytes eligible for automatic compression.
    #[arg(long, env = "DREAMWAKE_COMPRESSION_MIN_BYTES", default_value_t = 0)]
    pub compression_min_bytes: usize,
    #[command(flatten)]
    pub network: engine_server::ServerConfig,
}

impl ServerArgs {
    fn apply_network_defaults(&mut self) {
        self.network
            .egress
            .bytes_per_second
            .get_or_insert(DEFAULT_EGRESS_BYTES_PER_SECOND);
        self.network
            .egress
            .burst_bytes
            .get_or_insert(DEFAULT_EGRESS_BURST_BYTES);
    }
}

fn default_identity_store() -> std::path::PathBuf {
    use std::path::PathBuf;
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
    };
    base.unwrap_or_else(|| PathBuf::from("."))
        .join("Dreamwake/server-identities.bin")
}

fn parse_replication_distance(value: &str) -> Result<f32, String> {
    let distance: f32 = value.parse().map_err(|_| "expected a distance in meters")?;
    if !distance.is_finite() || !(4.0..=80.0).contains(&distance) {
        return Err("replication distance must be between 4 and 80 meters".into());
    }
    Ok(distance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn game_network_defaults_preserve_explicit_engine_values() {
        let mut defaults = ServerArgs::try_parse_from(["server"]).unwrap();
        defaults.apply_network_defaults();
        assert_eq!(
            defaults.network.egress.bytes_per_second,
            Some(DEFAULT_EGRESS_BYTES_PER_SECOND)
        );
        assert_eq!(
            defaults.network.egress.burst_bytes,
            Some(DEFAULT_EGRESS_BURST_BYTES)
        );
        let mut explicit = ServerArgs::try_parse_from([
            "server",
            "--egress-bytes-per-second",
            "60000",
            "--egress-burst-bytes",
            "6000",
        ])
        .unwrap();
        explicit.apply_network_defaults();
        assert_eq!(explicit.network.egress.bytes_per_second, Some(60000));
        assert_eq!(explicit.network.egress.burst_bytes, Some(6000));
    }

    #[test]
    fn server_compression_cli_defaults_and_overrides() {
        let defaults = ServerArgs::try_parse_from(["server"]).unwrap();
        assert_eq!(
            defaults.compression.policy(defaults.compression_min_bytes),
            engine_net::CompressionPolicy::default()
        );
        let configured = ServerArgs::try_parse_from([
            "server",
            "--compression",
            "off",
            "--compression-min-bytes",
            "128",
        ])
        .unwrap();
        assert_eq!(
            configured
                .compression
                .policy(configured.compression_min_bytes),
            engine_net::CompressionPolicy {
                mode: engine_net::CompressionMode::Off,
                minimum_bytes: 128
            }
        );
        assert!(ServerArgs::try_parse_from(["server", "--compression", "invalid"]).is_err());
        assert!(ServerArgs::try_parse_from(["server", "--compression-min-bytes", "-1"]).is_err());
    }

    #[test]
    fn default_game_and_host_cli_keep_configurable_admission() {
        let args = ServerArgs::try_parse_from(["server"]).unwrap();
        assert_eq!(args.seed, 8192);
        assert!(!args.lucid);
        assert_eq!(args.network.max_clients, 8);
        assert_eq!(args.replication_distance, 80.0);
        let args = ServerArgs::try_parse_from([
            "server",
            "--seed",
            "12",
            "--lucid",
            "--max-clients",
            "64",
        ])
        .unwrap();
        assert_eq!(args.seed, 12);
        assert!(args.lucid);
        assert_eq!(args.network.max_clients, 64);
        for invalid in ["0", "1025"] {
            assert!(ServerArgs::try_parse_from(["server", "--max-clients", invalid]).is_err());
        }
        for invalid in ["NaN", "inf", "0", "81"] {
            assert!(
                ServerArgs::try_parse_from(["server", "--replication-distance", invalid]).is_err()
            );
        }
        assert_eq!(
            ServerArgs::try_parse_from(["server", "--replication-distance", "12"])
                .unwrap()
                .replication_distance,
            12.0
        );
    }
}
pub fn run(mut args: ServerArgs) -> Result<(), Box<dyn std::error::Error>> {
    let trace_file = args
        .action_trace
        .as_ref()
        .map(|path| engine_net::trace::TraceFile::create(path, 8 * 1024 * 1024))
        .transpose()?;
    args.apply_network_defaults();
    let shared = engine_server::start_with_admission(
        args.network,
        dreamwake_protocol::PROTOCOL_ID,
        admission::GuestAdmission::open(args.identity_store)?,
    )?;
    let mut app = build_app_with_replication_policy(
        shared,
        args.seed,
        args.lucid,
        args.replication_distance,
        args.compression.policy(args.compression_min_bytes),
    );
    if let Some(file) = trace_file {
        plugins::server::trace::install(&mut app, file)?;
    }
    engine_server::run_app(app).map_err(|e| std::io::Error::other(e).into())
}

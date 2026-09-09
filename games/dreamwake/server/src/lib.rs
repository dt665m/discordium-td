//! Dreamwake's game authority, composed with the engine's server services.
mod authority;
pub use authority::DreamwakeServerPlugin;

/// Compose the headless runner with the game's authority plugin. Native hosts
/// can run this app on their server thread; tests may supply `ServerElapsed`.
pub fn build_app(shared: engine_server::SharedNet, seed: u64, lucid: bool) -> bevy::prelude::App {
    use bevy::{app::ScheduleRunnerPlugin, prelude::*};
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(
        std::time::Duration::from_millis(1),
    )))
    .add_plugins(DreamwakeServerPlugin {
        shared,
        seed,
        lucid,
    });
    app
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
    #[command(flatten)]
    pub network: engine_server::ServerConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn default_game_and_host_cli_keep_configurable_admission() {
        let args = ServerArgs::try_parse_from(["server"]).unwrap();
        assert_eq!(args.seed, 8192);
        assert!(!args.lucid);
        assert_eq!(args.network.max_clients, 8);
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
    }
}
pub fn run(args: ServerArgs) -> Result<(), Box<dyn std::error::Error>> {
    let shared = engine_server::start(args.network, dreamwake_protocol::PROTOCOL_ID)?;
    engine_server::runtime::run_app(build_app(shared, args.seed, args.lucid))
        .map_err(|e| std::io::Error::other(e).into())
}

//! Dreamwake's game authority, composed with the engine's server services.
mod authority;
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
    authority::run_dreamwake(shared, args.seed, args.lucid)
        .map_err(|e| std::io::Error::other(e).into())
}

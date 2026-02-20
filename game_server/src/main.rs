use clap::Parser;

use game_server::{ServerArgs, run};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = ServerArgs::parse();
    run(args)
}

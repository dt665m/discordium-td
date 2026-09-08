//! Repeatable capacity eval; run via scripts/run-netcode-eval.sh.
#[path = "netcode_eval/network.rs"]
mod network;
#[path = "../src/replication.rs"]
mod replication;
#[path = "netcode_eval/workload.rs"]
mod workload;
use clap::Parser;
use serde::Serialize;
use std::{path::PathBuf, time::Instant};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "target/reports/netcode-eval/latest.json")]
    output: PathBuf,
    #[arg(long, default_value_t = 0x8e6e_7a21)]
    seed: u32,
    #[arg(long, default_value_t=180, value_parser=clap::value_parser!(u32).range(120..=3600))]
    ticks: u32,
    #[arg(long, default_value_t=3, value_parser=clap::value_parser!(u8).range(1..=10))]
    repetitions: u8,
    #[arg(long, value_delimiter = ',', default_value = "1,8,32")]
    players: Vec<usize>,
    #[arg(long, value_delimiter = ',', default_value = "84,256,1024")]
    entities: Vec<usize>,
}
#[derive(Serialize)]
struct Report {
    schema_version: u32,
    players: Vec<usize>,
    entities: Vec<usize>,
    seed: u32,
    warmup_ticks: usize,
    protocol: u64,
    profile: String,
    target: String,
    repetitions: u8,
    ticks: u32,
    elapsed_seconds: f64,
    cases: Vec<serde_json::Value>,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.players.is_empty()
        || args.entities.is_empty()
        || args.players.iter().any(|n| !(1..=1024).contains(n))
        || args.entities.iter().any(|n| !(1..=4096).contains(n))
    {
        return Err("players must be 1..1024, entities 1..4096".into());
    }
    let started = Instant::now();
    let mut cases = Vec::new();
    for &players in &args.players {
        for &entities in &args.entities {
            for repetition in 0..args.repetitions {
                let work = workload::generate(players, entities, args.ticks as usize);
                for condition in [network::Condition::Clean, network::Condition::MixedLoss] {
                    let mut result =
                        network::evaluate(&work.worlds, players, condition, args.seed)?;
                    result.simulation_us = network::distribution(&work.simulation_us[60..]);
                    let row = serde_json::json!({"players":players,"entities":entities,"repetition":repetition,"condition":condition,"workload_signature":format!("{:016x}",work.signature),"result":result});
                    eprintln!(
                        "players={players} entities={entities} condition={condition:?} repetition={repetition}: {} bytes/client/update, server p95={}us",
                        result.mean_payload_bytes, result.replication_us.p95
                    );
                    cases.push(row);
                }
            }
        }
    }
    let report = Report {
        schema_version: 2,
        players: args.players.clone(),
        entities: args.entities.clone(),
        seed: args.seed,
        warmup_ticks: 60,
        protocol: game_shared::PROTOCOL_ID,
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .into(),
        target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        repetitions: args.repetitions,
        ticks: args.ticks,
        elapsed_seconds: started.elapsed().as_secs_f64(),
        cases,
    };
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.output, serde_json::to_vec_pretty(&report)?)?;
    println!("Eval report: {}", args.output.display());
    Ok(())
}

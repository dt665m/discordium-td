//! Focused two-thread decode/handoff comparison, complementing live server tests.
//! Batches rendezvous outside timed regions, so this measures ownership/decode
//! costs and cross-core handoff, not saturation throughput or mutex contention.
//! Run: cargo run --release -p game_server --example ingress_profile -- --output report.json
#[allow(dead_code)]
#[path = "../src/network/ingress.rs"]
mod ingress;

use bytes::Bytes;
use clap::Parser;
use game_shared::{ClientAction, ClientCommand, ClientMoveBundle, decode_with_limit, encode};
use ingress::{DecodedInput, Ingress, decode_input, ingress_channel};
use serde::Serialize;
use std::{
    hint::black_box,
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 1000)]
    batches: usize,
    #[arg(long, default_value_t = 100)]
    warmup: usize,
    #[arg(long, default_value_t = 5)]
    repetitions: usize,
    #[arg(long)]
    output: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    MutexCopyDoubleDecode,
    MutexBytesDoubleDecode,
    MutexPrepared,
    SpscPrepared,
}

enum Batch {
    Empty,
    Copied(Vec<(u64, Vec<u8>)>),
    Shared(Vec<(u64, Bytes)>),
    Prepared(Vec<Ingress>),
}

#[derive(Serialize)]
struct Distribution {
    p50: f64,
    p95: f64,
    mean: f64,
}

impl Distribution {
    fn from(mut values: Vec<f64>) -> Self {
        values.sort_by(f64::total_cmp);
        Self {
            p50: values[values.len() / 2],
            p95: values[(values.len() - 1) * 95 / 100],
            mean: values.iter().sum::<f64>() / values.len() as f64,
        }
    }
}

#[derive(Serialize)]
struct ResultRow {
    mode: Mode,
    repetition: usize,
    players: usize,
    moves_per_bundle: usize,
    actions_per_bundle: usize,
    messages_per_batch: usize,
    payload_bytes_per_batch: usize,
    producer_us_per_batch: Distribution,
    consumer_us_per_batch: Distribution,
    checksum: u64,
}

fn prepared(id: u64, payload: &[u8]) -> Ingress {
    let DecodedInput::Command(command) = decode_input(id, false, payload) else {
        panic!("valid benchmark command")
    };
    command
}

fn consume(id: u64, bundle: ClientMoveBundle) -> u64 {
    bundle
        .moves
        .iter()
        .fold(id, |sum, (seq, _)| sum.wrapping_add(*seq as u64))
        .wrapping_add(bundle.actions.iter().map(|a| a.command.seq() as u64).sum())
}

fn consume_prepared(command: Ingress) -> u64 {
    let Ingress::Moves(id, bundle) = command else {
        panic!("movement workload")
    };
    consume(id, bundle)
}

fn run(args: &Args, mode: Mode, repetition: usize, players: usize, redundant: bool) -> ResultRow {
    let moves = if redundant { 16 } else { 1 };
    let actions = if redundant { 4 } else { 0 };
    let payloads: Vec<(u64, Bytes)> = (1..=players as u64)
        .flat_map(|id| {
            (0..4).map(move |packet| {
                let bundle = ClientMoveBundle {
                    match_epoch: 7,
                    moves: (0..moves as u32)
                        .map(|n| (packet * 100 + n, [0.25, -0.75]))
                        .collect(),
                    actions: (0..actions as u32)
                        .map(|n| ClientAction {
                            match_epoch: 7,
                            command: ClientCommand::BasicAttack {
                                seq: packet * 100 + 20 + n,
                            },
                            after_move_seq: Some(packet * 100),
                            view_tick: Some(10),
                        })
                        .collect(),
                };
                (id, Bytes::from(encode(&bundle)))
            })
        })
        .collect();
    let count = payloads.len();
    let bytes = payloads.iter().map(|(_, p)| p.len()).sum();
    let expected: u64 = payloads
        .iter()
        .map(|(id, p)| consume_prepared(prepared(*id, p)))
        .sum();
    let mailbox = Arc::new(Mutex::new(Batch::Empty));
    let writer_mailbox = Arc::clone(&mailbox);
    let (mut sender, mut receiver) = ingress_channel(count);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let mut consumer_times = Vec::with_capacity(args.batches);
    let producer_times = std::thread::scope(|scope| {
        let producer = scope.spawn(move || {
            let mut samples = Vec::with_capacity(args.batches);
            for round in 0..args.warmup + args.batches {
                // Renet already owns its Bytes. Header cloning to recreate that
                // starting state is deliberately outside the timed operation.
                let incoming = payloads.clone();
                let started = Instant::now();
                match mode {
                    Mode::MutexCopyDoubleDecode => {
                        let entries = incoming
                            .into_iter()
                            .map(|(id, bytes)| {
                                // Previous worker decoded to distinguish bundles from ACKs,
                                // then copied bytes and discarded the decoded bundle.
                                black_box(
                                    decode_with_limit::<ClientMoveBundle>(&bytes, 2048).unwrap(),
                                );
                                (id, bytes.to_vec())
                            })
                            .collect();
                        *writer_mailbox.lock().unwrap() = Batch::Copied(entries);
                    }
                    Mode::MutexBytesDoubleDecode => {
                        let entries = incoming
                            .into_iter()
                            .map(|(id, bytes)| {
                                black_box(
                                    decode_with_limit::<ClientMoveBundle>(&bytes, 2048).unwrap(),
                                );
                                (id, bytes)
                            })
                            .collect();
                        *writer_mailbox.lock().unwrap() = Batch::Shared(entries);
                    }
                    Mode::MutexPrepared => {
                        let entries = incoming
                            .into_iter()
                            .map(|(id, bytes)| prepared(id, &bytes))
                            .collect();
                        *writer_mailbox.lock().unwrap() = Batch::Prepared(entries);
                    }
                    Mode::SpscPrepared => {
                        for (id, bytes) in incoming {
                            assert!(sender.try_send(prepared(id, &bytes)).is_ok());
                        }
                    }
                }
                let elapsed = started.elapsed().as_secs_f64() * 1e6;
                if round >= args.warmup {
                    samples.push(elapsed);
                }
                ready_tx.send(()).unwrap();
                done_rx.recv().unwrap();
            }
            samples
        });
        for round in 0..args.warmup + args.batches {
            ready_rx.recv().unwrap();
            let started = Instant::now();
            let checksum: u64 = if matches!(mode, Mode::SpscPrepared) {
                assert_eq!(receiver.pending(), count);
                (0..count)
                    .map(|_| consume_prepared(receiver.try_recv().unwrap()))
                    .sum()
            } else {
                let batch = std::mem::replace(&mut *mailbox.lock().unwrap(), Batch::Empty);
                match batch {
                    Batch::Copied(entries) => entries
                        .into_iter()
                        .map(|(id, bytes)| consume(id, decode_with_limit(&bytes, 2048).unwrap()))
                        .sum(),
                    Batch::Shared(entries) => entries
                        .into_iter()
                        .map(|(id, bytes)| consume(id, decode_with_limit(&bytes, 2048).unwrap()))
                        .sum(),
                    Batch::Prepared(entries) => entries.into_iter().map(consume_prepared).sum(),
                    Batch::Empty => panic!("missing benchmark batch"),
                }
            };
            black_box(checksum);
            let elapsed = started.elapsed().as_secs_f64() * 1e6;
            if round >= args.warmup {
                consumer_times.push(elapsed);
            }
            assert_eq!(
                checksum, expected,
                "every mode must consume identical input"
            );
            done_tx.send(()).unwrap();
        }
        producer.join().unwrap()
    });
    ResultRow {
        mode,
        repetition,
        players,
        moves_per_bundle: moves,
        actions_per_bundle: actions,
        messages_per_batch: count,
        payload_bytes_per_batch: bytes,
        producer_us_per_batch: Distribution::from(producer_times),
        consumer_us_per_batch: Distribution::from(consumer_times),
        checksum: expected,
    }
}

fn main() {
    let args = Args::parse();
    assert!(args.batches > 0 && args.repetitions > 0);
    let mut rows = Vec::new();
    for repetition in 0..args.repetitions {
        for players in [1, 8, 32] {
            for redundant in [false, true] {
                let mut modes = [
                    Mode::MutexCopyDoubleDecode,
                    Mode::MutexBytesDoubleDecode,
                    Mode::MutexPrepared,
                    Mode::SpscPrepared,
                ];
                // Alternate run order to reduce systematic thermal/order bias.
                if repetition % 2 == 1 {
                    modes.reverse();
                }
                for mode in modes {
                    rows.push(run(&args, mode, repetition, players, redundant));
                }
            }
        }
    }
    let report = serde_json::json!({
        "schema_version": 1, "batches": args.batches, "warmup": args.warmup,
        "repetitions": args.repetitions, "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS, "rows": rows,
    });
    let text = serde_json::to_string_pretty(&report).unwrap();
    if let Some(path) = args.output {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    } else {
        println!("{text}");
    }
}

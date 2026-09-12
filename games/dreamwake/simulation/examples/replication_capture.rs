//! Bounded current-state capture probe; not a server throughput qualification.
use dreamwake_sim::{DreamInput, DreamSimulation, replication::ReplicationStamp};
use std::time::Instant;

fn main() {
    let mut sim = DreamSimulation::new(8192, false);
    assert!(sim.add_player(2));
    sim.continue_run();
    assert!(sim.continue_run_for(2));
    for _ in 0..64 {
        sim.step_multiplayer(&[(1, DreamInput::default()), (2, DreamInput::default())]);
    }
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.tick, 64);
    assert_eq!(
        snapshot.covers.len(),
        dreamwake_sim::DEMO_COVER_POSITIONS.len()
    );
    assert!(snapshot.enemies.len() >= dreamwake_sim::AMBIENT_ENEMY_POSITIONS.len());
    let mut micros = Vec::with_capacity(120);
    for revision in 1..=120 {
        let start = Instant::now();
        let capture = sim
            .capture_replication(ReplicationStamp {
                match_epoch: 1,
                server_tick: 1000,
                gameplay_tick: sim.gameplay_tick(),
                scene_revision: 1,
                revision,
            })
            .expect("valid committed replication capture");
        std::hint::black_box(capture);
        micros.push(start.elapsed().as_micros());
    }
    micros.sort_unstable();
    println!(
        "{}",
        serde_json::json!({
            "kind": "current_replication_capture_cpu",
            "captures": micros.len(),
            "gameplay_tick": sim.gameplay_tick(),
            "heroes": snapshot.heroes.len(),
            "enemies": snapshot.enemies.len(),
            "covers": snapshot.covers.len(),
            "mean_micros": micros.iter().sum::<u128>() as f64 / micros.len() as f64,
            "p50_micros": micros[59],
            "p95_micros": micros[113],
            "p99_micros": micros[118],
            "max_micros": micros[119],
            "debug_assertions": cfg!(debug_assertions),
            "scope": "Isolated current-state capture, excluding simulation, per-peer gather, encoding, transport and renderer."
        })
    );
}

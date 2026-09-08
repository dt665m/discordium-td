//! Replay a server NDJSON capture without starting a server.
//! cargo run --release -p game_server --example replication_profile -- capture/server.ndjson
#[path = "../src/replication.rs"]
mod replication;
use game_shared::{ServerWorldMessage, WorldDelta, apply_world_patch, decode};
use replication::{ClientNetState, ReplicationHistory};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{BufRead, BufReader},
    sync::Arc,
    time::{Duration, Instant},
};
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("provide a server.ndjson capture");
    let mut worlds = Vec::new();
    for line in BufReader::new(File::open(path).unwrap()).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(client) = value["frame"]["clients"]
            .as_array()
            .and_then(|clients| clients.first())
        else {
            continue;
        };
        let mut world: WorldDelta = serde_json::from_value(client["world"].clone()).unwrap();
        world.heroes.sort_by_key(|h| h.client_id);
        world.enemies.sort_by_key(|e| e.id);
        world.towers.sort_by_key(|t| t.id);
        if worlds
            .last()
            .is_some_and(|last: &WorldDelta| last.tick == world.tick)
        {
            continue;
        }
        worlds.push(world);
    }
    for ages in [
        vec![2],
        vec![9],
        vec![18],
        vec![2, 2, 2, 2],
        vec![2, 9, 18, 60],
    ] {
        let mut history = ReplicationHistory::default();
        let mut clients: HashMap<_, _> = (0..ages.len())
            .map(|n| (n as u64, ClientNetState::default()))
            .collect();
        let mut sent: Vec<VecDeque<_>> = (0..ages.len()).map(|_| VecDeque::new()).collect();
        let mut encode_time = Duration::ZERO;
        let mut decode_time = Duration::ZERO;
        let mut bytes = 0usize;
        let mut max_bytes = 0;
        let mut full = 0;
        let mut samples = 0;
        let mut retained = VecDeque::new();
        for (index, world) in worlds.iter().enumerate() {
            // Complete versions are acknowledged after the specified delay.
            let start = Instant::now();
            for (peer, age) in ages.iter().enumerate() {
                while sent[peer]
                    .front()
                    .is_some_and(|(at, _)| index - *at >= *age)
                {
                    let (_, tick) = sent[peer].pop_front().unwrap();
                    clients.get_mut(&(peer as u64)).unwrap().acknowledge(tick);
                }
            }
            history.record(world);
            let baselines: Vec<_> = (0..ages.len())
                .map(|peer| clients[&(peer as u64)].last_acked_tick)
                .collect();
            history.prepare_packets(&baselines);
            let packets: Vec<_> = (0..ages.len())
                .map(|peer| {
                    let client = clients.get_mut(&(peer as u64)).unwrap();
                    let packet = history.packet_for(client.last_acked_tick);
                    client.sent(Arc::clone(&packet));
                    sent[peer].push_back((index, packet.tick));
                    packet
                })
                .collect();
            history.retire_acknowledged(&clients);
            let elapsed = start.elapsed();
            let measured = (7..=12).contains(&world.wave);
            if measured {
                encode_time += elapsed;
            }
            for packet in packets {
                let start = Instant::now();
                let restored = match decode(&packet.payload).unwrap() {
                    ServerWorldMessage::Full(w) => w,
                    ServerWorldMessage::Patch(p) => {
                        let baseline = retained
                            .iter()
                            .find(|w: &&WorldDelta| w.tick == p.baseline_tick)
                            .unwrap();
                        apply_world_patch(baseline, &p).unwrap()
                    }
                };
                if measured {
                    decode_time += start.elapsed();
                }
                assert_eq!(&restored, world, "tick {}", world.tick);
                if measured {
                    bytes += packet.payload.len();
                    max_bytes = max_bytes.max(packet.payload.len());
                    full += usize::from(packet.baseline_tick.is_none());
                    samples += 1;
                }
            }
            retained.push_back(world.clone());
            while retained.len() > 64 {
                retained.pop_front();
            }
        }
        println!(
            "{}",
            serde_json::json!({"ages":ages,"samples":samples,"avg_bytes":bytes/samples.max(1),"max_bytes":max_bytes,"full":full,"server_us_per_tick":encode_time.as_micros() as f64/(samples/ages.len()).max(1)as f64,"decode_us_per_packet":decode_time.as_micros()as f64/samples.max(1)as f64})
        );
    }
}

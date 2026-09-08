use crate::replication::{ClientNetState, ReplicationHistory};
use game_shared::{
    ClientAck, ServerWorldMessage, WorldDelta, apply_world_patch, decode, encode,
    is_newer_input_seq,
};
use renet::{ConnectionConfig, DefaultChannel, RenetClient, RenetServer};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

type EvalResult<T> = Result<T, Box<dyn std::error::Error>>;
#[derive(Clone, Copy, Debug, Serialize)]
pub enum Condition {
    Clean,
    MixedLoss,
}
#[derive(Default, Serialize)]
pub struct Distribution {
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}
pub fn distribution(values: &[f64]) -> Distribution {
    if values.is_empty() {
        return Distribution::default();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Distribution {
        mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
        p50: sorted[(sorted.len() - 1) / 2],
        p95: sorted[((sorted.len() * 95).div_ceil(100) - 1).min(sorted.len() - 1)],
        max: *sorted.last().unwrap(),
    }
}
#[derive(Default, Serialize)]
pub struct Metrics {
    pub simulation_us: Distribution,
    pub replication_us: Distribution,
    pub transport_us: Distribution,
    pub decode_us: Distribution,
    pub measured_ticks: usize,
    pub mean_payload_bytes: f64,
    pub max_payload_bytes: usize,
    pub server_payload_kib_per_second: f64,
    pub server_renet_kib_per_second: f64,
    pub server_renet_packets_per_tick: f64,
    pub full_snapshots: usize,
    pub offered_updates: usize,
    pub received_updates: usize,
    pub baseline_age: Distribution,
    pub measured_snapshot_age_ticks: Distribution,
    pub minimum_client_delivery_ratio: f64,
    pub peak_accounted_history_bytes: usize,
    pub max_outstanding_updates: usize,
    pub recovered_clients: usize,
    pub max_final_tick_lag: u32,
    pub dropped_datagrams: usize,
}
struct Peer {
    client: RenetClient,
    known: VecDeque<WorldDelta>,
    forward: Vec<(usize, Vec<u8>)>,
    reverse: Vec<(usize, Vec<u8>)>,
    random: u32,
}
fn schedule(
    queue: &mut Vec<(usize, Vec<u8>)>,
    random: &mut u32,
    packet: Vec<u8>,
    step: usize,
    peer: usize,
    condition: Condition,
    recovery: bool,
) -> bool {
    *random = random.wrapping_mul(1664525).wrapping_add(1013904223);
    let impaired = matches!(condition, Condition::MixedLoss) && !recovery;
    if impaired && ((*random >> 16) % 100 < 5 || (peer == 0 && (90..120).contains(&step))) {
        return false;
    }
    let delay = if impaired {
        2 + peer % 5 + ((*random >> 8) % 3) as usize
    } else {
        0
    };
    queue.push((step + delay, packet));
    true
}
fn due(queue: &mut Vec<(usize, Vec<u8>)>, step: usize) -> Vec<Vec<u8>> {
    let mut ready = Vec::new();
    let mut pending = Vec::new();
    for (at, packet) in queue.drain(..) {
        if at <= step {
            ready.push(packet)
        } else {
            pending.push((at, packet))
        }
    }
    *queue = pending;
    ready
}

pub fn evaluate(
    worlds: &[WorldDelta],
    players: usize,
    condition: Condition,
    seed: u32,
) -> EvalResult<Metrics> {
    if worlds.len() < 120 || players == 0 {
        return Err("eval requires >=120 worlds and at least one client".into());
    }
    let mut server = RenetServer::new(ConnectionConfig::default());
    let mut peers = Vec::new();
    let mut clients = HashMap::new();
    for peer in 0..players {
        server.add_connection(peer as u64 + 1);
        let mut client = RenetClient::new(ConnectionConfig::default());
        client.set_connected();
        peers.push(Peer {
            client,
            known: VecDeque::new(),
            forward: Vec::new(),
            reverse: Vec::new(),
            random: seed.wrapping_add(peer as u32 * 17),
        });
        clients.insert(peer as u64 + 1, ClientNetState::default());
    }
    let mut history = ReplicationHistory::default();
    let mut metrics = Metrics::default();
    let mut timings = Vec::new();
    let mut transports = Vec::new();
    let mut decodes = Vec::new();
    let mut ages = Vec::new();
    let mut snapshot_ages = Vec::new();
    let mut deliveries_per_client = vec![0usize; players];
    let mut payload_bytes = 0;
    let mut packet_bytes = 0;
    let mut packet_count = 0;
    let mut expected = Vec::with_capacity(worlds.len() + 128);
    for step in 0..worlds.len() + 128 {
        let recovery = step >= worlds.len();
        let measured = (60..worlds.len()).contains(&step);
        let mut world = worlds[step.min(worlds.len() - 1)].clone();
        if recovery {
            world.tick = world.tick.wrapping_add((step - worlds.len() + 1) as u32);
        }
        expected.push(world.clone());
        let transport_start = Instant::now();
        server.update(Duration::from_secs_f64(1.0 / 30.0));
        for (index, peer) in peers.iter_mut().enumerate() {
            peer.client.update(Duration::from_secs_f64(1.0 / 30.0));
            for packet in due(&mut peer.reverse, step) {
                server.process_packet_from(&packet, index as u64 + 1)?;
            }
            while let Some(bytes) =
                server.receive_message(index as u64 + 1, DefaultChannel::Unreliable)
            {
                clients
                    .get_mut(&(index as u64 + 1))
                    .unwrap()
                    .acknowledge(decode::<ClientAck>(&bytes)?.tick);
            }
        }
        let mut transport_time = transport_start.elapsed();
        let start = Instant::now();
        history.record(&world);
        let baselines: Vec<_> = (0..players)
            .map(|peer| clients[&(peer as u64 + 1)].last_acked_tick)
            .collect();
        history.prepare_packets(&baselines);
        let mut packets = Vec::with_capacity(players);
        for index in 0..players {
            let client = clients.get_mut(&(index as u64 + 1)).unwrap();
            let packet = history.packet_for(client.last_acked_tick);
            client.sent(Arc::clone(&packet));
            metrics.max_outstanding_updates = metrics
                .max_outstanding_updates
                .max(client.outstanding.len());
            if measured {
                metrics.offered_updates += 1;
                payload_bytes += packet.payload.len();
                metrics.max_payload_bytes = metrics.max_payload_bytes.max(packet.payload.len());
                if let Some(baseline) = packet.baseline_tick {
                    ages.push(world.tick.wrapping_sub(baseline) as f64);
                } else {
                    metrics.full_snapshots += 1;
                }
            }
            packets.push(packet);
        }
        history.retire_acknowledged(&clients);
        let replication_time = start.elapsed();
        metrics.peak_accounted_history_bytes = metrics
            .peak_accounted_history_bytes
            .max(history.memory_bytes());
        let start = Instant::now();
        for (index, packet) in packets.into_iter().enumerate() {
            server.send_message(
                index as u64 + 1,
                DefaultChannel::Unreliable,
                packet.payload.clone(),
            );
            for datagram in server.get_packets_to_send(index as u64 + 1)? {
                if measured {
                    packet_bytes += datagram.len();
                    packet_count += 1;
                }
                let peer = &mut peers[index];
                if !schedule(
                    &mut peer.forward,
                    &mut peer.random,
                    datagram,
                    step,
                    index,
                    condition,
                    recovery,
                ) {
                    metrics.dropped_datagrams += 1;
                }
            }
        }
        transport_time += start.elapsed();
        for (index, peer) in peers.iter_mut().enumerate() {
            let start = Instant::now();
            for packet in due(&mut peer.forward, step) {
                peer.client.process_packet(&packet);
            }
            transport_time += start.elapsed();
            while let Some(bytes) = peer.client.receive_message(DefaultChannel::Unreliable) {
                let start = Instant::now();
                let message: ServerWorldMessage = decode(&bytes)?;
                let tick = match &message {
                    ServerWorldMessage::Full(w) => w.tick,
                    ServerWorldMessage::Patch(p) => p.tick,
                };
                if peer
                    .known
                    .back()
                    .is_some_and(|last| !is_newer_input_seq(tick, last.tick))
                {
                    continue;
                }
                let restored = match message {
                    ServerWorldMessage::Full(w) => w,
                    ServerWorldMessage::Patch(p) => {
                        let baseline = peer
                            .known
                            .iter()
                            .find(|w| w.tick == p.baseline_tick)
                            .ok_or("missing confirmed baseline")?;
                        apply_world_patch(baseline, &p)?
                    }
                };
                if measured {
                    decodes.push(start.elapsed().as_secs_f64() * 1e6);
                    metrics.received_updates += 1;
                    deliveries_per_client[index] += 1;
                }
                let actual = expected
                    .iter()
                    .rev()
                    .find(|w| w.tick == restored.tick)
                    .ok_or("received unknown tick")?;
                if &restored != actual {
                    return Err(format!("peer {index} diverged at tick {}", restored.tick).into());
                }
                peer.client.send_message(
                    DefaultChannel::Unreliable,
                    encode(&ClientAck {
                        tick: restored.tick,
                    }),
                );
                peer.known.push_back(restored);
                while peer.known.len() > 64 {
                    peer.known.pop_front();
                }
            }
            if measured {
                // Missing authority is visible even if frozen recovery later succeeds.
                let age = peer
                    .known
                    .back()
                    .map_or(step + 1, |last| world.tick.wrapping_sub(last.tick) as usize);
                snapshot_ages.push(age as f64);
            }
            let start = Instant::now();
            for datagram in peer.client.get_packets_to_send() {
                if !schedule(
                    &mut peer.reverse,
                    &mut peer.random,
                    datagram,
                    step,
                    index,
                    condition,
                    recovery,
                ) {
                    metrics.dropped_datagrams += 1;
                }
            }
            if !peer.client.is_connected() {
                return Err(format!("peer {index} disconnected").into());
            }
            transport_time += start.elapsed();
        }
        if measured {
            metrics.measured_ticks += 1;
            timings.push(replication_time.as_secs_f64() * 1e6);
            transports.push(transport_time.as_secs_f64() * 1e6);
        }
    }
    let final_tick = expected.last().unwrap().tick;
    for peer in &peers {
        let last = peer
            .known
            .back()
            .ok_or("client never reconstructed a snapshot")?;
        let lag = final_tick.wrapping_sub(last.tick);
        metrics.max_final_tick_lag = metrics.max_final_tick_lag.max(lag);
        if lag > 2 {
            return Err(format!("failed to recover after impairment: {lag} ticks behind").into());
        }
        metrics.recovered_clients += 1;
    }
    metrics.mean_payload_bytes = payload_bytes as f64 / metrics.offered_updates as f64;
    metrics.server_payload_kib_per_second =
        payload_bytes as f64 / metrics.measured_ticks as f64 * 30.0 / 1024.0;
    metrics.server_renet_kib_per_second =
        packet_bytes as f64 / metrics.measured_ticks as f64 * 30.0 / 1024.0;
    metrics.server_renet_packets_per_tick = packet_count as f64 / metrics.measured_ticks as f64;
    metrics.replication_us = distribution(&timings);
    metrics.transport_us = distribution(&transports);
    metrics.decode_us = distribution(&decodes);
    metrics.baseline_age = distribution(&ages);
    metrics.measured_snapshot_age_ticks = distribution(&snapshot_ages);
    metrics.minimum_client_delivery_ratio =
        *deliveries_per_client.iter().min().unwrap() as f64 / metrics.measured_ticks as f64;
    if metrics.minimum_client_delivery_ratio == 0.0 {
        return Err("a client received no snapshots during measured combat".into());
    }
    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eval_exercises_real_fragmentation_loss_and_recovery() {
        let work = crate::workload::generate(2, 84, 140);
        let clean = evaluate(&work.worlds, 2, Condition::Clean, 7).unwrap();
        let lossy = evaluate(&work.worlds, 2, Condition::MixedLoss, 7).unwrap();
        assert_eq!(clean.measured_ticks, 80);
        assert_eq!(clean.offered_updates, 160);
        assert_eq!(clean.received_updates, 160);
        assert_eq!(clean.dropped_datagrams, 0);
        assert!(lossy.dropped_datagrams > 0);
        assert!(lossy.received_updates < clean.received_updates);
        assert!(lossy.baseline_age.max > clean.baseline_age.max);
        assert!(lossy.measured_snapshot_age_ticks.max > clean.measured_snapshot_age_ticks.max);
        assert!(
            lossy.minimum_client_delivery_ratio > 0.0 && lossy.minimum_client_delivery_ratio < 1.0
        );
        assert_eq!(lossy.recovered_clients, 2);
        assert!(lossy.max_outstanding_updates <= 64);
        let repeat = evaluate(&work.worlds, 2, Condition::MixedLoss, 7).unwrap();
        assert_eq!(lossy.mean_payload_bytes, repeat.mean_payload_bytes);
        assert_eq!(lossy.dropped_datagrams, repeat.dropped_datagrams);
    }
}

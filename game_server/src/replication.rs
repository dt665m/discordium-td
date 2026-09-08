//! Shared frame/change history and cached payloads, plus small per-client ghosts.
//! Loss never advances a ghost's known version. Recovery unions missed changes
//! and serializes their current values; equivalent frame ranges reuse one packet.
use bevy::prelude::Resource;
use bytes::Bytes;
use game_shared::{
    ServerWorldMessage, WorldDelta, encode, is_newer_input_seq,
    replication::{
        ChangeSet, ReplicationState, capture_world, changed_records, patch_from_changes,
        union_changes,
    },
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

pub(crate) const HISTORY_FRAMES: usize = 64;
const MAX_BASELINE_AGE: u32 = 60;
const MAX_HISTORY_BYTES: usize = 32 * 1024 * 1024;

pub(crate) struct ReplicationPacket {
    pub tick: u32,
    pub baseline_tick: Option<u32>,
    pub payload: Bytes,
}

#[derive(Default)]
pub(crate) struct ClientNetState {
    // The same worker-owned peer record holds admission and replication state.
    #[allow(
        dead_code,
        reason = "replication-only profiling examples do not admit input"
    )]
    pub ingress_counts: [usize; 2],
    pub last_acked_tick: Option<u32>,
    pub outstanding: VecDeque<Arc<ReplicationPacket>>,
}

impl ClientNetState {
    pub fn sent(&mut self, packet: Arc<ReplicationPacket>) {
        self.outstanding.push_back(packet);
        while self.outstanding.len() > HISTORY_FRAMES {
            self.outstanding.pop_front();
        }
    }
    pub fn acknowledge(&mut self, tick: u32) {
        if self
            .last_acked_tick
            .is_some_and(|old| !is_newer_input_seq(tick, old))
        {
            return;
        }
        // Only a complete version actually offered to this client can become its
        // baseline. Transport ACKs are insufficient: the client must reconstruct.
        if let Some(index) = self
            .outstanding
            .iter()
            .position(|packet| packet.tick == tick)
        {
            self.last_acked_tick = Some(tick);
            self.outstanding.drain(..=index);
        }
    }
}

struct Frame {
    tick: u32,
    state: ReplicationState,
    changes: ChangeSet,
    packets: HashMap<Option<u32>, Arc<ReplicationPacket>>,
    retained_bytes: usize,
}

#[derive(Resource, Default)]
pub(crate) struct ReplicationHistory {
    epoch: Option<u32>,
    frames: VecDeque<Frame>,
    retained_bytes: usize,
}

impl ReplicationHistory {
    pub fn record(&mut self, world: &WorldDelta) {
        let epoch = world.sim_meta.map(|meta| meta.match_epoch);
        if epoch != self.epoch {
            self.frames.clear();
            self.retained_bytes = 0;
            self.epoch = epoch;
        }
        if self
            .frames
            .back()
            .is_some_and(|frame| frame.tick == world.tick)
        {
            return;
        }
        let state = capture_world(world);
        let changes = self.frames.back().map_or_else(ChangeSet::new, |previous| {
            changed_records(&state, &previous.state)
        });
        let full = Arc::new(ReplicationPacket {
            tick: world.tick,
            baseline_tick: None,
            payload: ServerWorldMessage::encode_full(world).into(),
        });
        let retained_bytes = state.values().map(Vec::len).sum::<usize>()
            + changes
                .values()
                .map(|change| change.words.len() * 8)
                .sum::<usize>()
            + full.payload.len();
        self.retained_bytes += retained_bytes;
        self.frames.push_back(Frame {
            tick: world.tick,
            state,
            changes,
            packets: HashMap::from([(None, full)]),
            retained_bytes,
        });
        self.enforce_budget();
    }

    /// All clients for this tick share one backward history traversal. Adjacent
    /// requested baselines reuse the accumulated union instead of recomputing it.
    pub fn prepare_packets(&mut self, acknowledged: &[Option<u32>]) {
        let latest = self.frames.back().expect("record a frame before sending");
        let tick = latest.tick;
        let requested: std::collections::HashSet<u32> = acknowledged
            .iter()
            .flatten()
            .copied()
            .filter(|ack| {
                let age = tick.wrapping_sub(*ack);
                age > 0 && age <= MAX_BASELINE_AGE && !latest.packets.contains_key(&Some(*ack))
            })
            .collect();
        if requested.is_empty() {
            return;
        }
        let mut pending = Vec::new();
        let mut changes = ChangeSet::new();
        for index in (1..self.frames.len()).rev() {
            union_changes(&mut changes, &self.frames[index].changes);
            let baseline = self.frames[index - 1].tick;
            if !requested.contains(&baseline) {
                continue;
            }
            let patch = patch_from_changes(tick, baseline, &latest.state, &changes);
            let payload = encode(&ServerWorldMessage::Patch(patch));
            let full = &latest.packets[&None];
            let packet = if payload.len() < full.payload.len() {
                Arc::new(ReplicationPacket {
                    tick,
                    baseline_tick: Some(baseline),
                    payload: payload.into(),
                })
            } else {
                Arc::clone(full)
            };
            pending.push((Some(baseline), packet));
            if pending.len() == requested.len() {
                break;
            }
        }
        let latest = self.frames.back_mut().unwrap();
        for (key, packet) in pending {
            // Full fallback entries share the allocation already accounted for.
            if packet.baseline_tick.is_some() {
                latest.retained_bytes += packet.payload.len();
                self.retained_bytes += packet.payload.len();
            }
            latest.packets.insert(key, packet);
        }
    }

    pub fn packet_for(&mut self, acknowledged: Option<u32>) -> Arc<ReplicationPacket> {
        if let Some(packet) = self.frames.back().unwrap().packets.get(&acknowledged) {
            return Arc::clone(packet);
        }
        self.prepare_packets(&[acknowledged]);
        let latest = self.frames.back().unwrap();
        Arc::clone(
            latest
                .packets
                .get(&acknowledged)
                .unwrap_or(&latest.packets[&None]),
        )
    }

    pub fn retire_acknowledged(&mut self, clients: &HashMap<u64, ClientNetState>) {
        // Keep the oldest still-needed baseline. A stalled/new client cannot pin
        // history indefinitely: age/frame/byte bounds eventually require full state.
        while self.frames.len() > 1 {
            let next = self.frames[1].tick;
            if !clients.values().all(|client| {
                client
                    .last_acked_tick
                    .is_some_and(|ack| ack == next || is_newer_input_seq(ack, next))
            }) {
                break;
            }
            self.pop_front();
        }
        self.enforce_budget();
    }

    fn enforce_budget(&mut self) {
        while self.frames.len() > HISTORY_FRAMES
            || (self.frames.len() > 1 && self.memory_bytes() > MAX_HISTORY_BYTES)
        {
            self.pop_front();
        }
    }

    pub(crate) fn memory_bytes(&self) -> usize {
        self.retained_bytes
    }

    fn pop_front(&mut self) {
        if let Some(frame) = self.frames.pop_front() {
            self.retained_bytes -= frame.retained_bytes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_shared::{apply_world_patch, decode};
    use game_sim::Simulation;

    fn rebuild(baseline: &WorldDelta, packet: &ReplicationPacket) -> WorldDelta {
        match decode(&packet.payload).unwrap() {
            ServerWorldMessage::Full(world) => world,
            ServerWorldMessage::Patch(patch) => apply_world_patch(baseline, &patch).unwrap(),
        }
    }

    fn recompute_accounted_bytes(history: &ReplicationHistory) -> usize {
        history
            .frames
            .iter()
            .map(|frame| {
                let mut pointers = std::collections::HashSet::new();
                frame.state.values().map(Vec::len).sum::<usize>()
                    + frame
                        .changes
                        .values()
                        .map(|change| change.words.len() * 8)
                        .sum::<usize>()
                    + frame
                        .packets
                        .values()
                        .filter(|packet| pointers.insert(Arc::as_ptr(packet)))
                        .map(|packet| packet.payload.len())
                        .sum::<usize>()
            })
            .sum()
    }

    #[test]
    fn accounting_tracks_shared_payloads_eviction_and_epoch_reset() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let mut history = ReplicationHistory::default();
        let mut first_tick = None;
        for index in 0..150 {
            sim.step();
            let mut world = sim.world_delta();
            if index == 100 {
                world.sim_meta.as_mut().unwrap().match_epoch += 1;
            }
            history.record(&world);
            first_tick.get_or_insert(world.tick);
            let full = history.packet_for(None);
            assert_eq!(
                full.payload.as_ref(),
                ServerWorldMessage::encode_full(&world)
            );
            assert_eq!(
                full.payload.as_ref(),
                encode(&ServerWorldMessage::Full(world.clone()))
            );
            for age in [1, 2, 9, 60] {
                history.packet_for(Some(world.tick.wrapping_sub(age)));
            }
            // Multiple fallback cache keys reference this same allocation.
            history
                .frames
                .back_mut()
                .unwrap()
                .packets
                .insert(Some(u32::MAX - 1), Arc::clone(&full));
            history
                .frames
                .back_mut()
                .unwrap()
                .packets
                .insert(Some(u32::MAX - 2), full);
            assert_eq!(history.memory_bytes(), recompute_accounted_bytes(&history));
            assert!(history.frames.len() <= HISTORY_FRAMES);
        }
        history.retire_acknowledged(&HashMap::new());
        assert_eq!(history.frames.len(), 1);
        assert_eq!(history.memory_bytes(), recompute_accounted_bytes(&history));
        history.pop_front();
        assert_eq!(history.memory_bytes(), 0);
    }

    #[test]
    fn batched_baselines_match_individual_recovery_and_share_duplicate_payloads() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let mut batch = ReplicationHistory::default();
        let mut individual = ReplicationHistory::default();
        for step in 0..90 {
            sim.step();
            let world = sim.world_delta();
            batch.record(&world);
            individual.record(&world);
            let baselines = [
                None,
                Some(world.tick.wrapping_sub(1)),
                Some(world.tick.wrapping_sub(2)),
                Some(world.tick.wrapping_sub(9)),
                Some(world.tick.wrapping_sub(60)),
                Some(world.tick.wrapping_sub(90)),
                Some(world.tick.wrapping_add(1)),
            ];
            batch.prepare_packets(&baselines);
            for baseline in baselines {
                let actual = batch.packet_for(baseline);
                let expected = individual.packet_for(baseline);
                assert_eq!(
                    actual.payload, expected.payload,
                    "step {step}, baseline {baseline:?}"
                );
                assert_eq!(actual.baseline_tick, expected.baseline_tick);
                assert!(Arc::ptr_eq(&actual, &batch.packet_for(baseline)));
            }
            assert_eq!(batch.memory_bytes(), recompute_accounted_bytes(&batch));
        }
    }

    #[test]
    fn independent_clients_recover_after_delayed_lost_updates_and_acknowledgements() {
        struct Peer {
            ghost: ClientNetState,
            known: VecDeque<WorldDelta>,
            deliveries: Vec<(u32, Arc<ReplicationPacket>, WorldDelta)>,
            acks: Vec<(u32, u32)>,
            accepted: usize,
        }
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
            .map(|index| {
                let mut enemy = enemy;
                enemy.id += index;
                enemy.pos[0] += index as f32 * 0.1;
                enemy
            })
            .collect();
        let mut history = ReplicationHistory::default();
        let mut peers: Vec<Peer> = (0..2)
            .map(|_| Peer {
                ghost: ClientNetState::default(),
                known: VecDeque::new(),
                deliveries: Vec::new(),
                acks: Vec::new(),
                accepted: 0,
            })
            .collect();
        let mut random = 17u32;
        for step in 0..400u32 {
            world.tick += 1;
            for (index, hero) in world.heroes.iter_mut().enumerate() {
                hero.last_move_seq = Some(step);
                hero.pending_moves = (0..(step as usize + index) % 3)
                    .map(|n| (step + n as u32 + 1, [1.0, 0.0]))
                    .collect();
            }
            for enemy in &mut world.enemies {
                enemy.pos[0] += 0.01;
                enemy.repath_cooldown = step % 12;
            }
            if step == 70 {
                world.enemies.remove(0);
            }
            if step == 80 {
                let mut replacement = enemy;
                replacement.id = 2_000_000;
                world.enemies.push(replacement);
            }
            if step == 90 {
                world.towers.push(game_shared::TowerSnapshot {
                    id: enemy.id,
                    owner: 1,
                    lane: 0,
                    node_id: 1,
                    pos: [3.0, 4.0],
                    reload_ticks_remaining: 0,
                });
            }
            if step == 250 {
                world.sim_meta.as_mut().unwrap().match_epoch += 1;
                world.heroes[0].respawn_generation += 1;
            }
            history.record(&world);
            for (index, peer) in peers.iter_mut().enumerate() {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let packet = history.packet_for(peer.ghost.last_acked_tick);
                peer.ghost.sent(Arc::clone(&packet));
                let outage = index == 0 && (120..180).contains(&step);
                if !outage && (step > 300 || random % 10 != 0) {
                    peer.deliveries
                        .push((step + 3 + random % 5, packet, world.clone()));
                }
                // An application stall batches already-delivered messages.
                if !(index == 1 && (50..56).contains(&step)) {
                    let mut pending = Vec::new();
                    for (due, packet, expected) in peer.deliveries.drain(..) {
                        if due > step {
                            pending.push((due, packet, expected));
                            continue;
                        }
                        if peer
                            .known
                            .back()
                            .is_some_and(|known| !is_newer_input_seq(packet.tick, known.tick))
                        {
                            continue;
                        }
                        let received = match decode(&packet.payload).unwrap() {
                            ServerWorldMessage::Full(world) => world,
                            ServerWorldMessage::Patch(patch) => {
                                let baseline = peer
                                    .known
                                    .iter()
                                    .find(|known| known.tick == patch.baseline_tick)
                                    .expect("confirmed baseline must be retained");
                                apply_world_patch(baseline, &patch).unwrap()
                            }
                        };
                        assert_eq!(received, expected, "peer {index}, step {step}");
                        peer.known.push_back(received);
                        while peer.known.len() > HISTORY_FRAMES {
                            peer.known.pop_front();
                        }
                        peer.accepted += 1;
                        if step > 300 || (random + packet.tick) % 7 != 0 {
                            peer.acks.push((step + 4, packet.tick));
                        }
                    }
                    peer.deliveries = pending;
                }
                peer.acks.retain(|&(due, tick)| {
                    if due > step {
                        true
                    } else {
                        peer.ghost.acknowledge(tick);
                        false
                    }
                });
            }
        }
        for peer in peers {
            assert!(peer.accepted > 150);
            assert!(world.tick.wrapping_sub(peer.known.back().unwrap().tick) <= 7);
            assert!(peer.ghost.outstanding.len() <= HISTORY_FRAMES);
        }
    }

    #[test]
    fn missing_updates_union_changes_and_equivalent_clients_share_packets() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.add_player(2);
        for _ in 0..=game_shared::WAVE_PREP_TICKS {
            sim.step();
        }
        let mut initial = sim.world_delta();
        initial.heroes[0].last_move_seq = Some(0);
        let enemy = initial.enemies[0];
        initial.enemies = (0..84)
            .map(|i| {
                let mut enemy = enemy;
                enemy.id += i;
                enemy.pos = [i as f32 * 0.25, (i as f32).sin()];
                enemy
            })
            .collect();
        let mut current = initial.clone();
        let mut history = ReplicationHistory::default();
        history.record(&initial);
        let mut client = ClientNetState::default();
        client.sent(history.packet_for(None));
        client.acknowledge(initial.tick);
        for seq in 1..=12 {
            current.tick += 1;
            current.heroes[0].pos[0] += 0.1;
            current.heroes[0].last_move_seq = Some(seq);
            history.record(&current);
            // All intermediate updates and their ACKs are lost.
            client.sent(history.packet_for(client.last_acked_tick));
        }
        let packet = history.packet_for(client.last_acked_tick);
        assert_eq!(packet.baseline_tick, Some(initial.tick));
        assert!(Arc::ptr_eq(
            &packet,
            &history.packet_for(client.last_acked_tick)
        ));
        assert_eq!(rebuild(&initial, &packet), current);
        assert_eq!(client.last_acked_tick, Some(initial.tick));
        client.acknowledge(packet.tick);
        assert!(client.outstanding.is_empty());
        assert_eq!(client.last_acked_tick, Some(current.tick));
    }

    #[test]
    fn unknown_old_and_expired_acknowledgements_do_not_select_bad_baselines() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.set_tick(u32::MAX - 20);
        let mut history = ReplicationHistory::default();
        history.record(&sim.world_delta());
        let initial = sim.tick();
        let mut client = ClientNetState::default();
        client.sent(history.packet_for(None));
        client.acknowledge(initial.wrapping_add(100));
        assert_eq!(client.last_acked_tick, None);
        client.acknowledge(initial);
        client.acknowledge(initial.wrapping_sub(1));
        assert_eq!(client.last_acked_tick, Some(initial));
        for _ in 0..61 {
            sim.step();
            history.record(&sim.world_delta());
        }
        let packet = history.packet_for(client.last_acked_tick);
        assert_eq!(packet.baseline_tick, None);
        assert_eq!(rebuild(&sim.world_delta(), &packet), sim.world_delta());
        for _ in 0..100 {
            sim.step();
            history.record(&sim.world_delta());
        }
        assert!(history.frames.len() <= HISTORY_FRAMES);
    }

    #[test]
    fn all_client_acknowledgements_release_history_and_cached_packets() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let mut history = ReplicationHistory::default();
        let mut clients = HashMap::from([
            (1, ClientNetState::default()),
            (2, ClientNetState::default()),
        ]);
        for _ in 0..10 {
            sim.step();
            history.record(&sim.world_delta());
            for client in clients.values_mut() {
                client.sent(history.packet_for(client.last_acked_tick));
            }
        }
        for client in clients.values_mut() {
            client.acknowledge(sim.tick());
        }
        history.retire_acknowledged(&clients);
        assert_eq!(history.frames.len(), 1);
        assert!(clients.values().all(|client| client.outstanding.is_empty()));
    }
}

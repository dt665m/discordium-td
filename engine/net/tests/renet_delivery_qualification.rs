//! Legacy smoke fixture; do not use its A/B/C totals to select a transport.
//!
//! Its cyclic A scheduler, whole-unreliable bursts and short reliable resend
//! interval are not production-equivalent policies. Use
//! `renet_adverse_delivery.rs` for the paced A/C transport experiment instead;
//! neither fixture is a complete game/transport acceptance test.
//!
//! Short deterministic comparison of three snapshot-delivery shapes over
//! stock Renet's actual packet generation, reassembly, acknowledgements and
//! reliable retransmission paths.
//!
//! It intentionally measures an average 360 KiB/s state budget, but lets the
//! whole-unreliable variant accumulate that budget before releasing a message.
//! Renet 2.0.0 otherwise drops a sliced unreliable message before packetizing
//! it when `available_bytes_per_tick < message.len()`. The release itself is a
//! burst because the stock unreliable channel has no paced-slice API. This is
//! a transport policy difference under test, not an application-frame cap.

use engine_net::{
    CompressionMode, CompressionPolicy,
    codec::{FrameHeader, FrameLimit, Lane, decode_frame, encode_frame},
    decode, encode_with_compression,
    replication::{ByteAssembler, ByteFragment, ByteLimits, GroupPublication, fragment_group},
    types::{ConnectionEpoch, GroupId, GroupRevision, SnapshotId},
};
use octets::Octets;
use renet::{ChannelConfig, ConnectionConfig, RenetClient, RenetServer, SendType};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

const CLIENT: u64 = 7;
const CONTROL: u8 = 0;
const STATE: u8 = 1;
const REMOTE: u8 = 2;
const INPUT: u8 = 3;
const TICK_MS: u64 = 10;
const RUN_MS: u64 = 2_000;
const SETTLE_MS: u64 = 1_000;
const AVERAGE_STATE_BUDGET: usize = 3_600;
const STATE_QUEUE_CAPACITY: usize = 128 * 1024;
const FRAME_LIMIT: usize = 1_100;
const APPLICATION_FRAGMENT_PAYLOAD: usize = FRAME_LIMIT
    - engine_net::codec::HEADER_BYTES
    - engine_net::replication::BYTE_FRAGMENT_HEADER_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryPath {
    ApplicationFragments,
    WholeUnreliable,
    WholeReliableUnordered,
}

impl DeliveryPath {
    const ALL: [Self; 3] = [
        Self::ApplicationFragments,
        Self::WholeUnreliable,
        Self::WholeReliableUnordered,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::ApplicationFragments => "application-fragments/unreliable",
            Self::WholeUnreliable => "whole/unreliable",
            Self::WholeReliableUnordered => "whole/reliable-unordered",
        }
    }

    const fn state_send_type(self) -> SendType {
        match self {
            Self::ApplicationFragments | Self::WholeUnreliable => SendType::Unreliable,
            Self::WholeReliableUnordered => SendType::ReliableUnordered {
                resend_time: Duration::from_millis(80),
            },
        }
    }

    const fn state_budget(self) -> usize {
        match self {
            // The stock unreliable channel compares this value to the complete
            // message before producing any slices. The application token bucket
            // below still limits its *average* state admission to 3,600 B/tick.
            Self::WholeUnreliable => 64 * 1024,
            Self::ApplicationFragments | Self::WholeReliableUnordered => AVERAGE_STATE_BUDGET,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Impairment {
    name: &'static str,
    loss_percent: u64,
    duplicate_percent: u64,
    reorder_percent: u64,
    jitter_ms: u64,
}

const CLEAN: Impairment = Impairment {
    name: "clean",
    loss_percent: 0,
    duplicate_percent: 0,
    reorder_percent: 0,
    jitter_ms: 0,
};
const IMPAIRED: Impairment = Impairment {
    name: "3pct-loss-jitter-reorder-dup",
    loss_percent: 3,
    duplicate_percent: 3,
    reorder_percent: 17,
    jitter_ms: 20,
};
const HARSH: Impairment = Impairment {
    name: "10pct-loss-jitter-reorder-dup",
    loss_percent: 10,
    duplicate_percent: 5,
    reorder_percent: 25,
    jitter_ms: 30,
};

#[derive(Debug, Clone)]
struct Snapshot {
    topology: u64,
    id: u64,
    created_ms: u64,
    body: Vec<u8>,
}

#[derive(Debug, Default)]
struct Metrics {
    generated: usize,
    applied: Vec<(u64, u64)>,
    deadline_misses: usize,
    state_message_calls: usize,
    down_packets: usize,
    up_packets: usize,
    down_wire_bytes: usize,
    up_wire_bytes: usize,
    dropped_down_packets: usize,
    dropped_up_packets: usize,
    max_transport_queued: usize,
    max_application_staged: usize,
    max_reliable_outstanding: usize,
    control_latencies: Vec<u64>,
    remote_messages: usize,
    input_messages: usize,
    reliable_state_attempts: usize,
    reliable_state_unique_packets: BTreeSet<(u64, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PacketKey {
    due_ms: u64,
    serial: u64,
}

#[derive(Debug)]
struct ScheduledPacket {
    to_client: bool,
    bytes: Vec<u8>,
}

struct Link {
    impairment: Impairment,
    random: u64,
    serial: u64,
    packets: BTreeMap<PacketKey, ScheduledPacket>,
}

impl Link {
    fn new(impairment: Impairment) -> Self {
        Self {
            impairment,
            random: 0x63f4_3c59_2c44_96d7,
            serial: 0,
            packets: BTreeMap::new(),
        }
    }

    fn next_random(&mut self) -> u64 {
        self.random = self
            .random
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.random
    }

    fn send(&mut self, now_ms: u64, to_client: bool, bytes: Vec<u8>, metrics: &mut Metrics) {
        let random = self.next_random();
        if random % 100 < self.impairment.loss_percent {
            if to_client {
                metrics.dropped_down_packets += 1;
            } else {
                metrics.dropped_up_packets += 1;
            }
            return;
        }
        let jitter_span = self
            .impairment
            .jitter_ms
            .saturating_mul(2)
            .saturating_add(1);
        let jitter = if jitter_span == 0 {
            0
        } else {
            random.rotate_left(13) % jitter_span
        };
        let mut due_ms = now_ms + 75 - self.impairment.jitter_ms + jitter;
        if random.rotate_left(29) % 100 < self.impairment.reorder_percent {
            due_ms += 45;
        }
        self.insert(due_ms, to_client, bytes.clone());
        if random.rotate_left(43) % 100 < self.impairment.duplicate_percent {
            self.insert(due_ms + 1, to_client, bytes);
        }
    }

    fn insert(&mut self, due_ms: u64, to_client: bool, bytes: Vec<u8>) {
        let key = PacketKey {
            due_ms,
            serial: self.serial,
        };
        self.serial += 1;
        self.packets
            .insert(key, ScheduledPacket { to_client, bytes });
    }

    fn receive_due(&mut self, now_ms: u64, server: &mut RenetServer, client: &mut RenetClient) {
        while self
            .packets
            .first_key_value()
            .is_some_and(|(key, _)| key.due_ms <= now_ms)
        {
            let (_, packet) = self.packets.pop_first().unwrap();
            if packet.to_client {
                client.process_packet(&packet.bytes);
            } else {
                server.process_packet_from(&packet.bytes, CLIENT).unwrap();
            }
        }
    }
}

fn channels(state: SendType) -> Vec<ChannelConfig> {
    vec![
        ChannelConfig {
            channel_id: CONTROL,
            max_memory_usage_bytes: 32 * 1024,
            send_type: SendType::ReliableOrdered {
                resend_time: Duration::from_millis(80),
            },
        },
        ChannelConfig {
            channel_id: STATE,
            max_memory_usage_bytes: STATE_QUEUE_CAPACITY,
            send_type: state,
        },
        ChannelConfig {
            channel_id: REMOTE,
            max_memory_usage_bytes: 32 * 1024,
            send_type: SendType::Unreliable,
        },
        ChannelConfig {
            channel_id: INPUT,
            max_memory_usage_bytes: 32 * 1024,
            send_type: SendType::Unreliable,
        },
    ]
}

fn connection_config(path: DeliveryPath) -> ConnectionConfig {
    let channels = channels(path.state_send_type());
    ConnectionConfig {
        available_bytes_per_tick: path.state_budget() as u64,
        server_channels_config: channels.clone(),
        client_channels_config: channels,
    }
}

fn application_limits() -> ByteLimits {
    ByteLimits {
        fragment_payload_bytes: APPLICATION_FRAGMENT_PAYLOAD,
        group_bytes: 32 * 1024,
        fragments: 32,
        incomplete: 2,
        known_groups: 4,
        staging_bytes: 64 * 1024,
        max_age_ms: 500,
    }
}

fn logical_payload(size: usize, snapshot: u64) -> Vec<u8> {
    let mut bytes = vec![0; size];
    for (index, byte) in bytes.iter_mut().enumerate() {
        // Repeated low-entropy structure is representative of snapshots with
        // stable schema fields, without making every payload all zeroes.
        *byte = ((index % 31) as u8).wrapping_add((snapshot % 7) as u8);
    }
    bytes[..8].copy_from_slice(&snapshot.to_le_bytes());
    bytes
}

fn whole_message(snapshot: &Snapshot) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32 + snapshot.body.len());
    bytes.extend_from_slice(&snapshot.topology.to_le_bytes());
    bytes.extend_from_slice(&snapshot.id.to_le_bytes());
    bytes.extend_from_slice(&snapshot.created_ms.to_le_bytes());
    bytes.extend_from_slice(&(snapshot.body.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&snapshot.body);
    bytes
}

fn decode_whole(bytes: &[u8]) -> Option<(u64, u64, u64, &[u8])> {
    if bytes.len() < 32 {
        return None;
    }
    let read = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    let topology = read(0);
    let id = read(8);
    let created_ms = read(16);
    let length = read(24) as usize;
    (bytes.len() == 32 + length).then_some((topology, id, created_ms, &bytes[32..]))
}

fn application_frames(snapshot: &Snapshot) -> Vec<Vec<u8>> {
    let publication = GroupPublication {
        connection: ConnectionEpoch(1),
        group: GroupId(1),
        revision: GroupRevision(snapshot.topology),
        snapshot: SnapshotId(snapshot.id),
    };
    let limit = FrameLimit::new(FRAME_LIMIT).unwrap();
    fragment_group(publication, &snapshot.body, application_limits())
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(sequence, fragment)| {
            let payload = fragment.encode(application_limits()).unwrap();
            encode_frame(
                FrameHeader {
                    lane: Lane::State,
                    connection: ConnectionEpoch(1),
                    sequence: sequence as u32,
                },
                &payload,
                limit,
            )
            .unwrap()
        })
        .collect()
}

fn reliable_state_packets(bytes: &[u8], metrics: &mut Metrics) {
    // Telemetry only: this follows Renet 2.0.0's packet layout so retries can
    // be counted from real emitted packets. Packet parsing itself remains owned
    // by Renet; neither delivery nor correctness depends on this observer.
    let mut packet = Octets::with_slice(bytes);
    let Ok(kind) = packet.get_u8() else {
        return;
    };
    let Ok(_) = packet.get_varint() else {
        return;
    };
    match kind {
        0 => {
            let (Ok(channel), Ok(count)) = (packet.get_u8(), packet.get_u16()) else {
                return;
            };
            for _ in 0..count {
                let (Ok(message), Ok(length)) = (packet.get_varint(), packet.get_varint()) else {
                    return;
                };
                if channel == STATE {
                    metrics.reliable_state_attempts += 1;
                    metrics.reliable_state_unique_packets.insert((message, 0));
                }
                if packet.skip(length as usize).is_err() {
                    return;
                }
            }
        }
        2 => {
            let (Ok(channel), Ok(message), Ok(slice), Ok(length)) = (
                packet.get_u8(),
                packet.get_varint(),
                packet.get_varint(),
                packet.get_varint(),
            ) else {
                return;
            };
            if channel == STATE {
                metrics.reliable_state_attempts += 1;
                metrics
                    .reliable_state_unique_packets
                    .insert((message, slice as usize));
            }
            let _ = packet.skip(length as usize);
        }
        _ => {}
    }
}

fn is_newer(candidate: (u64, u64), current: Option<(u64, u64)>) -> bool {
    current.is_none_or(|value| candidate > value)
}

fn send_receipt(client: &mut RenetClient, topology: u64, snapshot: u64) {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&topology.to_le_bytes());
    bytes.extend_from_slice(&snapshot.to_le_bytes());
    client.send_message(CONTROL, bytes);
}

fn qualify(path: DeliveryPath, compression: CompressionPolicy, impairment: Impairment) -> Metrics {
    let config = connection_config(path);
    let mut server = RenetServer::new(config.clone());
    server.add_connection(CLIENT);
    let mut client = RenetClient::new(config);
    client.set_connected();
    let mut link = Link::new(impairment);
    let mut assembler = ByteAssembler::new(ConnectionEpoch(1), application_limits()).unwrap();
    let mut metrics = Metrics::default();
    let mut current: Option<Snapshot> = None;
    let mut frames = Vec::new();
    let mut frame_cursor = 0usize;
    let mut last_whole_sent_ms = None;
    let mut whole_credit = 0usize;
    let mut reliable_pending = VecDeque::<(u64, u64)>::new();
    let mut created = BTreeMap::<(u64, u64), u64>::new();
    let mut applied = None;
    let mut next_snapshot = 1u64;
    let mut topology = 1u64;
    let mut last = Duration::ZERO;

    for now_ms in (0..=RUN_MS + SETTLE_MS).step_by(TICK_MS as usize) {
        let now = Duration::from_millis(now_ms);
        let elapsed = now - last;
        last = now;
        server.update(elapsed);
        client.update(elapsed);
        link.receive_due(now_ms, &mut server, &mut client);

        // Contention: control must remain usable while remote state and input
        // continue to flow around the snapshot lane.
        if now_ms < RUN_MS && now_ms % 50 == 0 {
            server.send_message(CLIENT, CONTROL, now_ms.to_le_bytes().to_vec());
            server.send_message(CLIENT, REMOTE, vec![0x5a; 512]);
            client.send_message(INPUT, vec![0xa5; 48]);
        }

        // Two close maximum snapshots exercise supersession; the later topology
        // revision exercises an application reset without confusing it for an
        // acknowledgement or a transport-level reliable ordering guarantee.
        if now_ms < RUN_MS && now_ms % 100 == 0 {
            if now_ms == 800 {
                topology += 1;
            }
            let size = if matches!(now_ms, 0 | 700 | 750 | 1_600) {
                29_216
            } else {
                4_096
            };
            let logical = logical_payload(size, next_snapshot);
            let body = encode_with_compression(&logical, compression);
            let snapshot = Snapshot {
                topology,
                id: next_snapshot,
                created_ms: now_ms,
                body,
            };
            created.insert((topology, next_snapshot), now_ms);
            metrics.generated += 1;
            frames = application_frames(&snapshot);
            frame_cursor = 0;
            current = Some(snapshot);
            last_whole_sent_ms = None;
            next_snapshot += 1;
        }

        if let Some(snapshot) = &current {
            match path {
                DeliveryPath::ApplicationFragments => {
                    // Send at most two complete application frames per tick.
                    // Cycling keeps a fresh snapshot recoverable after an
                    // unreliable slice loss without retaining old snapshots.
                    for _ in 0..2 {
                        server.send_message(CLIENT, STATE, frames[frame_cursor].clone());
                        metrics.state_message_calls += 1;
                        frame_cursor = (frame_cursor + 1) % frames.len();
                    }
                }
                DeliveryPath::WholeUnreliable => {
                    let message = whole_message(snapshot);
                    whole_credit = whole_credit.saturating_add(AVERAGE_STATE_BUDGET);
                    if whole_credit >= message.len()
                        && last_whole_sent_ms.is_none_or(|sent| now_ms - sent >= 100)
                    {
                        whole_credit -= message.len();
                        server.send_message(CLIENT, STATE, message);
                        metrics.state_message_calls += 1;
                        last_whole_sent_ms = Some(now_ms);
                    }
                }
                DeliveryPath::WholeReliableUnordered => {
                    if reliable_pending.len() < 2
                        && !reliable_pending
                            .iter()
                            .any(|pending| *pending == (snapshot.topology, snapshot.id))
                        && server.can_send_message(CLIENT, STATE, whole_message(snapshot).len())
                    {
                        server.send_message(CLIENT, STATE, whole_message(snapshot));
                        metrics.state_message_calls += 1;
                        reliable_pending.push_back((snapshot.topology, snapshot.id));
                    }
                }
            }
        }

        while let Some(message) = client.receive_message(CONTROL) {
            let sent_ms = u64::from_le_bytes(message[..8].try_into().unwrap());
            metrics.control_latencies.push(now_ms - sent_ms);
        }
        while client.receive_message(REMOTE).is_some() {
            metrics.remote_messages += 1;
        }
        while server.receive_message(CLIENT, INPUT).is_some() {
            metrics.input_messages += 1;
        }
        while let Some(receipt) = server.receive_message(CLIENT, CONTROL) {
            if receipt.len() == 16 {
                let topology = u64::from_le_bytes(receipt[..8].try_into().unwrap());
                let snapshot = u64::from_le_bytes(receipt[8..].try_into().unwrap());
                reliable_pending.retain(|pending| *pending != (topology, snapshot));
            }
        }

        while let Some(message) = client.receive_message(STATE) {
            let candidate = match path {
                DeliveryPath::ApplicationFragments => {
                    let limit = FrameLimit::new(FRAME_LIMIT).unwrap();
                    let Ok((_, payload)) = decode_frame(&message, ConnectionEpoch(1), limit) else {
                        continue;
                    };
                    let Ok(fragment) = ByteFragment::decode(payload, application_limits()) else {
                        continue;
                    };
                    let Ok(complete) = assembler.receive(fragment, now_ms) else {
                        continue;
                    };
                    let Some(complete) = complete else {
                        continue;
                    };
                    let publication = complete.publication();
                    let Ok(value) = decode::<Vec<u8>>(complete.as_bytes()) else {
                        continue;
                    };
                    if value.len() < 8
                        || u64::from_le_bytes(value[..8].try_into().unwrap())
                            != publication.snapshot.0
                    {
                        continue;
                    }
                    Some((publication.revision.0, publication.snapshot.0, value))
                }
                DeliveryPath::WholeUnreliable | DeliveryPath::WholeReliableUnordered => {
                    let Some((topology, id, _, body)) = decode_whole(&message) else {
                        continue;
                    };
                    let Ok(value) = decode::<Vec<u8>>(body) else {
                        continue;
                    };
                    if value.len() < 8 || u64::from_le_bytes(value[..8].try_into().unwrap()) != id {
                        continue;
                    }
                    Some((topology, id, value))
                }
            };
            let Some((topology, id, _)) = candidate else {
                continue;
            };
            if is_newer((topology, id), applied) {
                applied = Some((topology, id));
                metrics.applied.push((topology, id));
                send_receipt(&mut client, topology, id);
            }
        }

        metrics.max_application_staged =
            metrics.max_application_staged.max(assembler.staged_bytes());
        metrics.max_transport_queued = metrics
            .max_transport_queued
            .max(STATE_QUEUE_CAPACITY - server.channel_available_memory(CLIENT, STATE));
        metrics.max_reliable_outstanding =
            metrics.max_reliable_outstanding.max(reliable_pending.len());

        if now_ms >= 500 && now_ms <= RUN_MS && now_ms % 500 == 0 {
            let required = created
                .iter()
                .filter(|(_, created_ms)| **created_ms <= now_ms - 500)
                .map(|(identity, _)| *identity)
                .max();
            if required.is_some_and(|required| applied.is_none_or(|seen| seen < required)) {
                metrics.deadline_misses += 1;
            }
        }

        for packet in server.get_packets_to_send(CLIENT).unwrap() {
            metrics.down_packets += 1;
            metrics.down_wire_bytes += packet.len();
            reliable_state_packets(&packet, &mut metrics);
            link.send(now_ms, true, packet, &mut metrics);
        }
        for packet in client.get_packets_to_send() {
            metrics.up_packets += 1;
            metrics.up_wire_bytes += packet.len();
            link.send(now_ms, false, packet, &mut metrics);
        }
    }

    let latest = current.as_ref().unwrap();
    if impairment.loss_percent <= 3 {
        assert_eq!(
            applied,
            Some((latest.topology, latest.id)),
            "{} did not recover the current snapshot under {}",
            path.name(),
            impairment.name
        );
    }
    assert!(
        metrics
            .control_latencies
            .iter()
            .all(|latency| *latency <= 500)
    );
    assert!(metrics.input_messages > 0 && metrics.remote_messages > 0);
    assert!(metrics.max_transport_queued <= STATE_QUEUE_CAPACITY);
    assert!(
        metrics
            .applied
            .windows(2)
            .all(|window| window[0] < window[1])
    );
    for &(topology, id) in &metrics.applied {
        assert!(created.contains_key(&(topology, id)));
    }
    metrics
}

fn percentile_95(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    values[(values.len() - 1) * 95 / 100]
}

#[test]
fn legacy_delivery_fixture_exercises_packet_paths() {
    // Acceptance criteria defined before interpreting the measurements:
    // - never apply an invalid, older, or wrong-topology snapshot;
    // - keep control delivery below 500 ms under the bounded fixture;
    // - keep application and transport state bounded by their configured caps;
    // - record whether the 500 ms current-snapshot target is missed, rather
    //   than declaring a lane successful merely because it eventually recovers.
    for compression in [
        CompressionPolicy {
            mode: CompressionMode::Off,
            minimum_bytes: 0,
        },
        CompressionPolicy {
            mode: CompressionMode::Auto,
            minimum_bytes: 0,
        },
    ] {
        for impairment in [CLEAN, IMPAIRED, HARSH] {
            for path in DeliveryPath::ALL {
                let mut metrics = qualify(path, compression, impairment);
                let p95_control = percentile_95(&mut metrics.control_latencies);
                let retries = metrics
                    .reliable_state_attempts
                    .saturating_sub(metrics.reliable_state_unique_packets.len());
                eprintln!(
                    "path={} compression={:?} profile={} generated={} applied={} last={:?} deadline_misses={} down={}B/{}pkt up={}B/{}pkt dropped={}/{} queue={}B staged={}B outstanding={} control_p95={}ms remote={} input={} reliable_attempts={} retries={}",
                    path.name(),
                    compression.mode,
                    impairment.name,
                    metrics.generated,
                    metrics.applied.len(),
                    metrics.applied.last(),
                    metrics.deadline_misses,
                    metrics.down_wire_bytes,
                    metrics.down_packets,
                    metrics.up_wire_bytes,
                    metrics.up_packets,
                    metrics.dropped_down_packets,
                    metrics.dropped_up_packets,
                    metrics.max_transport_queued,
                    metrics.max_application_staged,
                    metrics.max_reliable_outstanding,
                    p95_control,
                    metrics.remote_messages,
                    metrics.input_messages,
                    metrics.reliable_state_attempts,
                    retries,
                );
            }
        }
    }
}

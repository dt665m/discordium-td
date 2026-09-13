//! Reproducible A/C transport comparison, not a production acceptance test.
//!
//! A uses a test-only copy of the removed byte fragmentation/assembly. C uses stock Renet
//! reliable-unordered messages. Both publish and flush at 20 Hz, with the same
//! 7,000 B Renet payload allowance per flush (140,000 B/s), then pass through a
//! 160,000 B/s, 16,000 B burst final egress bucket. State/remote packets preserve
//! 2,400 B of that bucket for control and ACKs. Every datagram is charged its
//! actual Renet length plus 73 B modeled IPv6/UDP/netcode overhead.
//! Stock C uses 1,200 B slice payloads, so its modeled IP packets exceed the
//! production 1,200 B IP packet policy. No path-MTU safety claim is made here;
//! this fixture does not model IP fragmentation or MTU-dependent packet loss.
//!
//! The network additionally serializes packets through a bounded 100 ms queue;
//! loss occurs after serialization. Downlink and uplink have independent random
//! streams. The same five seeds, generation schedule and competing traffic are
//! used for every path. This is a packet simulation, not native UDP or WebRTC.
//!
//! Deliberate scope: payloads are independent, already-encoded full states;
//! no compression, delta/baseline, interest, actor admission, or game reset
//! exchange is modeled. A keeps its first pass until completion except for a
//! topology change; newer generated states coalesce while that pass is pending.
//! A repeats only the final pass after the profile's repair interval. This is a
//! steady-state transfer fixture; bootstrap/join admission is outside its scope.
//! Its real 2 s assembler deadline is retained. An expired fence is reported as
//! `reset_required`; freshness remains measured afterward, with no silent reset.
//! C's exact decoded receipts free a bounded application admission window even
//! for obsolete messages, and an identity is never re-enqueued after admission.
//! C4/C16 share the same 128 KiB actual Renet queue capacity. CQ applies only
//! that actual queue capacity; exact receipts are diagnostic, not admission
//! gates. This distinguishes receipt-window head-of-line blocking from Renet.
//! CQ32 applies the same queue-only policy with a 32 KiB Renet sender capacity.
//! All clients retain the common 128 KiB receive capacity: stock sliced-message
//! reception reserves whole 1,200 B slices, exceeding sender payload accounting.
//!
//! The 50 ms network flush is common to both candidates and is not the game's
//! independent worker polling cadence. A admits a conservative 6,000 B prefix
//! (normally five 1,100 B frames) per flush, preserving space for competing work.
//! Receiver Renet allocation is not exposed by stock APIs and is not measured.
//! Connection status is checked after receive and send. A disconnect terminates
//! that case's transport work, reports the exact reason, and continues stale-age
//! sampling to 10 s; undelivered planned control messages count as deadline
//! misses. Such rows are failures, not valid steady-state latency comparisons.
//! The test asserts integrity and bounds, while freshness/deadline failures are
//! outputs to analyze, not silently treated as passing service-level objectives.

#[path = "fixtures/application_fragments.rs"]
mod application_fragments;
use application_fragments::{ByteAssembler, ByteFragment, ByteLimits, fragment_group};
use engine_net::{
    codec::{FrameHeader, FrameLimit, Lane, decode_frame, encode_frame},
    replication::GroupPublication,
    types::{ConnectionEpoch, GroupId, GroupRevision, SnapshotId},
};
use octets::Octets;
use renet::{ChannelConfig, ConnectionConfig, RenetClient, RenetServer, SendType};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

const CLIENT: u64 = 7;
const CONTROL: u8 = 0;
const REMOTE: u8 = 1;
const STATE: u8 = 2;
const INPUT: u8 = 3;
const TICK_MS: u64 = 10;
const FLUSH_MS: u64 = 50;
const RUN_MS: u64 = 10_000;
const SETTLE_MS: u64 = 3_000;
const RENET_BUDGET: u64 = 7_000;
const PREFIX_BUDGET: usize = 6_000;
const STATE_CAPACITY: usize = 128 * 1024;
const FRAME_LIMIT: usize = 1_100;
const DATAGRAM_OVERHEAD: usize = 73;
const EGRESS_RATE: u64 = 160_000;
const EGRESS_BURST: u64 = 16_000;
const CONTROL_RESERVE: u64 = 2_400;
const CONTROL_BYTES: usize = 380;
const SEEDS: [u64; 5] = [7, 29, 101, 307, 997];

type Identity = (u64, u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Path {
    A,
    C4,
    C16,
    CQ,
    CQ32,
}

impl Path {
    const ALL: [Self; 5] = [Self::A, Self::C4, Self::C16, Self::CQ, Self::CQ32];
    fn name(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::C4 => "C4",
            Self::C16 => "C16",
            Self::CQ => "CQ",
            Self::CQ32 => "CQ32",
        }
    }
    fn window(self) -> usize {
        match self {
            Self::A => 0,
            Self::C4 => 4,
            Self::C16 => 16,
            Self::CQ | Self::CQ32 => usize::MAX,
        }
    }
    fn capacity(self) -> usize {
        if self == Self::CQ32 {
            32 * 1024
        } else {
            STATE_CAPACITY
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Workload {
    Compact,
    Steady,
    Mixed,
    Overload,
}

impl Workload {
    const ALL: [Self; 4] = [Self::Compact, Self::Steady, Self::Mixed, Self::Overload];
    fn name(self) -> &'static str {
        match self {
            Self::Compact => "compact512",
            Self::Steady => "steady4096",
            Self::Mixed => "mixed4096_29216",
            Self::Overload => "overload29216",
        }
    }
    fn size(self, now_ms: u64) -> usize {
        match self {
            Self::Compact => 512,
            Self::Steady => 4_096,
            Self::Mixed
                if matches!(
                    now_ms,
                    0 | 1_000 | 1_050 | 3_000 | 4_950 | 5_000 | 7_000 | 9_950
                ) =>
            {
                29_216
            }
            Self::Mixed => 4_096,
            Self::Overload => 29_216,
        }
    }
}

#[derive(Clone, Copy)]
struct Profile {
    name: &'static str,
    one_way_ms: u64,
    jitter_ms: u64,
    down_loss: u64,
    up_loss: u64,
    duplicate: u64,
    reorder: u64,
    reorder_ms: u64,
    down_rate: u64,
    up_rate: u64,
    rto_ms: u64,
    outage: Option<(u64, u64)>,
    up_extra_delay_ms: u64,
}

const CLEAN: Profile = Profile {
    name: "clean80",
    one_way_ms: 40,
    jitter_ms: 0,
    down_loss: 0,
    up_loss: 0,
    duplicate: 0,
    reorder: 0,
    reorder_ms: 0,
    down_rate: 160_000,
    up_rate: 160_000,
    rto_ms: 250,
    outage: None,
    up_extra_delay_ms: 0,
};
const BAD: Profile = Profile {
    name: "bad150_loss3",
    one_way_ms: 75,
    jitter_ms: 30,
    down_loss: 3,
    up_loss: 3,
    duplicate: 2,
    reorder: 15,
    reorder_ms: 30,
    rto_ms: 450,
    ..CLEAN
};
const HARSH: Profile = Profile {
    name: "harsh500_loss10",
    one_way_ms: 250,
    jitter_ms: 100,
    down_loss: 10,
    up_loss: 10,
    duplicate: 2,
    reorder: 20,
    reorder_ms: 80,
    rto_ms: 1_100,
    ..CLEAN
};
const OUTAGE: Profile = Profile {
    name: "bad150_outage800",
    outage: Some((4_000, 4_800)),
    ..BAD
};
const CONGESTED: Profile = Profile {
    name: "bad150_link80000",
    down_rate: 80_000,
    rto_ms: 600,
    ..BAD
};
const REVERSE: Profile = Profile {
    name: "bad150_ack_loss20_delay200",
    up_loss: 20,
    up_extra_delay_ms: 200,
    rto_ms: 700,
    ..BAD
};
const CLEAN_SLOW: Profile = Profile {
    name: "clean80_link80000",
    down_rate: 80_000,
    rto_ms: 450,
    ..CLEAN
};
const PROFILES: [Profile; 7] = [CLEAN, BAD, HARSH, OUTAGE, CONGESTED, REVERSE, CLEAN_SLOW];

#[derive(Clone)]
struct Snapshot {
    identity: Identity,
    created_ms: u64,
    bytes: Vec<u8>,
}

fn fixture(workload: Workload) -> Vec<Snapshot> {
    (0..RUN_MS)
        .step_by(FLUSH_MS as usize)
        .enumerate()
        .map(|(index, now)| {
            let identity: Identity = (if now < 5_000 { 1 } else { 2 }, index as u64 + 1);
            let mut random = identity.1.wrapping_mul(0xa076_1d64_78bd_642f);
            let mut bytes = vec![0; workload.size(now)];
            for byte in &mut bytes {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                *byte = random as u8;
            }
            bytes[..8].copy_from_slice(&identity.0.to_le_bytes());
            bytes[8..16].copy_from_slice(&identity.1.to_le_bytes());
            bytes[16..24].copy_from_slice(&now.to_le_bytes());
            Snapshot {
                identity,
                created_ms: now,
                bytes,
            }
        })
        .collect()
}

fn identity(bytes: &[u8]) -> Identity {
    assert!(bytes.len() >= 24);
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
    )
}

fn byte_limits() -> ByteLimits {
    ByteLimits {
        fragment_payload_bytes: FRAME_LIMIT
            - engine_net::codec::HEADER_BYTES
            - application_fragments::BYTE_FRAGMENT_HEADER_BYTES,
        group_bytes: 32 * 1024,
        fragments: 32,
        incomplete: 2,
        known_groups: 4,
        staging_bytes: 64 * 1024,
        max_age_ms: 2_000,
    }
}

fn application_frames(snapshot: &Snapshot) -> Vec<Vec<u8>> {
    fragment_group(
        GroupPublication {
            connection: ConnectionEpoch(1),
            group: GroupId(1),
            revision: GroupRevision(snapshot.identity.0),
            snapshot: SnapshotId(snapshot.identity.1),
        },
        &snapshot.bytes,
        byte_limits(),
    )
    .unwrap()
    .into_iter()
    .enumerate()
    .map(|(sequence, fragment)| {
        encode_frame(
            FrameHeader {
                lane: Lane::State,
                connection: ConnectionEpoch(1),
                sequence: sequence as u32,
            },
            &fragment.encode(byte_limits()).unwrap(),
            FrameLimit::new(FRAME_LIMIT).unwrap(),
        )
        .unwrap()
    })
    .collect()
}

fn config(path: Path, profile: Profile) -> ConnectionConfig {
    let resend_time = Duration::from_millis(profile.rto_ms);
    let channels = vec![
        ChannelConfig {
            channel_id: CONTROL,
            max_memory_usage_bytes: 32 * 1024,
            send_type: SendType::ReliableOrdered { resend_time },
        },
        ChannelConfig {
            channel_id: REMOTE,
            max_memory_usage_bytes: 32 * 1024,
            send_type: SendType::Unreliable,
        },
        ChannelConfig {
            channel_id: STATE,
            max_memory_usage_bytes: path.capacity(),
            send_type: if path == Path::A {
                SendType::Unreliable
            } else {
                SendType::ReliableUnordered { resend_time }
            },
        },
        ChannelConfig {
            channel_id: INPUT,
            max_memory_usage_bytes: 32 * 1024,
            send_type: SendType::Unreliable,
        },
    ];
    ConnectionConfig {
        available_bytes_per_tick: RENET_BUDGET,
        server_channels_config: channels.clone(),
        client_channels_config: channels,
    }
}

#[derive(Default)]
struct Metrics {
    generated: usize,
    admitted: usize,
    applied: usize,
    obsolete_completed: usize,
    down_packets: usize,
    down_renet_bytes: usize,
    down_wire_bytes: usize,
    up_wire_bytes: usize,
    state_payload_bytes: usize,
    state_retry_bytes: usize,
    retry_obsolete_generated_bytes: usize,
    retry_obsolete_applied_bytes: usize,
    retry_obsolete_receipted_bytes: usize,
    obsolete_generated_bytes: usize,
    obsolete_applied_bytes: usize,
    random_down_drops: usize,
    random_up_drops: usize,
    outage_drops: usize,
    queue_down_drops: usize,
    queue_up_drops: usize,
    pacer_down_drops: usize,
    pacer_up_drops: usize,
    duplicated: usize,
    max_sender_bytes: usize,
    max_pending: usize,
    max_assembler_bytes: usize,
    max_down_queue_us: u64,
    max_up_queue_us: u64,
    reset_required_ms: Option<u64>,
    disconnected_ms: Option<u64>,
    disconnect_reason: Option<String>,
    final_applied_ms: Option<u64>,
    outage_recovery_ms: Option<u64>,
    state_age_ms: Vec<u64>,
    controls: BTreeMap<u64, u64>,
    remote_delivered: usize,
    input_delivered: usize,
    max_blackout_ms: u64,
    a_admitted_frames: usize,
    a_packetized_frames: usize,
    unique_state_parts: BTreeSet<(Identity, u64)>,
}

/// Test observer for the production adapter's unavailable stock `send_due`
/// query. It observes actual emitted control message IDs and delivered transport
/// ACK ranges; it never invents acknowledgements or alters reliable delivery.
/// A uses only its resulting due-byte count to avoid enqueueing an unreliable
/// frame that stock Renet would immediately discard for insufficient credit.
#[derive(Default)]
struct ControlTracker {
    messages: BTreeMap<u64, Option<u64>>,
    packets: BTreeMap<u64, Vec<u64>>,
}

impl ControlTracker {
    fn due_bytes(&self, now_ms: u64, rto_ms: u64) -> usize {
        self.messages
            .values()
            .filter(|sent| sent.is_none_or(|sent| now_ms - sent >= rto_ms))
            .count()
            * CONTROL_BYTES
    }
    fn sent(&mut self, now_ms: u64, bytes: &[u8]) {
        let mut packet = Octets::with_slice(bytes);
        if packet.get_u8().unwrap() != 0 {
            return;
        }
        let sequence = packet.get_varint().unwrap();
        if packet.get_u8().unwrap() != CONTROL {
            return;
        }
        let count = packet.get_u16().unwrap();
        let mut ids = Vec::new();
        for _ in 0..count {
            let id = packet.get_varint().unwrap();
            let len = packet.get_varint().unwrap() as usize;
            assert_eq!(len, CONTROL_BYTES);
            packet.skip(len).unwrap();
            *self.messages.get_mut(&id).unwrap() = Some(now_ms);
            ids.push(id);
        }
        self.packets.insert(sequence, ids);
    }
    fn receive_ack(&mut self, bytes: &[u8]) {
        let mut packet = Octets::with_slice(bytes);
        if packet.get_u8().unwrap() != 4 {
            return;
        }
        packet.get_varint().unwrap();
        let end = packet.get_varint().unwrap();
        let size = packet.get_varint().unwrap();
        let count = packet.get_varint().unwrap();
        let mut start = end - size;
        self.ack_range(start, end);
        for _ in 0..count {
            let gap = packet.get_varint().unwrap();
            let end = start - gap - 2;
            let size = packet.get_varint().unwrap();
            start = end - size;
            self.ack_range(start, end);
        }
    }
    fn ack_range(&mut self, start: u64, end: u64) {
        let sequences: Vec<_> = self
            .packets
            .range(start..=end)
            .map(|(sequence, _)| *sequence)
            .collect();
        for sequence in sequences {
            for id in self.packets.remove(&sequence).unwrap() {
                self.messages.remove(&id);
            }
        }
    }
}

struct Pacer {
    tokens: u64,
    last_ms: u64,
}

impl Pacer {
    fn new() -> Self {
        Self {
            tokens: EGRESS_BURST,
            last_ms: 0,
        }
    }
    fn take(&mut self, now_ms: u64, bytes: usize, priority: bool) -> bool {
        self.tokens =
            (self.tokens + (now_ms - self.last_ms) * EGRESS_RATE / 1_000).min(EGRESS_BURST);
        self.last_ms = now_ms;
        let spendable = if priority {
            self.tokens
        } else {
            self.tokens.saturating_sub(CONTROL_RESERVE)
        };
        if spendable < bytes as u64 {
            return false;
        }
        self.tokens -= bytes as u64;
        true
    }
}

#[derive(Clone)]
struct Datagram {
    down: bool,
    bytes: Vec<u8>,
}

struct Link {
    profile: Profile,
    random: [u64; 2],
    serial: u64,
    free_us: [u64; 2],
    packets: BTreeMap<(u64, u64), Datagram>,
    pacers: [Pacer; 2],
}

impl Link {
    fn new(profile: Profile, seed: u64) -> Self {
        Self {
            profile,
            random: [seed ^ 0xb5ad_4ece_da1c_e2a9, seed ^ 0x94d0_49bb_1331_11eb],
            serial: 0,
            free_us: [0; 2],
            packets: BTreeMap::new(),
            pacers: [Pacer::new(), Pacer::new()],
        }
    }
    fn random(&mut self, direction: usize) -> u64 {
        let random = &mut self.random[direction];
        *random ^= *random << 13;
        *random ^= *random >> 7;
        *random ^= *random << 17;
        *random
    }
    fn insert(&mut self, due_us: u64, down: bool, bytes: Vec<u8>) {
        self.packets
            .insert((due_us, self.serial), Datagram { down, bytes });
        self.serial += 1;
    }
    fn send(&mut self, now_ms: u64, down: bool, bytes: Vec<u8>, metrics: &mut Metrics) {
        let direction = usize::from(!down);
        let mut header = Octets::with_slice(&bytes);
        let kind = header.get_u8().unwrap();
        header.get_varint().unwrap();
        let priority = kind == 4 || header.get_u8().unwrap() == CONTROL;
        let wire_bytes = bytes.len() + DATAGRAM_OVERHEAD;
        if !self.pacers[direction].take(now_ms, wire_bytes, priority) {
            if down {
                metrics.pacer_down_drops += 1;
            } else {
                metrics.pacer_up_drops += 1;
            }
            return;
        }
        if down {
            metrics.down_packets += 1;
            metrics.down_renet_bytes += bytes.len();
            metrics.down_wire_bytes += wire_bytes;
        } else {
            metrics.up_wire_bytes += wire_bytes;
        }
        let now_us = now_ms * 1_000;
        let rate = if down {
            self.profile.down_rate
        } else {
            self.profile.up_rate
        };
        let serialized_us = (wire_bytes as u64 * 1_000_000).div_ceil(rate);
        let complete_us = self.free_us[direction].max(now_us) + serialized_us;
        if complete_us > now_us + 100_000 {
            if down {
                metrics.queue_down_drops += 1;
            } else {
                metrics.queue_up_drops += 1;
            }
            return;
        }
        self.free_us[direction] = complete_us;
        if down {
            metrics.max_down_queue_us = metrics.max_down_queue_us.max(complete_us - now_us);
        } else {
            metrics.max_up_queue_us = metrics.max_up_queue_us.max(complete_us - now_us);
        }
        let random = self.random(direction);
        if self
            .profile
            .outage
            .is_some_and(|(start, end)| now_ms >= start && now_ms < end)
        {
            metrics.outage_drops += 1;
            return;
        }
        let loss = if down {
            self.profile.down_loss
        } else {
            self.profile.up_loss
        };
        if random % 100 < loss {
            if down {
                metrics.random_down_drops += 1;
            } else {
                metrics.random_up_drops += 1;
            }
            return;
        }
        let jitter = random.rotate_left(17) % (2 * self.profile.jitter_ms + 1);
        let mut delay_ms = self.profile.one_way_ms - self.profile.jitter_ms + jitter;
        if !down {
            delay_ms += self.profile.up_extra_delay_ms;
        }
        if random.rotate_left(31) % 100 < self.profile.reorder {
            delay_ms += self.profile.reorder_ms;
        }
        let due_us = complete_us + delay_ms * 1_000;
        if random.rotate_left(47) % 100 < self.profile.duplicate {
            metrics.duplicated += 1;
            self.insert(due_us + 1_000, down, bytes.clone());
        }
        self.insert(due_us, down, bytes);
    }
    fn receive(
        &mut self,
        now_ms: u64,
        server: &mut RenetServer,
        client: &mut RenetClient,
        control: &mut ControlTracker,
    ) {
        while self
            .packets
            .first_key_value()
            .is_some_and(|((due, _), _)| *due <= now_ms * 1_000)
        {
            let (_, datagram) = self.packets.pop_first().unwrap();
            if datagram.down {
                client.process_packet(&datagram.bytes);
            } else {
                control.receive_ack(&datagram.bytes);
                server.process_packet_from(&datagram.bytes, CLIENT).unwrap();
            }
            if !client.is_connected() || !server.is_connected(CLIENT) {
                break;
            }
        }
    }
}

fn observe_part(
    metrics: &mut Metrics,
    key: Identity,
    part: u64,
    bytes: usize,
    generated: Identity,
    applied: Option<Identity>,
    receipted: Option<Identity>,
) {
    metrics.state_payload_bytes += bytes;
    let retry = !metrics.unique_state_parts.insert((key, part));
    let older_generated = key < generated;
    let older_applied = applied.is_some_and(|current| key < current);
    if older_generated {
        metrics.obsolete_generated_bytes += bytes;
    }
    if older_applied {
        metrics.obsolete_applied_bytes += bytes;
    }
    if retry {
        metrics.state_retry_bytes += bytes;
        if older_generated {
            metrics.retry_obsolete_generated_bytes += bytes;
        }
        if older_applied {
            metrics.retry_obsolete_applied_bytes += bytes;
        }
        if receipted.is_some_and(|current| key < current) {
            metrics.retry_obsolete_receipted_bytes += bytes;
        }
    }
}

fn observe_packet(
    bytes: &[u8],
    path: Path,
    transport_ids: &[Identity],
    generated: Identity,
    applied: Option<Identity>,
    receipted: Option<Identity>,
    metrics: &mut Metrics,
) {
    // Observer only. In particular, kind2 has BOTH num_slices and length.
    let mut packet = Octets::with_slice(bytes);
    let kind = packet.get_u8().unwrap();
    packet.get_varint().unwrap();
    if kind == 4 {
        return;
    }
    let channel = packet.get_u8().unwrap();
    if channel != STATE {
        return;
    }
    match kind {
        0 => {
            assert_ne!(path, Path::A);
            let count = packet.get_u16().unwrap();
            for _ in 0..count {
                let message = packet.get_varint().unwrap() as usize;
                let length = packet.get_varint().unwrap() as usize;
                let payload = packet.get_bytes(length).unwrap();
                let key = identity(payload.as_ref());
                assert_eq!(transport_ids[message], key);
                observe_part(metrics, key, 0, length, generated, applied, receipted);
            }
        }
        1 => {
            assert_eq!(path, Path::A);
            let count = packet.get_u16().unwrap();
            for _ in 0..count {
                let length = packet.get_varint().unwrap() as usize;
                let payload = packet.get_bytes(length).unwrap();
                let (_, body) = decode_frame(
                    payload.as_ref(),
                    ConnectionEpoch(1),
                    FrameLimit::new(FRAME_LIMIT).unwrap(),
                )
                .unwrap();
                let fragment = ByteFragment::decode(body, byte_limits()).unwrap();
                let p = fragment.header.publication;
                metrics.a_packetized_frames += 1;
                observe_part(
                    metrics,
                    (p.revision.0, p.snapshot.0),
                    fragment.index as u64,
                    fragment.payload.len(),
                    generated,
                    applied,
                    receipted,
                );
            }
        }
        2 => {
            assert_ne!(path, Path::A);
            let message = packet.get_varint().unwrap() as usize;
            let slice = packet.get_varint().unwrap();
            let num_slices = packet.get_varint().unwrap();
            let length = packet.get_varint().unwrap() as usize;
            assert!(slice < num_slices && length <= 1_200);
            packet.get_bytes(length).unwrap();
            observe_part(
                metrics,
                transport_ids[message],
                slice,
                length,
                generated,
                applied,
                receipted,
            );
        }
        other => panic!("unexpected state packet kind {other}"),
    }
    assert_eq!(packet.cap(), 0);
}

struct Transfer {
    snapshot: usize,
    frames: Vec<Vec<u8>>,
    cursor: usize,
    pass_done_ms: Option<u64>,
}

impl Transfer {
    fn new(snapshot: usize, fixture: &[Snapshot]) -> Self {
        Self {
            snapshot,
            frames: application_frames(&fixture[snapshot]),
            cursor: 0,
            pass_done_ms: None,
        }
    }
}

fn capture_disconnect(
    now_ms: u64,
    server: &RenetServer,
    client: &RenetClient,
    metrics: &mut Metrics,
    applied_created_ms: u64,
    last_applied_ms: u64,
    has_applied: bool,
) -> bool {
    if client.is_connected() && server.is_connected(CLIENT) {
        return false;
    }
    metrics.disconnected_ms = Some(now_ms);
    metrics.final_applied_ms = None;
    metrics.disconnect_reason = Some(format!(
        "client={:?};server={:?}",
        client.disconnect_reason(),
        server.disconnect_reason(CLIENT)
    ));
    // Preserve the planned denominator and account for the outage after failure.
    metrics.generated = (RUN_MS / FLUSH_MS) as usize;
    if now_ms < RUN_MS {
        metrics.max_blackout_ms = metrics
            .max_blackout_ms
            .max(RUN_MS - TICK_MS - last_applied_ms);
    }
    for now in (now_ms..RUN_MS).step_by(TICK_MS as usize) {
        metrics.state_age_ms.push(if has_applied {
            now - applied_created_ms
        } else {
            now + TICK_MS
        });
    }
    true
}

fn run(path: Path, profile: Profile, workload: Workload, seed: u64) -> Metrics {
    let fixture = fixture(workload);
    let config = config(path, profile);
    let mut server = RenetServer::new(config.clone());
    server.add_connection(CLIENT);
    let mut client_config = config;
    client_config
        .server_channels_config
        .iter_mut()
        .find(|channel| channel.channel_id == STATE)
        .unwrap()
        .max_memory_usage_bytes = STATE_CAPACITY;
    let mut client = RenetClient::new(client_config);
    client.set_connected();
    let mut link = Link::new(profile, seed);
    let mut assembler = ByteAssembler::new(ConnectionEpoch(1), byte_limits()).unwrap();
    let mut metrics = Metrics::default();
    let mut transfer: Option<Transfer> = None;
    let mut latest = 0usize;
    let mut applied: Option<Identity> = None;
    let mut applied_created_ms = 0;
    let mut last_applied_ms = 0;
    let mut received = BTreeSet::<Identity>::new();
    let mut admitted = BTreeSet::<Identity>::new();
    let mut pending = BTreeSet::<Identity>::new();
    let mut transport_ids = Vec::<Identity>::new();
    let mut control_tracker = ControlTracker::default();
    let mut previous_ms = 0;

    for now_ms in (0..=RUN_MS + SETTLE_MS).step_by(TICK_MS as usize) {
        let elapsed = Duration::from_millis(now_ms - previous_ms);
        previous_ms = now_ms;
        server.update(elapsed);
        client.update(elapsed);
        link.receive(now_ms, &mut server, &mut client, &mut control_tracker);
        if capture_disconnect(
            now_ms,
            &server,
            &client,
            &mut metrics,
            applied_created_ms,
            last_applied_ms,
            applied.is_some(),
        ) {
            break;
        }
        if path == Path::A && assembler.expire(now_ms).unwrap() > 0 {
            metrics.reset_required_ms.get_or_insert(now_ms);
        }
        while let Some(message) = client.receive_message(CONTROL) {
            assert_eq!(message.len(), CONTROL_BYTES);
            let id = u64::from_le_bytes(message[..8].try_into().unwrap());
            let sent = u64::from_le_bytes(message[8..16].try_into().unwrap());
            assert!(message[16..].iter().all(|byte| *byte == 0xc7));
            assert_eq!(sent, (id - 1) * FLUSH_MS);
            assert!(metrics.controls.insert(id, now_ms - sent).is_none());
        }
        while client.receive_message(REMOTE).is_some() {
            metrics.remote_delivered += 1;
        }
        while server.receive_message(CLIENT, INPUT).is_some() {
            metrics.input_delivered += 1;
        }
        while let Some(message) = server.receive_message(CLIENT, CONTROL) {
            assert_eq!(message.len(), 16);
            let key = (
                u64::from_le_bytes(message[..8].try_into().unwrap()),
                u64::from_le_bytes(message[8..].try_into().unwrap()),
            );
            received.insert(key);
            pending.remove(&key);
        }
        while let Some(message) = client.receive_message(STATE) {
            let complete = if path == Path::A {
                let (_, payload) = decode_frame(
                    &message,
                    ConnectionEpoch(1),
                    FrameLimit::new(FRAME_LIMIT).unwrap(),
                )
                .unwrap();
                let fragment = ByteFragment::decode(payload, byte_limits()).unwrap();
                match assembler.receive(fragment, now_ms) {
                    Ok(Some(complete)) => {
                        let p = complete.publication();
                        assert_eq!(identity(complete.as_bytes()), (p.revision.0, p.snapshot.0));
                        Some(complete.as_bytes().to_vec())
                    }
                    Ok(None) => None,
                    Err(engine_net::replication::ReplicationError::Obsolete(_)) => None,
                    Err(engine_net::replication::ReplicationError::Expired) => {
                        metrics.reset_required_ms.get_or_insert(now_ms);
                        None
                    }
                    Err(error) => panic!("unexpected assembler error: {error:?}"),
                }
            } else {
                Some(message.to_vec())
            };
            let Some(bytes) = complete else {
                continue;
            };
            let key = identity(&bytes);
            let snapshot = &fixture[key.1 as usize - 1];
            assert_eq!(snapshot.identity, key);
            assert_eq!(
                bytes, snapshot.bytes,
                "all bytes must match one immutable publication"
            );
            // Receipts are for decode, not current-state publication. Older C
            // messages must free their admission slots too.
            let mut receipt = Vec::with_capacity(16);
            receipt.extend_from_slice(&key.0.to_le_bytes());
            receipt.extend_from_slice(&key.1.to_le_bytes());
            client.send_message(CONTROL, receipt);
            if applied.is_none_or(|current| key > current) {
                metrics.max_blackout_ms = metrics.max_blackout_ms.max(now_ms - last_applied_ms);
                last_applied_ms = now_ms;
                applied_created_ms = snapshot.created_ms;
                applied = Some(key);
                metrics.applied += 1;
                if key == fixture.last().unwrap().identity {
                    metrics.final_applied_ms.get_or_insert(now_ms);
                }
                if let Some((_, end)) = profile.outage
                    && snapshot.created_ms >= end
                {
                    metrics.outage_recovery_ms.get_or_insert(now_ms - end);
                }
            } else {
                metrics.obsolete_completed += 1;
            }
        }

        if now_ms < RUN_MS && now_ms % FLUSH_MS == 0 {
            latest = (now_ms / FLUSH_MS) as usize;
            metrics.generated += 1;
            let mut control = Vec::with_capacity(CONTROL_BYTES);
            control.extend_from_slice(&(latest as u64 + 1).to_le_bytes());
            control.extend_from_slice(&now_ms.to_le_bytes());
            control.resize(CONTROL_BYTES, 0xc7);
            control_tracker.messages.insert(latest as u64, None);
            server.send_message(CLIENT, CONTROL, control);
            server.send_message(CLIENT, REMOTE, vec![0x5a; 512]);
            client.send_message(INPUT, vec![0xa5; 48]);
        }
        if now_ms % FLUSH_MS == 0 {
            if path == Path::A {
                let replace = transfer.as_ref().is_none_or(|t| {
                    fixture[t.snapshot].identity.0 != fixture[latest].identity.0
                        || (t.pass_done_ms.is_some() && t.snapshot != latest)
                });
                if replace {
                    transfer = Some(Transfer::new(latest, &fixture));
                    metrics.admitted += 1;
                }
                let t = transfer.as_mut().unwrap();
                let key = fixture[t.snapshot].identity;
                let repair = t
                    .pass_done_ms
                    .is_some_and(|done| now_ms - done >= profile.rto_ms)
                    && !received.contains(&key)
                    && now_ms >= RUN_MS;
                if repair {
                    t.cursor = 0;
                    t.pass_done_ms = None;
                }
                let competing = control_tracker.due_bytes(now_ms, profile.rto_ms)
                    + if now_ms < RUN_MS { 512 } else { 0 };
                let mut remaining =
                    PREFIX_BUDGET.min((RENET_BUDGET as usize).saturating_sub(competing));
                while t.pass_done_ms.is_none()
                    && t.cursor < t.frames.len()
                    && t.frames[t.cursor].len() <= remaining
                {
                    let frame = &t.frames[t.cursor];
                    assert!(server.can_send_message(CLIENT, STATE, frame.len()));
                    server.send_message(CLIENT, STATE, frame.clone());
                    metrics.a_admitted_frames += 1;
                    remaining -= frame.len();
                    t.cursor += 1;
                    if t.cursor == t.frames.len() {
                        t.pass_done_ms = Some(now_ms);
                    }
                }
            } else {
                let snapshot = &fixture[latest];
                if pending.len() < path.window()
                    && !admitted.contains(&snapshot.identity)
                    && server.can_send_message(CLIENT, STATE, snapshot.bytes.len())
                {
                    server.send_message(CLIENT, STATE, snapshot.bytes.clone());
                    assert!(admitted.insert(snapshot.identity));
                    pending.insert(snapshot.identity);
                    transport_ids.push(snapshot.identity);
                    metrics.admitted += 1;
                }
            }
            metrics.max_sender_bytes = metrics
                .max_sender_bytes
                .max(path.capacity() - server.channel_available_memory(CLIENT, STATE));
            metrics.max_pending = metrics.max_pending.max(pending.len());
            for packet in server.get_packets_to_send(CLIENT).unwrap() {
                control_tracker.sent(now_ms, &packet);
                observe_packet(
                    &packet,
                    path,
                    &transport_ids,
                    fixture[latest].identity,
                    applied,
                    received.last().copied(),
                    &mut metrics,
                );
                link.send(now_ms, true, packet, &mut metrics);
            }
            for packet in client.get_packets_to_send() {
                link.send(now_ms, false, packet, &mut metrics);
            }
        }
        metrics.max_assembler_bytes = metrics.max_assembler_bytes.max(assembler.staged_bytes());
        if capture_disconnect(
            now_ms,
            &server,
            &client,
            &mut metrics,
            applied_created_ms,
            last_applied_ms,
            applied.is_some(),
        ) {
            break;
        }
        if now_ms < RUN_MS {
            metrics.state_age_ms.push(if applied.is_some() {
                now_ms - applied_created_ms
            } else {
                now_ms + TICK_MS
            });
            metrics.max_blackout_ms = metrics.max_blackout_ms.max(now_ms - last_applied_ms);
        }
    }
    assert_eq!(metrics.generated, fixture.len());
    assert_eq!(metrics.state_age_ms.len(), (RUN_MS / TICK_MS) as usize);
    assert!(metrics.max_sender_bytes <= path.capacity());
    assert!(metrics.max_assembler_bytes <= byte_limits().staging_bytes);
    assert!(path == Path::A || metrics.max_pending <= path.window());
    assert_eq!(
        metrics.a_admitted_frames, metrics.a_packetized_frames,
        "A must not silently lose fragments to Renet's per-flush allowance"
    );
    metrics
}

fn percentile(values: &[u64], percent: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    values[((values.len() * percent).div_ceil(100))
        .saturating_sub(1)
        .min(values.len() - 1)]
}

fn optional(value: Option<u64>) -> String {
    value.map_or_else(|| "null".into(), |value| value.to_string())
}

fn print_row(path: Path, profile: Profile, workload: Workload, seed: u64, m: &Metrics) {
    let controls: Vec<_> = m.controls.values().copied().collect();
    let control_missed =
        m.generated - m.controls.len() + controls.iter().filter(|v| **v > 500).count();
    let over500 = m.state_age_ms.iter().filter(|v| **v > 500).count();
    eprintln!(
        concat!(
            "ADVERSE {{\"path\":\"{}\",\"profile\":\"{}\",\"workload\":\"{}\",\"seed\":{},",
            "\"generated\":{},\"admitted\":{},\"applied\":{},\"obsolete_completed\":{},",
            "\"age_p50_ms\":{},\"age_p95_ms\":{},\"age_p99_ms\":{},\"age_max_ms\":{},\"age_over500_samples\":{},\"age_samples\":{},\"blackout_max_ms\":{},",
            "\"control_delivered\":{},\"control_p95_ms\":{},\"control_max_ms\":{},\"control_missed500\":{},\"remote_delivered\":{},\"input_delivered\":{},",
            "\"down_renet_bytes\":{},\"down_wire_bytes\":{},\"down_packets\":{},\"up_wire_bytes\":{},\"state_payload_bytes\":{},\"state_retry_bytes\":{},",
            "\"obsolete_generated_bytes\":{},\"obsolete_applied_bytes\":{},\"retry_obsolete_generated_bytes\":{},\"retry_obsolete_applied_bytes\":{},\"retry_obsolete_receipted_bytes\":{},",
            "\"random_down_drops\":{},\"random_up_drops\":{},\"queue_down_drops\":{},\"queue_up_drops\":{},\"pacer_down_drops\":{},\"pacer_up_drops\":{},\"outage_drops\":{},\"duplicates\":{},",
            "\"max_sender_bytes\":{},\"max_pending\":{},\"max_assembler_bytes\":{},\"max_down_queue_us\":{},\"max_up_queue_us\":{},",
            "\"reset_required_ms\":{},\"disconnected_ms\":{},\"disconnect_reason\":{},\"final_lag_ms\":{},\"outage_recovery_ms\":{} }}"
        ),
        path.name(),
        profile.name,
        workload.name(),
        seed,
        m.generated,
        m.admitted,
        m.applied,
        m.obsolete_completed,
        percentile(&m.state_age_ms, 50),
        percentile(&m.state_age_ms, 95),
        percentile(&m.state_age_ms, 99),
        m.state_age_ms.iter().max().unwrap(),
        over500,
        m.state_age_ms.len(),
        m.max_blackout_ms,
        controls.len(),
        percentile(&controls, 95),
        controls.iter().max().unwrap_or(&0),
        control_missed,
        m.remote_delivered,
        m.input_delivered,
        m.down_renet_bytes,
        m.down_wire_bytes,
        m.down_packets,
        m.up_wire_bytes,
        m.state_payload_bytes,
        m.state_retry_bytes,
        m.obsolete_generated_bytes,
        m.obsolete_applied_bytes,
        m.retry_obsolete_generated_bytes,
        m.retry_obsolete_applied_bytes,
        m.retry_obsolete_receipted_bytes,
        m.random_down_drops,
        m.random_up_drops,
        m.queue_down_drops,
        m.queue_up_drops,
        m.pacer_down_drops,
        m.pacer_up_drops,
        m.outage_drops,
        m.duplicated,
        m.max_sender_bytes,
        m.max_pending,
        m.max_assembler_bytes,
        m.max_down_queue_us,
        m.max_up_queue_us,
        optional(m.reset_required_ms),
        optional(m.disconnected_ms),
        m.disconnect_reason
            .as_ref()
            .map_or_else(|| "null".into(), |reason| format!("\"{reason}\"")),
        optional(m.final_applied_ms.map(|now| now - (RUN_MS - FLUSH_MS))),
        optional(m.outage_recovery_ms)
    );
}

#[test]
fn compare_adverse_application_fragments_and_reliable_unordered() {
    for workload in Workload::ALL {
        for profile in PROFILES {
            for path in Path::ALL {
                for seed in SEEDS {
                    let metrics = run(path, profile, workload, seed);
                    print_row(path, profile, workload, seed, &metrics);
                }
            }
        }
    }
}

#[test]
fn clean_fixture_has_atomic_final_delivery_and_no_reliable_retries() {
    for workload in [Workload::Compact, Workload::Steady, Workload::Mixed] {
        for path in Path::ALL {
            let metrics = run(path, CLEAN, workload, SEEDS[0]);
            assert!(metrics.final_applied_ms.is_some(), "{path:?} {workload:?}");
            assert_eq!(metrics.reset_required_ms, None);
            assert_eq!(metrics.disconnected_ms, None);
            assert_eq!(metrics.controls.len(), 200);
            assert_eq!(metrics.pacer_down_drops + metrics.queue_down_drops, 0);
            if path != Path::A {
                assert_eq!(
                    metrics.state_retry_bytes, 0,
                    "clean RTO must exceed RTT+batching"
                );
            }
        }
    }
}

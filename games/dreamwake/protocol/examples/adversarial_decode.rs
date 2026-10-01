//! Deterministic hostile corpus qualification of real protocol/replica codecs.
//! This is not coverage-guided fuzzing or a complete security qualification.
//! Run through scripts/qualify-codecs.py for CPU limits and source provenance.
#[path = "adversarial/allocator.rs"]
mod allocator;
#[path = "adversarial/corpus.rs"]
mod corpus;
#[path = "adversarial/generated.rs"]
mod generated;
#[path = "adversarial/p11.rs"]
mod p11;
use corpus::{Corpus, entity};
use dreamwake_protocol::{DreamAction, live};
use dreamwake_sim::{
    DreamInput,
    replication::{OwnerExpectation, PublicReplica, Replica, ReplicaState, ReplicationStamp},
};
use engine_net::{
    codec::{self, BoundedVec, FrameHeader, FrameLimit, Lane},
    commands::{ActionEdge, OwnerStream},
    replication::stream::{Base, ClientStream, StreamAck},
    types::*,
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    time::Instant,
};
const EPOCH: ConnectionEpoch = ConnectionEpoch(2);
const MAX_INPUT: usize = 32_768;
const NAMES: [&str; 14] = [
    "input",
    "client_control",
    "server_control",
    "welcome",
    "public_state",
    "private_forgery",
    "state_delta",
    "state_packet",
    "compression",
    "nonfinite",
    "generated_fields",
    "combat_view_timing",
    "action_outcomes",
    "outcomes_ack",
];
fn limit() -> FrameLimit {
    FrameLimit::new(1100).unwrap()
}
fn state_limit() -> FrameLimit {
    FrameLimit::new(live::MAX_STATE_FRAME_BYTES).unwrap()
}
struct Random(u64);
impl Random {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn index(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}
fn mutate(original: &[u8], rng: &mut Random) -> Vec<u8> {
    let mut bytes = original.to_vec();
    match rng.index(16) {
        0 => {} // Retained valid controls distinguish rejection-only harnesses.
        1 => {
            let n = rng.index(bytes.len() + 1);
            bytes.truncate(n);
        }
        2 => {
            if !bytes.is_empty() {
                let at = rng.index(bytes.len());
                bytes[at] ^= 1 << rng.index(8);
            }
        }
        3 => {
            if !bytes.is_empty() {
                let at = rng.index(bytes.len());
                bytes[at] = 255;
            }
        }
        4 => {
            let at = rng.index(bytes.len() + 1);
            bytes.insert(at, 0);
        }
        5 => {
            if !bytes.is_empty() {
                let at = rng.index(bytes.len());
                bytes.remove(at);
            }
        }
        6 | 7 => {
            if bytes.len() >= 8 {
                let at = rng.index(bytes.len() - 7);
                bytes[at..at + 8].fill(255);
            }
        }
        8 => {
            if bytes.len() >= 4 {
                let at = rng.index(bytes.len() - 3);
                bytes[at..at + 4].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
            }
        }
        9 => {
            let len = rng.index(257);
            bytes = (0..len).map(|_| rng.next() as u8).collect();
        }
        10 => {
            bytes.resize(MAX_INPUT, 255);
        }
        11 => {
            bytes.extend_from_slice(&[0; 16]);
        }
        12 => {
            if bytes.len() >= 8 {
                bytes[..8].copy_from_slice(&u64::MAX.to_le_bytes());
            }
        }
        13 => {
            if bytes.len() >= 24 {
                bytes[10..18].copy_from_slice(&99u64.to_le_bytes());
            }
        }
        14 => {
            for _ in 0..8 {
                if !bytes.is_empty() {
                    let at = rng.index(bytes.len());
                    bytes[at] = rng.next() as u8;
                }
            }
        }
        _ => {
            if bytes.len() >= 4 {
                let at = rng.index(bytes.len() - 3);
                bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            }
        }
    }
    assert!(bytes.len() <= MAX_INPUT);
    bytes
}
#[derive(Default, Clone, Copy)]
struct Outcome {
    accepted: bool,
    published: bool,
    reason: &'static str,
}
fn accepted(value: bool) -> Outcome {
    Outcome {
        accepted: value,
        published: false,
        reason: if value { "accepted" } else { "decode_rejected" },
    }
}
/// A receiving stream holding the entities the corpus packet names as bases.
fn client(c: &Corpus) -> ClientStream<EntityId, ReplicaState> {
    let mut client = ClientStream::new(8);
    client
        .accept(3, vec![(entity(7), c.delta_base.clone())], &[])
        .unwrap();
    client
}
fn run(family: usize, bytes: &[u8], case: usize, c: &Corpus) -> Outcome {
    match family {
        0 => accepted(live::decode_input(bytes, EPOCH, limit()).is_ok()),
        1 => accepted(live::decode_control::<live::ClientControl>(bytes, EPOCH, limit()).is_ok()),
        2 => accepted(live::decode_control::<live::ServerControl>(bytes, EPOCH, limit()).is_ok()),
        3 => accepted(live::decode_welcome(bytes).is_ok()),
        4 => {
            let value = ReplicaState::decode(bytes, None);
            let restored = value.as_ref().ok().map(|state| state.restore(None));
            assert!(
                !matches!(restored, Some(Ok(Replica::Owner(_)))),
                "public decode disclosed owner state"
            );
            if let (Ok(state), Some(Ok(_))) = (&value, &restored) {
                let mut roundtrip = Vec::new();
                state.encode(None, &mut roundtrip).unwrap();
                assert_eq!(ReplicaState::decode(&roundtrip, None).unwrap(), *state);
            }
            let result = value
                .and_then(|state| state.restore(None))
                .map(|_| ());
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: result.as_ref().err().map_or("accepted", generated::reason),
            }
        }
        5 => {
            let owner = ReplicaState::decode(&c.owner_state, None).unwrap();
            assert!(owner.restore(None).is_err(), "owner restored without expectation");
            let mut wrong = c.expected;
            match case % 5 {
                0 => wrong.owner += 1,
                1 => wrong.match_epoch += 1,
                2 => wrong.scene_revision += 1,
                3 => wrong.ownership_revision += 1,
                _ => wrong.minimum_revision = u64::MAX,
            }
            assert!(
                owner.restore(Some(wrong)).is_err(),
                "forged owner expectation accepted"
            );
            let result = ReplicaState::decode(bytes, None).and_then(|state| state.restore(Some(c.expected)));
            if let Ok(Replica::Owner(owner)) = &result {
                assert_eq!(owner.expectation().owner, c.expected.owner);
            }
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: result.as_ref().err().map_or("accepted", generated::reason),
            }
        }
        6 => {
            let result = ReplicaState::decode(bytes, Some(&c.delta_base))
                .and_then(|state| state.restore(None));
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: result.as_ref().err().map_or("accepted", generated::reason),
            }
        }
        7 => {
            let mut stream = client(c);
            let before = (stream.ack(), stream.current(entity(7)).map(|(seq, s)| (seq, s.clone())));
            let result = live::decode_state(bytes, EPOCH, state_limit())
                .map_err(|_| "packet_rejected")
                .and_then(|packet| {
                    let mut decoded = Vec::new();
                    for entry in &packet.entries {
                        let base = match stream
                            .base(entry.entity, entry.base, packet.seq)
                            .map_err(|_| "base_rejected")?
                        {
                            Base::Superseded => continue,
                            Base::Whole => None,
                            Base::Delta(base) => Some(base),
                        };
                        let state = ReplicaState::decode(&entry.body, base)
                            .map_err(|_| "entry_rejected")?;
                        state.restore(None).map_err(|_| "entry_rejected")?;
                        decoded.push((entry.entity, state));
                    }
                    stream
                        .accept(packet.seq, decoded, &packet.removed)
                        .map_err(|_| "stream_rejected")
                });
            if result.is_err() {
                assert_eq!(
                    (stream.ack(), stream.current(entity(7)).map(|(seq, s)| (seq, s.clone()))),
                    before,
                    "a rejected packet changed the stream"
                );
            }
            Outcome {
                accepted: result.is_ok(),
                published: result.is_ok(),
                reason: result.err().unwrap_or("accepted"),
            }
        }
        8 => {
            accepted(engine_net::decode_with_limit::<BoundedVec<u8, 16384>>(bytes, 16384).is_ok())
        }
        9 => {
            for invalid in &c.nonfinite {
                assert!(
                    live::decode_input(invalid, EPOCH, limit()).is_err(),
                    "nonfinite/unbounded intent accepted"
                );
            }
            let mut bomb = c.compressed.clone();
            assert_eq!(bomb[0], 1);
            bomb[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(engine_net::decode_with_limit::<BoundedVec<u8, 16384>>(&bomb, 16384).is_err());
            let mut count = vec![0];
            count.extend_from_slice(&u64::MAX.to_le_bytes());
            assert!(engine_net::decode_with_limit::<BoundedVec<u8, 16384>>(&count, 16384).is_err());
            accepted(live::decode_input(bytes, EPOCH, limit()).is_ok())
        }
        10 => {
            let item = &c.schema_cases[(case / NAMES.len()) % c.schema_cases.len()];
            let result = generated::decode(item.root, bytes, c.expected);
            assert_eq!(
                result.is_err(),
                item.rejected,
                "generated schema {} {}",
                item.root,
                item.mutation
            );
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: result.as_ref().err().map_or("accepted", generated::reason),
            }
        }
        11 => {
            let item = &c.combat_cases[(case / NAMES.len()) % c.combat_cases.len()];
            let result = live::decode_input(bytes, EPOCH, limit());
            assert_eq!(
                result.is_err(),
                item.rejected,
                "P11 view timing {}",
                item.mutation
            );
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: if result.is_ok() {
                    "accepted"
                } else {
                    item.mutation
                },
            }
        }
        12 => {
            let item = &c.outcome_cases[(case / NAMES.len()) % c.outcome_cases.len()];
            let result = live::decode_control::<live::ServerControl>(bytes, EPOCH, limit());
            assert_eq!(
                result.is_err(),
                item.rejected,
                "P11 action outcome {}",
                item.mutation
            );
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: if result.is_ok() {
                    "accepted"
                } else {
                    item.mutation
                },
            }
        }
        13 => {
            let item = &c.outcomes_ack_cases[(case / NAMES.len()) % c.outcomes_ack_cases.len()];
            let result = live::decode_control::<live::ClientControl>(bytes, EPOCH, limit());
            assert_eq!(
                result.is_err(),
                item.rejected,
                "OutcomesAck {}",
                item.mutation
            );
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: if result.is_ok() {
                    "accepted"
                } else {
                    item.mutation
                },
            }
        }
        _ => unreachable!(),
    }
}
fn main() {
    let mut seed = 0x7a29_d10c_00de_cafeu64;
    let mut cases = 100_000usize;
    let mut replay = None;
    let mut failure_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().expect("flag requires value");
        match arg.as_str() {
            "--seed" => seed = value.parse().expect("decimal seed"),
            "--cases" => cases = value.parse().unwrap(),
            "--case" => replay = Some(value.parse::<usize>().unwrap()),
            "--failure-dir" => failure_dir = Some(std::path::PathBuf::from(value)),
            _ => panic!("unknown flag"),
        }
    }
    assert!(seed != 0 && (1..=1_000_000).contains(&cases));
    let c = Corpus::new();
    // Positive controls ensure each real codec is exercised.
    assert!(
        ReplicaState::decode(&c.owner_state, None)
            .unwrap()
            .restore(Some(c.expected))
            .is_ok()
    );
    assert!(c.welcome.compatible(ConnectionId(9)));
    assert!(live::decode_input(&c.input, EPOCH, limit()).is_ok());
    assert_eq!(run(7, &c.state_packet, 0, &c).published, true);
    assert_eq!(run(6, &c.delta, 0, &c).accepted, true);
    assert_eq!(c.schema_seeds.len(), generated::ROOT_NAMES.len());
    assert_eq!(
        c.schema_seeds
            .iter()
            .map(|(name, _)| *name)
            .collect::<std::collections::BTreeSet<_>>(),
        generated::ROOT_NAMES.into_iter().collect(),
    );
    for (root, seed) in &c.schema_seeds {
        assert!(generated::decode(root, seed, c.expected).is_ok());
    }
    let mut counts = [[0u64; 3]; NAMES.len()];
    let mut family_alloc = [0usize; NAMES.len()];
    let mut family_live = [0usize; NAMES.len()];
    let mut family_calls = [0usize; NAMES.len()];
    let mut reasons: Vec<std::collections::BTreeMap<&'static str, u64>> =
        vec![Default::default(); NAMES.len()];
    let mut schema_counts: std::collections::BTreeMap<(&'static str, &'static str), [u64; 2]> =
        Default::default();
    let mut max_alloc = 0;
    let mut max_live = 0;
    let mut max_calls = 0;
    let started = Instant::now();
    for ordinal in 0..replay.map_or(cases, |_| 1) {
        let case = replay.unwrap_or(ordinal);
        let family = case % NAMES.len();
        let mut rng = Random(
            seed.wrapping_add((case as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
                .max(1),
        );
        let original = match family {
            0 | 9 => &c.input,
            1 => &c.client_control[rng.index(c.client_control.len())],
            2 => &c.server_control[rng.index(c.server_control.len())],
            3 => &c.welcome_bytes,
            4 => &c.live_states[rng.index(c.live_states.len())].1,
            5 => &c.owner_state,
            6 => &c.delta,
            7 => &c.state_packet,
            8 => &c.compressed,
            10 => &c.schema_cases[(case / NAMES.len()) % c.schema_cases.len()].bytes,
            11 => &c.combat_cases[(case / NAMES.len()) % c.combat_cases.len()].bytes,
            12 => &c.outcome_cases[(case / NAMES.len()) % c.outcome_cases.len()].bytes,
            _ => &c.outcomes_ack_cases[(case / NAMES.len()) % c.outcomes_ack_cases.len()].bytes,
        };
        let bytes = if family >= 10 {
            original.clone()
        } else {
            mutate(original, &mut rng)
        };
        allocator::start(case);
        let outcome = catch_unwind(AssertUnwindSafe(|| run(family, &bytes, case, &c)));
        let (allocated, live, calls) = allocator::finish();
        max_alloc = max_alloc.max(allocated);
        max_live = max_live.max(live);
        max_calls = max_calls.max(calls);
        family_alloc[family] = family_alloc[family].max(allocated);
        family_live[family] = family_live[family].max(live);
        family_calls[family] = family_calls[family].max(calls);
        match outcome {
            Ok(value) => {
                counts[family][usize::from(value.accepted)] += 1;
                counts[family][2] += u64::from(value.published);
                *reasons[family].entry(value.reason).or_default() += 1;
                if family == 10 {
                    let item = &c.schema_cases[(case / NAMES.len()) % c.schema_cases.len()];
                    schema_counts.entry((item.root, item.mutation)).or_default()
                        [usize::from(value.accepted)] += 1;
                }
            }
            Err(_) => {
                if let Some(dir) = &failure_dir {
                    std::fs::create_dir_all(dir).unwrap();
                    std::fs::write(
                        dir.join(format!("case-{case}-{}.bin", NAMES[family])),
                        &bytes,
                    )
                    .unwrap();
                }
                eprintln!(
                    "campaign_failure seed={seed} case={case} family={} bytes={} replay=--seed {seed} --case {case}",
                    NAMES[family],
                    bytes.len()
                );
                std::process::exit(1);
            }
        }
    }
    println!(
        "{{\"kind\":\"deterministic_adversarial_codec_campaign\",\"seed\":{seed},\"production_qualified\":false,\"security_qualified\":false,\"generated_roots\":11,\"cases\":{},\"elapsed_seconds\":{:.6},\"max_input_bytes\":{MAX_INPUT},\"case_allocation_cap_bytes\":{},\"max_case_allocated_bytes\":{max_alloc},\"max_case_live_bytes\":{max_live},\"max_case_allocations\":{max_calls},\"families\":[",
        replay.map_or(cases, |_| 1),
        started.elapsed().as_secs_f64(),
        allocator::CASE_BYTES
    );
    for (i, name) in NAMES.iter().enumerate() {
        let reason_json = reasons[i]
            .iter()
            .map(|(reason, count)| format!("\"{reason}\":{count}"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{}{{\"name\":\"{name}\",\"rejected\":{},\"accepted\":{},\"published\":{},\"max_allocated_bytes\":{},\"max_live_bytes\":{},\"max_allocations\":{},\"reasons\":{{{reason_json}}}}}",
            if i == 0 { "" } else { "," },
            counts[i][0],
            counts[i][1],
            counts[i][2],
            family_alloc[i],
            family_live[i],
            family_calls[i]
        );
    }
    println!("],\"generated_cases\":[");
    for (i, ((root, mutation), count)) in schema_counts.iter().enumerate() {
        println!(
            "{}{{\"root\":\"{root}\",\"mutation\":\"{mutation}\",\"rejected\":{},\"accepted\":{}}}",
            if i == 0 { "" } else { "," },
            count[0],
            count[1]
        );
    }
    println!("]}}");
}

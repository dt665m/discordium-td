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
use corpus::{Corpus, entity, state};
use dreamwake_protocol::{
    DreamAction, live,
    replication::{ReplicaPayload, decode_replica, encode_replica},
};
use dreamwake_sim::{
    DreamInput,
    replication::{OwnerExpectation, ReplicationStamp},
};
use engine_net::replication::baselines::*;
use engine_net::{
    codec::{self, BoundedVec, FrameHeader, FrameLimit, Lane},
    commands::{ActionEdge, Command, OwnerStream},
    replication::*,
    types::*,
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    time::Instant,
};
const EPOCH: ConnectionEpoch = ConnectionEpoch(2);
const MAX_INPUT: usize = 32_768;
const NAMES: [&str; 19] = [
    "commands",
    "client_control",
    "server_control",
    "welcome",
    "public_replica",
    "private_forgery",
    "full_scope",
    "group_manifest",
    "byte_fragments",
    "baseline_delta",
    "compression",
    "nonfinite",
    "live_state",
    "generated_fields",
    "owner_dependencies",
    "combat_view_timing",
    "action_outcomes",
    "finalized_ack",
    "outcomes_ack",
];
fn limit() -> FrameLimit {
    FrameLimit::new(1100).unwrap()
}
fn group_limits() -> GroupLimits {
    GroupLimits {
        members: 3,
        ..Default::default()
    }
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
fn client(c: &Corpus) -> ClientScopes<Vec<u8>> {
    let mut client = ClientScopes::new(
        EPOCH,
        ScopeLimits {
            known_entities: 8,
            ..Default::default()
        },
    )
    .unwrap();
    client
        .apply_full(state(1, c.global.clone(), 1), |_| true)
        .unwrap();
    client
        .apply_full(state(2, c.owner.clone(), 1), |_| true)
        .unwrap();
    client
        .apply_full(state(3, c.collision.clone(), 1), |_| true)
        .unwrap();
    client
}
fn unchanged(client: &ClientScopes<Vec<u8>>, c: &Corpus) {
    assert_eq!(
        client.known_entities(),
        3,
        "failure changed published entity count"
    );
    assert_eq!(
        client.state(entity(1)),
        Some(&state(1, c.global.clone(), 1)),
        "failure changed global publication"
    );
    assert_eq!(
        client.state(entity(2)),
        Some(&state(2, c.owner.clone(), 1)),
        "failure changed private publication"
    );
    assert_eq!(
        client.state(entity(3)),
        Some(&state(3, c.collision.clone(), 1)),
        "failure changed collision publication"
    );
}
fn decode_owner_group(bytes: &[u8], c: &Corpus) -> Result<GroupChunk<Vec<u8>>, ReplicationError> {
    // Same positive binding constraints as the live owner's three-member group.
    let group = decode_group(bytes, group_limits(), |p| {
        decode_replica(p, Some(c.expected)).map_err(|_| ReplicationError::InvalidPayload)?;
        Ok(p.to_vec())
    })?;
    if group.publication.group != live::OWNER_GROUP || group.members.len() != 3 {
        return Err(ReplicationError::InvalidPayload);
    }
    let mut global = None;
    let mut owner = None;
    let mut collision = None;
    for value in &group.members {
        match decode_replica(&value.payload, Some(c.expected))
            .map_err(|_| ReplicationError::InvalidPayload)?
        {
            ReplicaPayload::Global(g) if value.scope.entity == entity(1) => global = Some(g),
            ReplicaPayload::Owner(o) if value.scope.entity == entity(2) => owner = Some(o),
            ReplicaPayload::Collision(m) if value.scope.entity == entity(3) => collision = Some(m),
            _ => return Err(ReplicationError::InvalidPayload),
        }
    }
    if global
        .as_ref()
        .zip(owner.as_ref())
        .is_none_or(|(g, o)| o.context() != g || g.stamp.server_tick != group.end_tick.0)
    {
        return Err(ReplicationError::InvalidPayload);
    }
    if owner
        .as_ref()
        .zip(collision.as_ref())
        .is_none_or(|(owner, collision)| {
            owner.collision_identity() != collision.identity()
                || owner.stamp().scene_revision != collision.scene_revision()
        })
    {
        return Err(ReplicationError::InvalidPayload);
    }
    Ok(group)
}
fn apply_full(decoded: Result<FullState<Vec<u8>>, ReplicationError>, c: &Corpus) -> Outcome {
    let mut client = client(c);
    let result = decoded.and_then(|s| client.apply_full(s, |p| decode_replica(p, None).is_ok()));
    if result.is_err() {
        unchanged(&client, c);
    }
    assert!(client.retained_bytes() <= ScopeLimits::default().retained_payload_bytes);
    Outcome {
        accepted: result.is_ok(),
        published: result.is_ok(),
        reason: if result.is_ok() {
            "accepted"
        } else {
            "publication_rejected"
        },
    }
}
fn run(family: usize, bytes: &[u8], case: usize, c: &Corpus) -> Outcome {
    match family {
        0 => accepted(live::decode_commands(bytes, EPOCH, limit()).is_ok()),
        1 => accepted(live::decode_control::<live::ClientControl>(bytes, EPOCH, limit()).is_ok()),
        2 => accepted(live::decode_control::<live::ServerControl>(bytes, EPOCH, limit()).is_ok()),
        3 => accepted(live::decode_welcome(bytes).is_ok()),
        4 => {
            let value = decode_replica(bytes, None);
            assert!(
                !matches!(value, Ok(ReplicaPayload::Owner(_))),
                "public decode disclosed owner payload"
            );
            if let Ok(ref value) = value {
                let roundtrip = encode_replica(value).unwrap();
                assert_eq!(decode_replica(&roundtrip, None).unwrap(), *value);
            }
            Outcome {
                accepted: value.is_ok(),
                published: false,
                reason: value.as_ref().err().map_or("accepted", generated::reason),
            }
        }
        5 => {
            assert!(decode_replica(&c.owner, None).is_err());
            let mut wrong = c.expected;
            match case % 5 {
                0 => wrong.owner += 1,
                1 => wrong.match_epoch += 1,
                2 => wrong.scene_revision += 1,
                3 => wrong.ownership_revision += 1,
                _ => wrong.minimum_revision = u64::MAX,
            }
            assert!(
                decode_replica(&c.owner, Some(wrong)).is_err(),
                "forged owner expectation accepted"
            );
            for tag in [0, 2, 3, 255] {
                let mut retagged = c.owner.clone();
                retagged[0] = tag;
                assert!(
                    decode_replica(&retagged, Some(c.expected)).is_err(),
                    "private payload retagged public"
                );
            }
            let result = decode_replica(bytes, Some(c.expected));
            if let Ok(ReplicaPayload::Owner(owner)) = &result {
                assert_eq!(owner.expectation().owner, c.expected.owner);
            }
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: result.as_ref().err().map_or("accepted", generated::reason),
            }
        }
        6 => apply_full(decode_full_state(bytes, EPOCH, 16384), c),
        7 => {
            let mut client = client(c);
            let mut groups = GroupAssembler::new(EPOCH, group_limits()).unwrap();
            let result = decode_owner_group(bytes, c).and_then(|g| {
                groups.receive(g, 0, &mut client, |p| {
                    decode_replica(p, Some(c.expected)).is_ok()
                })
            });
            if !matches!(result, Ok(GroupProgress::Published(_))) {
                unchanged(&client, c);
            }
            assert!(groups.staged_bytes() <= group_limits().staging_bytes);
            Outcome {
                accepted: result.is_ok(),
                published: matches!(result, Ok(GroupProgress::Published(_))),
                reason: if result.is_ok() {
                    "accepted"
                } else {
                    "group_rejected"
                },
            }
        }
        8 => {
            let mut client = client(c);
            let mut groups = GroupAssembler::new(EPOCH, group_limits()).unwrap();
            let limits = live::byte_limits(limit()).unwrap();
            let mut assembler = ByteAssembler::new(EPOCH, limits).unwrap();
            let mut published = false;
            let decoded = live::decode_state(bytes, EPOCH, limit());
            let result = match decoded {
                Ok(live::StateFrame::Fragment(fragment)) => {
                    let selected = case % c.fragments.len();
                    let mut fragments = vec![fragment];
                    for (i, encoded) in c.fragments.iter().enumerate() {
                        if i != selected {
                            let live::StateFrame::Fragment(f) =
                                live::decode_state(encoded, EPOCH, limit()).unwrap()
                            else {
                                unreachable!()
                            };
                            fragments.push(f);
                        }
                    }
                    let mut result = Ok(());
                    for fragment in fragments {
                        let next = assembler.receive(fragment, 0).and_then(|complete| {
                            if let Some(complete) = complete {
                                let progress = complete.decode_and_publish(
                                    &mut groups,
                                    &mut client,
                                    0,
                                    |b| decode_owner_group(b, c),
                                    |p| decode_replica(p, Some(c.expected)).is_ok(),
                                )?;
                                published = matches!(progress, GroupProgress::Published(_));
                            }
                            Ok(())
                        });
                        if next.is_err() {
                            result = next;
                            break;
                        }
                    }
                    result
                }
                _ => Err(ReplicationError::InvalidPayload),
            };
            if !published {
                unchanged(&client, c);
            }
            assert!(assembler.staged_bytes() <= limits.staging_bytes);
            assert!(assembler.incomplete() <= limits.incomplete);
            Outcome {
                accepted: result.is_ok(),
                published,
                reason: if result.is_ok() {
                    "accepted"
                } else {
                    "fragment_rejected"
                },
            }
        }
        9 => {
            let mut client = ClientBaselines::new(
                c.baseline.target.context,
                c.baseline.target.generation,
                BaselineLimits::default(),
            )
            .unwrap();
            client
                .receive(&c.baseline, &BytePatch, |p| decode_replica(p, None).is_ok())
                .unwrap();
            let before = client.current().cloned();
            let mut packet = BaselinePacket {
                target: BaselineReceipt {
                    snapshot: SnapshotId(2),
                    version: StateVersion(2),
                    ..c.baseline.target
                },
                encoding: BaselineEncoding::Delta {
                    base: c.baseline.target,
                },
                bytes: bytes.to_vec(),
            };
            match case % 8 {
                0 => packet.target.context.scope.connection = ConnectionEpoch(99),
                1 => packet.target.context.scope.scope = ScopeEpoch(2),
                2 => packet.target.context.scope.entity.generation = 2,
                3 => packet.target.generation = BaselineGeneration(2),
                _ => {}
            }
            let result = client.receive(&packet, &BytePatch, |p| decode_replica(p, None).is_ok());
            if result.is_err() {
                assert_eq!(
                    client.current(),
                    before.as_ref(),
                    "failed delta changed published baseline"
                );
                assert_eq!(client.retained_states(), 1);
            }
            assert!(client.retained_bytes() <= BaselineLimits::default().resident_bytes);
            Outcome {
                accepted: result.is_ok(),
                published: result.is_ok(),
                reason: if result.is_ok() {
                    "accepted"
                } else {
                    "publication_rejected"
                },
            }
        }
        10 => {
            accepted(engine_net::decode_with_limit::<BoundedVec<u8, 16384>>(bytes, 16384).is_ok())
        }
        11 => {
            for invalid in &c.nonfinite {
                assert!(
                    live::decode_commands(invalid, EPOCH, limit()).is_err(),
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
            accepted(live::decode_commands(bytes, EPOCH, limit()).is_ok())
        }
        12 => {
            let decoded = match live::decode_state(bytes, EPOCH, limit()) {
                Ok(live::StateFrame::Full(v)) => Ok(v),
                _ => Err(ReplicationError::InvalidPayload),
            };
            apply_full(decoded, c)
        }
        13 => {
            let item = &c.schema_cases[(case / NAMES.len()) % c.schema_cases.len()];
            let result = decode_replica(bytes, Some(c.expected));
            assert_eq!(
                result.is_err(),
                item.rejected,
                "generated schema {} {}",
                item.root,
                item.mutation
            );
            if let Ok(ref decoded) = result {
                assert_eq!(
                    decode_replica(&encode_replica(decoded).unwrap(), Some(c.expected)).unwrap(),
                    *decoded
                );
            }
            Outcome {
                accepted: result.is_ok(),
                published: false,
                reason: result.as_ref().err().map_or("accepted", generated::reason),
            }
        }
        14 => {
            let item = &c.dependency_cases[(case / NAMES.len()) % c.dependency_cases.len()];
            let mut client = client(c);
            let mut groups = GroupAssembler::new(EPOCH, group_limits()).unwrap();
            let result = decode_owner_group(bytes, c).and_then(|g| {
                groups.receive(g, 0, &mut client, |p| {
                    decode_replica(p, Some(c.expected)).is_ok()
                })
            });
            assert_eq!(
                result.is_err(),
                item.rejected,
                "owner group {}",
                item.mutation
            );
            if result.is_err() {
                unchanged(&client, c);
            }
            assert!(groups.staged_bytes() <= group_limits().staging_bytes);
            Outcome {
                accepted: result.is_ok(),
                published: matches!(result, Ok(GroupProgress::Published(_))),
                reason: if result.is_ok() {
                    "accepted"
                } else {
                    item.mutation
                },
            }
        }
        15 => {
            let item = &c.combat_cases[(case / NAMES.len()) % c.combat_cases.len()];
            let result = live::decode_commands(bytes, EPOCH, limit());
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
        16 => {
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
        17 => {
            let item = &c.finalized_ack_cases[(case / NAMES.len()) % c.finalized_ack_cases.len()];
            let result = live::decode_control::<live::ClientControl>(bytes, EPOCH, limit());
            assert_eq!(
                result.is_err(),
                item.rejected,
                "FinalizedAck {}",
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
        18 => {
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
    // Positive controls ensure each real codec and the complete private group are exercised.
    assert!(decode_replica(&c.owner, Some(c.expected)).is_ok());
    assert!(c.welcome.compatible(ConnectionId(9)));
    assert_eq!(decode_owner_group(&c.group, &c).unwrap().members.len(), 3);
    assert_eq!(c.schema_seeds.len(), generated::ROOT_NAMES.len());
    assert_eq!(
        c.schema_seeds
            .iter()
            .map(|(name, _)| *name)
            .collect::<std::collections::BTreeSet<_>>(),
        generated::ROOT_NAMES.into_iter().collect(),
    );
    for (_, seed) in &c.schema_seeds {
        assert!(decode_replica(seed, Some(c.expected)).is_ok());
    }
    assert_eq!(c.state.payload, c.global);
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
            0 | 11 => &c.command,
            1 => &c.client_control[rng.index(c.client_control.len())],
            2 => &c.server_control[rng.index(c.server_control.len())],
            3 => &c.welcome_bytes,
            4 => {
                let public: Vec<_> = c
                    .schema_seeds
                    .iter()
                    .filter(|(name, _)| *name != "owner")
                    .collect();
                &public[rng.index(public.len())].1
            }
            5 => &c.owner,
            6 => &c.full,
            7 => &c.group,
            8 => &c.fragments[case % c.fragments.len()],
            9 => &c.delta,
            10 => &c.compressed,
            12 => &c.live_state,
            13 => &c.schema_cases[(case / NAMES.len()) % c.schema_cases.len()].bytes,
            14 => &c.dependency_cases[(case / NAMES.len()) % c.dependency_cases.len()].bytes,
            15 => &c.combat_cases[(case / NAMES.len()) % c.combat_cases.len()].bytes,
            16 => &c.outcome_cases[(case / NAMES.len()) % c.outcome_cases.len()].bytes,
            17 => &c.finalized_ack_cases[(case / NAMES.len()) % c.finalized_ack_cases.len()].bytes,
            _ => &c.outcomes_ack_cases[(case / NAMES.len()) % c.outcomes_ack_cases.len()].bytes,
        };
        let bytes = if family >= 13 {
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
                if family == 13 {
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

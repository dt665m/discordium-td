use super::*;
use engine_net::commands::{ActionEdge, FinalizedStatus};

// Form hostile wire payloads without letting the safe encoder reject them first.
// A serde newtype preserves the wrapped payload's exact representation.
#[derive(Serialize)]
struct Unchecked<T>(T);
impl<T> Validate for Unchecked<T> {
    fn validate(&self) -> Result<(), CodecError> {
        Ok(())
    }
}

fn finalized(arrival: ArrivalFeedback) -> ServerControl {
    ServerControl::Finalized {
        through: ServerTick(10),
        receipts: BoundedVec::new(vec![FinalizedReceipt {
            tick: ServerTick(10),
            sequence: Some(CommandSeq(4)),
            status: FinalizedStatus::Executed,
        }])
        .unwrap(),
        menu_applied: Some(CommandSeq(2)),
        arrival,
    }
}

#[test]
fn arrival_feedback_roundtrips_identity_counters_and_separate_finalization() {
    for sample in [
        Some(ArrivalSample {
            sequence: CommandSeq((1_u64 << 40) + 6),
            target: TargetTick((1_u64 << 40) + 18),
            arrival_tick: ServerTick((1_u64 << 40) + 10),
            slack: 8,
        }),
        Some(ArrivalSample {
            sequence: CommandSeq(6),
            target: TargetTick(22),
            arrival_tick: ServerTick(10),
            slack: 12,
        }),
        Some(ArrivalSample {
            sequence: CommandSeq(3),
            target: TargetTick(9),
            arrival_tick: ServerTick(10),
            slack: 0,
        }),
        Some(ArrivalSample {
            sequence: CommandSeq(4),
            target: TargetTick(10),
            arrival_tick: ServerTick(10),
            slack: 0,
        }),
        None,
    ] {
        // Feedback can concern an unconsumed command or an earlier missed one.
        // It must neither replace nor infer the independent finalized receipt.
        let control = finalized(ArrivalFeedback {
            sample,
            observed_commands: u64::MAX,
            late_commands: u64::MAX - 1,
        });
        let wire = encode_control(&control, ConnectionEpoch(2), 17, limit()).unwrap();
        assert!(wire.len() <= limit().payload_bytes() + codec::HEADER_BYTES);
        assert_eq!(
            decode_control::<ServerControl>(&wire, ConnectionEpoch(2), limit()).unwrap(),
            control
        );
        assert_eq!(
            decode_control::<ServerControl>(&wire, ConnectionEpoch(3), limit()).unwrap_err(),
            CodecError::WrongEpoch
        );
        let mut trailing = wire.clone();
        trailing.push(0);
        assert!(decode_control::<ServerControl>(&trailing, ConnectionEpoch(2), limit()).is_err());
        assert!(
            decode_control::<ServerControl>(&wire[..wire.len() - 1], ConnectionEpoch(2), limit())
                .is_err()
        );
    }
}

#[test]
fn arrival_feedback_rejects_impossible_samples_on_encoding_and_decoding() {
    let good = ArrivalFeedback {
        sample: Some(ArrivalSample {
            sequence: CommandSeq(6),
            target: TargetTick(18),
            arrival_tick: ServerTick(10),
            slack: 8,
        }),
        observed_commands: 6,
        late_commands: 2,
    };
    for mutation in 0..10 {
        let mut bad = good;
        match mutation {
            0 => bad.late_commands = bad.observed_commands + 1,
            1 => {
                bad.observed_commands = 0;
                bad.late_commands = 0;
            }
            2 => bad.sample.as_mut().unwrap().sequence = CommandSeq(0),
            3 => bad.sample.as_mut().unwrap().target = TargetTick(0),
            4 => {
                bad.sample.as_mut().unwrap().target = TargetTick(23);
                bad.sample.as_mut().unwrap().slack = 13;
            }
            5 => bad.sample.as_mut().unwrap().slack = 7,
            6 => bad.sample.as_mut().unwrap().slack = 0,
            7 => bad.sample.as_mut().unwrap().target = TargetTick(9),
            8 => {
                bad.sample.as_mut().unwrap().target = TargetTick(9);
                bad.sample.as_mut().unwrap().slack = 0;
                bad.late_commands = 0;
            }
            _ => bad.late_commands = bad.observed_commands,
        }
        let control = finalized(bad);
        assert!(
            encode_control(&control, ConnectionEpoch(2), 17, limit()).is_err(),
            "encoder accepted mutation {mutation}"
        );
        let malformed =
            encode_control(&Unchecked(control), ConnectionEpoch(2), 17, limit()).unwrap();
        assert!(
            decode_control::<ServerControl>(&malformed, ConnectionEpoch(2), limit()).is_err(),
            "decoder accepted mutation {mutation}"
        );
    }
}

#[test]
fn pre_identity_feedback_layout_and_previous_protocol_are_rejected() {
    let ServerControl::Finalized {
        through,
        receipts,
        menu_applied,
        ..
    } = finalized(ArrivalFeedback::default())
    else {
        unreachable!()
    };
    // Protocol 19 encoded Finalized as variant 4, followed by these fields and
    // only Option<u8> slack. It cannot be interpreted as identified feedback.
    for old_slack in [None, Some(0_u8), Some(12)] {
        let legacy = Unchecked((4_u32, through, &receipts, menu_applied, old_slack));
        let bytes = encode_control(&legacy, ConnectionEpoch(2), 17, limit()).unwrap();
        assert!(decode_control::<ServerControl>(&bytes, ConnectionEpoch(2), limit()).is_err());
    }
    let mut old = welcome();
    old.protocol = 0x4452_4541_4d00_0013;
    assert!(!old.compatible(old.client));
    let mut bytes = WELCOME_MAGIC.to_vec();
    bytes.extend(codec::encode_payload(&Unchecked(old), WELCOME_BYTES - 4).unwrap());
    assert!(decode_welcome(&bytes).is_err());
}

fn entity(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
fn welcome() -> Welcome {
    Welcome {
        server_instance: 1,
        protocol: PROTOCOL_ID,
        client: ConnectionId(9),
        player: crate::player::PlayerId::new(99).unwrap(),
        stream: OwnerStream {
            connection: ConnectionId(9),
            epoch: ConnectionEpoch(2),
            stream: CommandStream(1),
            owner: entity(2),
            ownership: OwnershipEpoch(1),
        },
        match_epoch: 3,
        tick: ServerTick(10),
        server_time_nanos: 100,
        content: ContentIdentity::current(SceneRevision(1)),
        global_entity: entity(1),
        owner_entity: entity(2),
        collision_entity: entity(3),
        application_frame_bytes: 1100,
    }
}
fn limit() -> FrameLimit {
    FrameLimit::new(1100).unwrap()
}
fn command() -> Command<TickInput, DreamAction> {
    Command {
        owner: welcome().stream,
        sequence: CommandSeq(1),
        target: TargetTick(11),
        input: DreamInput {
            movement: [0.6, 0.8],
            aim: [1.0, 0.0],
            attack: true,
            ..Default::default()
        }
        .into(),
        actions: BoundedVec::new(vec![ActionEdge {
            slot: 0,
            action: DreamAction::Dash {
                direction: [1.0, 0.0],
            },
        }])
        .unwrap(),
    }
}
fn bundle(command: Command<TickInput, DreamAction>) -> CommandBundle {
    CommandBundle {
        records: BoundedVec::new(vec![command]).unwrap(),
    }
}
fn state(index: u64, incarnation: u64, version: u64, payload: Vec<u8>) -> FullState<Vec<u8>> {
    FullState {
        scope: ScopeIdentity {
            connection: ConnectionEpoch(2),
            entity: entity(index),
            scope: ScopeEpoch(incarnation),
            representation: RepresentationRevision(1),
        },
        baseline_generation: BaselineGeneration(version),
        snapshot: SnapshotId(version),
        version: StateVersion(version),
        end_tick: ServerTick(10),
        payload,
    }
}

#[test]
fn welcome_binds_authenticated_client_content_protocol_and_usable_frame_limit() {
    let good = welcome();
    assert_eq!(
        decode_welcome(&encode_welcome(&good).unwrap()).unwrap(),
        good
    );
    assert!(good.compatible(ConnectionId(9)));
    assert!(!good.compatible(ConnectionId(8)));
    let mut bad = good.clone();
    bad.content.ruleset[0] ^= 1;
    assert!(!bad.compatible(good.client));
    bad = good.clone();
    bad.content.scene[0] ^= 1;
    assert!(!bad.compatible(good.client));
    for change in 0..8 {
        let mut bad = good.clone();
        match change {
            0 => bad.protocol ^= 1,
            1 => bad.stream.connection = ConnectionId(8),
            2 => bad.stream.epoch = ConnectionEpoch(0),
            3 => bad.match_epoch = 0,
            4 => bad.stream.owner = entity(3),
            5 => bad.application_frame_bytes = 1087,
            6 => bad.content.scene_revision = SceneRevision(0),
            _ => bad.global_entity = bad.owner_entity,
        }
        assert!(encode_welcome(&bad).is_err(), "mutation {change}");
    }
    assert!(byte_limits(FrameLimit::new(1087).unwrap()).is_err());
    assert!(byte_limits(FrameLimit::new(1088).unwrap()).is_ok());
    let mut wrong_protocol = encode_welcome(&good).unwrap();
    wrong_protocol[4] ^= 1;
    assert!(decode_welcome(&wrong_protocol).is_err());
    let mut wire = encode_welcome(&good).unwrap();
    wire.push(0);
    assert!(decode_welcome(&wire).is_err());
    wire[0] ^= 1;
    assert!(decode_welcome(&wire).is_err());
}

#[test]
fn commands_reject_nonfinite_out_of_range_and_mixed_action_lanes() {
    let valid = bundle(command());
    let wire = encode_commands(&valid, ConnectionEpoch(2), 1, limit()).unwrap();
    assert_eq!(
        decode_commands(&wire, ConnectionEpoch(2), limit()).unwrap(),
        valid
    );
    for change in 0..11 {
        let mut bad = command();
        match change {
            0 => bad.input.held.movement[0] = f32::NAN,
            1 => bad.input.held.aim[1] = f32::INFINITY,
            2 => bad.input.held.movement = [1.0, 1.0],
            3 => bad.input.held.dash = true,
            4 => bad.input.held.casts[0] = true,
            5 => bad.input.held.action_sequences[0] = 1,
            6 => {
                bad.actions = BoundedVec::new(vec![ActionEdge {
                    slot: 0,
                    action: DreamAction::Restart,
                }])
                .unwrap()
            }
            7 => {
                bad.actions = BoundedVec::new(vec![ActionEdge {
                    slot: 0,
                    action: DreamAction::Cast {
                        slot: 4,
                        aim: [1.0, 0.0],
                    },
                }])
                .unwrap()
            }
            8 => bad.actions.push(bad.actions.as_slice()[0].clone()).unwrap(),
            9 => bad.sequence = CommandSeq(0),
            _ => bad.target = TargetTick(0),
        }
        assert!(
            encode_commands(&bundle(bad), ConnectionEpoch(2), 1, limit()).is_err(),
            "mutation {change}"
        );
    }
    for action in [
        DreamAction::Dash {
            direction: [1.0, 0.0],
        },
        DreamAction::Cast {
            slot: 0,
            aim: [1.0, 0.0],
        },
        DreamAction::Choose { choice: 3, slot: 0 },
    ] {
        assert!(
            encode_control(
                &ClientControl::Menu {
                    sequence: CommandSeq(1),
                    action
                },
                ConnectionEpoch(2),
                1,
                limit()
            )
            .is_err()
        );
    }
    assert!(BoundedVec::<_, 8>::new(vec![command(); 9]).is_err());
    assert!(
        encode_commands(
            &CommandBundle {
                records: BoundedVec::default()
            },
            ConnectionEpoch(2),
            1,
            limit()
        )
        .is_err()
    );
}

#[test]
fn command_envelopes_reject_wrong_lane_epoch_lengths_and_hostile_counts() {
    let good = bundle(command());
    let wire = encode_commands(&good, ConnectionEpoch(2), 1, limit()).unwrap();
    assert_eq!(
        decode_commands(&wire, ConnectionEpoch(3), limit()).unwrap_err(),
        CodecError::WrongEpoch
    );
    assert_eq!(
        encode_commands(&good, ConnectionEpoch(3), 1, limit()).unwrap_err(),
        CodecError::WrongEpoch
    );
    let mut changed = wire.clone();
    changed[8] = Lane::State as u8;
    assert_eq!(
        decode_commands(&changed, ConnectionEpoch(2), limit()).unwrap_err(),
        CodecError::InvalidLane
    );
    changed = wire.clone();
    changed.push(0);
    assert!(decode_commands(&changed, ConnectionEpoch(2), limit()).is_err());
    changed = wire.clone();
    changed[22..24].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(decode_commands(&changed, ConnectionEpoch(2), limit()).is_err());
    for count in [9_u64, u64::MAX] {
        // Fixed-integer bincode collection count, without any element bodies.
        // A bounded visitor must reject the declaration before reading/reserving elements.
        let frame = codec::encode_frame(
            FrameHeader {
                lane: Lane::Input,
                connection: ConnectionEpoch(2),
                sequence: 1,
            },
            &count.to_le_bytes(),
            limit(),
        )
        .unwrap();
        let error = decode_commands(&frame, ConnectionEpoch(2), limit()).unwrap_err();
        assert!(
            matches!(error, CodecError::InvalidPayload(ref message) if message.contains("collection count exceeds limit")),
            "{error:?}"
        );
    }
    let mut payload = codec::encode_payload(&good, limit().payload_bytes()).unwrap();
    payload.push(0);
    let frame = codec::encode_frame(
        FrameHeader {
            lane: Lane::Input,
            connection: ConnectionEpoch(2),
            sequence: 1,
        },
        &payload,
        limit(),
    )
    .unwrap();
    assert!(decode_commands(&frame, ConnectionEpoch(2), limit()).is_err());
}

#[test]
fn complete_scope_rejects_malformed_lengths_and_stale_scope_without_mutating_client() {
    let current = state(2, 2, 2, vec![7; 900]);
    let wire = encode_state(
        &StateFrame::Full(current.clone()),
        ConnectionEpoch(2),
        1,
        limit(),
    )
    .unwrap();
    assert!(wire.len() <= 1100);
    let StateFrame::Full(decoded) = decode_state(&wire, ConnectionEpoch(2), limit()).unwrap()
    else {
        panic!("full expected")
    };
    assert_eq!(decoded, current);
    let mut client = ClientScopes::new(ConnectionEpoch(2), ScopeLimits::default()).unwrap();
    client.apply_full(decoded, |_| true).unwrap();
    let stale = state(2, 1, 1, vec![9]);
    let bytes = encode_state(&StateFrame::Full(stale), ConnectionEpoch(2), 2, limit()).unwrap();
    let StateFrame::Full(stale) = decode_state(&bytes, ConnectionEpoch(2), limit()).unwrap() else {
        panic!("full expected")
    };
    assert!(client.apply_full(stale, |_| true).is_err());
    for length in [0, 23, 24, wire.len() - 1] {
        assert!(decode_state(&wire[..length], ConnectionEpoch(2), limit()).is_err());
    }
    let mut hostile = wire.clone();
    // This fixture's eight identity/tick integers each occupy one byte. Change
    // the canonical two-byte payload length from 900 to 16,383 without changing
    // the enclosing frame's valid length or allocating the claimed payload.
    let length_offset = codec::HEADER_BYTES + 1 + 8;
    assert_eq!(&hostile[length_offset..length_offset + 2], &[0x84, 0x07]);
    hostile[length_offset..length_offset + 2].copy_from_slice(&[0xff, 0x7f]);
    assert!(decode_state(&hostile, ConnectionEpoch(2), limit()).is_err());
    assert_eq!(client.state(entity(2)), Some(&current));
}

#[test]
fn fragmented_atomic_group_fits_1100_bytes_and_publishes_only_after_valid_complete_decode() {
    let members = vec![state(1, 1, 1, vec![3; 7000]), state(2, 1, 1, vec![5; 7000])];
    let group = GroupChunk {
        publication: GroupPublication {
            connection: ConnectionEpoch(2),
            group: OWNER_GROUP,
            revision: GroupRevision(1),
            snapshot: SnapshotId(1),
        },
        end_tick: ServerTick(10),
        manifest: members.iter().map(|m| m.scope).collect(),
        index: 0,
        count: 1,
        members,
    };
    let groups = group_limits();
    let original = encode_group(&group, groups, |p| Ok(p.clone())).unwrap();
    let mut assembler =
        ByteAssembler::new(ConnectionEpoch(2), byte_limits(limit()).unwrap()).unwrap();
    let mut client =
        ClientScopes::<Vec<u8>>::new(ConnectionEpoch(2), ScopeLimits::default()).unwrap();
    let mut completed = None;
    for (i, fragment) in fragment_group(group.publication, &original, byte_limits(limit()).unwrap())
        .unwrap()
        .into_iter()
        .rev()
        .enumerate()
    {
        let wire = encode_state(
            &StateFrame::Fragment(fragment.clone()),
            ConnectionEpoch(2),
            i as u32,
            limit(),
        )
        .unwrap();
        assert!(wire.len() <= 1100);
        let StateFrame::Fragment(decoded) =
            decode_state(&wire, ConnectionEpoch(2), limit()).unwrap()
        else {
            panic!("fragment expected")
        };
        assert_eq!(decoded, fragment);
        let mut malformed = wire.clone();
        let header_start = codec::HEADER_BYTES + 1;
        malformed[header_start + 29..header_start + 33].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_state(&malformed, ConnectionEpoch(2), limit()).is_err());
        let mut malformed = wire.clone();
        malformed[header_start + 33..header_start + 35].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(decode_state(&malformed, ConnectionEpoch(2), limit()).is_err());
        assert!(decode_state(&wire, ConnectionEpoch(3), limit()).is_err());
        completed = assembler.receive(decoded, i as u64).unwrap().or(completed);
        assert!(client.state(entity(1)).is_none() && client.state(entity(2)).is_none());
    }
    let mut group_assembler = GroupAssembler::new(ConnectionEpoch(2), groups).unwrap();
    let result = completed
        .unwrap()
        .decode_and_publish(
            &mut group_assembler,
            &mut client,
            100,
            |bytes| decode_group(bytes, groups, |p| Ok(p.to_vec())),
            |_| true,
        )
        .unwrap();
    assert!(matches!(result, GroupProgress::Published(receipts) if receipts.len() == 2));
    assert_eq!(client.state(entity(1)).unwrap().payload, vec![3; 7000]);
    assert_eq!(client.state(entity(2)).unwrap().payload, vec![5; 7000]);
    let mut hostile = original;
    hostile[37..39].copy_from_slice(&u16::MAX.to_le_bytes());
    let called = std::cell::Cell::new(false);
    assert!(
        decode_group(&hostile, groups, |p| {
            called.set(true);
            Ok(p.to_vec())
        })
        .is_err()
    );
    assert!(!called.get());
    assert_eq!(client.state(entity(1)).unwrap().payload, vec![3; 7000]);
}

#[test]
fn complete_enemy_replicas_use_exact_compact_fields() {
    use crate::replication::{ReplicaPayload, decode_replica, encode_replica};
    use dreamwake_sim::replication::{PublicEnemyView, PublicReplica};
    for (kind, windup, radius, hp) in [
        (dreamwake_sim::EnemyKind::Melee, 0.0, 0.0, 100.0),
        (dreamwake_sim::EnemyKind::Ambusher, 0.72, 1.7, 53.25),
        (dreamwake_sim::EnemyKind::Boss, 1.05, 4.9, 1729.75),
    ] {
        let actor = PublicReplica::Enemy(PublicEnemyView {
            id: 9223372036854775901,
            position: [-48.251358, 1.3120865],
            facing: [0.8, 0.6],
            hp,
            max_hp: 2000.0,
            kind,
            windup,
            target: [-44.71875, 3.625],
            warn_radius: radius,
            phase: 1,
            slowed: true,
            hit_flash: 0.125,
        });
        let canonical = actor.encode().unwrap();
        assert_eq!(PublicReplica::decode(&canonical).unwrap(), actor);
        let compact = actor.encode_compact().unwrap();
        assert_eq!(
            compact.len(),
            57,
            "all 56 field bytes and the actor subtype remain"
        );
        assert_eq!(PublicReplica::decode_compact(&compact).unwrap(), actor);
        let payload = ReplicaPayload::Actor(actor);
        let encoded = encode_replica(&payload).unwrap();
        assert_eq!(encoded[1], 0, "compact enemies need no general compression");
        assert_eq!(encoded.len(), 67);
        assert_eq!(decode_replica(&encoded, None).unwrap(), payload);
        let frame_bytes = |bytes: Vec<u8>| {
            encode_state(
                &StateFrame::Full(state(4, 1, 1, bytes)),
                ConnectionEpoch(2),
                1,
                limit(),
            )
            .unwrap()
            .len()
        };
        eprintln!(
            "enemy {kind:?}: canonical actor={} compact actor={} compact replica={} compact full-frame={}",
            canonical.len(),
            compact.len(),
            encoded.len(),
            frame_bytes(encoded)
        );
    }
}

#[test]
fn private_owner_payload_requires_authenticated_expectation_and_cannot_be_retagged_public() {
    use crate::replication::{ReplicaPayload, decode_replica, encode_replica};
    use dreamwake_sim::{DreamSimulation, replication::ReplicationStamp};
    let sim = DreamSimulation::new(123, false);
    let capture = sim
        .capture_replication(ReplicationStamp {
            match_epoch: 3,
            server_tick: 10,
            gameplay_tick: 0,
            scene_revision: 1,
            revision: 1,
        })
        .unwrap();
    let owner = capture.owner_checkpoint(1, 1).unwrap();
    let bytes = encode_replica(&ReplicaPayload::Owner(owner.clone())).unwrap();
    assert_eq!(
        bytes[1], 1,
        "owner checkpoint should use bounded lossless compression"
    );
    assert!(bytes.len() * 4 < owner.encode().unwrap().len() * 3);
    assert!(decode_replica(&bytes, None).is_err());
    assert_eq!(
        decode_replica(&bytes, Some(owner.expectation())).unwrap(),
        ReplicaPayload::Owner(owner.clone())
    );
    for policy in [
        engine_net::CompressionPolicy {
            mode: engine_net::CompressionMode::Off,
            minimum_bytes: 0,
        },
        engine_net::CompressionPolicy {
            minimum_bytes: usize::MAX,
            ..Default::default()
        },
    ] {
        let raw = crate::replication::encode_replica_with_compression(
            &ReplicaPayload::Owner(owner.clone()),
            policy,
        )
        .unwrap();
        assert_eq!(raw[1], 0);
        assert!(decode_replica(&raw, None).is_err());
        assert_eq!(
            decode_replica(&raw, Some(owner.expectation())).unwrap(),
            ReplicaPayload::Owner(owner.clone())
        );
    }
    let mut wrong = owner.expectation();
    wrong.owner = 2;
    assert!(decode_replica(&bytes, Some(wrong)).is_err());
    for tag in [0, 2, 3, 255] {
        let mut retagged = bytes.clone();
        retagged[0] = tag;
        assert!(decode_replica(&retagged, Some(owner.expectation())).is_err());
    }
    let public = encode_replica(&ReplicaPayload::Global(capture.global().clone())).unwrap();
    let ReplicaPayload::Global(decoded) = decode_replica(&public, None).unwrap() else {
        panic!("global representation");
    };
    assert_eq!(&decoded, capture.global());
    let mut public_as_owner = public.clone();
    public_as_owner[0] = 1;
    assert!(decode_replica(&public_as_owner, Some(owner.expectation())).is_err());
    assert!(decode_replica(&public, None).is_ok());

    let mut oversized_expansion = bytes.clone();
    oversized_expansion[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode_replica(&oversized_expansion, Some(owner.expectation())).is_err());
    let mut wrong_expansion = bytes.clone();
    wrong_expansion[2..6].copy_from_slice(&8_u32.to_le_bytes());
    assert!(decode_replica(&wrong_expansion, Some(owner.expectation())).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode_replica(&trailing, Some(owner.expectation())).is_err());
    assert!(decode_replica(&bytes[..bytes.len() / 2], Some(owner.expectation())).is_err());
    let mut hostile_count = vec![0, 0];
    hostile_count.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(decode_replica(&hostile_count, None).is_err());
}

#[test]
fn maximum_declared_schema_envelopes_roundtrip_without_compression() {
    let maxima = dreamwake_sim::replication::owner_group_schema_maxima().unwrap();
    assert_eq!(maxima, [988, 15_354, 11_980, 346]);
    let publication = GroupPublication {
        connection: ConnectionEpoch(2),
        group: OWNER_GROUP,
        revision: GroupRevision(1),
        snapshot: SnapshotId(1),
    };
    // Capacity proof at every generated schema's declared maximum. The semantic
    // schema tests separately restore all maximal collections. Deliberately use
    // raw replica envelopes: compression never supplies the capacity guarantee.
    let mut original_payloads = Vec::new();
    let members: Vec<_> = maxima
        .into_iter()
        .enumerate()
        .map(|(index, bytes)| {
            let actor_tag = usize::from(index == 3);
            let mut payload = vec![index as u8; bytes + 10 + actor_tag];
            payload[0] = [0, 1, 3, 2][index]; // replica type
            payload[1] = 0; // raw, never compressed
            payload[2..10].copy_from_slice(&((bytes + actor_tag) as u64).to_le_bytes());
            if actor_tag != 0 {
                payload[10] = 8;
            } // typed public Platform
            assert!(payload.len() <= baselines::PAYLOAD_BYTES);
            original_payloads.push(payload.clone());
            let mut state = FullState {
                scope: ScopeIdentity {
                    connection: publication.connection,
                    entity: entity(index as u64 + 1),
                    scope: ScopeEpoch(1),
                    representation: RepresentationRevision(1),
                },
                baseline_generation: BaselineGeneration(1),
                snapshot: SnapshotId(1),
                version: StateVersion(1),
                end_tick: ServerTick(10),
                payload,
            };
            let packet = engine_net::replication::baselines::BaselinePacket {
                target: baselines::target(&state, publication, BaselineGeneration(1)),
                encoding: engine_net::replication::baselines::BaselineEncoding::Full,
                bytes: state.payload.clone(),
            };
            state.payload = baselines::encode_member(&packet).unwrap();
            state
        })
        .collect();
    let group = GroupChunk {
        publication,
        end_tick: ServerTick(10),
        manifest: members.iter().map(|state| state.scope).collect(),
        index: 0,
        count: 1,
        members,
    };
    let encoded = encode_group(&group, group_limits(), |payload| Ok(payload.clone())).unwrap();
    assert_eq!(encoded.len(), 29_216);
    assert!(encoded.len() > 16 * 1024);
    assert!(encoded.len() <= baselines::GROUP_BYTES);
    let fragments = fragment_group(publication, &encoded, byte_limits(limit()).unwrap()).unwrap();
    assert!(fragments.len() <= baselines::GROUP_FRAGMENT_COUNT);
    for loss_percent in [0u64, 3] {
        let mut assembler =
            ByteAssembler::new(publication.connection, byte_limits(limit()).unwrap()).unwrap();
        let mut delayed = std::collections::BTreeMap::<(u64, u64), Vec<u8>>::new();
        let mut cursor = 0;
        let mut serial = 0u64;
        let mut completed_at = None;
        let mut result = None;
        // Conservative two application fragments per50ms paced prefix. A stable
        // bootstrap repeats its exact missing-byte transfer under loss; delivery
        // is75ms each direction and no partial group becomes visible.
        for millis in (0u64..2000).step_by(5) {
            if millis % 50 == 0 {
                for _ in 0..2 {
                    let frame = encode_state(
                        &StateFrame::Fragment(fragments[cursor].clone()),
                        publication.connection,
                        serial as u32,
                        limit(),
                    )
                    .unwrap();
                    cursor = (cursor + 1) % fragments.len();
                    serial += 1;
                    if loss_percent == 0 || ![7, 41, 83].contains(&(serial % 100)) {
                        delayed.insert((millis + 75, serial), frame);
                    }
                }
            }
            while delayed
                .first_key_value()
                .is_some_and(|((at, _), _)| *at <= millis)
            {
                let (_, frame) = delayed.pop_first().unwrap();
                let StateFrame::Fragment(fragment) =
                    decode_state(&frame, publication.connection, limit()).unwrap()
                else {
                    unreachable!()
                };
                if let Some(complete) = assembler.receive(fragment, millis).unwrap() {
                    let mut scopes = ClientScopes::<Vec<u8>>::new(
                        publication.connection,
                        ScopeLimits::default(),
                    )
                    .unwrap();
                    let mut groups =
                        GroupAssembler::new(publication.connection, group_limits()).unwrap();
                    complete
                        .decode_and_publish(
                            &mut groups,
                            &mut scopes,
                            millis,
                            |bytes| {
                                let mut decoded = decode_group(bytes, group_limits(), |payload| {
                                    Ok(payload.to_vec())
                                })?;
                                for state in &mut decoded.members {
                                    state.payload = baselines::decode_member(
                                        &state.payload,
                                        state,
                                        decoded.publication,
                                    )
                                    .map_err(|_| ReplicationError::InvalidPayload)?
                                    .bytes;
                                }
                                Ok(decoded)
                            },
                            |_| true,
                        )
                        .unwrap();
                    assert!(
                        group
                            .manifest
                            .iter()
                            .enumerate()
                            .all(|(index, scope)| scopes.state(scope.entity).unwrap().payload
                                == original_payloads[index])
                    );
                    result = Some(scopes);
                    completed_at = Some(millis + 75); // exact decode receipt reaches authority
                    break;
                }
            }
            if result.is_some() {
                break;
            }
        }
        assert!(
            completed_at.is_some_and(|millis| millis < 2000),
            "bounded entry at {loss_percent}% loss: {completed_at:?}"
        );
        eprintln!(
            "maximum uncompressed group={} fragments={} RTT150 loss={loss_percent}% decoded+ack_ms={}",
            encoded.len(),
            fragments.len(),
            completed_at.unwrap()
        );
    }
}

#[test]
fn charge_is_an_explicit_edge_and_never_a_held_duration() {
    assert!(valid_tick_action(&DreamAction::ChargeBegin));
    assert!(valid_tick_action(&DreamAction::ChargeRelease {
        episode: 7
    }));
    assert!(valid_tick_action(&DreamAction::ChargeCancel { episode: 7 }));
    assert!(!valid_tick_action(&DreamAction::ChargeRelease {
        episode: 0
    }));
    assert!(!valid_tick_action(&DreamAction::ChargeCancel {
        episode: 0
    }));
    let mut held = dreamwake_sim::DreamInput::default();
    held.charge = dreamwake_sim::ChargeCommand::Begin { episode: 7 };
    assert!(!valid_held_input(&held));
    for action in [
        DreamAction::ChargeBegin,
        DreamAction::ChargeRelease { episode: 7 },
        DreamAction::ChargeCancel { episode: 7 },
    ] {
        let mut edge = command();
        edge.actions = BoundedVec::new(vec![ActionEdge { slot: 0, action }]).unwrap();
        let encoded = codec::encode_payload(&bundle(edge), limit().payload_bytes()).unwrap();
        let decoded: CommandBundle =
            codec::decode_payload(&encoded, limit().payload_bytes()).unwrap();
        assert_eq!(
            decoded.records.as_slice()[0].actions.as_slice()[0].action,
            action
        );
    }
}

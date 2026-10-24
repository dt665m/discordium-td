use super::*;

#[test]
fn signed_arrival_slack_preserves_lateness_and_saturates_without_overflow() {
    let cap = i32::from(engine_net::types::MAX_COMMAND_LEAD_TICKS);
    for (target, arrival, expected) in [
        (20, 27, -7),
        (20, 20, 0),
        (27, 20, 7),
        (1, u64::MAX, -cap),
        (u64::MAX, 1, cap),
    ] {
        let sample = ArrivalSample {
            sequence: CommandSeq(1),
            target: TargetTick(target),
            arrival_tick: ServerTick(arrival),
            slack: expected.max(0) as u8,
        };
        let feedback = ArrivalFeedback {
            sample: Some(sample),
            observed_commands: 1,
            late_commands: u64::from(expected <= 0),
        };
        assert!(feedback.validate());
        assert_eq!(sample.signed_slack(), expected);
    }
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
        application_frame_bytes: 1100,
    }
}
fn limit() -> FrameLimit {
    FrameLimit::new(1100).unwrap()
}
fn record(sequence: u64) -> InputRecord {
    InputRecord {
        sequence: CommandSeq(sequence),
        target: TargetTick(10 + sequence),
        input: quantize_tick_input(
            DreamInput {
                movement: [0.6, 0.8],
                aim: [1.0, 0.0],
                attack: true,
                ..Default::default()
            }
            .into(),
        ),
        actions: BoundedVec::new(vec![ActionEdge {
            slot: 0,
            action: DreamAction::Dash {
                direction: [1.0, 0.0],
            },
        }])
        .unwrap(),
    }
}
fn input(records: Vec<InputRecord>) -> InputPacket {
    InputPacket {
        ack: StreamAck {
            latest: 40,
            received: 0b1011,
        },
        records: BoundedVec::new(records).unwrap(),
    }
}
fn state() -> StatePacket {
    StatePacket {
        seq: 70,
        tick: ServerTick(1234),
        status: OwnerStatus {
            finalized: ServerTick(1230),
            menu_applied: Some(CommandSeq(3)),
            received: CommandSeq(8),
            arrival: ArrivalFeedback {
                sample: Some(ArrivalSample {
                    sequence: CommandSeq(9),
                    target: TargetTick(1236),
                    arrival_tick: ServerTick(1233),
                    slack: 3,
                }),
                observed_commands: 9,
                late_commands: 1,
            },
        },
        entries: vec![
            StateEntry {
                entity: entity(2),
                base: Some(68),
                body: vec![1, 2, 3],
            },
            StateEntry {
                entity: entity(5),
                base: None,
                body: vec![9; 40],
            },
        ],
        removed: vec![entity(7)],
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
    let mut wire = encode_welcome(&good).unwrap();
    wire.push(0);
    assert!(decode_welcome(&wire).is_err());
    wire[0] ^= 1;
    assert!(decode_welcome(&wire).is_err());
}

#[test]
fn input_packets_round_trip_and_reject_invalid_records() {
    let valid = input(vec![record(3), record(1), record(2)]);
    let wire = encode_input(&valid, ConnectionEpoch(2), 3, limit()).unwrap();
    assert_eq!(
        decode_input(&wire, ConnectionEpoch(2), limit()).unwrap(),
        valid
    );
    // An acknowledgement alone is a valid packet.
    let ack_only = input(vec![]);
    let wire = encode_input(&ack_only, ConnectionEpoch(2), 0, limit()).unwrap();
    assert_eq!(
        decode_input(&wire, ConnectionEpoch(2), limit()).unwrap(),
        ack_only
    );
    assert_eq!(
        decode_input(&wire, ConnectionEpoch(3), limit()),
        Err(CodecError::WrongEpoch)
    );
    for change in 0..9 {
        let mut bad = record(1);
        match change {
            0 => bad.input.held.movement[0] = f32::NAN,
            1 => bad.input.held.aim[1] = f32::INFINITY,
            2 => bad.input.held.movement = [1.0, 1.0],
            3 => bad.input.held.dash = true,
            4 => bad.sequence = CommandSeq(0),
            5 => bad.target = TargetTick(0),
            6 => bad
                .actions
                .push(record(1).actions.as_slice()[0].clone())
                .unwrap(),
            7 => {
                bad.actions = BoundedVec::new(vec![ActionEdge {
                    slot: 9,
                    action: DreamAction::BeamStop,
                }])
                .unwrap()
            }
            _ => {
                bad.actions = BoundedVec::new(vec![ActionEdge {
                    slot: 0,
                    action: DreamAction::Restart,
                }])
                .unwrap()
            }
        }
        let packet = input(vec![bad]);
        assert!(
            encode_input(&packet, ConnectionEpoch(2), 1, limit()).is_err(),
            "{change}"
        );
        // Non-finite axes have no wire form, and held edges are not carried;
        // whatever does reach the wire never decodes into the invalid record.
        if let Ok(frame) = encode_input_unvalidated(&packet, ConnectionEpoch(2), 1, limit()) {
            assert!(
                decode_input(&frame, ConnectionEpoch(2), limit())
                    .map_or(true, |decoded| decoded != packet),
                "{change}"
            );
        }
    }
    let repeated = input(vec![record(1), record(1)]);
    assert!(encode_input(&repeated, ConnectionEpoch(2), 1, limit()).is_err());
    // Commands come back with the connection's own stream.
    let command = record(4).command(welcome().stream);
    assert_eq!(command.owner, welcome().stream);
    assert_eq!(InputRecord::from_command(&command), record(4));
}

#[test]
fn state_packets_round_trip_with_compact_bases() {
    let packet = state();
    let wire = encode_state(&packet, ConnectionEpoch(2), limit()).unwrap();
    assert_eq!(
        decode_state(&wire, ConnectionEpoch(2), limit()).unwrap(),
        packet
    );
    // Header, ticks and two small entries stay small.
    assert!(packet.encoded_len().unwrap() < 24 + 80);
    let control = encode_control(
        &ServerControl::Notice {
            utf8: BoundedVec::new(b"hi".to_vec()).unwrap(),
        },
        ConnectionEpoch(2),
        1,
        limit(),
    )
    .unwrap();
    assert_eq!(
        decode_state(&control, ConnectionEpoch(2), limit()),
        Err(CodecError::InvalidLane)
    );
}

#[test]
fn state_packets_reject_bases_outside_the_window_and_malformed_bodies() {
    let mut far = state();
    far.entries[0].base = Some(far.seq - STREAM_WINDOW - 1);
    assert!(encode_state(&far, ConnectionEpoch(2), limit()).is_err());
    let mut ahead = state();
    ahead.entries[0].base = Some(ahead.seq);
    assert!(encode_state(&ahead, ConnectionEpoch(2), limit()).is_err());
    let mut feedback = state();
    feedback.status.arrival.late_commands = 99;
    assert!(encode_state(&feedback, ConnectionEpoch(2), limit()).is_err());
    let good = state().body().unwrap();
    for (name, mut body) in [
        ("truncated", good[..good.len() - 1].to_vec()),
        ("trailing", [good.clone(), vec![0]].concat()),
    ] {
        body.truncate(body.len());
        assert!(StatePacket::parse(70, &body).is_err(), "{name}");
    }
    // A zero entity identity is never valid.
    let mut zero = state();
    zero.removed = vec![EntityId {
        index: 0,
        generation: 1,
    }];
    let body = zero.body().unwrap();
    assert!(StatePacket::parse(70, &body).is_err());
    // A sequence of zero is never a packet.
    assert!(StatePacket::parse(0, &good).is_err());
}

#[test]
fn controls_reject_invalid_values() {
    let menu = ClientControl::Menu {
        sequence: CommandSeq(1),
        action: DreamAction::Dash {
            direction: [1.0, 0.0],
        },
    };
    assert!(encode_control(&menu, ConnectionEpoch(2), 1, limit()).is_err());
    let lookup = ClientControl::OutcomeLookup {
        keys: BoundedVec::default(),
    };
    assert!(encode_control(&lookup, ConnectionEpoch(2), 1, limit()).is_err());
    let ready = ClientControl::Ready {
        content: ContentIdentity::current(SceneRevision(1)),
    };
    let wire = encode_control(&ready, ConnectionEpoch(2), 1, limit()).unwrap();
    assert_eq!(
        decode_control::<ClientControl>(&wire, ConnectionEpoch(2), limit()).unwrap(),
        ready
    );
}

#[test]
fn charge_is_an_explicit_edge_and_never_a_held_duration() {
    let mut held = record(1);
    held.input.held.charge = dreamwake_sim::ChargeCommand::Begin { episode: 1 };
    assert!(!held.valid());
    let mut edge = record(1);
    edge.actions = BoundedVec::new(vec![ActionEdge {
        slot: 0,
        action: DreamAction::ChargeBegin,
    }])
    .unwrap();
    assert!(edge.valid());
}

#[test]
fn a_full_input_packet_fits_one_application_frame() {
    // Every record distinct, aimed differently and carrying an action: the
    // least compressible packet the client sends.
    let records = (1..=MAX_INPUT_RECORDS as u64)
        .map(|sequence| {
            let angle = sequence as f32 * 0.37;
            let mut record = record(sequence);
            record.input = quantize_tick_input(
                DreamInput {
                    movement: [angle.cos(), angle.sin()],
                    aim: [(angle * 1.7).sin(), (angle * 1.7).cos()],
                    attack: sequence % 2 == 0,
                    ..Default::default()
                }
                .into(),
            );
            record
        })
        .collect();
    let packet = input(records);
    let wire = encode_input(&packet, ConnectionEpoch(2), 3, input_limit()).unwrap();
    assert!(
        wire.len() <= APPLICATION_FRAME_BYTES,
        "{} bytes",
        wire.len()
    );
    assert_eq!(
        decode_input(&wire, ConnectionEpoch(2), input_limit()).unwrap(),
        packet
    );
}

#[test]
fn repeated_input_records_cost_a_few_bytes_each() {
    // A player holding one direction: contiguous commands, newest first, with
    // the same input and no actions.
    let records = (1..=MAX_INPUT_RECORDS as u64)
        .rev()
        .map(|sequence| {
            let mut record = record(sequence);
            record.actions = BoundedVec::default();
            record
        })
        .collect();
    let packet = input(records);
    let wire = encode_input(&packet, ConnectionEpoch(2), 3, input_limit()).unwrap();
    assert!(wire.len() <= 200, "{} bytes", wire.len());
    assert_eq!(
        decode_input(&wire, ConnectionEpoch(2), input_limit()).unwrap(),
        packet
    );
}

#[test]
fn input_off_the_grid_is_not_encoded() {
    let mut off = record(1);
    off.input.held.movement = [0.6, 0.8];
    assert!(encode_input(&input(vec![off]), ConnectionEpoch(2), 1, input_limit()).is_err());
    let on = quantize_direction([0.6, 0.8]);
    assert!(f64::from(on[0]).hypot(f64::from(on[1])) <= 1.0);
    assert_eq!(quantize_direction(on), on);
}

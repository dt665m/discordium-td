//! Synthetic malformed values bypass only fixture encoding validation, so real
//! production frame decoders receive structurally valid transport envelopes.
use super::*;
use generated::SchemaCase;
#[derive(serde::Serialize)]
struct Fixture<T>(T);
impl<T> codec::Validate for Fixture<T> {
    fn validate(&self) -> Result<(), codec::CodecError> {
        Ok(())
    }
}
fn framed<T: serde::Serialize>(value: &T, lane: Lane) -> Vec<u8> {
    let payload = codec::encode_payload(&Fixture(value), limit().payload_bytes()).unwrap();
    codec::encode_frame(
        FrameHeader {
            lane,
            connection: EPOCH,
            sequence: 1,
        },
        &payload,
        limit(),
    )
    .unwrap()
}
pub fn cases(
    command: &Command<live::TickInput, DreamAction>,
    reference: StateReceipt,
) -> (Vec<SchemaCase>, Vec<SchemaCase>) {
    let mut commands = Vec::new();
    let mut controls = Vec::new();
    let view = live::CombatViewStamp {
        sampled_fraction: 0,
        viewed_fraction: 0,
        sampled_at: ServerTick(10),
        viewed_at: ServerTick(9),
        reference,
    };
    for mutation in [
        "valid",
        "future_view",
        "zero_sample",
        "zero_reference_epoch",
        "zero_reference_generation",
        "zero_baseline_generation",
        "nonfinite_aim",
        "invalid_slot",
        "valid_beam_begin",
        "valid_beam_hold",
        "valid_beam_stop",
        "future_beam_hold",
        "nonfinite_beam_hold",
    ] {
        let mut view = view;
        let mut aim = [1.0, 0.0];
        let mut slot = 0;
        match mutation {
            "future_view" | "future_beam_hold" => view.viewed_at = ServerTick(11),
            "zero_sample" => view.sampled_at = ServerTick(0),
            "zero_reference_epoch" => view.reference.scope.connection = ConnectionEpoch(0),
            "zero_reference_generation" => view.reference.scope.entity.generation = 0,
            "zero_baseline_generation" => {
                view.reference.baseline_generation = BaselineGeneration(0)
            }
            "nonfinite_aim" | "nonfinite_beam_hold" => aim[0] = f32::NAN,
            "invalid_slot" => slot = 8,
            _ => {}
        }
        let mut record = command.clone();
        record.actions = BoundedVec::new(vec![ActionEdge {
            slot,
            action: DreamAction::Dreamlance { aim, view },
        }])
        .unwrap();
        if mutation.contains("beam_hold") {
            record.actions = BoundedVec::default();
            record.input.beam = Some(live::BeamAimSample { aim, view });
        } else if mutation == "valid_beam_begin" {
            record.actions = BoundedVec::new(vec![ActionEdge {
                slot: 0,
                action: DreamAction::BeamBegin { aim, view },
            }])
            .unwrap();
        } else if mutation == "valid_beam_stop" {
            record.actions = BoundedVec::new(vec![ActionEdge {
                slot: 0,
                action: DreamAction::BeamStop,
            }])
            .unwrap();
        }
        let bytes = framed(
            &live::CommandBundle {
                records: BoundedVec::new(vec![record]).unwrap(),
            },
            Lane::Input,
        );
        commands.push(SchemaCase {
            root: "dreamlance",
            mutation,
            bytes,
            rejected: !matches!(
                mutation,
                "valid" | "valid_beam_begin" | "valid_beam_hold" | "valid_beam_stop"
            ),
        });
    }
    let key = ActionKey {
        connection: EPOCH,
        stream: CommandStream(1),
        command: CommandSeq(u64::MAX - 1),
        slot: 7,
    };
    let target = dreamwake_sim::combat::CombatTarget {
        id: 8,
        generation: 1,
    };
    let outcome = live::ActionOutcome {
        sequence: u64::MAX - 1,
        key,
        execution_tick: ServerTick(11),
        data: live::ActionOutcomeData {
            status: live::TerminalStatus::Accepted,
            reason: live::OutcomeReason::Combat(dreamwake_sim::combat::CombatReason::Hit),
            query_tick: Some(ServerTick(9)),
            target: Some(target),
            hit_region: u8::MAX,
            damage: 32.0,
            transaction: Some(dreamwake_sim::combat::DamageTransaction {
                action: dreamwake_sim::combat::RayActionKey {
                    match_epoch: 3,
                    connection_epoch: EPOCH.0,
                    command_stream: 1,
                    ownership_epoch: 1,
                    actor: 1,
                    actor_generation: 1,
                    command_sequence: key.command.0,
                    action_slot: key.slot,
                },
                target,
                hit_slot: u16::MAX,
            }),
            binding: Some(entity(u64::MAX)),
            spawn_bindings: Default::default(),
        },
    };
    for mutation in [
        "valid",
        "zero_sequence",
        "zero_batch",
        "zero_command",
        "negative_damage",
        "nonfinite_damage",
        "future_query",
        "target_generation",
        "transaction_key",
        "rejected_damage",
        "empty_records",
        "maximum_records",
    ] {
        let mut item = outcome.clone();
        match mutation {
            "zero_sequence" => item.sequence = 0,
            "zero_command" => item.key.command = CommandSeq(0),
            "negative_damage" => item.data.damage = -1.0,
            "nonfinite_damage" => item.data.damage = f32::NAN,
            "future_query" => item.data.query_tick = Some(ServerTick(12)),
            "target_generation" => item.data.target.as_mut().unwrap().generation = 0,
            "transaction_key" => {
                item.data
                    .transaction
                    .as_mut()
                    .unwrap()
                    .action
                    .command_sequence = 1
            }
            "rejected_damage" => item.data.status = live::TerminalStatus::Rejected,
            _ => {}
        }
        let records = match mutation {
            "empty_records" => vec![],
            "maximum_records" => (1..=4)
                .map(|i| live::ActionOutcome {
                    sequence: i,
                    ..item.clone()
                })
                .collect(),
            _ => vec![item],
        };
        let control = live::ServerControl::ActionOutcomes {
            batch: if mutation == "zero_batch" { 0 } else { 1 },
            records: BoundedVec::new(records).unwrap(),
        };
        let bytes = framed(&control, Lane::Control);
        if mutation == "maximum_records" {
            assert!(bytes.len() <= 1100);
            assert!(live::encode_control(&control, EPOCH, 1, limit()).is_ok());
        }
        controls.push(SchemaCase {
            root: "action_outcomes",
            mutation,
            bytes,
            rejected: !matches!(mutation, "valid" | "maximum_records"),
        });
    }
    let control = live::ServerControl::ActionOutcomes {
        batch: 1,
        records: BoundedVec::new(vec![outcome]).unwrap(),
    };
    let mut payload = codec::encode_payload(&Fixture(control), limit().payload_bytes()).unwrap();
    payload[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
    controls.push(SchemaCase {
        root: "action_outcomes",
        mutation: "record_count",
        bytes: codec::encode_frame(
            FrameHeader {
                lane: Lane::Control,
                connection: EPOCH,
                sequence: 1,
            },
            &payload,
            limit(),
        )
        .unwrap(),
        rejected: true,
    });
    (commands, controls)
}
pub fn lookup() -> live::ClientControl {
    live::ClientControl::OutcomeLookup {
        keys: BoundedVec::new(
            (1..=8)
                .map(|sequence| ActionKey {
                    connection: EPOCH,
                    stream: CommandStream(1),
                    command: CommandSeq(sequence),
                    slot: 0,
                })
                .collect(),
        )
        .unwrap(),
    }
}
pub fn unavailable() -> live::ServerControl {
    let live::ClientControl::OutcomeLookup { keys } = lookup() else {
        unreachable!()
    };
    live::ServerControl::OutcomeUnavailable { keys }
}

/// Decoder validity only; a valid positive tick still requires an exact sent proof at authority.
pub fn finalized_ack_cases() -> Vec<SchemaCase> {
    [
        (10, false, "valid"),
        (u64::MAX, false, "full_width_tick"),
        (0, true, "zero_tick"),
    ]
    .into_iter()
    .map(|(tick, rejected, mutation)| SchemaCase {
        root: "finalized_ack",
        mutation,
        rejected,
        bytes: framed(
            &live::ClientControl::FinalizedAck {
                through: ServerTick(tick),
            },
            Lane::Control,
        ),
    })
    .collect()
}

/// A delivery batch acknowledgment never proves an action was accepted.
pub fn outcomes_ack_cases() -> Vec<SchemaCase> {
    [
        (1, false, "valid"),
        (u64::MAX, false, "full_width_batch"),
        (0, true, "zero_batch"),
    ]
    .into_iter()
    .map(|(through, rejected, mutation)| SchemaCase {
        root: "outcomes_ack",
        mutation,
        rejected,
        bytes: framed(&live::ClientControl::OutcomesAck { through }, Lane::Control),
    })
    .collect()
}

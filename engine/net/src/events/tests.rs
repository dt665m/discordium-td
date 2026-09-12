use super::*;
use crate::types::*;
fn scope() -> JournalScope {
    JournalScope {
        connection: ConnectionEpoch(1),
        stream: CommandStream(2),
    }
}
fn event(command: u64) -> EventRecord<Vec<u8>> {
    EventRecord {
        identity: EventIdentity {
            key: EventKey {
                action: ActionKey {
                    connection: scope().connection,
                    stream: scope().stream,
                    command: CommandSeq(command),
                    slot: 3,
                },
                kind: EventKind(7),
                spawn_ordinal: 0,
            },
            origin_tick: ServerTick(1),
            schema: SchemaId(4),
        },
        expires_at: ServerTick(20),
        delivery: Delivery::Reversible,
        payload: vec![1, 2, 3],
    }
}
fn journal() -> EventJournal<Vec<u8>> {
    EventJournal::new(scope(), Limits::default()).unwrap()
}
fn range() -> ReplayRange {
    ReplayRange {
        first: ServerTick(1),
        last: ServerTick(1),
    }
}
fn apply(journal: &mut EventJournal<Vec<u8>>, events: &[EventRecord<Vec<u8>>]) -> Changes<Vec<u8>> {
    journal
        .reconcile(ServerTick(1), range(), events, |_| true)
        .unwrap()
}
fn entity(generation: u32) -> EntityId {
    EntityId {
        index: 9,
        generation,
    }
}

#[test]
fn ten_replays_keep_effect_and_parameter_correction_updates_instead_of_recreating() {
    let mut journal = journal();
    let mut record = event(1);
    assert!(matches!(
        apply(&mut journal, &[record.clone()]).as_slice(),
        [Change::Create { .. }]
    ));
    for _ in 0..10 {
        assert!(apply(&mut journal, &[record.clone()]).as_slice().is_empty());
    }
    record.payload[0] = 9;
    assert!(matches!(
        apply(&mut journal, &[record.clone()]).as_slice(),
        [Change::Update(_)]
    ));
    assert_eq!(
        journal.get(record.identity.key).unwrap().payload,
        Some(&record.payload)
    );
    assert!(matches!(
        apply(&mut journal, &[]).as_slice(),
        [Change::Cancel {
            reason: CancelReason::ReplayAbsent,
            ..
        }]
    ));
    assert!(apply(&mut journal, &[]).as_slice().is_empty());
    assert!(matches!(
        apply(&mut journal, &[record]).as_slice(),
        [Change::Create { .. }]
    ));
}
#[test]
fn one_shot_absence_never_unplays_or_replays_already_delivered_cue() {
    let mut journal = journal();
    let mut record = event(1);
    record.delivery = Delivery::OneShot;
    assert!(matches!(
        apply(&mut journal, &[record.clone()]).as_slice(),
        [Change::Create { .. }]
    ));
    assert!(apply(&mut journal, &[]).as_slice().is_empty());
    assert!(apply(&mut journal, &[record.clone()]).as_slice().is_empty());
    assert!(
        journal
            .expire(ServerTick(2), record.identity)
            .unwrap()
            .as_slice()
            .is_empty()
    );
    assert_eq!(
        journal.get(record.identity.key).unwrap().phase,
        Phase::Expired
    );
}
#[test]
fn rejection_cancels_once_and_fences_changed_origin_schema_and_late_acceptance() {
    let mut journal = journal();
    let record = event(1);
    apply(&mut journal, &[record.clone()]);
    assert!(matches!(
        journal
            .resolve(ServerTick(1), record.identity, Outcome::Rejected)
            .unwrap()
            .as_slice(),
        [Change::Cancel {
            reason: CancelReason::Rejected,
            ..
        }]
    ));
    assert!(apply(&mut journal, &[record.clone()]).as_slice().is_empty());
    assert!(
        journal
            .resolve(ServerTick(1), record.identity, Outcome::Rejected)
            .unwrap()
            .as_slice()
            .is_empty()
    );
    assert_eq!(
        journal
            .resolve(
                ServerTick(1),
                record.identity,
                Outcome::Accepted {
                    binding: Some(entity(1))
                }
            )
            .unwrap_err(),
        EventError::Conflict
    );
    let mut changed = record.clone();
    changed.identity.schema = SchemaId(5);
    assert_eq!(
        journal
            .reconcile(ServerTick(1), range(), &[changed], |_| true)
            .unwrap_err(),
        EventError::Conflict
    );
    let mut changed = record;
    changed.identity.origin_tick = ServerTick(2);
    assert_eq!(
        journal
            .reconcile(
                ServerTick(2),
                ReplayRange {
                    first: ServerTick(2),
                    last: ServerTick(2)
                },
                &[changed],
                |_| true
            )
            .unwrap_err(),
        EventError::Conflict
    );
    assert_eq!(journal.now(), ServerTick(1));
}
#[test]
fn action_rejection_fences_unseen_semantic_kinds_and_spawn_ordinals() {
    let mut journal = journal();
    let mut record = event(1);
    journal
        .reject_action(ServerTick(1), record.identity.key.action)
        .unwrap();
    record.identity.key.kind = EventKind(8);
    record.identity.key.spawn_ordinal = 19;
    assert!(apply(&mut journal, &[record.clone()]).as_slice().is_empty());
    assert_eq!(
        journal.get(record.identity.key).unwrap().phase,
        Phase::Rejected
    );
    assert_eq!(
        journal
            .resolve(
                ServerTick(1),
                record.identity,
                Outcome::Accepted { binding: None }
            )
            .unwrap_err(),
        EventError::Conflict
    );
}
#[test]
fn accepted_effect_survives_provisional_absence_and_requires_explicit_retirement() {
    let mut journal = journal();
    let record = event(1);
    apply(&mut journal, &[record.clone()]);
    let accepted = journal
        .resolve(
            ServerTick(1),
            record.identity,
            Outcome::Accepted {
                binding: Some(entity(1)),
            },
        )
        .unwrap();
    assert!(matches!(
        accepted.as_slice(),
        [Change::Confirm { .. }, Change::Bind { .. }]
    ));
    assert!(apply(&mut journal, &[]).as_slice().is_empty());
    assert!(
        journal
            .advance(ServerTick(50))
            .unwrap()
            .as_slice()
            .is_empty()
    );
    assert_eq!(journal.get(record.identity.key).unwrap().phase, Phase::Live);
    assert_eq!(
        journal
            .resolve(ServerTick(50), record.identity, Outcome::Rejected)
            .unwrap_err(),
        EventError::Conflict
    );
    assert!(matches!(
        journal
            .expire(ServerTick(50), record.identity)
            .unwrap()
            .as_slice(),
        [Change::Cancel {
            reason: CancelReason::Expired,
            ..
        }]
    ));
}
#[test]
fn accept_after_local_expiry_binds_retired_identity_without_resurrection() {
    let mut journal = journal();
    let record = event(1);
    apply(&mut journal, &[record.clone()]);
    assert!(matches!(
        journal.advance(ServerTick(20)).unwrap().as_slice(),
        [Change::Cancel {
            reason: CancelReason::Expired,
            ..
        }]
    ));
    assert!(matches!(
        journal
            .resolve(
                ServerTick(21),
                record.identity,
                Outcome::Accepted {
                    binding: Some(entity(1))
                }
            )
            .unwrap()
            .as_slice(),
        [Change::Confirm { .. }, Change::BindRetired { .. }]
    ));
    assert!(
        journal
            .reconcile(ServerTick(21), range(), &[record.clone()], |_| true)
            .unwrap()
            .as_slice()
            .is_empty()
    );
    assert_eq!(
        journal.get(record.identity.key).unwrap().phase,
        Phase::Expired
    );
    assert_eq!(
        journal.get(record.identity.key).unwrap().binding,
        Some(entity(1))
    );
    let mut extended = record;
    extended.expires_at = ServerTick(30);
    assert_eq!(
        journal
            .reconcile(ServerTick(21), range(), &[extended], |_| true)
            .unwrap_err(),
        EventError::Conflict
    );
}
#[test]
fn acceptance_before_prediction_and_reordered_binding_are_idempotent() {
    let record = event(1);
    for binding_first in [false, true] {
        let mut journal = journal();
        for binding in if binding_first {
            [Some(entity(1)), None, Some(entity(1))]
        } else {
            [None, Some(entity(1)), None]
        } {
            journal
                .resolve(
                    ServerTick(1),
                    record.identity,
                    Outcome::Accepted { binding },
                )
                .unwrap();
        }
        assert!(
            matches!(apply(&mut journal,&[record.clone()]).as_slice(),[Change::Create { committed: true,binding: Some(id),.. }] if *id == entity(1))
        );
        assert!(
            journal
                .resolve(
                    ServerTick(1),
                    record.identity,
                    Outcome::Accepted {
                        binding: Some(entity(1))
                    }
                )
                .unwrap()
                .as_slice()
                .is_empty()
        );
        assert_eq!(journal.records(), 1);
        assert_eq!(journal.binding_fences(), 1);
    }
}
#[test]
fn newer_generation_retires_live_binding_but_conflicts_cannot_rebind_or_reuse_it() {
    let mut journal = journal();
    let first = event(1);
    let second = event(2);
    apply(&mut journal, &[first.clone(), second.clone()]);
    journal
        .resolve(
            ServerTick(1),
            first.identity,
            Outcome::Accepted {
                binding: Some(entity(1)),
            },
        )
        .unwrap();
    assert_eq!(
        journal
            .resolve(
                ServerTick(1),
                second.identity,
                Outcome::Accepted {
                    binding: Some(entity(1))
                }
            )
            .unwrap_err(),
        EventError::Conflict
    );
    assert_eq!(
        journal.get(second.identity.key).unwrap().decision,
        Decision::Unresolved
    );
    let changes = journal
        .resolve(
            ServerTick(1),
            second.identity,
            Outcome::Accepted {
                binding: Some(entity(2)),
            },
        )
        .unwrap();
    assert_eq!(
        changes.as_slice(),
        &[
            Change::Confirm {
                key: second.identity.key
            },
            Change::Cancel {
                key: first.identity.key,
                reason: CancelReason::Expired
            },
            Change::Bind {
                key: second.identity.key,
                entity: entity(2)
            },
        ]
    );
    assert_eq!(
        journal.get(first.identity.key).unwrap().phase,
        Phase::Expired
    );
    assert_eq!(journal.get(first.identity.key).unwrap().payload, None);
    assert_eq!(journal.get(second.identity.key).unwrap().phase, Phase::Live);
    assert!(
        apply(&mut journal, &[first.clone(), second.clone()])
            .as_slice()
            .is_empty()
    );
    assert!(
        journal
            .resolve(
                ServerTick(1),
                first.identity,
                Outcome::Accepted {
                    binding: Some(entity(1))
                }
            )
            .unwrap()
            .as_slice()
            .is_empty()
    );
    assert_eq!(
        journal.get(second.identity.key).unwrap().binding,
        Some(entity(2))
    );
    let third = event(3);
    assert_eq!(
        journal
            .resolve(
                ServerTick(1),
                third.identity,
                Outcome::Accepted {
                    binding: Some(entity(1))
                }
            )
            .unwrap_err(),
        EventError::Stale
    );
    assert!(journal.get(third.identity.key).is_none());
    assert_eq!(
        journal
            .resolve(
                ServerTick(1),
                second.identity,
                Outcome::Accepted {
                    binding: Some(entity(3))
                }
            )
            .unwrap_err(),
        EventError::Conflict
    );
    assert_eq!(
        journal.get(second.identity.key).unwrap().binding,
        Some(entity(2))
    );
}

#[test]
fn generation_replacement_waits_for_prediction_to_create_the_new_effect() {
    for delivery in [
        Delivery::Reversible,
        Delivery::SimulationOwned,
        Delivery::OneShot,
    ] {
        let mut journal = journal();
        let mut first = event(1);
        first.delivery = delivery;
        let second = event(2);
        apply(&mut journal, &[first.clone()]);
        journal
            .resolve(
                ServerTick(1),
                first.identity,
                Outcome::Accepted {
                    binding: Some(entity(1)),
                },
            )
            .unwrap();
        let changes = journal
            .resolve(
                ServerTick(1),
                second.identity,
                Outcome::Accepted {
                    binding: Some(entity(2)),
                },
            )
            .unwrap();
        assert!(
            !changes
                .as_slice()
                .iter()
                .any(|change| matches!(change, Change::Create { .. }))
        );
        assert_eq!(
            changes
                .as_slice()
                .iter()
                .filter(|change| matches!(change,
            Change::Cancel { key, .. } if *key == first.identity.key))
                .count(),
            usize::from(delivery != Delivery::OneShot)
        );
        assert_eq!(
            journal.get(first.identity.key).unwrap().phase,
            Phase::Expired
        );
        assert_eq!(
            journal.get(second.identity.key).unwrap().phase,
            Phase::Unseen
        );
        let changes = apply(&mut journal, &[first, second.clone()]);
        assert!(
            matches!(changes.as_slice(), [Change::Create { event, committed: true,
            binding: Some(id) }] if event.identity == second.identity && *id == entity(2))
        );
    }
}

#[test]
fn generation_replacement_rolls_back_cancellation_when_transition_capacity_is_exhausted() {
    let mut journal = EventJournal::new(
        scope(),
        Limits {
            transitions: 2,
            ..Limits::default()
        },
    )
    .unwrap();
    let first = event(1);
    let second = event(2);
    apply(&mut journal, &[first.clone(), second.clone()]);
    journal
        .resolve(
            ServerTick(1),
            first.identity,
            Outcome::Accepted {
                binding: Some(entity(1)),
            },
        )
        .unwrap();
    let bytes = journal.retained_bytes();
    assert_eq!(
        journal
            .resolve(
                ServerTick(1),
                second.identity,
                Outcome::Accepted {
                    binding: Some(entity(2))
                }
            )
            .unwrap_err(),
        EventError::Capacity
    );
    let old = journal.get(first.identity.key).unwrap();
    assert_eq!(
        (old.phase, old.binding, old.payload),
        (Phase::Live, Some(entity(1)), Some(&first.payload))
    );
    let next = journal.get(second.identity.key).unwrap();
    assert_eq!(
        (next.phase, next.binding, next.decision),
        (Phase::Live, None, Decision::Unresolved)
    );
    assert_eq!(journal.retained_bytes(), bytes);
    assert_eq!(journal.binding_fences(), 1);
}
#[test]
fn expiry_before_prediction_records_a_fence_and_delayed_binding_only() {
    let mut journal = journal();
    let record = event(1);
    journal.expire(ServerTick(1), record.identity).unwrap();
    assert!(apply(&mut journal, &[record.clone()]).as_slice().is_empty());
    assert!(matches!(
        journal
            .resolve(
                ServerTick(1),
                record.identity,
                Outcome::Accepted {
                    binding: Some(entity(1))
                }
            )
            .unwrap()
            .as_slice(),
        [Change::Confirm { .. }, Change::BindRetired { .. }]
    ));
}
#[test]
fn retirement_requires_no_live_lower_sequence_and_rejects_keys_after_pruning() {
    let mut journal = journal();
    let mut record = event(1);
    apply(&mut journal, &[record.clone()]);
    assert_eq!(
        journal.retire_through(CommandSeq(1)),
        Err(EventError::LiveHistory)
    );
    journal.expire(ServerTick(1), record.identity).unwrap();
    journal.retire_through(CommandSeq(1)).unwrap();
    assert_eq!(journal.records(), 0);
    record.identity.origin_tick = ServerTick(2);
    assert_eq!(
        journal
            .reconcile(
                ServerTick(2),
                ReplayRange {
                    first: ServerTick(2),
                    last: ServerTick(2)
                },
                &[record],
                |_| true
            )
            .unwrap_err(),
        EventError::Stale
    );
    assert_eq!(journal.retired_through(), CommandSeq(1));
}
#[test]
fn reset_drops_old_bindings_and_rejects_old_pool_metadata() {
    let mut journal = journal();
    let record = event(1);
    apply(&mut journal, &[record.clone()]);
    journal
        .resolve(
            ServerTick(1),
            record.identity,
            Outcome::Accepted {
                binding: Some(entity(1)),
            },
        )
        .unwrap();
    let newer = JournalScope {
        connection: ConnectionEpoch(2),
        stream: CommandStream(1),
    };
    assert!(matches!(
        journal.reset(newer).unwrap().as_slice(),
        [Change::Reset { .. }]
    ));
    assert_eq!(journal.records(), 0);
    assert_eq!(journal.binding_fences(), 0);
    assert_eq!(
        journal
            .resolve(ServerTick(1), record.identity, Outcome::Rejected)
            .unwrap_err(),
        EventError::WrongScope
    );
    assert_eq!(journal.reset(scope()).unwrap_err(), EventError::Stale);
}
#[test]
fn record_transition_and_payload_limits_fail_transactionally() {
    let mut journal = EventJournal::new(
        scope(),
        Limits {
            records: 1,
            transitions: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    let record = event(1);
    apply(&mut journal, &[record.clone()]);
    let bytes = journal.retained_bytes();
    assert_eq!(
        journal
            .reconcile(ServerTick(1), range(), &[record.clone(), event(2)], |_| {
                true
            })
            .unwrap_err(),
        EventError::Capacity
    );
    assert_eq!(
        journal
            .resolve(
                ServerTick(1),
                record.identity,
                Outcome::Accepted {
                    binding: Some(entity(1))
                }
            )
            .unwrap_err(),
        EventError::Capacity
    );
    assert_eq!(
        journal.get(record.identity.key).unwrap().decision,
        Decision::Unresolved
    );
    assert_eq!(journal.binding_fences(), 0);
    assert_eq!(journal.retained_bytes(), bytes);
    let mut bad = record.clone();
    bad.payload = vec![0; Limits::default().payload_bytes + 1];
    assert_eq!(
        journal
            .reconcile(ServerTick(2), range(), &[bad], |_| true)
            .unwrap_err(),
        EventError::InvalidPayload
    );
    assert_eq!(journal.now(), ServerTick(1));
    assert_eq!(
        journal.get(record.identity.key).unwrap().payload,
        Some(&record.payload)
    );
    assert_eq!(
        journal
            .reconcile(ServerTick(1), range(), &[record], |_| false)
            .unwrap_err(),
        EventError::InvalidPayload
    );
}
#[test]
fn inconsistent_peak_memory_and_history_configurations_are_rejected() {
    assert!(matches!(
        EventJournal::<Vec<u8>>::new(
            scope(),
            Limits {
                peak_bytes: 1,
                ..Limits::default()
            }
        ),
        Err(EventError::InvalidConfiguration)
    ));
    assert!(matches!(
        EventJournal::<Vec<u8>>::new(
            scope(),
            Limits {
                records: usize::MAX,
                ..Limits::default()
            }
        ),
        Err(EventError::InvalidConfiguration)
    ));
    let mut journal = journal();
    assert_eq!(
        journal
            .reconcile(ServerTick(257), range(), &[event(1)], |_| true)
            .unwrap_err(),
        EventError::OutsideHistory
    );
    assert_eq!(journal.now(), ServerTick(0));
}

#[test]
fn aggregate_resident_and_output_payload_limits_preserve_previous_state() {
    let base = Limits {
        records: 4,
        bindings: 4,
        action_fences: 4,
        payload_bytes: 64,
        transitions: 4,
        ..Limits::default()
    };
    let probe = EventJournal::<Vec<u8>>::new(scope(), base).unwrap();
    let resident = probe.retained_bytes();
    let mut journal = EventJournal::new(
        scope(),
        Limits {
            resident_bytes: resident + 64,
            ..base
        },
    )
    .unwrap();
    let mut first = event(1);
    first.payload = vec![1; 40];
    apply(&mut journal, &[first.clone()]);
    let mut second = event(2);
    second.payload = vec![2; 40];
    assert_eq!(
        journal
            .reconcile(ServerTick(1), range(), &[first.clone(), second], |_| true)
            .unwrap_err(),
        EventError::Capacity
    );
    assert_eq!(journal.records(), 1);
    assert_eq!(
        journal.get(first.identity.key).unwrap().payload,
        Some(&first.payload)
    );

    let output_metadata = 4 * std::mem::size_of::<Change<Vec<u8>>>();
    let mut journal = EventJournal::new(
        scope(),
        Limits {
            transition_bytes: output_metadata + 64,
            ..base
        },
    )
    .unwrap();
    let mut second = event(2);
    second.payload = vec![2; 40];
    assert_eq!(
        journal
            .reconcile(ServerTick(1), range(), &[first, second], |_| true)
            .unwrap_err(),
        EventError::Capacity
    );
    assert_eq!(journal.records(), 0);
    assert_eq!(journal.now(), ServerTick(0));
}

#[test]
fn binding_and_action_fences_have_independent_transactional_caps() {
    let limits = Limits {
        bindings: 1,
        action_fences: 1,
        ..Limits::default()
    };
    let mut journal = EventJournal::<Vec<u8>>::new(scope(), limits).unwrap();
    let first = event(1);
    let second = event(2);
    journal
        .resolve(
            ServerTick(1),
            first.identity,
            Outcome::Accepted {
                binding: Some(entity(1)),
            },
        )
        .unwrap();
    let other = EntityId {
        index: 10,
        generation: 1,
    };
    assert_eq!(
        journal
            .resolve(
                ServerTick(1),
                second.identity,
                Outcome::Accepted {
                    binding: Some(other)
                }
            )
            .unwrap_err(),
        EventError::ResetRequired
    );
    assert!(journal.get(second.identity.key).is_none());
    assert_eq!(journal.binding_fences(), 1);
    journal
        .reject_action(ServerTick(1), second.identity.key.action)
        .unwrap();
    assert_eq!(
        journal
            .reject_action(ServerTick(2), event(3).identity.key.action)
            .unwrap_err(),
        EventError::ResetRequired
    );
    assert_eq!(journal.action_fences(), 1);
    assert_eq!(journal.now(), ServerTick(1));
}

#[test]
fn newer_generation_retires_binding_whose_prediction_has_not_arrived() {
    let mut journal = journal();
    let first = event(1);
    let second = event(2);
    journal
        .resolve(
            ServerTick(1),
            first.identity,
            Outcome::Accepted {
                binding: Some(entity(1)),
            },
        )
        .unwrap();
    journal
        .resolve(
            ServerTick(1),
            second.identity,
            Outcome::Accepted {
                binding: Some(entity(2)),
            },
        )
        .unwrap();
    let changes = apply(&mut journal, &[first.clone(), second.clone()]);
    assert!(
        matches!(changes.as_slice(),[Change::Create { event,binding: Some(id),.. }] if event.identity == second.identity && *id == entity(2))
    );
    assert_eq!(
        journal.get(first.identity.key).unwrap().phase,
        Phase::Expired
    );
}

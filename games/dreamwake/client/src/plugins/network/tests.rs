use super::*;
use dreamwake_protocol::replication::{ReplicaPayload, encode_replica};
use dreamwake_sim::replication::ReplicationStamp;
use engine_net::{commands::OwnerStream, replication::*};

pub(super) fn fixture() -> (Session, Vec<Vec<u8>>) {
    fixture_publication(10, 1, 1)
}

pub(super) fn public_full_state(
    session: &Session,
    index: u64,
    revision: u64,
) -> FullState<Vec<u8>> {
    use dreamwake_sim::replication::{PublicEnemyView, PublicReplica};
    FullState {
        scope: ScopeIdentity {
            connection: session.model.welcome.stream.epoch,
            entity: EntityId {
                index,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        },
        baseline_generation: BaselineGeneration(revision),
        snapshot: SnapshotId(revision),
        version: StateVersion(revision),
        end_tick: ServerTick(10 + revision),
        payload: encode_replica(&ReplicaPayload::Actor(PublicReplica::Enemy(
            PublicEnemyView {
                id: index,
                position: [0.0, 0.0],
                facing: [0.0, -1.0],
                hp: 10.0,
                max_hp: 10.0,
                kind: dreamwake_sim::EnemyKind::Melee,
                windup: 0.0,
                target: [0.0, 0.0],
                warn_radius: 0.0,
                phase: 0,
                slowed: false,
                hit_flash: 0.0,
            },
        )))
        .unwrap(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn receipt_runtime(session: Session) -> Runtime {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let client_id = session.model.welcome.client.0;
    let transport = Transport::new(
        Duration::ZERO,
        renet_cross::ClientAuthentication::Unsecure {
            protocol_id: PROTOCOL_ID,
            client_id,
            server_addr: socket.local_addr().unwrap(),
            user_data: None,
        },
        socket,
    )
    .unwrap();
    // Only Renet's local outbound queue is used; no packets are transmitted.
    Runtime {
        renet: RenetClient::new(connection_config()),
        transport,
        client_id,
        action_seq: CommandSeq(0),
        pending: VecDeque::new(),
        prediction: Some(session),
        last_received: 0.0,
        last_send: 0.0,
        last_snapshot: None,
        in_flight_action: None,
        outcomes: Default::default(),
    }
}

#[test]
fn seventy_decoded_full_states_batch_exact_receipts_across_scope_reentry() {
    let (mut session, _) = fixture();
    let epoch = session.model.welcome.stream.epoch;
    let limit = session.model.welcome.frame_limit().unwrap();
    let mut expected = Vec::new();
    let mut envelopes = Vec::new();
    for index in 0..70 {
        let mut state = public_full_state(&session, 100 + index % 35, index + 1);
        if index >= 35 {
            session
                .scopes
                .apply_exit(ScopeExit { scope: state.scope })
                .unwrap();
            state.scope.scope = ScopeEpoch(2);
            state.scope.representation = RepresentationRevision(2);
        }
        expected.push(state.receipt());
        let frame = encode_state(&StateFrame::Full(state), epoch, index as u32, limit).unwrap();
        let Some(ClientControl::Decoded { receipts }) = session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap()
        else {
            panic!("validated public Full must return its decoded receipt");
        };
        assert_eq!(receipts.as_slice(), &expected[expected.len() - 1..]);
        for receipt in receipts.into_vec() {
            if let Some(batch) = session.queue_decoded_receipt(receipt) {
                envelopes.push(session.control(&batch).unwrap());
            }
        }
    }
    assert_eq!(envelopes.len(), 8);
    let partial = session.take_decoded_receipts().unwrap();
    envelopes.push(session.control(&partial).unwrap());
    assert!(session.take_decoded_receipts().is_none());
    assert_eq!(envelopes.len(), 9);
    let mut decoded = Vec::new();
    for (index, envelope) in envelopes.iter().enumerate() {
        let ClientControl::Decoded { receipts } = decode_control(envelope, epoch, limit).unwrap()
        else {
            panic!("batched decoded control");
        };
        assert_eq!(receipts.len(), if index < 8 { 8 } else { 6 });
        decoded.extend(receipts.into_vec());
    }
    assert_eq!(decoded, expected);
    assert_eq!(session.control_sequence, 9);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn disconnect_timeout_boundary_distinguishes_waiting_for_welcome_from_updates() {
    for before_welcome in [true, false] {
        let (session, _) = fixture();
        let mut runtime = receipt_runtime(session);
        runtime.last_received = 100.0;
        if before_welcome {
            runtime.prediction = None;
        }
        assert!(disconnect_report(&runtime, Duration::from_secs(112), None).is_none());
        let report = disconnect_report(&runtime, Duration::from_millis(112_001), None).unwrap();
        assert_eq!(
            report.status,
            if before_welcome {
                "Timed out waiting for the server welcome. Try joining again."
            } else {
                "Timed out waiting for server updates. Join again to reconnect."
            }
        );
        assert!(report.diagnostic.starts_with(if before_welcome {
            "before Welcome:"
        } else {
            "after Welcome:"
        }));
        assert!(
            report
                .diagnostic
                .contains("application receive timeout after 12.001s")
        );
        assert!(report.diagnostic.contains("limit 12.000s"));
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn renet_disconnect_cause_takes_precedence_over_receive_timeout() {
    for before_welcome in [true, false] {
        for transport_failure in [true, false] {
            let (session, _) = fixture();
            let mut runtime = receipt_runtime(session);
            runtime.last_received = 100.0;
            if before_welcome {
                runtime.prediction = None;
            }
            let transport_error = if transport_failure {
                runtime.renet.disconnect_due_to_transport();
                Some("netcode connection timed out")
            } else {
                runtime.renet.disconnect();
                None
            };
            let reason = runtime.renet.disconnect_reason().unwrap();
            let report =
                disconnect_report(&runtime, Duration::from_secs(120), transport_error).unwrap();
            assert_eq!(
                report.status,
                if before_welcome {
                    "Connection ended before the server welcome. Try joining again."
                } else {
                    "Disconnected. Join again to reconnect with the same Traveler ID."
                }
            );
            assert!(
                report
                    .diagnostic
                    .contains(&format!("Renet disconnect {reason:?}"))
            );
            assert!(report.diagnostic.contains("receive gap 20.000s"));
            assert!(!report.diagnostic.contains("application receive timeout"));
            if let Some(error) = transport_error {
                assert!(report.diagnostic.ends_with(&format!("transport: {error}")));
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn disconnection_cleanup_unlocks_identity_and_retains_outcomes_without_double_counting() {
    for transport_error_recorded in [false, true] {
        let (session, _) = fixture();
        let command = Command {
            owner: session.model.welcome.stream,
            sequence: CommandSeq(7),
            target: TargetTick(11),
            input: DreamInput::default().into(),
            actions: BoundedVec::new(vec![ActionEdge {
                slot: 0,
                action: DreamAction::Dash {
                    direction: [1.0, 0.0],
                },
            }])
            .unwrap(),
        };
        let mut runtime = receipt_runtime(session);
        runtime.last_received = 100.0;
        runtime.outcomes.bind_instance(42);
        runtime
            .outcomes
            .track(1, &command, Duration::from_secs(100))
            .unwrap();
        let report = disconnect_report(&runtime, Duration::from_secs(113), None).unwrap();
        let expected_status = report.status;
        let expected_diagnostic = report.diagnostic.clone();
        let mut world = World::new();
        let mut profile = crate::identity::TravelerProfile::default();
        profile.lock().unwrap();
        assert!(profile.is_locked());
        world.insert_resource(profile);
        world.insert_resource(DreamConnection {
            connected: true,
            ..Default::default()
        });
        world.init_resource::<diagnostics::Telemetry>();
        world
            .resource_mut::<diagnostics::Telemetry>()
            .transport_errors = 4;

        finish_disconnection(&mut world, runtime, report, transport_error_recorded);

        assert!(
            !world
                .resource::<crate::identity::TravelerProfile>()
                .is_locked()
        );
        let connection = world.resource::<DreamConnection>();
        assert!(!connection.connected);
        assert_eq!(connection.status, expected_status);
        let telemetry = world.resource::<diagnostics::Telemetry>();
        assert_eq!(
            telemetry.transport_errors,
            if transport_error_recorded { 4 } else { 5 }
        );
        assert_eq!(
            telemetry.transport_issue.as_deref(),
            Some(expected_diagnostic.as_str())
        );
        let retained = world.resource::<outcomes::ClientOutcomes>().trace(1, 2);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].key, command.action_key(0));
        assert_eq!(retained[0].server_instance, 42);
        assert_eq!(retained[0].target_c, ServerTick(11));
        assert_eq!(retained[0].created_at, Duration::from_secs(100));
        assert_eq!(retained[0].kind, "dash");
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn decoded_receipts_flush_in_receive_pass_and_leave_owner_feedback_immediate() {
    use engine_net::replication::baselines::BaselineRepair;
    let (session, owner_frames) = fixture();
    let epoch = session.model.welcome.stream.epoch;
    let limit = session.model.welcome.frame_limit().unwrap();
    let mut runtime = receipt_runtime(session);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    for index in 0..3 {
        let state = public_full_state(runtime.prediction.as_ref().unwrap(), 100 + index, 1);
        let bytes = encode_state(&StateFrame::Full(state), epoch, index as u32, limit).unwrap();
        receive_message(
            &mut runtime,
            STATE_CHANNEL,
            &bytes,
            Duration::from_millis(1),
            &mut world,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(runtime.prediction.as_ref().unwrap().control_sequence, 0);
    }
    let mut owner = None;
    for frame in owner_frames {
        owner = runtime
            .prediction
            .as_mut()
            .unwrap()
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap()
            .or(owner);
    }
    let owner = owner.unwrap();
    let ClientControl::OwnerDecoded { proofs } = &owner else {
        panic!("completed owner group proof");
    };
    let baseline = proofs.as_slice()[0].baseline;
    let repair = ClientControl::BaselineRepair {
        requests: BoundedVec::new(vec![BaselineRepair {
            context: baseline.context,
            generation: baseline.generation,
        }])
        .unwrap(),
    };
    send_state_receipt(&mut runtime, owner).unwrap();
    assert_eq!(runtime.prediction.as_ref().unwrap().control_sequence, 1);
    send_state_receipt(&mut runtime, repair).unwrap();
    assert_eq!(runtime.prediction.as_ref().unwrap().control_sequence, 2);
    flush_decoded_receipts(&mut runtime).unwrap();
    assert_eq!(runtime.prediction.as_ref().unwrap().control_sequence, 3);
    flush_decoded_receipts(&mut runtime).unwrap();
    assert_eq!(runtime.prediction.as_ref().unwrap().control_sequence, 3);

    for index in 0..8 {
        let state = public_full_state(runtime.prediction.as_ref().unwrap(), 200 + index, 1);
        let bytes = encode_state(&StateFrame::Full(state), epoch, index as u32, limit).unwrap();
        receive_message(
            &mut runtime,
            STATE_CHANNEL,
            &bytes,
            Duration::from_millis(2),
            &mut world,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            runtime.prediction.as_ref().unwrap().control_sequence,
            if index < 7 { 3 } else { 4 }
        );
    }
    flush_decoded_receipts(&mut runtime).unwrap();
    assert_eq!(runtime.prediction.as_ref().unwrap().control_sequence, 4);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn rejected_full_states_and_failed_dtos_never_queue_decoded_receipts() {
    let (session, _) = fixture();
    let epoch = session.model.welcome.stream.epoch;
    let limit = session.model.welcome.frame_limit().unwrap();
    let mut reserved = public_full_state(&session, 100, 1);
    reserved.scope.entity = session.model.welcome.owner_entity;
    let mut bad_dto = public_full_state(&session, 101, 1);
    bad_dto.payload = vec![255];
    let mut runtime = receipt_runtime(session);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    for (sequence, state) in [reserved, bad_dto].into_iter().enumerate() {
        let bytes = encode_state(&StateFrame::Full(state), epoch, sequence as u32, limit).unwrap();
        assert!(
            receive_message(
                &mut runtime,
                STATE_CHANNEL,
                &bytes,
                Duration::from_millis(1),
                &mut world,
            )
            .is_err()
        );
        assert!(
            runtime
                .prediction
                .as_mut()
                .unwrap()
                .take_decoded_receipts()
                .is_none()
        );
    }
    flush_decoded_receipts(&mut runtime).unwrap();
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.control_sequence, 0);
    assert_eq!(session.scopes.known_entities(), 0);
    assert_eq!(world.resource::<diagnostics::Telemetry>().snapshots, 0);
}

#[test]
fn replacing_session_discards_pending_decoded_receipts() {
    let (mut session, _) = fixture();
    let old = public_full_state(&session, 100, 1).receipt();
    assert!(session.queue_decoded_receipt(old).is_none());
    let mut welcome = session.model.welcome.clone();
    welcome.stream.epoch = ConnectionEpoch(2);
    session = Session::new(welcome).unwrap();
    assert!(session.take_decoded_receipts().is_none());
    let new = public_full_state(&session, 100, 1).receipt();
    assert!(session.queue_decoded_receipt(new).is_none());
    let Some(ClientControl::Decoded { receipts }) = session.take_decoded_receipts() else {
        panic!("new session's pending receipt");
    };
    assert_eq!(receipts.as_slice(), &[new]);
    assert_ne!(new.scope.connection, old.scope.connection);
    assert_eq!(session.control_sequence, 0);
}

#[test]
fn eight_maximum_width_decoded_receipts_fit_smallest_admitted_frame() {
    let (mut session, _) = fixture();
    let limit = engine_net::codec::FrameLimit::new(1088).unwrap();
    assert!(byte_limits(limit).is_ok());
    let mut batch = None;
    for index in 0..8 {
        let receipt = StateReceipt {
            scope: ScopeIdentity {
                connection: ConnectionEpoch(u64::MAX),
                entity: EntityId {
                    index: u64::MAX - index,
                    generation: u32::MAX,
                },
                scope: ScopeEpoch(u64::MAX),
                representation: RepresentationRevision(u32::MAX),
            },
            baseline_generation: BaselineGeneration(u64::MAX),
            snapshot: SnapshotId(u64::MAX),
            version: StateVersion(u64::MAX),
        };
        batch = session.queue_decoded_receipt(receipt);
        assert_eq!(batch.is_some(), index == 7);
    }
    let batch = batch.unwrap();
    let bytes = encode_control(&batch, ConnectionEpoch(u64::MAX), u32::MAX, limit).unwrap();
    assert!(bytes.len() <= 1088);
    assert_eq!(
        decode_control::<ClientControl>(&bytes, ConnectionEpoch(u64::MAX), limit).unwrap(),
        batch
    );
    assert!(session.take_decoded_receipts().is_none());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn finalized_arrival_feedback_preserves_typed_clock_recovery() {
    use engine_net::{
        commands::{FinalizedReceipt, FinalizedStatus},
        synchronization::{LeadController, ResyncReason},
    };
    let (mut session, _) = fixture();
    session.lead = LeadController::new(12).unwrap();
    let through = session.finalized.checked_next().unwrap();
    session.sequence = CommandSeq(1);
    session.record_command_issuance(
        session.sequence,
        TargetTick(through.0),
        TargetTick(through.0),
    );
    let bytes = encode_control(
        &ServerControl::Finalized {
            through,
            receipts: BoundedVec::new(vec![FinalizedReceipt {
                tick: through,
                sequence: None,
                status: FinalizedStatus::Substituted,
            }])
            .unwrap(),
            menu_applied: None,
            arrival: ArrivalFeedback {
                sample: Some(ArrivalSample {
                    sequence: session.sequence,
                    target: TargetTick(through.0),
                    arrival_tick: through,
                    slack: 0,
                }),
                observed_commands: 1,
                late_commands: 1,
            },
        },
        session.model.welcome.stream.epoch,
        1,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let transport = Transport::new(
        Duration::ZERO,
        renet_cross::ClientAuthentication::Unsecure {
            protocol_id: PROTOCOL_ID,
            client_id: 1,
            server_addr: socket.local_addr().unwrap(),
            user_data: None,
        },
        socket,
    )
    .unwrap();
    // The transport is only a local fixture; this test sends no packets.
    let mut runtime = Runtime {
        renet: RenetClient::new(connection_config()),
        transport,
        client_id: 1,
        action_seq: CommandSeq(0),
        pending: VecDeque::new(),
        prediction: Some(session),
        last_received: 0.0,
        last_send: 0.0,
        last_snapshot: None,
        in_flight_action: None,
        outcomes: Default::default(),
    };
    let mut world = World::new();
    assert!(matches!(
        receive_message(
            &mut runtime,
            CONTROL_CHANNEL,
            &bytes,
            Duration::ZERO,
            &mut world
        ),
        Err(ReceiveRejection::Clock(SyncError::ResyncRequired(
            ResyncReason::LeadExceeded
        )))
    ));
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.finalized, through);
    assert_eq!(session.finalized_ack_pending, None);
    assert_eq!(
        session.lead.needs_resynchronization(),
        Some(ResyncReason::LeadExceeded)
    );
    assert!(matches!(
        receive_message(
            &mut runtime,
            CONTROL_CHANNEL,
            b"invalid",
            Duration::ZERO,
            &mut world
        ),
        Err(ReceiveRejection::Scoped(_))
    ));
}

#[test]
fn cover_marker_stages_until_exact_parent_and_disappears_on_absence() {
    use dreamwake_sim::replication::{PublicCoverMarkerView, PublicCoverView, PublicReplica};
    let (mut session, frames) = fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    fn publish(session: &mut Session, scope: ScopeIdentity, revision: u64, actor: PublicReplica) {
        let state = FullState {
            scope,
            baseline_generation: BaselineGeneration(1),
            snapshot: SnapshotId(revision),
            version: StateVersion(revision),
            end_tick: ServerTick(10 + revision),
            payload: encode_replica(&ReplicaPayload::Actor(actor)).unwrap(),
        };
        session.scopes.apply_full(state, |_| true).unwrap();
        session.entities.insert(scope.entity);
    }
    let mut parent = ScopeIdentity {
        connection: ConnectionEpoch(1),
        entity: EntityId {
            index: 90,
            generation: 1,
        },
        scope: ScopeEpoch(1),
        representation: RepresentationRevision(1),
    };
    let child = ScopeIdentity {
        entity: EntityId {
            index: 91,
            generation: 1,
        },
        ..parent
    };
    let mut marker = PublicCoverMarkerView {
        id: 701,
        parent,
        revision: 1,
        local_offset: [0.0, 1.35, 0.0],
        open: false,
    };
    let mut cover = PublicCoverView {
        id: 700,
        revision: 1,
        present: true,
        position: [0.0, 1.0, -6.0],
        half_extents: [1.5, 1.0, 0.15],
        open: false,
    };
    publish(&mut session, child, 1, PublicReplica::CoverMarker(marker));
    assert!(
        session
            .presentation(true)
            .unwrap()
            .unwrap()
            .cover_markers
            .is_empty()
    );
    publish(&mut session, parent, 1, PublicReplica::Cover(cover));
    let shown = session.presentation(true).unwrap().unwrap();
    assert_eq!(shown.cover_markers[0].position, [0.0, 2.35, -6.0]);
    session
        .scopes
        .apply_exit(ScopeExit { scope: parent })
        .unwrap();
    parent.scope = ScopeEpoch(2);
    publish(&mut session, parent, 2, PublicReplica::Cover(cover));
    assert!(
        session
            .presentation(true)
            .unwrap()
            .unwrap()
            .cover_markers
            .is_empty()
    );
    marker.parent = parent;
    marker.revision = 2;
    publish(&mut session, child, 2, PublicReplica::CoverMarker(marker));
    assert_eq!(
        session
            .presentation(true)
            .unwrap()
            .unwrap()
            .cover_markers
            .len(),
        1
    );
    parent.entity.generation = 2;
    parent.scope = ScopeEpoch(1);
    publish(&mut session, parent, 3, PublicReplica::Cover(cover));
    assert!(
        session
            .presentation(true)
            .unwrap()
            .unwrap()
            .cover_markers
            .is_empty()
    );
    marker.parent = parent;
    marker.revision = 3;
    publish(&mut session, child, 3, PublicReplica::CoverMarker(marker));
    assert_eq!(
        session
            .presentation(true)
            .unwrap()
            .unwrap()
            .cover_markers
            .len(),
        1
    );
    cover.present = false;
    cover.revision = 2;
    publish(&mut session, parent, 4, PublicReplica::Cover(cover));
    let shown = session.presentation(true).unwrap().unwrap();
    assert!(shown.cover_markers.is_empty());
    assert!(shown.covers.is_empty());
}

#[test]
fn stable_player_identity_is_distinct_from_transport_identity_in_client_view() {
    let (session, _) = fixture();
    let mut welcome = session.model.welcome.clone();
    welcome.client = ConnectionId(77);
    welcome.stream.connection = ConnectionId(77);
    let session = Session::new(welcome).unwrap();
    let mut connection = crate::DreamConnection::default();
    session.update_connection(&mut connection, 0.0, 1);
    assert_eq!(connection.client_id, 77);
    assert_eq!(connection.player_id, 1);
    assert_eq!(session.model.expectation().owner, 1);
}

#[test]
fn slow_first_clock_reply_does_not_partially_initialize_prediction() {
    let (mut session, _) = fixture();
    let delayed = TimeExchange {
        client_send: Duration::from_secs(1),
        server_receive: Duration::from_millis(2020),
        server_send: Duration::from_millis(2020),
        client_receive: Duration::from_millis(1700),
    };
    assert_eq!(
        session.observe_clock(delayed),
        Err(engine_net::synchronization::SyncError::ResyncRequired(
            engine_net::synchronization::ResyncReason::LeadExceeded
        ))
    );
    assert_eq!(session.clock.sample_count(), 0);
    assert_eq!(session.clock.target_offset_nanos(), None);
    assert_eq!(session.lead.lead(), 3);
    assert_eq!(
        session.observe_clock(TimeExchange {
            client_receive: Duration::ZERO,
            ..delayed
        }),
        Err(engine_net::synchronization::SyncError::InvalidSample)
    );
    assert_eq!(session.clock.sample_count(), 0);
    session
        .observe_clock(TimeExchange {
            client_send: Duration::from_secs(2),
            server_receive: Duration::from_millis(3020),
            server_send: Duration::from_millis(3020),
            client_receive: Duration::from_millis(2040),
        })
        .unwrap();
    assert_eq!(session.clock.sample_count(), 1);
    assert_eq!(session.clock.target_offset_nanos(), Some(1_000_000_000));
    assert_eq!(session.lead.lead(), 3);
    assert_eq!(
        session
            .clock
            .estimate_server_tick(Duration::from_millis(2040))
            .unwrap(),
        ServerTick(182)
    );
    let suspended = engine_net::synchronization::SyncError::ResyncRequired(
        engine_net::synchronization::ResyncReason::Suspension,
    );
    assert_eq!(
        session.clock.estimate_server_tick(Duration::from_secs(6)),
        Err(suspended)
    );
    assert_eq!(
        session.observe_clock(TimeExchange {
            client_send: Duration::from_secs(6),
            server_receive: Duration::from_millis(7020),
            server_send: Duration::from_millis(7020),
            client_receive: Duration::from_millis(6040),
        }),
        Err(suspended)
    );
    assert_eq!(session.clock.sample_count(), 1);
}

pub(super) fn fixture_publication(
    tick: u64,
    snapshot: u64,
    topology: u64,
) -> (Session, Vec<Vec<u8>>) {
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    fixture_from_sim(&sim, tick, snapshot, topology)
}

pub(super) fn fixture_from_sim(
    sim: &DreamSimulation,
    tick: u64,
    snapshot: u64,
    topology: u64,
) -> (Session, Vec<Vec<u8>>) {
    let stamp = ReplicationStamp {
        match_epoch: 1,
        server_tick: tick,
        gameplay_tick: sim.snapshot().tick,
        scene_revision: 1,
        revision: snapshot,
    };
    let capture = sim.capture_replication(stamp).unwrap();
    let global_entity = EntityId {
        index: 1,
        generation: 1,
    };
    let owner_entity = EntityId {
        index: 2,
        generation: 1,
    };
    let collision_entity = EntityId {
        index: 3,
        generation: 1,
    };
    let welcome = Welcome {
        server_instance: 1,
        protocol: PROTOCOL_ID,
        client: ConnectionId(1),
        player: dreamwake_protocol::player::PlayerId::new(1).unwrap(),
        stream: OwnerStream {
            connection: ConnectionId(1),
            epoch: ConnectionEpoch(1),
            owner: owner_entity,
            ownership: OwnershipEpoch(1),
            stream: CommandStream(1),
        },
        match_epoch: 1,
        tick: ServerTick(10),
        server_time_nanos: 166_666_667,
        content: ContentIdentity::current(SceneRevision(1)),
        global_entity,
        owner_entity,
        collision_entity,
        application_frame_bytes: 1100,
    };
    let mut members = [
        (
            collision_entity,
            ReplicaPayload::Collision(sim.snapshot().collision_manifest().clone()),
        ),
        (
            global_entity,
            ReplicaPayload::Global(capture.global().clone()),
        ),
        (
            owner_entity,
            ReplicaPayload::Owner(capture.owner_checkpoint(1, 1).unwrap()),
        ),
    ]
    .into_iter()
    .map(|(entity, payload)| FullState {
        scope: ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity,
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        },
        baseline_generation: BaselineGeneration(1),
        snapshot: SnapshotId(snapshot),
        version: StateVersion(snapshot),
        end_tick: ServerTick(tick),
        payload: encode_replica(&payload).unwrap(),
    })
    .collect::<Vec<_>>();
    let sources = dreamwake_sim::replication::ApprovedSources::new([1]).unwrap();
    for key in capture.keys() {
        let dreamwake_sim::replication::ReplicationKey::Platform(_) = key else {
            continue;
        };
        let Some(actor @ dreamwake_sim::replication::PublicReplica::Platform(_)) =
            capture.project(key, &sources)
        else {
            continue;
        };
        let dreamwake_sim::replication::PublicReplica::Platform(view) = &actor else {
            unreachable!()
        };
        if capture.required_bases(1).contains(&view.collider) {
            let mut state = members[0].clone();
            state.scope.entity = EntityId {
                index: 4,
                generation: 1,
            };
            state.payload = encode_replica(&ReplicaPayload::Actor(actor)).unwrap();
            members.push(state);
        }
    }
    members.sort_by_key(|state| state.scope);
    let publication = GroupPublication {
        connection: ConnectionEpoch(1),
        group: OWNER_GROUP,
        revision: GroupRevision(topology),
        snapshot: SnapshotId(snapshot),
    };
    for state in &mut members {
        wrap_full_baseline(state, publication);
    }
    let group = GroupChunk {
        publication,
        end_tick: ServerTick(tick),
        manifest: members.iter().map(|s| s.scope).collect(),
        index: 0,
        count: 1,
        members,
    };
    let encoded = encode_group(&group, group_limits(), |p| Ok(p.clone())).unwrap();
    let limit = welcome.frame_limit().unwrap();
    let fragments = fragment_group(publication, &encoded, byte_limits(limit).unwrap()).unwrap();
    let frames = fragments
        .into_iter()
        .enumerate()
        .map(|(sequence, fragment)| {
            encode_state(
                &StateFrame::Fragment(fragment),
                ConnectionEpoch(1),
                sequence as u32,
                limit,
            )
            .unwrap()
        })
        .collect();
    (Session::new(welcome).unwrap(), frames)
}
#[test]
fn live_owner_bootstrap_requires_complete_decoded_group_and_active_proof() {
    let (mut session, mut frames) = fixture();
    session.begin_frame().unwrap();
    assert!(frames.len() > 1);
    frames.reverse();
    for frame in &frames[..frames.len() - 1] {
        assert!(
            session
                .receive_state(frame, Duration::from_millis(1))
                .unwrap()
                .is_none()
        );
        session.reconcile().unwrap();
        assert_eq!(session.predictor.group_count(), 0);
        assert!(!session.active);
    }
    let receipt = session
        .receive_state(frames.last().unwrap(), Duration::from_millis(2))
        .unwrap()
        .unwrap();
    let ClientControl::OwnerDecoded { proofs } = receipt else {
        panic!("decoded receipt")
    };
    assert_eq!(proofs.len(), 4);
    session.reconcile().unwrap();
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(10))
    );
    assert!(!session.active);
    session.acknowledged_active = Some((SnapshotId(99), ServerTick(10)));
    session.reconcile().unwrap();
    assert!(!session.active);
    session.acknowledged_active = Some((SnapshotId(1), ServerTick(10)));
    session.reconcile().unwrap();
    assert!(session.active);
    assert!(has_local_hero(
        &session.presentation(true).unwrap().unwrap(),
        1
    ));
}
#[test]
fn isolated_live_predictor_uses_shared_owner_motion_without_advancing_authority_stamp() {
    let (mut session, frames) = fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    let before = session.predictor.state(OWNER_GROUP).unwrap().0.clone();
    let input = DreamInput {
        movement: [1.0, 0.0],
        aim: [1.0, 0.0],
        ..Default::default()
    };
    let mut expected = before.clone();
    expected.record_predicted_input(input);
    let deps = current_dependencies(&session);
    let engine_net::prediction::DependencyValue::Known(
        model::CollisionDependency::StaticCollision(collision),
    ) = deps.value(session.model.welcome.collision_entity).unwrap()
    else {
        panic!("missing collision");
    };
    let bases = deps
        .records
        .values()
        .filter_map(|record| match &record.value {
            engine_net::prediction::DependencyValue::Known(
                model::CollisionDependency::Platform(base),
            ) => Some(base.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    expected
        .step_restricted_combat_with_bases(input, collision, &bases, None, None)
        .unwrap();
    let command = Command {
        owner: session.model.welcome.stream,
        sequence: CommandSeq(1),
        target: TargetTick(11),
        input: input.into(),
        actions: BoundedVec::default(),
    };
    session
        .predictor
        .predict(
            OWNER_GROUP,
            InputCommand(command),
            current_dependencies(&session),
            &session.model,
        )
        .unwrap();
    let after = &session.predictor.state(OWNER_GROUP).unwrap().0;
    assert_eq!(after, &expected);
    assert_eq!(after.stamp(), before.stamp());
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(11))
    );
    assert_ne!(after.hero_view().position, before.hero_view().position);
}
#[test]
fn private_standalone_and_incompatible_welcome_never_bootstrap_prediction() {
    let (mut session, frames) = fixture();
    let mut incompatible = session.model.welcome.clone();
    incompatible.client = ConnectionId(2);
    assert!(!incompatible.compatible(ConnectionId(1)));
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    let state = session
        .scopes
        .state(session.model.welcome.global_entity)
        .unwrap()
        .clone();
    let bytes = encode_state(
        &StateFrame::Full(state),
        ConnectionEpoch(1),
        2,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap();
    let mut empty = Session::new(session.model.welcome.clone()).unwrap();
    assert!(
        empty
            .receive_state(&bytes, Duration::from_millis(2))
            .is_err()
    );
    assert_eq!(empty.scopes.known_entities(), 0);
    assert_eq!(empty.predictor.group_count(), 0);
}

#[test]
fn active_owner_reconciles_publications_and_replaces_advanced_group_revisions() {
    let (mut session, frames) = fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    session.acknowledged_active = Some((SnapshotId(1), ServerTick(10)));
    session.reconcile().unwrap();
    assert!(session.active);
    let identity = session.predictor.identity(OWNER_GROUP).unwrap().clone();
    for tick in 11..=12 {
        session
            .predictor
            .predict(
                OWNER_GROUP,
                InputCommand(Command {
                    owner: session.model.welcome.stream,
                    sequence: CommandSeq(tick - 10),
                    target: TargetTick(tick),
                    input: DreamInput {
                        movement: [1.0, 0.0],
                        aim: [1.0, 0.0],
                        ..Default::default()
                    }
                    .into(),
                    actions: BoundedVec::default(),
                }),
                current_dependencies(&session),
                &session.model,
            )
            .unwrap();
    }
    session.sequence = CommandSeq(2);
    let (_, frames) = fixture_publication(11, 2, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(20))
            .unwrap();
    }
    // State may beat the independent reliable finalization lane. Keep prediction
    // live and retain the checkpoint until that proof arrives.
    session.reconcile().unwrap();
    assert!(session.active);
    assert_eq!(
        session
            .predictor
            .state(OWNER_GROUP)
            .unwrap()
            .0
            .stamp()
            .revision,
        1
    );
    session.finalized = ServerTick(11);
    session.reconcile().unwrap();
    assert!(session.active);
    assert!(!session.recovering);
    assert_eq!(session.predictor.identity(OWNER_GROUP), Some(&identity));
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(12))
    );
    let predicted = session.predictor.state(OWNER_GROUP).unwrap();
    assert_eq!(predicted.0.stamp().revision, 2);
    assert_eq!(predicted.0.stamp().server_tick, 11);
    session
        .predictor
        .predict(
            OWNER_GROUP,
            InputCommand(Command {
                owner: session.model.welcome.stream,
                sequence: CommandSeq(3),
                target: TargetTick(13),
                input: DreamInput::default().into(),
                actions: BoundedVec::default(),
            }),
            current_dependencies(&session),
            &session.model,
        )
        .unwrap();
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(13))
    );
    // A later topology revision replaces prediction even when an intermediate
    // closure was not received and the final manifest has the same scopes.
    let (_, frames) = fixture_publication(12, 3, 2);
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(30))
            .unwrap();
    }
    session.finalized = ServerTick(12);
    session.reconcile().unwrap();
    let replaced = session.predictor.identity(OWNER_GROUP).unwrap();
    assert_eq!(replaced.group_revision, GroupRevision(2));
    assert_eq!(replaced.binding, identity.binding);
    assert_eq!(replaced.scopes, identity.scopes);
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(13))
    );
    assert!(
        session
            .predictor
            .command_history(OWNER_GROUP, ServerTick(13))
            .is_some()
    );
}

#[test]
fn timestamped_held_intent_and_edges_match_at_30_60_and_240_render_fps() {
    fn trace(fps: u32) -> Vec<(DreamInput, Vec<DreamAction>)> {
        let (mut session, _) = fixture();
        let movement = DreamInput {
            movement: [1.0, 0.0],
            aim: [1.0, 0.0],
            ..Default::default()
        };
        // Raw input timestamps are independent of render batching. The press and
        // release at 5/9ms both occur inside a single 30Hz rendered frame.
        for (millis, held, edges) in [
            (0, movement, vec![]),
            (
                5,
                DreamInput {
                    attack: true,
                    ..movement
                },
                vec![DreamAction::Dash {
                    direction: [1.0, 0.0],
                }],
            ),
            (9, movement, vec![]),
            (
                200,
                DreamInput {
                    attack: true,
                    ..movement
                },
                vec![DreamAction::Cast {
                    slot: 0,
                    aim: [1.0, 0.0],
                }],
            ),
            (
                400,
                DreamInput {
                    attack: true,
                    ..Default::default()
                },
                vec![],
            ),
            (600, DreamInput::default(), vec![]),
        ] {
            session
                .journal
                .capture(HardwareSample {
                    at: Duration::from_millis(millis),
                    held: Some(held.into()),
                    edges: BoundedVec::new(edges).unwrap(),
                    mouse_delta: [0.0; 2],
                })
                .unwrap();
        }
        let rate = TickRate::new(60).unwrap();
        let mut current = 0;
        let mut trace = Vec::new();
        for frame in 1..=fps {
            let now = TickRate::new(fps)
                .unwrap()
                .deadline(ServerTick(u64::from(frame)))
                .unwrap();
            let newest = rate.elapsed_ticks(now).unwrap().0;
            for tick in current + 1..=newest {
                let sample = session
                    .command_sample(TargetTick(tick), TargetTick(newest), now)
                    .unwrap();
                trace.push((sample.held.held, sample.edges.into_vec()));
            }
            current = newest;
        }
        trace
    }
    let sixty = trace(60);
    assert_eq!(trace(30), sixty);
    assert_eq!(trace(240), sixty);
    assert_eq!(sixty.len(), 60);
    assert_eq!(
        sixty
            .iter()
            .filter(|(input, _)| input.movement == [1.0, 0.0])
            .count(),
        23
    );
    assert_eq!(sixty.iter().filter(|(input, _)| input.attack).count(), 24);
    assert_eq!(sixty.iter().map(|(_, edges)| edges.len()).sum::<usize>(), 2);
    assert!(matches!(sixty[0].1.as_slice(), [DreamAction::Dash { .. }]));
    assert!(matches!(sixty[11].1.as_slice(), [DreamAction::Cast { .. }]));
}

#[test]
fn small_finalized_checkpoint_ahead_is_adopted_without_resync_or_input_loss() {
    let (mut session, frames) = fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    session.acknowledged_active = Some((SnapshotId(1), ServerTick(10)));
    session.reconcile().unwrap();
    session.sequence = CommandSeq(4);
    let movement = DreamInput {
        movement: [1.0, 0.0],
        aim: [1.0, 0.0],
        ..Default::default()
    };
    session
        .journal
        .capture(HardwareSample {
            at: Duration::from_millis(5),
            held: Some(movement.into()),
            edges: BoundedVec::new(vec![DreamAction::Dash {
                direction: [1.0, 0.0],
            }])
            .unwrap(),
            mouse_delta: [0.0; 2],
        })
        .unwrap();
    let identity = session.predictor.identity(OWNER_GROUP).unwrap().clone();
    let (_, frames) = fixture_publication(11, 2, 1);
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(10))
            .unwrap();
    }
    // Not a finalized-input proof yet: do not adopt or request recovery.
    session.reconcile().unwrap();
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(10))
    );
    session.finalized = ServerTick(11);
    session.reconcile().unwrap();
    assert!(session.active && !session.recovering);
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(11))
    );
    assert_eq!(session.predictor.identity(OWNER_GROUP), Some(&identity));
    assert_eq!(session.sequence, CommandSeq(4));
    assert_eq!(session.journal.pending_samples(), 1);
    let sample = session
        .command_sample(TargetTick(12), TargetTick(12), Duration::from_millis(20))
        .unwrap();
    assert_eq!(sample.held.held, movement);
    assert_eq!(sample.edges.len(), 1);
    session
        .predictor
        .predict(
            OWNER_GROUP,
            InputCommand(Command {
                owner: session.model.welcome.stream,
                sequence: CommandSeq(5),
                target: TargetTick(12),
                input: sample.held,
                actions: BoundedVec::default(),
            }),
            current_dependencies(&session),
            &session.model,
        )
        .unwrap();
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(12))
    );
    let (_, frames) = fixture_publication(30, 3, 1);
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(30))
            .unwrap();
    }
    session.finalized = ServerTick(30);
    assert!(session.reconcile().unwrap_err().contains("CheckpointAhead"));
}

#[test]
fn active_control_keeps_external_connection_playable_after_initial_proof_is_evicted() {
    let (mut session, frames) = fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    let mut connection = crate::DreamConnection::default();
    session.update_connection(&mut connection, 14.0, 1);
    assert!(!connection.connected);
    let control = ServerControl::Active {
        owner_snapshot: SnapshotId(1),
        through: ServerTick(10),
    };
    let bytes = encode_control(
        &control,
        ConnectionEpoch(1),
        1,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap();
    let ServerControl::Active {
        owner_snapshot,
        through,
    } = decode_control::<ServerControl>(
        &bytes,
        ConnectionEpoch(1),
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap()
    else {
        panic!("active control")
    };
    session.acknowledge_active(owner_snapshot, through);
    session.reconcile().unwrap();
    for snapshot in 2..=40 {
        session.begin_frame().unwrap();
        let (_, frames) = fixture_publication(9 + snapshot, snapshot, 1);
        for frame in frames {
            session
                .receive_state(&frame, Duration::from_millis(snapshot * 50))
                .unwrap();
        }
        let fences = session.baselines.retirements().unwrap();
        session.baselines.acknowledge_retirements(&fences).unwrap();
        session.finalized = ServerTick(9 + snapshot);
        session.reconcile().unwrap();
        let displayed = session.presentation(true).unwrap().unwrap();
        session.update_connection(&mut connection, 14.0, displayed.heroes.len());
        assert!(connection.connected, "publication {snapshot}");
        assert_eq!(connection.status, "Connected to the shared dream");
        assert_eq!(connection.client_id, 1);
        assert_eq!(connection.party_size, 1);
    }
    assert!(!session.decoded_groups.contains_key(&SnapshotId(1)));
    assert!(connection.connected);
    let metrics = session.metrics();
    assert_eq!(metrics.transport_connection, connection.client_id);
    assert!(metrics.active && !metrics.recovering);
    assert_eq!(metrics.finalized_tick, ServerTick(49));
    assert_eq!(metrics.checkpoint_server_tick, Some(ServerTick(49)));
    assert_eq!(metrics.predicted_tick, Some(ServerTick(49)));
    assert_eq!(metrics.gameplay_tick, Some(0));
    assert_eq!(metrics.live_scopes, 4);
    assert_eq!(metrics.incomplete_fragments, 0);
    assert!(metrics.prediction_bytes > 0 && metrics.scope_bytes > 0);

    session.recover();
    session.update_connection(&mut connection, 14.0, 1);
    assert!(!connection.connected);
    // A stream reset does not renumber the server's transport peer.
    session.model.welcome.stream.epoch = ConnectionEpoch(77);
    assert_eq!(session.metrics().connection_epoch, ConnectionEpoch(77));
    assert_eq!(session.metrics().transport_connection, 1);
}

#[test]
fn committed_clock_floor_only_affects_new_command_targets() {
    let (mut session, _) = fixture();
    session.simulation_clock.commit(ServerTick(100));
    let estimate = session
        .simulation_clock
        .estimate(Duration::from_millis(1500), session.lead.lead())
        .unwrap();
    let first = session.assign_command_target(estimate).unwrap().unwrap();
    assert_eq!(first, TargetTick(103));
    session.simulation_clock.commit(ServerTick(102));
    let estimate = session
        .simulation_clock
        .estimate(Duration::from_millis(1516), session.lead.lead())
        .unwrap();
    let next = session.assign_command_target(estimate).unwrap().unwrap();
    assert_eq!(next, TargetTick(105));
    assert_eq!(first, TargetTick(103));
    assert_eq!(session.assign_command_target(estimate).unwrap(), None);
}

fn current_dependencies(
    session: &Session,
) -> engine_net::prediction::DependencyFrame<model::CollisionDependency> {
    session
        .predictor
        .dependency_history(
            OWNER_GROUP,
            session.predictor.predicted_tick(OWNER_GROUP).unwrap(),
        )
        .unwrap()
        .clone()
}

#[test]
fn collision_dependency_is_retained_and_absence_never_predicts_empty_space() {
    use engine_net::prediction::{DependencyValue, PredictionModel};
    let (mut session, frames) = fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    let dependencies = current_dependencies(&session);
    let collision_entity = session.model.welcome.collision_entity;
    let record = dependencies.records.get(&collision_entity).unwrap();
    assert_eq!(
        record.scope,
        session.scopes.state(collision_entity).unwrap().scope
    );
    assert_eq!(
        record.version,
        session.scopes.state(collision_entity).unwrap().version
    );
    let DependencyValue::Known(model::CollisionDependency::StaticCollision(collision)) =
        &record.value
    else {
        panic!("known immutable geometry");
    };
    assert!(collision.retained_bytes() > 16 * 1024);
    for predicted in [false, true] {
        let view = session.presentation(predicted).unwrap().unwrap();
        assert!(std::sync::Arc::ptr_eq(
            view.collision.as_ref().unwrap(),
            &collision.manifest_arc(),
        ));
    }
    let original = session.predictor.state(OWNER_GROUP).unwrap().clone();
    let command = InputCommand(Command {
        owner: session.model.welcome.stream,
        sequence: CommandSeq(1),
        target: TargetTick(11),
        input: DreamInput {
            movement: [1.0, 0.0],
            ..Default::default()
        }
        .into(),
        actions: BoundedVec::default(),
    });
    for value in [
        DependencyValue::ExplicitlyAbsent,
        DependencyValue::Unavailable,
    ] {
        let mut missing = dependencies.clone();
        missing.records.get_mut(&collision_entity).unwrap().value = value;
        let mut staged = original.clone();
        assert!(
            session
                .model
                .step(&mut staged, ServerTick(11), &command, &missing)
                .is_err()
        );
        assert!(staged == original);
    }
    let mut wrong = dependencies;
    wrong
        .records
        .get_mut(&collision_entity)
        .unwrap()
        .scene_revision = SceneRevision(2);
    let mut staged = original.clone();
    assert!(
        session
            .model
            .step(&mut staged, ServerTick(11), &command, &wrong)
            .is_err()
    );
    assert!(staged == original);
    let state = session.scopes.state(collision_entity).unwrap();
    let exit = engine_net::replication::ScopeExit { scope: state.scope };
    session.scopes.apply_exit(exit).unwrap();
    assert!(session.presentation(true).unwrap().is_none());
}

#[test]
fn newer_collision_publication_keeps_retained_prediction_visible_until_reconciliation() {
    let (mut session, frames) = fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    let retained = current_dependencies(&session);
    let collision_entity = session.model.welcome.collision_entity;
    let retained_record = retained.records.get(&collision_entity).unwrap();
    let (_, newer) = fixture_publication(13, 2, 1);
    session.begin_frame().unwrap();
    for frame in newer {
        session
            .receive_state(&frame, Duration::from_millis(50))
            .unwrap();
    }
    let current = session.scopes.state(collision_entity).unwrap();
    assert_eq!(retained_record.scope, current.scope);
    assert_ne!(retained_record.version, current.version);
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(10))
    );
    assert_eq!(
        session.authoritative_owner_pose().unwrap().unwrap().0,
        ServerTick(13)
    );
    for predicted in [false, true] {
        let view = session.presentation(predicted).unwrap().unwrap();
        assert!(view.collision.is_some());
    }
    session
        .scopes
        .apply_exit(ScopeExit {
            scope: current.scope,
        })
        .unwrap();
    assert!(session.presentation(true).unwrap().is_none());
}

#[test]
fn platform_topology_changes_replay_moving_inputs_without_changing_owner_binding() {
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    // Begin outside the charge-expanded support dependency envelope so the
    // test performs actual 3→4→3 manifest changes, not revision-only changes.
    {
        let world = sim.world_mut();
        *world
            .query::<&mut engine_core::KinematicState>()
            .single_mut(world)
            .unwrap() = engine_core::KinematicState::new([0.0, 0.0, 15.0], 1);
    }
    sim.step(DreamInput::default());
    let (mut session, first) = fixture_from_sim(&sim, 10, 1, 1);
    session.begin_frame().unwrap();
    for frame in first {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    let binding = session.model.binding();
    session.active = true;
    for tick in 11..=15 {
        let dependencies = current_dependencies(&session);
        if tick == 14 {
            session
                .predictor
                .predict_substitute(OWNER_GROUP, ServerTick(tick), dependencies, &session.model)
                .unwrap();
            continue;
        }
        let input = DreamInput {
            movement: [1.0, 0.0],
            ..Default::default()
        };
        session
            .predictor
            .predict(
                OWNER_GROUP,
                InputCommand(Command {
                    owner: session.model.welcome.stream,
                    sequence: CommandSeq(tick - 10),
                    target: TargetTick(tick),
                    input: input.into(),
                    actions: BoundedVec::default(),
                }),
                dependencies,
                &session.model,
            )
            .unwrap();
    }
    {
        let world = sim.world_mut();
        *world
            .query::<&mut engine_core::KinematicState>()
            .single_mut(world)
            .unwrap() = engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
    }
    sim.step(DreamInput::default());
    let (_, platform_frames) = fixture_from_sim(&sim, 13, 2, 2);
    session.begin_frame().unwrap();
    session.finalized = ServerTick(13);
    for frame in &platform_frames {
        session
            .receive_state(frame, Duration::from_millis(50))
            .unwrap();
    }
    session.reconcile().unwrap();
    assert_eq!(
        session
            .predictor
            .identity(OWNER_GROUP)
            .unwrap()
            .scopes
            .len(),
        4
    );
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(15))
    );
    assert_eq!(
        session.predictor.identity(OWNER_GROUP).unwrap().binding,
        binding
    );
    assert!(
        session
            .predictor
            .command_history(OWNER_GROUP, ServerTick(15))
            .is_some()
    );
    assert!(matches!(
        session
            .predictor
            .replay_input_history(OWNER_GROUP, ServerTick(14)),
        Some(engine_net::prediction::ReplayInput::Substitute)
    ));
    assert!(
        session
            .predictor
            .command_history(OWNER_GROUP, ServerTick(14))
            .is_none()
    );
    {
        let world = sim.world_mut();
        *world
            .query::<&mut engine_core::KinematicState>()
            .single_mut(world)
            .unwrap() = engine_core::KinematicState::new([0.0, 0.0, 15.0], 1);
    }
    sim.step(DreamInput::default());
    let (_, reduced) = fixture_from_sim(&sim, 14, 3, 3);
    session.begin_frame().unwrap();
    session.finalized = ServerTick(14);
    for frame in reduced {
        session
            .receive_state(&frame, Duration::from_millis(100))
            .unwrap();
    }
    session.reconcile().unwrap();
    assert_eq!(
        session
            .predictor
            .identity(OWNER_GROUP)
            .unwrap()
            .scopes
            .len(),
        3
    );
    assert_eq!(
        session.predictor.identity(OWNER_GROUP).unwrap().binding,
        binding
    );
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(15))
    );
    for frame in platform_frames {
        assert!(matches!(
            session.receive_state(&frame, Duration::from_millis(150)),
            Err(ReceiveRejection::Replication {
                error: engine_net::replication::ReplicationError::Obsolete(
                    engine_net::replication::ObsoleteReason::Publication
                ),
                ..
            })
        ));
    }
    assert_eq!(
        session
            .groups
            .published_group(OWNER_GROUP, &session.scopes)
            .unwrap()
            .publication()
            .revision,
        GroupRevision(3)
    );
    assert_eq!(
        session
            .predictor
            .identity(OWNER_GROUP)
            .unwrap()
            .scopes
            .len(),
        3
    );
}

#[test]
fn platform_group_is_exact_and_retains_immutable_base_for_replay() {
    use engine_net::prediction::{DependencyValue, PredictionModel};
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    {
        let world = sim.world_mut();
        let mut query = world.query::<&mut engine_core::KinematicState>();
        *query.single_mut(world).unwrap() = engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
    }
    sim.step(DreamInput::default());
    let (mut session, frames) = fixture_from_sim(&sim, 10, 1, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    let original = session.predictor.state(OWNER_GROUP).unwrap().clone();
    assert_eq!(original.1.len(), 1);
    let (&collider, &entity) = original.1.first_key_value().unwrap();
    let dependencies = current_dependencies(&session);
    let DependencyValue::Known(model::CollisionDependency::Platform(view)) =
        &dependencies.records[&entity].value
    else {
        panic!("typed base")
    };
    assert_eq!(view.collider, collider);
    let mut raw = [
        session.model.welcome.global_entity,
        session.model.welcome.owner_entity,
        session.model.welcome.collision_entity,
        entity,
    ]
    .into_iter()
    .map(|id| session.scopes.state(id).unwrap().clone())
    .collect::<Vec<_>>();
    assert!(
        model::validate_owner_group(&session.model, OWNER_GROUP, ServerTick(10), raw.iter())
            .is_ok()
    );
    let mut platform_member = raw.pop().unwrap();
    assert!(
        model::validate_owner_group(&session.model, OWNER_GROUP, ServerTick(10), raw.iter())
            .is_err()
    );
    let mut wrong_view = *view;
    wrong_view.collider.generation += 1;
    platform_member.payload = encode_replica(&ReplicaPayload::Actor(
        dreamwake_sim::replication::PublicReplica::Platform(wrong_view),
    ))
    .unwrap();
    raw.push(platform_member);
    assert!(
        model::validate_owner_group(&session.model, OWNER_GROUP, ServerTick(10), raw.iter())
            .is_err()
    );
    let command = InputCommand(Command {
        owner: session.model.welcome.stream,
        sequence: CommandSeq(1),
        target: TargetTick(11),
        input: TickInput::default(),
        actions: BoundedVec::default(),
    });
    let mut predicted = original.clone();
    session
        .model
        .step(&mut predicted, ServerTick(11), &command, &dependencies)
        .unwrap();
    assert!(predicted.0.base_attachment().is_some());
    let mut missing = dependencies.clone();
    missing.records.remove(&entity);
    let mut stopped = original.clone();
    assert!(
        session
            .model
            .step(&mut stopped, ServerTick(11), &command, &missing)
            .is_err()
    );
    assert_eq!(stopped, original);
    // Current replica lifecycle does not mutate the privately retained base.
    session
        .scopes
        .apply_exit(ScopeExit {
            scope: session.scopes.state(entity).unwrap().scope,
        })
        .unwrap();
    let mut replayed = original.clone();
    session
        .model
        .step(&mut replayed, ServerTick(11), &command, &dependencies)
        .unwrap();
    assert_eq!(replayed, predicted);
    let mut wrong = dependencies.clone();
    if let DependencyValue::Known(model::CollisionDependency::Platform(value)) =
        &mut wrong.records.get_mut(&entity).unwrap().value
    {
        value.collider.generation += 1;
    }
    assert!(
        session
            .model
            .step(&mut original.clone(), ServerTick(11), &command, &wrong)
            .is_err()
    );
}

#[test]
fn collision_group_identity_is_validated_before_atomic_publication() {
    let (mut source, frames) = fixture();
    for frame in frames {
        source
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    let welcome = source.model.welcome.clone();
    let mut members = source
        .groups
        .published_group(OWNER_GROUP, &source.scopes)
        .unwrap()
        .manifest()
        .iter()
        .map(|scope| source.scopes.state(scope.entity).unwrap().clone())
        .collect::<Vec<_>>();
    members[2].payload = encode_replica(&ReplicaPayload::Collision(
        dreamwake_sim::collision::CollisionManifest::current(2),
    ))
    .unwrap();
    let publication = GroupPublication {
        connection: welcome.stream.epoch,
        group: OWNER_GROUP,
        revision: GroupRevision(1),
        snapshot: SnapshotId(1),
    };
    for state in &mut members {
        wrap_full_baseline(state, publication);
    }
    let group = GroupChunk {
        publication,
        end_tick: ServerTick(10),
        manifest: members.iter().map(|s| s.scope).collect(),
        index: 0,
        count: 1,
        members,
    };
    let encoded = encode_group(&group, group_limits(), |p| Ok(p.clone())).unwrap();
    let limit = welcome.frame_limit().unwrap();
    let mut empty = Session::new(welcome).unwrap();
    let mut rejected = false;
    for (index, fragment) in fragment_group(publication, &encoded, byte_limits(limit).unwrap())
        .unwrap()
        .into_iter()
        .enumerate()
    {
        let bytes = encode_state(
            &StateFrame::Fragment(fragment),
            ConnectionEpoch(1),
            index as u32,
            limit,
        )
        .unwrap();
        rejected |= empty
            .receive_state(&bytes, Duration::from_millis(1))
            .is_err();
    }
    assert!(rejected);
    assert_eq!(empty.baselines.retained_bytes(), 0);
    assert_eq!(empty.scopes.known_entities(), 0);
    assert_eq!(empty.predictor.group_count(), 0);
}

#[test]
fn moderate_join_gap_uses_budget_and_preserves_sample_deadlines() {
    let (mut session, frames) = fixture_publication(411, 1, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.acknowledge_active(SnapshotId(1), ServerTick(411));
    session.reconcile().unwrap();
    let now = Duration::from_secs(2);
    session
        .journal
        .capture(HardwareSample {
            at: now,
            held: Some(
                DreamInput {
                    movement: [1.0, 0.0],
                    ..Default::default()
                }
                .into(),
            ),
            edges: BoundedVec::new(vec![DreamAction::Dash {
                direction: [1.0, 0.0],
            }])
            .unwrap(),
            mouse_delta: [0.0; 2],
        })
        .unwrap();
    assert!(preflight_prediction_commands(&session, 0, ServerTick(411), TargetTick(444)).is_err());
    assert!(
        preflight_prediction_commands(&session, 242, ServerTick(411), TargetTick(426)).is_err()
    );
    assert_eq!(session.journal.pending_samples(), 1);
    assert_eq!(session.sequence, CommandSeq(0));
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(411))
    );
    preflight_prediction_commands(&session, 0, ServerTick(411), TargetTick(426)).unwrap();
    for tick in 412..=426 {
        let sample = session
            .command_sample(TargetTick(tick), TargetTick(426), now)
            .unwrap();
        assert_eq!(sample.edges.len(), usize::from(tick == 426));
        assert_eq!(
            sample.held.held.movement,
            if tick == 426 { [1.0, 0.0] } else { [0.0, 0.0] }
        );
        session.sequence = session.sequence.checked_next().unwrap();
        let actions = BoundedVec::new(
            sample
                .edges
                .into_vec()
                .into_iter()
                .enumerate()
                .map(|(slot, action)| ActionEdge {
                    slot: slot as u8,
                    action,
                })
                .collect(),
        )
        .unwrap();
        let command = Command {
            owner: session.model.welcome.stream,
            sequence: session.sequence,
            target: TargetTick(tick),
            input: sample.held,
            actions,
        };
        let dependencies = current_dependencies(&session);
        session
            .predictor
            .predict(
                OWNER_GROUP,
                InputCommand(command),
                dependencies,
                &session.model,
            )
            .unwrap();
    }
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(426))
    );
    assert_eq!(session.predictor.work().predicted_ticks, 15);
    assert_eq!(session.sequence, CommandSeq(15));
    assert_eq!(session.journal.pending_samples(), 0);
}

fn wrap_full_baseline(state: &mut FullState<Vec<u8>>, publication: GroupPublication) {
    use dreamwake_protocol::live::baselines as wire;
    use engine_net::replication::baselines::{BaselineEncoding, BaselinePacket};
    state.payload = wire::encode_member(&BaselinePacket {
        target: wire::target(state, publication, BaselineGeneration(1)),
        encoding: BaselineEncoding::Full,
        bytes: state.payload.clone(),
    })
    .unwrap();
}

#[test]
fn delta_owner_group_commits_complete_proofs_and_retires_only_after_fence() {
    use dreamwake_protocol::live::baselines as wire;
    use engine_net::replication::baselines::{
        BaselineCodec, BaselineEncoding, BaselinePacket, BytePatch,
    };
    let (mut session, first) = fixture();
    for frame in &first {
        session
            .receive_state(frame, Duration::from_millis(1))
            .unwrap();
    }
    let old_bytes = session.baselines.retained_bytes();
    let (mut next, second) = fixture_publication(11, 2, 1);
    for frame in second {
        next.receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    let publication = GroupPublication {
        connection: ConnectionEpoch(1),
        group: OWNER_GROUP,
        revision: GroupRevision(1),
        snapshot: SnapshotId(2),
    };
    let prior = GroupPublication {
        snapshot: SnapshotId(1),
        ..publication
    };
    let mut members = next
        .groups
        .published_group(OWNER_GROUP, &next.scopes)
        .unwrap()
        .manifest()
        .iter()
        .map(|scope| next.scopes.state(scope.entity).unwrap().clone())
        .collect::<Vec<_>>();
    for state in &mut members {
        let base = session.scopes.state(state.scope.entity).unwrap();
        state.payload = wire::encode_member(&BaselinePacket {
            target: wire::target(state, publication, BaselineGeneration(1)),
            encoding: BaselineEncoding::Delta {
                base: wire::target(base, prior, BaselineGeneration(1)),
            },
            bytes: BytePatch
                .encode_delta(&base.payload, &state.payload, wire::PAYLOAD_BYTES + 12)
                .unwrap(),
        })
        .unwrap();
    }
    let chunk = GroupChunk {
        publication,
        end_tick: ServerTick(11),
        manifest: members.iter().map(|m| m.scope).collect(),
        members,
        index: 0,
        count: 1,
    };
    let bytes = encode_group(&chunk, group_limits(), |p| Ok(p.clone())).unwrap();
    let limit = session.model.welcome.frame_limit().unwrap();
    let mut fragments = fragment_group(publication, &bytes, byte_limits(limit).unwrap()).unwrap();
    fragments.reverse();
    let mut proof = None;
    for (index, fragment) in fragments.into_iter().enumerate() {
        let frame = encode_state(
            &StateFrame::Fragment(fragment),
            ConnectionEpoch(1),
            index as u32,
            limit,
        )
        .unwrap();
        if let Some(value) = session
            .receive_state(&frame, Duration::from_millis(2))
            .unwrap()
        {
            proof = Some(value);
        }
    }
    let Some(ClientControl::OwnerDecoded { proofs }) = proof else {
        panic!("combined decoded proofs");
    };
    assert_eq!(proofs.len(), 4);
    assert!(
        proofs
            .as_slice()
            .iter()
            .all(|p| p.baseline.snapshot == SnapshotId(2) && p.state.snapshot == SnapshotId(2))
    );
    assert!(session.baselines.retained_bytes() > old_bytes);
    let before_fence = session.baselines.retained_bytes();
    let requests = session.baselines.retirements().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(session.baselines.retained_bytes(), before_fence);
    assert!(session.baselines.retirements().unwrap().is_empty());
    session
        .baselines
        .acknowledge_retirements(&requests)
        .unwrap();
    assert!(session.baselines.retained_bytes() < before_fence);
    for frame in first {
        let _ = session.receive_state(&frame, Duration::from_millis(3));
    }
    assert_eq!(
        session
            .scopes
            .state(session.model.welcome.owner_entity)
            .unwrap()
            .snapshot,
        SnapshotId(2)
    );
}

#[test]
fn full_baseline_before_reset_is_retried_and_reset_can_precede_first_group() {
    use dreamwake_protocol::live::baselines as wire;
    use engine_net::replication::baselines::{BaselineEncoding, BaselinePacket, BaselineReset};
    let (mut session, first) = fixture();
    for frame in first {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    let (mut source, frames) = fixture_publication(11, 2, 1);
    for frame in frames {
        source
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    let welcome = session.model.welcome.clone();
    let publication = GroupPublication {
        connection: ConnectionEpoch(1),
        group: OWNER_GROUP,
        revision: GroupRevision(1),
        snapshot: SnapshotId(2),
    };
    let mut members = source
        .groups
        .published_group(OWNER_GROUP, &source.scopes)
        .unwrap()
        .manifest()
        .iter()
        .map(|scope| source.scopes.state(scope.entity).unwrap().clone())
        .collect::<Vec<_>>();
    let resets = members
        .iter()
        .map(|state| BaselineReset {
            context: wire::context(state, publication),
            generation: BaselineGeneration(2),
        })
        .collect::<Vec<_>>();
    for state in &mut members {
        state.payload = wire::encode_member(&BaselinePacket {
            target: wire::target(state, publication, BaselineGeneration(2)),
            encoding: BaselineEncoding::Full,
            bytes: state.payload.clone(),
        })
        .unwrap();
    }
    let chunk = GroupChunk {
        publication,
        end_tick: ServerTick(11),
        manifest: members.iter().map(|m| m.scope).collect(),
        members,
        index: 0,
        count: 1,
    };
    let bytes = encode_group(&chunk, group_limits(), |p| Ok(p.clone())).unwrap();
    let limit = welcome.frame_limit().unwrap();
    let frames = fragment_group(publication, &bytes, byte_limits(limit).unwrap())
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(index, fragment)| {
            encode_state(
                &StateFrame::Fragment(fragment),
                ConnectionEpoch(1),
                index as u32,
                limit,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    for frame in &frames {
        assert!(
            session
                .receive_state(frame, Duration::from_millis(2))
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        session.scopes.state(welcome.owner_entity).unwrap().snapshot,
        SnapshotId(1)
    );
    session.baselines.apply_resets(&resets).unwrap();
    let mut decoded = false;
    for frame in &frames {
        decoded |= matches!(
            session
                .receive_state(frame, Duration::from_millis(3))
                .unwrap(),
            Some(ClientControl::OwnerDecoded { .. })
        );
    }
    assert!(decoded);
    assert_eq!(
        session.scopes.state(welcome.owner_entity).unwrap().snapshot,
        SnapshotId(2)
    );
    let mut empty = Session::new(welcome).unwrap();
    empty.baselines.apply_resets(&resets).unwrap();
    for frame in frames {
        empty
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    assert_eq!(empty.scopes.known_entities(), 4);
}

#[test]
fn starfall_pause_over_4096_server_ticks_keeps_published_object_and_resumes_flight() {
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run();
    let key = dreamwake_sim::combat::RayActionKey {
        match_epoch: 1,
        connection_epoch: 1,
        command_stream: 1,
        ownership_epoch: 1,
        actor: 1,
        actor_generation: 1,
        command_sequence: 1,
        action_slot: 0,
    };
    sim.step_multiplayer_with_actions(
        &[],
        &[dreamwake_sim::combat::CombatAction::Cast {
            key,
            execution_server_tick: 10,
            slot: 1,
            aim: [1.0, 0.0],
        }],
    )
    .unwrap();
    sim.set_paused(true);
    let (mut session, frames) = fixture_from_sim(&sim, 10, 1, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    session.acknowledge_active(SnapshotId(1), ServerTick(10));
    session.reconcile().unwrap();
    let before = session
        .predictor
        .state(OWNER_GROUP)
        .unwrap()
        .0
        .starfall_flights()[0]
        .clone();
    assert!(session.events.represented(before.key));
    for tick in 11..=4110 {
        session.begin_frame().unwrap();
        let command = Command {
            owner: session.model.welcome.stream,
            sequence: CommandSeq(tick - 9),
            target: TargetTick(tick),
            input: TickInput::default(),
            actions: BoundedVec::default(),
        };
        let dependencies = current_dependencies(&session);
        session
            .predictor
            .predict(
                OWNER_GROUP,
                InputCommand(command),
                dependencies,
                &session.model,
            )
            .unwrap();
        session.events.publish(&session.predictor).unwrap();
    }
    assert_eq!(
        session
            .predictor
            .state(OWNER_GROUP)
            .unwrap()
            .0
            .starfall_flights(),
        std::slice::from_ref(&before)
    );
    assert!(session.events.represented(before.key));
    assert_eq!(
        session.events.sounds, 0,
        "checkpoint bootstrap must not invent a cast sound"
    );
    sim.set_paused(false);
    let (_, frames) = fixture_from_sim(&sim, 4111, 2, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_secs(69))
            .unwrap();
    }
    session.finalized = ServerTick(4111);
    session.reconcile().unwrap();
    session.begin_frame().unwrap();
    let command = Command {
        owner: session.model.welcome.stream,
        sequence: CommandSeq(4103),
        target: TargetTick(4112),
        input: TickInput::default(),
        actions: BoundedVec::default(),
    };
    let dependencies = current_dependencies(&session);
    session
        .predictor
        .predict(
            OWNER_GROUP,
            InputCommand(command),
            dependencies,
            &session.model,
        )
        .unwrap();
    session.events.publish(&session.predictor).unwrap();
    assert!(session.events.represented(before.key));
    assert_ne!(
        session
            .predictor
            .state(OWNER_GROUP)
            .unwrap()
            .0
            .starfall_flights()[0]
            .position,
        before.position
    );
}

#[test]
fn coalesced_charge_press_release_use_distinct_commands_and_one_stable_episode() {
    let (mut session, _) = fixture();
    session.sequence = CommandSeq(41);
    assert_eq!(
        session
            .next_action(&[
                DreamAction::ChargeBegin,
                DreamAction::ChargeRelease { episode: 0 }
            ])
            .unwrap(),
        Some(DreamAction::ChargeBegin)
    );
    session.sequence = CommandSeq(42);
    assert_eq!(
        session.next_action(&[]).unwrap(),
        Some(DreamAction::ChargeRelease { episode: 41 })
    );
    assert!(session.next_action(&[]).unwrap().is_none());
    assert!(
        session
            .next_action(&[DreamAction::ChargeCancel { episode: 0 }])
            .unwrap()
            .is_none()
    );
    session.sequence = CommandSeq(43);
    assert_eq!(
        session
            .next_action(&[
                DreamAction::ChargeBegin,
                DreamAction::ChargeCancel { episode: 0 }
            ])
            .unwrap(),
        Some(DreamAction::ChargeBegin)
    );
    session.sequence = CommandSeq(44);
    assert_eq!(
        session.next_action(&[]).unwrap(),
        Some(DreamAction::ChargeCancel { episode: 43 })
    );
    // Recovery creates a fresh session and cannot release a previous stream.
    let mut replacement = Session::new(session.model.welcome.clone()).unwrap();
    assert!(
        replacement
            .next_action(&[DreamAction::ChargeRelease { episode: 0 }])
            .unwrap()
            .is_none()
    );
}

#[test]
fn new_prediction_frames_refresh_platform_samples_across_long_running_checkpoints() {
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    let (mut session, frames) = fixture_from_sim(&sim, 10, 1, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.reconcile().unwrap();
    session.active = true;
    assert_eq!(
        session
            .predictor
            .identity(OWNER_GROUP)
            .unwrap()
            .scopes
            .len(),
        4
    );
    // Always keep prediction ahead of authority, so checkpoint admission must
    // refresh new commands without rewriting retained historical replay inputs.
    for tick in 11..=350 {
        session.begin_frame().unwrap();
        let dependencies = session
            .predictor
            .checkpoint_dependencies(OWNER_GROUP)
            .unwrap()
            .clone();
        session
            .predictor
            .predict(
                OWNER_GROUP,
                InputCommand(Command {
                    owner: session.model.welcome.stream,
                    sequence: CommandSeq(tick - 10),
                    target: TargetTick(tick),
                    input: DreamInput::default().into(),
                    actions: BoundedVec::default(),
                }),
                dependencies,
                &session.model,
            )
            .unwrap_or_else(|error| panic!("predict tick {tick}: {error:?}"));
        if tick > 13 {
            sim.step(DreamInput::default());
            let authoritative_tick = tick - 3;
            let (_, frames) = fixture_from_sim(&sim, authoritative_tick, tick, 1);
            let retained = session
                .predictor
                .dependency_history(OWNER_GROUP, ServerTick(tick))
                .unwrap()
                .clone();
            session.finalized = ServerTick(authoritative_tick);
            for frame in frames {
                session
                    .receive_state(&frame, Duration::from_millis(tick * 17))
                    .unwrap();
            }
            session.reconcile().unwrap();
            let retirements = session.baselines.retirements().unwrap();
            session
                .baselines
                .acknowledge_retirements(&retirements)
                .unwrap();
            assert_eq!(
                session
                    .predictor
                    .dependency_history(OWNER_GROUP, ServerTick(tick))
                    .unwrap(),
                &retained
            );
            assert_eq!(
                session.predictor.predicted_tick(OWNER_GROUP),
                Some(ServerTick(tick))
            );
        }
    }
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(350))
    );
}

#[test]
fn stale_bootstrap_waits_for_fresh_checkpoint_without_assigning_or_replaying_commands() {
    let (mut session, frames) = fixture_publication(10, 1, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.reconcile().unwrap();
    session.active = true;
    session
        .journal
        .capture(HardwareSample {
            at: Duration::from_secs(1),
            held: Some(
                DreamInput {
                    movement: [1.0, 0.0],
                    ..Default::default()
                }
                .into(),
            ),
            edges: BoundedVec::new(vec![DreamAction::Dash {
                direction: [1.0, 0.0],
            }])
            .unwrap(),
            mouse_delta: [0.0; 2],
        })
        .unwrap();
    assert_eq!(
        session.assign_command_target(ServerTick(100)).unwrap(),
        None
    );
    assert_eq!(
        session.assign_command_target(ServerTick(120)).unwrap(),
        None
    );
    assert_eq!(session.sequence, CommandSeq(0));
    assert_eq!(session.journal.pending_samples(), 1);
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(10))
    );

    // A large authoritative advance is safe only because nothing was predicted.
    let (_, frames) = fixture_publication(105, 2, 1);
    session.begin_frame().unwrap();
    session.finalized = ServerTick(105);
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_secs(2))
            .unwrap();
    }
    session.reconcile().unwrap();
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(105))
    );
    assert_eq!(session.predictor.work().replay_ticks, 0);
    assert_eq!(session.predictor.work().predicted_ticks, 0);
    assert_eq!(
        session.assign_command_target(ServerTick(120)).unwrap(),
        Some(TargetTick(123))
    );
    assert_eq!(session.sequence, CommandSeq(0));
    assert_eq!(session.journal.pending_samples(), 1);
    preflight_prediction_commands(&session, 0, ServerTick(105), TargetTick(123)).unwrap();
    assert!(preflight_prediction_commands(&session, 0, ServerTick(105), TargetTick(138)).is_err());

    let command = InputCommand(Command {
        owner: session.model.welcome.stream,
        sequence: CommandSeq(1),
        target: TargetTick(106),
        input: DreamInput::default().into(),
        actions: BoundedVec::default(),
    });
    let dependencies = session
        .predictor
        .checkpoint_dependencies(OWNER_GROUP)
        .unwrap()
        .clone();
    session
        .predictor
        .predict(OWNER_GROUP, command, dependencies, &session.model)
        .unwrap();
    session.sequence = CommandSeq(1);
    // The bootstrap exception cannot discard an established speculative history.
    let (_, frames) = fixture_publication(150, 3, 1);
    session.begin_frame().unwrap();
    session.finalized = ServerTick(150);
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_secs(3))
            .unwrap();
    }
    assert!(session.reconcile().is_err());
}

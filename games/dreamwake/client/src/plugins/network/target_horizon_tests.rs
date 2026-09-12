//! Delayed delivery preserves adaptive lead; stalled authority bounds speculation.
use super::*;
use engine_net::{
    commands::{CommandLimits, FinalizedReceipt, FinalizedStatus},
    prediction::PredictionLimits,
    synchronization::{LeadController, ResyncReason},
};

fn active_runtime() -> Runtime {
    let (mut session, frames) = tests::fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.reconcile().unwrap();
    session.active = true;
    session
        .observe_clock(TimeExchange {
            client_send: Duration::ZERO,
            server_receive: Duration::ZERO,
            server_send: Duration::ZERO,
            client_receive: Duration::ZERO,
        })
        .unwrap();
    session.lead = LeadController::new(3).unwrap();
    let mut runtime = tests::receipt_runtime(session);
    runtime.renet.set_connected();
    generate_commands(&mut runtime, Duration::from_millis(200)).unwrap();
    runtime
}

fn clock_reply(session: &mut Session, tick: ServerTick, receive_millis: u64) {
    session
        .observe_clock_reply(
            TimeExchange {
                client_send: Duration::from_millis(receive_millis - 100),
                server_receive: Duration::from_millis(receive_millis - 50),
                server_send: Duration::from_millis(receive_millis - 50),
                client_receive: Duration::from_millis(receive_millis),
            },
            tick,
        )
        .unwrap();
}

fn finalized_bytes(
    session: &Session,
    first: u64,
    through: u64,
    arrival_slack: Option<u8>,
) -> Vec<u8> {
    encode_control(
        &ServerControl::Finalized {
            through: ServerTick(through),
            receipts: BoundedVec::new(
                (first..=through)
                    .map(|tick| FinalizedReceipt {
                        tick: ServerTick(tick),
                        sequence: None,
                        status: FinalizedStatus::Substituted,
                    })
                    .collect(),
            )
            .unwrap(),
            menu_applied: None,
            arrival: ArrivalFeedback {
                sample: arrival_slack.map(|slack| ArrivalSample {
                    sequence: session.sequence,
                    target: TargetTick(through + u64::from(slack)),
                    arrival_tick: ServerTick(through),
                    slack,
                }),
                observed_commands: session.metrics().arrival_feedback.observed_commands
                    + u64::from(arrival_slack.is_some()),
                late_commands: session.metrics().arrival_feedback.late_commands
                    + u64::from(arrival_slack == Some(0)),
            },
        },
        session.model.welcome.stream.epoch,
        0,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap()
}

#[test]
fn stalled_authority_horizon_preserves_input_and_sequence_then_resumes() {
    let mut runtime = active_runtime();
    let speculative_ticks = PredictionLimits::default().replay_ticks_per_frame as u64;
    // Ordinary frames approach the bound; an actual >12 target discontinuity
    // remains a separate recovery condition.
    for millis in (300..=1000).step_by(100) {
        runtime.prediction.as_mut().unwrap().begin_frame().unwrap();
        generate_commands(&mut runtime, Duration::from_millis(millis)).unwrap();
    }
    let session = runtime.prediction.as_mut().unwrap();
    let initial_finalized = session.finalized;
    let committed = session.simulation_clock.committed();
    let frontier = ServerTick(committed.0 + speculative_ticks);
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(frontier)
    );
    let issued_before_hold = session.sequence;
    assert!(
        issued_before_hold.0 < speculative_ticks,
        "forecast steps do not allocate commands"
    );
    assert_eq!(runtime.pending.len(), issued_before_hold.0 as usize);
    assert!(
        runtime
            .pending
            .iter()
            .all(|frame| frame.command.target.0 <= frontier.0)
    );
    assert!(session.estimated_server_tick.unwrap() > frontier);

    let action = DreamAction::Dash {
        direction: [1.0, 0.0],
    };
    session
        .journal
        .capture(HardwareSample {
            at: Duration::from_millis(1100),
            held: Some(
                DreamInput {
                    movement: [1.0, 0.0],
                    ..Default::default()
                }
                .into(),
            ),
            edges: BoundedVec::new(vec![action]).unwrap(),
            mouse_delta: [0.0; 2],
        })
        .unwrap();
    let deadline = session.input_deadline;
    let pending = runtime.pending.len();
    for millis in [1250, 1500, 1750] {
        let session = runtime.prediction.as_mut().unwrap();
        clock_reply(session, committed, millis);
        session.begin_frame().unwrap();
        generate_commands(&mut runtime, Duration::from_millis(millis)).unwrap();
        let session = runtime.prediction.as_ref().unwrap();
        assert_eq!(
            session.predictor.predicted_tick(OWNER_GROUP),
            Some(frontier)
        );
        assert_eq!(session.sequence, issued_before_hold);
        assert_eq!(session.simulation_clock.committed(), committed);
        assert_eq!(session.finalized, initial_finalized);
        assert_eq!(session.input_deadline, deadline);
        assert_eq!(session.journal.pending_samples(), 1);
        assert_eq!(session.predictor.work().predicted_ticks, 0);
        assert_eq!(runtime.pending.len(), pending);
    }

    // A complete owner publication proves progress independently of Finalized.
    // Reconciliation then retires the old checkpoint before speculation resumes.
    let (_, frames) = tests::fixture_publication(committed.0 + 4, 2, 1);
    let session = runtime.prediction.as_mut().unwrap();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1800))
            .unwrap();
    }
    assert_eq!(
        session.simulation_clock.committed(),
        ServerTick(committed.0 + 4)
    );
    assert_eq!(session.finalized, initial_finalized);
    assert_eq!(session.journal.pending_samples(), 1);
    let finalization = finalized_bytes(session, initial_finalized.0 + 1, committed.0 + 4, None);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    receive_message(
        &mut runtime,
        CONTROL_CHANNEL,
        &finalization,
        Duration::from_millis(1800),
        &mut world,
    )
    .unwrap();
    runtime.prediction.as_mut().unwrap().reconcile().unwrap();
    let retained_pending = runtime.pending.len();
    assert_eq!(
        retained_pending, pending,
        "the finalized prefix contained only forecast steps"
    );
    generate_commands(&mut runtime, Duration::from_millis(2300)).unwrap();
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.sequence, issued_before_hold.checked_next().unwrap());
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(frontier.0 + 4))
    );
    assert_eq!(session.journal.pending_samples(), 0);
    assert_eq!(session.predictor.work().predicted_ticks, 4);
    assert!(session.predictor.work().replay_ticks <= speculative_ticks as usize);
    let resumed = runtime
        .pending
        .iter()
        .skip(retained_pending)
        .collect::<Vec<_>>();
    assert_eq!(resumed.len(), 1);
    for (index, frame) in resumed.iter().enumerate() {
        assert_eq!(frame.command.target.0, frontier.0 + 4);
        assert_eq!(
            frame.command.sequence,
            CommandSeq(issued_before_hold.0 + index as u64 + 1)
        );
        assert!(
            frame.command.target.0 <= session.simulation_clock.committed().0 + speculative_ticks
        );
        assert_eq!(frame.command.input.held.movement, [1.0, 0.0]);
    }
    assert_eq!(
        resumed
            .iter()
            .flat_map(|frame| frame.command.actions.as_slice())
            .filter(|edge| edge.action == action)
            .count(),
        1
    );
}

#[test]
fn committed_clock_does_not_weaken_lead_or_discontinuity_recovery() {
    let mut runtime = active_runtime();
    let session = runtime.prediction.as_mut().unwrap();
    for slack in [0, -1] {
        session.lead = LeadController::new(CommandLimits::default().future_ticks).unwrap();
        assert!(matches!(
            session.lead.observe_arrival_slack(slack),
            Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded))
        ));
        assert!(
            session
                .assign_command_target(ServerTick(1000))
                .unwrap_err()
                .contains("LeadExceeded")
        );
    }
    let sequence = session.sequence;
    assert!(sequence.0 > 0);
    assert_eq!(session.journal.pending_samples(), 0);

    session.lead = LeadController::new(3).unwrap();
    assert_eq!(
        session.assign_command_target(ServerTick(20)).unwrap(),
        Some(TargetTick(23))
    );
    assert!(
        session
            .assign_command_target(ServerTick(33))
            .unwrap_err()
            .contains("TargetDiscontinuity")
    );
    assert_eq!(session.sequence, sequence);
}

#[test]
fn timestamped_progress_preserves_adaptive_lead_with_delayed_finalization() {
    let mut runtime = active_runtime();
    for (receive_millis, committed, lead, expected) in [(350, 18, 4, 25), (400, 21, 5, 29)] {
        let session = runtime.prediction.as_mut().unwrap();
        clock_reply(session, ServerTick(committed), receive_millis);
        session.lead.observe_arrival_slack(0).unwrap();
        session.begin_frame().unwrap();
        generate_commands(&mut runtime, Duration::from_millis(receive_millis)).unwrap();
        let session = runtime.prediction.as_ref().unwrap();
        assert_eq!(session.finalized, ServerTick(10));
        assert_eq!(session.lead.lead(), lead);
        assert_eq!(
            session.predictor.predicted_tick(OWNER_GROUP),
            Some(ServerTick(expected))
        );
        assert_eq!(
            session.estimated_simulation_tick.unwrap().0 + u64::from(lead),
            expected
        );
        assert!(expected > session.finalized.0 + u64::from(CommandLimits::default().future_ticks));
        assert_eq!(session.lead.needs_resynchronization(), None);
        assert!(session.active && !session.recovering);
    }
}

#[test]
fn duplicate_and_older_finalized_feedback_cannot_exhaust_lead() {
    let (mut session, _) = tests::fixture();
    session.lead = LeadController::new(11).unwrap();
    session.sequence = CommandSeq(1);
    session.record_command_issuance(session.sequence, TargetTick(11), TargetTick(11));
    let fresh = finalized_bytes(&session, 11, 11, Some(0));
    let older = finalized_bytes(&session, 10, 10, Some(0));
    let mut runtime = tests::receipt_runtime(session);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    receive_message(
        &mut runtime,
        CONTROL_CHANNEL,
        &fresh,
        Duration::from_millis(1),
        &mut world,
    )
    .unwrap();
    assert_eq!(runtime.prediction.as_ref().unwrap().lead.lead(), 12);
    for (index, bytes) in [&fresh, &older, &fresh, &older].into_iter().enumerate() {
        receive_message(
            &mut runtime,
            CONTROL_CHANNEL,
            bytes,
            Duration::from_millis(index as u64 + 2),
            &mut world,
        )
        .unwrap();
        let session = runtime.prediction.as_ref().unwrap();
        assert_eq!(session.lead.lead(), 12);
        assert_eq!(session.lead.needs_resynchronization(), None);
        assert_eq!(session.finalized, ServerTick(11));
        assert_eq!(session.simulation_clock.committed(), ServerTick(11));
        assert_eq!(session.finalized_ack_pending, Some(ServerTick(11)));
        assert!(!session.recovering);
    }
    // A new command authored under the maximum policy still carries genuine
    // evidence of a missed deadline; fencing old feedback must not hide it.
    let session = runtime.prediction.as_mut().unwrap();
    session.sequence = CommandSeq(2);
    session.record_command_issuance(session.sequence, TargetTick(12), TargetTick(12));
    let current_policy_late = finalized_bytes(session, 12, 12, Some(0));
    let error = receive_message(
        &mut runtime,
        CONTROL_CHANNEL,
        &current_policy_late,
        Duration::from_millis(10),
        &mut world,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ReceiveRejection::Clock(SyncError::ResyncRequired(ResyncReason::LeadExceeded))
    ));
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.metrics().arrival_feedback.late_commands, 2);
    assert_eq!(session.metrics().arrival_feedback_applied, 2);
    assert_eq!(
        session.lead.needs_resynchronization(),
        Some(ResyncReason::LeadExceeded)
    );
}

#[test]
fn rejected_clock_reply_does_not_partially_commit_simulation_anchor() {
    let (mut session, _) = tests::fixture();
    let anchor = session.simulation_clock.anchor();
    let exchange = TimeExchange {
        client_send: Duration::ZERO,
        server_receive: Duration::from_millis(400),
        server_send: Duration::from_millis(400),
        client_receive: Duration::from_millis(800),
    };
    assert_eq!(
        session.observe_clock_reply(exchange, ServerTick(24)),
        Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded))
    );
    assert_eq!(session.simulation_clock.anchor(), anchor);
    assert_eq!(session.simulation_clock.committed(), ServerTick(10));
    assert_eq!(session.clock.sample_count(), 0);
    clock_reply(&mut session, ServerTick(33), 600);
    let anchor = session.simulation_clock.anchor();
    assert_eq!(anchor, (ServerTick(33), Duration::from_millis(550)));
    assert_eq!(session.clock.sample_count(), 1);
    let exchange = TimeExchange {
        client_send: Duration::from_millis(600),
        server_receive: Duration::from_millis(650),
        server_send: Duration::from_millis(650),
        client_receive: Duration::from_millis(700),
    };
    assert_eq!(
        session.observe_clock_reply(exchange, ServerTick(32)),
        Err(SyncError::NonMonotonicTime)
    );
    assert_eq!(session.simulation_clock.anchor(), anchor);
    assert_eq!(session.simulation_clock.committed(), ServerTick(33));
    assert_eq!(session.clock.sample_count(), 1);
}

#[test]
fn combat_capture_and_presentation_share_fractional_simulation_time_under_wall_debt() {
    let (mut session, frames) = tests::fixture_publication(60, 1, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.acknowledge_active(SnapshotId(1), ServerTick(60));
    session.reconcile().unwrap();
    assert!(session.active);
    let checkpoint = session
        .scopes
        .state(session.model.welcome.owner_entity)
        .unwrap();
    assert_eq!(checkpoint.end_tick, ServerTick(60));
    let reference = checkpoint.receipt();
    session
        .observe_clock_reply(
            TimeExchange {
                client_send: Duration::ZERO,
                server_receive: Duration::from_secs(10),
                server_send: Duration::from_secs(10),
                client_receive: Duration::from_millis(100),
            },
            ServerTick(66),
        )
        .unwrap();

    let at = Duration::from_millis(125);
    session.advance_presentation_clock(at);
    let view = session.capture_combat_view(at).unwrap();
    assert_eq!(view.sampled_at, ServerTick(70));
    assert_eq!(view.sampled_fraction, 32768);
    assert_eq!(view.viewed_at, ServerTick(64));
    assert_eq!(view.viewed_fraction, 32768);
    assert_eq!(view.reference, reference);
    assert_eq!(session.poses.metrics().cursor, Duration::from_millis(1075));
    assert_eq!(session.estimated_simulation_tick, Some(ServerTick(70)));
    assert_eq!(
        session.clock.estimate_server_time(at).unwrap(),
        Duration::from_millis(10_075)
    );
    assert!(session.estimated_server_tick.unwrap().0 > view.sampled_at.0 + 500);
}

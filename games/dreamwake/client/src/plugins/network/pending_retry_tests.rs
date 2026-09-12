//! Retained immutable commands become deliverable when authenticated time advances.
use super::*;
use engine_net::commands::{
    Admission, CommandInbox, CommandLimits, CommandRules, FinalizedStatus, OwnerStream,
};
use renet::{RenetServer, ServerEvent};

type InputRecord = Command<TickInput, DreamAction>;

struct Rules(OwnerStream);

impl CommandRules<TickInput, DreamAction> for Rules {
    fn owns_at(&self, owner: OwnerStream, _: TargetTick) -> bool {
        owner == self.0
    }

    fn valid_input(&self, input: &TickInput) -> bool {
        valid_tick_input(input)
    }

    fn valid_action(&self, action: &DreamAction) -> bool {
        valid_tick_action(action)
    }

    fn held_input(&self, previous: &TickInput) -> TickInput {
        DreamInput {
            movement: previous.held.movement,
            aim: previous.held.aim,
            attack: previous.held.attack,
            ..Default::default()
        }
        .into()
    }

    fn neutral_input(&self) -> TickInput {
        TickInput::default()
    }
}

fn connected_runtime() -> (Runtime, RenetServer) {
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
    session.lead = engine_net::synchronization::LeadController::new(3).unwrap();
    assert_eq!(session.last_input_retry_committed, None);
    let mut runtime = tests::receipt_runtime(session);
    runtime.renet.set_connected();
    let mut server = RenetServer::new(connection_config());
    server.add_connection(runtime.client_id);
    assert!(matches!(
        server.get_event(),
        Some(ServerEvent::ClientConnected { client_id }) if client_id == runtime.client_id
    ));
    (runtime, server)
}

fn drain_inputs(runtime: &mut Runtime, server: &mut RenetServer) -> Vec<Vec<InputRecord>> {
    // Exercise Renet's packet encoding/decoding without invoking the UDP transport.
    for packet in runtime.renet.get_packets_to_send() {
        server
            .process_packet_from(&packet, runtime.client_id)
            .unwrap();
    }
    let welcome = &runtime.prediction.as_ref().unwrap().model.welcome;
    let mut bundles = Vec::new();
    while let Some(bytes) = server.receive_message(runtime.client_id, INPUT_CHANNEL) {
        assert!(bytes.len() <= usize::from(welcome.application_frame_bytes));
        let bundle =
            decode_commands(&bytes, welcome.stream.epoch, welcome.frame_limit().unwrap()).unwrap();
        assert!(!bundle.records.is_empty());
        assert!(bundle.records.len() <= 8);
        bundles.push(bundle.records.into_vec());
    }
    for packet in server.get_packets_to_send(runtime.client_id).unwrap() {
        runtime.renet.process_packet(&packet);
    }
    bundles
}

fn generate(runtime: &mut Runtime, server: &mut RenetServer, millis: u64) -> Vec<Vec<InputRecord>> {
    runtime.prediction.as_mut().unwrap().begin_frame().unwrap();
    generate_commands(runtime, Duration::from_millis(millis)).unwrap();
    drain_inputs(runtime, server)
}

fn stalled_frontier() -> (Runtime, RenetServer, InputRecord) {
    let (mut runtime, mut server) = connected_runtime();
    let mut sent = generate(&mut runtime, &mut server, 200);
    // Bootstrap forecasts ticks 11..14 and authors only the fresh tick-15 head.
    // Steady captures then keep the frontier ahead of the estimate, retaining
    // one real command per tick rather than introducing later forecast gaps.
    for millis in (216..=328).step_by(16) {
        sent.extend(generate(&mut runtime, &mut server, millis));
    }
    runtime
        .prediction
        .as_mut()
        .unwrap()
        .journal
        .capture(HardwareSample {
            at: Duration::from_millis(333),
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
    for millis in (344..=1000).step_by(16) {
        sent.extend(generate(&mut runtime, &mut server, millis));
    }
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.simulation_clock.committed(), ServerTick(10));
    assert_eq!(session.last_input_retry_committed, Some(ServerTick(10)));
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(42))
    );
    assert_eq!(session.sequence, CommandSeq(28));
    assert_eq!(
        runtime
            .pending
            .iter()
            .map(|frame| frame.command.target.0)
            .collect::<Vec<_>>(),
        (15..=42).collect::<Vec<_>>()
    );
    for tick in 11..15 {
        assert!(matches!(
            session
                .predictor
                .replay_input_history(OWNER_GROUP, ServerTick(tick)),
            Some(engine_net::prediction::ReplayInput::Substitute)
        ));
    }
    assert!(sent.iter().flatten().all(|record| record.target.0 >= 15));
    let action = runtime
        .pending
        .iter()
        .find(|frame| !frame.command.actions.is_empty())
        .unwrap()
        .command
        .clone();
    assert_eq!(action.target, TargetTick(23));
    assert_eq!(action.sequence, CommandSeq(9));
    assert!(sent.iter().flatten().any(|record| record == &action));
    assert!(
        runtime
            .pending
            .iter()
            .rev()
            .take(8)
            .all(|frame| frame.command != action)
    );
    (runtime, server, action)
}

fn clock_progress(runtime: &mut Runtime, tick: u64, receive_millis: u64) {
    let session = runtime.prediction.as_ref().unwrap();
    let bytes = encode_control(
        &ServerControl::ClockReply {
            client_send_nanos: (receive_millis - 100) * 1_000_000,
            server_receive_nanos: (receive_millis - 50) * 1_000_000,
            server_send_nanos: (receive_millis - 50) * 1_000_000,
            tick: ServerTick(tick),
        },
        session.model.welcome.stream.epoch,
        0,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap();
    receive_message(
        runtime,
        CONTROL_CHANNEL,
        &bytes,
        Duration::from_millis(receive_millis),
        &mut World::new(),
    )
    .unwrap();
}

fn pending_identity(runtime: &Runtime) -> Vec<(InputRecord, f64)> {
    runtime
        .pending
        .iter()
        .map(|frame| (frame.command.clone(), frame.sent_at))
        .collect()
}

#[test]
fn held_generation_retries_future_action_immutably_and_server_consumes_it_once() {
    let (mut runtime, mut server, action) = stalled_frontier();
    let owner = action.owner;
    let rules = Rules(owner);
    let limits = CommandLimits::default();
    assert_eq!(limits.future_ticks, 12);
    let mut inbox = CommandInbox::new(owner, ServerTick(10), limits).unwrap();
    assert_eq!(
        inbox
            .admit(owner.connection, action.clone(), &rules)
            .unwrap(),
        Admission::Future
    );
    let prepared = inbox.prepare_next(&rules).unwrap();
    assert_eq!(prepared.tick(), ServerTick(11));
    inbox.commit(prepared.tick()).unwrap();

    // New timestamped progress makes tick 23 admissible while its low clock
    // estimate leaves the already assigned tick-42 frontier unchanged.
    clock_progress(&mut runtime, 11, 1100);
    let session = runtime.prediction.as_mut().unwrap();
    session
        .journal
        .capture(HardwareSample {
            at: Duration::from_millis(1050),
            held: None,
            edges: BoundedVec::new(vec![DreamAction::Dash {
                direction: [0.0, 1.0],
            }])
            .unwrap(),
            mouse_delta: [0.0; 2],
        })
        .unwrap();
    let deadline = session.input_deadline;
    let before = pending_identity(&runtime);
    let trace = runtime.outcomes.latest_trace(1).unwrap();
    let packets = generate(&mut runtime, &mut server, 1100);
    let records = packets.into_iter().flatten().collect::<Vec<_>>();
    assert_eq!(records.len(), 9);
    let targets = records
        .iter()
        .map(|record| record.target.0)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(targets, (15..=23).collect());
    assert_eq!(
        records.iter().filter(|record| **record == action).count(),
        1
    );
    assert_eq!(pending_identity(&runtime), before);
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.sequence, CommandSeq(28));
    assert_eq!(session.input_deadline, deadline);
    assert_eq!(session.journal.pending_samples(), 1);
    assert_eq!(session.predictor.work().predicted_ticks, 0);
    assert_eq!(session.last_input_retry_committed, Some(ServerTick(11)));
    let after_trace = runtime.outcomes.latest_trace(1).unwrap();
    assert_eq!(after_trace.key, trace.key);
    assert_eq!(after_trace.created_at, trace.created_at);
    assert_eq!(after_trace.first_enqueued_at, trace.first_enqueued_at);
    assert_eq!(runtime.outcomes.trace(1, 8).len(), 1);

    for record in records {
        assert!(matches!(
            inbox.admit(owner.connection, record, &rules).unwrap(),
            Admission::Accepted {
                arrival_slack: 4..=12
            }
        ));
    }
    assert_eq!(
        inbox
            .admit(owner.connection, action.clone(), &rules)
            .unwrap(),
        Admission::Duplicate
    );
    let mut action_executions = 0;
    while inbox.finalized_through() < ServerTick(24) {
        let prepared = inbox.prepare_next(&rules).unwrap();
        if prepared.tick() == ServerTick(action.target.0) {
            assert_eq!(prepared.sequence(), Some(action.sequence));
            assert_eq!(prepared.input(), &action.input);
            assert_eq!(prepared.actions(), action.actions.as_slice());
        }
        action_executions += prepared.actions().len();
        let receipt = inbox.commit(prepared.tick()).unwrap();
        if receipt.tick == ServerTick(action.target.0) {
            assert_eq!(receipt.status, FinalizedStatus::Executed);
            assert_eq!(receipt.sequence, Some(action.sequence));
        }
    }
    assert_eq!(action_executions, 1);
    assert_eq!(
        inbox.admit(owner.connection, action, &rules).unwrap(),
        Admission::Duplicate
    );

    for millis in [1200, 1300] {
        clock_progress(&mut runtime, 11, millis);
        assert!(generate(&mut runtime, &mut server, millis).is_empty());
        assert_eq!(pending_identity(&runtime), before);
        let session = runtime.prediction.as_ref().unwrap();
        assert_eq!(session.sequence, CommandSeq(28));
        assert_eq!(session.journal.pending_samples(), 1);
        assert_eq!(session.input_deadline, deadline);
    }
}

#[test]
fn moving_speculation_ceiling_retries_old_eligible_edge_during_new_generation() {
    let (mut runtime, mut server, action) = stalled_frontier();
    let original = pending_identity(&runtime);
    let (_, frames) = tests::fixture_publication(11, 2, 1);
    let session = runtime.prediction.as_mut().unwrap();
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1100))
            .unwrap();
    }
    session.reconcile().unwrap();
    assert_eq!(session.simulation_clock.committed(), ServerTick(11));
    assert_eq!(session.finalized, ServerTick(10));
    generate_commands(&mut runtime, Duration::from_millis(1100)).unwrap();
    let packets = drain_inputs(&mut runtime, &mut server);
    let records = packets.iter().flatten().collect::<Vec<_>>();
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(43))
    );
    assert_eq!(session.sequence, CommandSeq(29));
    assert_eq!(session.predictor.work().predicted_ticks, 1);
    assert_eq!(session.last_input_retry_committed, Some(ServerTick(11)));
    assert_eq!(
        &pending_identity(&runtime)[..original.len()],
        original.as_slice()
    );
    assert_eq!(
        records.iter().filter(|record| ***record == action).count(),
        1
    );
    // The ordinary newest-eight packet contains ticks 36..43. The separate
    // bounded retry also covers all nine retained commands now admissible;
    // bootstrap substitutes have no commands to retransmit.
    let new_packet = packets
        .iter()
        .find(|packet| packet.iter().any(|record| record.target == TargetTick(43)))
        .unwrap();
    assert_eq!(
        new_packet
            .iter()
            .map(|record| record.target.0)
            .collect::<std::collections::BTreeSet<_>>(),
        (36..=43).collect()
    );
    assert!(new_packet.iter().all(|record| record != &action));
    let eligible = records
        .iter()
        .filter(|record| record.target.0 <= 23)
        .collect::<Vec<_>>();
    assert_eq!(eligible.len(), 9);
    assert_eq!(
        eligible
            .iter()
            .map(|record| record.target.0)
            .collect::<std::collections::BTreeSet<_>>(),
        (15..=23).collect()
    );
    let sequences = records
        .iter()
        .map(|record| record.sequence)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(sequences.len(), records.len());
    assert!(generate(&mut runtime, &mut server, 1150).is_empty());
}

#[test]
fn new_packet_coverage_suppresses_extra_retry_and_recovery_clears_progress() {
    let (mut runtime, mut server) = connected_runtime();
    assert!(!generate(&mut runtime, &mut server, 200).is_empty());
    assert_eq!(
        runtime
            .prediction
            .as_ref()
            .unwrap()
            .last_input_retry_committed,
        Some(ServerTick(10))
    );
    // Both newly generated packets already contain every eligible retained
    // command. Advancing the proof must not enqueue an extra retry copy.
    clock_progress(&mut runtime, 11, 250);
    let packets = generate(&mut runtime, &mut server, 250);
    assert_eq!(packets.len(), 2);
    assert_eq!(
        packets
            .iter()
            .map(|packet| packet.iter().map(|record| record.target.0).max().unwrap())
            .collect::<Vec<_>>(),
        [16, 17]
    );
    assert_eq!(
        runtime
            .prediction
            .as_ref()
            .unwrap()
            .last_input_retry_committed,
        Some(ServerTick(11))
    );
    assert!(generate(&mut runtime, &mut server, 250).is_empty());
    let before = pending_identity(&runtime);
    let session = runtime.prediction.as_mut().unwrap();
    session.recover();
    assert_eq!(session.last_input_retry_committed, None);
    assert!(generate(&mut runtime, &mut server, 300).is_empty());
    assert_eq!(pending_identity(&runtime), before);
    assert_eq!(
        runtime
            .prediction
            .as_ref()
            .unwrap()
            .last_input_retry_committed,
        None
    );

    let mut welcome = runtime.prediction.as_ref().unwrap().model.welcome.clone();
    welcome.stream.epoch = ConnectionEpoch(welcome.stream.epoch.0 + 1);
    let replacement = Session::new(welcome).unwrap();
    assert_eq!(replacement.last_input_retry_committed, None);
    assert_eq!(replacement.sequence, CommandSeq(0));
}

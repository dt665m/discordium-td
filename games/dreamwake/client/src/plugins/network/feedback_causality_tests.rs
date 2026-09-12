//! Generation-time causality with the production client and real Renet packets.
//!
//! The server crate already depends on the client in its tests. This boundary
//! therefore uses the real CommandInbox and explicitly mirrors only authority's
//! first-effective minimum-slack aggregation and bounded Finalized publication.
use super::*;
use engine_net::commands::{
    Admission, CommandError, CommandInbox, CommandLimits, CommandRules, FinalizedStatus,
    OwnerStream,
};
use engine_net::prediction::ReplayInput;
use renet::{RenetServer, ServerEvent};
use std::collections::{BTreeMap, BTreeSet};

type DelayedPackets = BTreeMap<u64, Vec<Vec<u8>>>;

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

#[derive(Debug)]
struct Authored {
    at_millis: u64,
    sequence: CommandSeq,
    target: TargetTick,
    action_count: usize,
    owner_checkpoint: ServerTick,
    known_committed: ServerTick,
    server_committed: ServerTick,
}

#[derive(Debug)]
struct FirstEffectiveAdmission {
    at_millis: u64,
    arrival_tick: ServerTick,
    sequence: CommandSeq,
    target: TargetTick,
    admission: Admission,
}

#[derive(Debug)]
struct Publication {
    sent_millis: u64,
    through: ServerTick,
    arrival: ArrivalFeedback,
    contributing_sequences: Vec<CommandSeq>,
}

#[derive(Debug, Default)]
struct Trace {
    authored: Vec<Authored>,
    admissions: Vec<FirstEffectiveAdmission>,
    publications: Vec<Publication>,
    repeated_late: usize,
    rate_limited_bundles: usize,
    maximum_queued_input_bundles: usize,
    maximum_deferred_input_bundles: usize,
    maximum_input_bundles_per_tick: usize,
    clock_replies: usize,
}

impl Trace {
    fn validate_causality(&self) {
        let authored = self
            .authored
            .iter()
            .map(|entry| (entry.sequence, entry))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(authored.len(), self.authored.len());
        assert!(self.authored.iter().all(|entry| {
            entry.owner_checkpoint <= entry.known_committed
                && entry.known_committed <= entry.server_committed
        }));
        assert!(
            self.authored
                .iter()
                .map(|entry| entry.action_count)
                .sum::<usize>()
                <= 1
        );
        let effective = self
            .admissions
            .iter()
            .map(|entry| (entry.sequence, entry))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            effective.len(),
            self.admissions.len(),
            "redundancy must not become fresh feedback"
        );
        for admission in &self.admissions {
            let generated = authored[&admission.sequence];
            assert_eq!(generated.target, admission.target);
            assert!(admission.at_millis >= generated.at_millis + 50);
        }
        let mut included = BTreeSet::new();
        let mut previous_through = ServerTick(10);
        let mut observed_commands = 0;
        let mut late_commands = 0;
        for publication in &self.publications {
            assert_eq!(publication.sent_millis % 50, 0);
            assert!(publication.through > previous_through);
            previous_through = publication.through;
            let minimum = publication
                .contributing_sequences
                .iter()
                .map(|sequence| {
                    assert!(included.insert(*sequence));
                    let admission = effective[sequence];
                    assert!(admission.at_millis <= publication.sent_millis);
                    let slack = match admission.admission {
                        Admission::Accepted { arrival_slack } => arrival_slack.min(12) as u8,
                        Admission::Late => 0,
                        other => panic!("ineffective feedback: {other:?}"),
                    };
                    observed_commands += 1;
                    late_commands += u64::from(slack == 0);
                    ArrivalSample {
                        sequence: admission.sequence,
                        target: admission.target,
                        arrival_tick: admission.arrival_tick,
                        slack,
                    }
                })
                .min_by_key(|sample| (sample.slack, std::cmp::Reverse(sample.sequence)));
            assert_eq!(publication.arrival.sample, minimum);
            assert_eq!(publication.arrival.observed_commands, observed_commands);
            assert_eq!(publication.arrival.late_commands, late_commands);
        }
    }
}

fn take_due(packets: &mut DelayedPackets, millis: u64) -> Vec<Vec<u8>> {
    let mut due = Vec::new();
    while packets
        .first_key_value()
        .is_some_and(|(at, _)| *at <= millis)
    {
        due.extend(packets.pop_first().unwrap().1);
    }
    due
}

fn queue_control(server: &mut RenetServer, runtime: &Runtime, control: ServerControl, seq: u32) {
    let welcome = &runtime.prediction.as_ref().unwrap().model.welcome;
    let bytes = encode_control(
        &control,
        welcome.stream.epoch,
        seq,
        welcome.frame_limit().unwrap(),
    )
    .unwrap();
    server.send_message(runtime.client_id, CONTROL_CHANNEL, bytes);
}

fn delayed_initial_owner(frame_millis: u64) -> Trace {
    let (session, owner_frames) = tests::fixture();
    let owner = session.model.welcome.stream;
    let rules = Rules(owner);
    let mut inbox = CommandInbox::new(owner, ServerTick(10), CommandLimits::default()).unwrap();
    let mut runtime = tests::receipt_runtime(session);
    runtime.renet.set_connected();
    let mut server = RenetServer::new(connection_config());
    server.add_connection(runtime.client_id);
    assert!(matches!(
        server.get_event(),
        Some(ServerEvent::ClientConnected { .. })
    ));
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    let mut trace = Trace::default();
    let mut uplink = DelayedPackets::new();
    let mut downlink = DelayedPackets::new();
    let mut input_bundles_due = BTreeMap::<u64, usize>::new();
    let mut queued_input_bundles = 0;
    let mut input_bundles_this_tick = 0;
    let input_bundle_budget = CommandLimits::default().submissions_per_tick / 8;
    let mut active = false;
    let mut probe_sent = false;
    let mut finalized_sent = ServerTick(10);
    let mut arrival = ArrivalFeedback::default();
    let mut contributing_sequences = Vec::new();
    let mut control_sequence = 0;
    let mut previous_client_frame = 167;

    // Welcome anchors committed tick 10 at 166.666667 ms. Its complete owner
    // publication crosses a 50 ms downlink, then its decoded proof crosses the
    // same uplink before Active is sent. No clock estimate is injected directly.
    for frame in owner_frames {
        server.send_message(runtime.client_id, STATE_CHANNEL, frame);
    }
    downlink.insert(217, server.get_packets_to_send(runtime.client_id).unwrap());
    runtime
        .prediction
        .as_mut()
        .unwrap()
        .journal
        .capture(HardwareSample {
            at: Duration::from_millis(200),
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

    for millis in 168..=900 {
        server.update(Duration::from_millis(1));
        while inbox.finalized_through().0 < millis * u64::from(dreamwake_sim::TICK_HZ) / 1000 {
            let prepared = inbox.prepare_next(&rules).unwrap();
            inbox.commit(prepared.tick()).unwrap();
            input_bundles_this_tick = 0;
        }
        for packet in take_due(&mut uplink, millis) {
            server
                .process_packet_from(&packet, runtime.client_id)
                .unwrap();
        }
        queued_input_bundles += input_bundles_due.remove(&millis).unwrap_or(0);
        trace.maximum_queued_input_bundles =
            trace.maximum_queued_input_bundles.max(queued_input_bundles);
        while let Some(bytes) = server.receive_message(runtime.client_id, CONTROL_CHANNEL) {
            let welcome = &runtime.prediction.as_ref().unwrap().model.welcome;
            let control = decode_control::<ClientControl>(
                &bytes,
                owner.epoch,
                welcome.frame_limit().unwrap(),
            )
            .unwrap();
            match control {
                ClientControl::OwnerDecoded { .. } if !active => {
                    active = true;
                    queue_control(
                        &mut server,
                        &runtime,
                        ServerControl::Active {
                            owner_snapshot: SnapshotId(1),
                            through: ServerTick(10),
                        },
                        control_sequence,
                    );
                    control_sequence += 1;
                }
                ClientControl::ClockProbe { client_send_nanos } => {
                    queue_control(
                        &mut server,
                        &runtime,
                        ServerControl::ClockReply {
                            client_send_nanos,
                            server_receive_nanos: millis * 1_000_000,
                            server_send_nanos: millis * 1_000_000,
                            tick: inbox.finalized_through(),
                        },
                        control_sequence,
                    );
                    control_sequence += 1;
                }
                _ => {}
            }
        }
        // Match authority.receive: reserve a full eight-record bundle BEFORE
        // dequeue, leaving every surplus message inside Renet until a commit.
        while input_bundles_this_tick < input_bundle_budget {
            let Some(bytes) = server.receive_message(runtime.client_id, INPUT_CHANNEL) else {
                break;
            };
            input_bundles_this_tick += 1;
            queued_input_bundles -= 1;
            trace.maximum_input_bundles_per_tick = trace
                .maximum_input_bundles_per_tick
                .max(input_bundles_this_tick);
            let welcome = &runtime.prediction.as_ref().unwrap().model.welcome;
            let bundle =
                decode_commands(&bytes, owner.epoch, welcome.frame_limit().unwrap()).unwrap();
            assert!(bundle.records.len() <= 8);
            for command in bundle.records.as_slice() {
                let previous = inbox.retained_admission(command.sequence);
                let arrival_tick = inbox.finalized_through();
                let admission = match inbox.admit(owner.connection, command.clone(), &rules) {
                    Ok(admission) => admission,
                    // Production aborts this bundle on the per-tick ingress cap.
                    Err(CommandError::RateLimit) => {
                        trace.rate_limited_bundles += 1;
                        break;
                    }
                    Err(error) => panic!("admission at {millis} ms: {error:?}"),
                };
                if matches!(previous, Some(Admission::Accepted { .. } | Admission::Late)) {
                    trace.repeated_late += usize::from(admission == Admission::Late);
                    continue;
                }
                let slack = match admission {
                    Admission::Accepted { arrival_slack } => Some(arrival_slack.min(12) as u8),
                    Admission::Late => Some(0),
                    _ => None,
                };
                if let Some(slack) = slack {
                    arrival.observed_commands += 1;
                    arrival.late_commands += u64::from(slack == 0);
                    let sample = ArrivalSample {
                        sequence: command.sequence,
                        target: command.target,
                        arrival_tick,
                        slack,
                    };
                    if arrival.sample.is_none_or(|old| {
                        sample.slack < old.slack
                            || (sample.slack == old.slack && sample.sequence > old.sequence)
                    }) {
                        arrival.sample = Some(sample);
                    }
                    contributing_sequences.push(command.sequence);
                    trace.admissions.push(FirstEffectiveAdmission {
                        at_millis: millis,
                        arrival_tick,
                        sequence: command.sequence,
                        target: command.target,
                        admission,
                    });
                }
            }
        }
        trace.maximum_deferred_input_bundles = trace
            .maximum_deferred_input_bundles
            .max(queued_input_bundles);
        // Match the server's 20 Hz publication boundary and bounded receipts.
        if active && millis % 50 == 0 {
            let receipts = inbox
                .receipts()
                .filter(|receipt| receipt.tick > finalized_sent)
                .take(32)
                .copied()
                .collect::<Vec<_>>();
            if let Some(last) = receipts.last() {
                let through = last.tick;
                trace.publications.push(Publication {
                    sent_millis: millis,
                    through,
                    arrival,
                    contributing_sequences: std::mem::take(&mut contributing_sequences),
                });
                queue_control(
                    &mut server,
                    &runtime,
                    ServerControl::Finalized {
                        through,
                        receipts: BoundedVec::new(receipts).unwrap(),
                        menu_applied: None,
                        arrival,
                    },
                    control_sequence,
                );
                control_sequence += 1;
                finalized_sent = through;
                arrival.sample = None;
            }
        }
        downlink
            .entry(millis + 50)
            .or_default()
            .extend(server.get_packets_to_send(runtime.client_id).unwrap());

        if millis >= 200 && (millis - 200) % frame_millis == 0 {
            runtime
                .renet
                .update(Duration::from_millis(millis - previous_client_frame));
            previous_client_frame = millis;
            runtime.prediction.as_mut().unwrap().begin_frame().unwrap();
            for packet in take_due(&mut downlink, millis) {
                runtime.renet.process_packet(&packet);
            }
            // The production receive pass drains reliable control before state.
            for channel in [CONTROL_CHANNEL, STATE_CHANNEL] {
                while let Some(bytes) = runtime.renet.receive_message(channel) {
                    receive_message(
                        &mut runtime,
                        channel,
                        &bytes,
                        Duration::from_millis(millis),
                        &mut world,
                    )
                    .unwrap();
                }
            }
            runtime.prediction.as_mut().unwrap().reconcile().unwrap();
            if runtime.prediction.as_ref().unwrap().active && !probe_sent {
                send_control(
                    &mut runtime,
                    ClientControl::ClockProbe {
                        client_send_nanos: millis * 1_000_000,
                    },
                )
                .unwrap();
                probe_sent = true;
            }
            let session = runtime.prediction.as_ref().unwrap();
            let previous_sequence = session.sequence;
            let known_committed = session.simulation_clock.committed();
            let owner_checkpoint = session
                .scopes
                .state(owner.owner)
                .map_or(ServerTick(10), |state| state.end_tick);
            generate_commands(&mut runtime, Duration::from_millis(millis)).unwrap();
            trace.authored.extend(
                runtime
                    .pending
                    .iter()
                    .filter(|frame| frame.command.sequence > previous_sequence)
                    .map(|frame| Authored {
                        at_millis: millis,
                        sequence: frame.command.sequence,
                        target: frame.command.target,
                        action_count: frame.command.actions.len(),
                        owner_checkpoint,
                        known_committed,
                        server_committed: inbox.finalized_through(),
                    }),
            );
            trace.clock_replies = runtime.prediction.as_ref().unwrap().clock.sample_count();
            let queued = runtime.renet.channel_send_queue(INPUT_CHANNEL).messages;
            let packets = runtime.renet.get_packets_to_send();
            let remaining = runtime.renet.channel_send_queue(INPUT_CHANNEL).messages;
            *input_bundles_due.entry(millis + 50).or_default() += queued - remaining;
            uplink.entry(millis + 50).or_default().extend(packets);
        }
    }
    assert!(active && trace.clock_replies > 0);
    assert_eq!(
        trace.rate_limited_bundles, 0,
        "the pre-dequeue bundle budget prevents admission drops"
    );
    assert!(trace.maximum_input_bundles_per_tick <= input_bundle_budget);
    trace.validate_causality();
    trace
}

fn assert_generation_causality(frame_millis: u64) {
    let trace = delayed_initial_owner(frame_millis);
    let known_past = trace
        .authored
        .iter()
        .filter(|entry| entry.target.0 <= entry.known_committed.0)
        .collect::<Vec<_>>();
    let server_past = trace
        .authored
        .iter()
        .filter(|entry| entry.target.0 <= entry.server_committed.0)
        .collect::<Vec<_>>();
    assert!(
        known_past.is_empty() && server_past.is_empty(),
        "new commands must never be authored into committed history ({frame_millis} ms client frames).\n\
         Violates client-known proof: {known_past:#?}\n\
         Violates actual server commit: {server_past:#?}\nFull causal trace: {trace:#?}"
    );
}

#[test]
fn delayed_owner_with_33_ms_frames_never_authors_committed_input() {
    assert_generation_causality(33);
}

#[test]
fn delayed_owner_with_50_ms_frames_never_authors_committed_input() {
    assert_generation_causality(50);
}

#[test]
fn delayed_owner_with_63_ms_frames_never_authors_committed_input() {
    assert_generation_causality(63);
}

fn receive_server_batch(
    runtime: &mut Runtime,
    server: &mut RenetServer,
    world: &mut World,
    receive_millis: u64,
) {
    for packet in server.get_packets_to_send(runtime.client_id).unwrap() {
        runtime.renet.process_packet(&packet);
    }
    for channel in [CONTROL_CHANNEL, STATE_CHANNEL] {
        while let Some(bytes) = runtime.renet.receive_message(channel) {
            receive_message(
                runtime,
                channel,
                &bytes,
                Duration::from_millis(receive_millis),
                world,
            )
            .unwrap();
        }
    }
    for packet in runtime.renet.get_packets_to_send() {
        server
            .process_packet_from(&packet, runtime.client_id)
            .unwrap();
    }
    while server
        .receive_message(runtime.client_id, CONTROL_CHANNEL)
        .is_some()
    {}
}

fn exact_stale_frontier() -> (Runtime, RenetServer, CommandInbox<TickInput, DreamAction>) {
    let (fixture, owner_frames) = tests::fixture_publication(537, 1, 1);
    let mut welcome = fixture.model.welcome;
    welcome.tick = ServerTick(537);
    welcome.server_time_nanos = 8_950_000_000;
    let mut runtime = tests::receipt_runtime(Session::new(welcome).unwrap());
    runtime.renet.set_connected();
    let mut server = RenetServer::new(connection_config());
    server.add_connection(runtime.client_id);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    runtime.prediction.as_mut().unwrap().begin_frame().unwrap();
    for frame in owner_frames {
        server.send_message(runtime.client_id, STATE_CHANNEL, frame);
    }
    receive_server_batch(&mut runtime, &mut server, &mut world, 9000);
    queue_control(
        &mut server,
        &runtime,
        ServerControl::Active {
            owner_snapshot: SnapshotId(1),
            through: ServerTick(537),
        },
        0,
    );
    receive_server_batch(&mut runtime, &mut server, &mut world, 9100);
    runtime.prediction.as_mut().unwrap().reconcile().unwrap();
    assert!(runtime.prediction.as_ref().unwrap().active);

    // A physical 50 ms uplink/downlink yields a 100 ms RTT and five-tick lead.
    // The later Finalized publication advances proof without replacing owner537.
    queue_control(
        &mut server,
        &runtime,
        ServerControl::ClockReply {
            client_send_nanos: 9_000_000_000,
            server_receive_nanos: 9_050_000_000,
            server_send_nanos: 9_050_000_000,
            tick: ServerTick(543),
        },
        1,
    );
    receive_server_batch(&mut runtime, &mut server, &mut world, 9100);
    let owner = runtime.prediction.as_ref().unwrap().model.welcome.stream;
    let rules = Rules(owner);
    let mut inbox = CommandInbox::new(owner, ServerTick(537), CommandLimits::default()).unwrap();
    while inbox.finalized_through() < ServerTick(549) {
        let prepared = inbox.prepare_next(&rules).unwrap();
        inbox.commit(prepared.tick()).unwrap();
    }
    queue_control(
        &mut server,
        &runtime,
        ServerControl::Finalized {
            through: inbox.finalized_through(),
            receipts: BoundedVec::new(inbox.receipts().copied().collect()).unwrap(),
            menu_applied: None,
            arrival: ArrivalFeedback::default(),
        },
        2,
    );
    receive_server_batch(&mut runtime, &mut server, &mut world, 9200);
    let session = runtime.prediction.as_mut().unwrap();
    session
        .journal
        .capture(HardwareSample {
            at: Duration::from_millis(9075),
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
    session.begin_frame().unwrap();
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(537))
    );
    assert_eq!(session.finalized, ServerTick(549));
    assert_eq!(session.simulation_clock.committed(), ServerTick(549));
    assert_eq!(session.lead.lead(), 5);
    assert_eq!(session.sequence, CommandSeq(0));
    assert_eq!(session.journal.pending_samples(), 1);
    assert_eq!(session.input_deadline, None);
    (runtime, server, inbox)
}

#[test]
fn stale_frontier_forecasts_without_authoring_then_preserves_edge_at_future_target() {
    let (mut runtime, mut server, mut inbox) = exact_stale_frontier();
    generate_commands(&mut runtime, Duration::from_millis(9200)).unwrap();
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.estimated_simulation_tick, Some(ServerTick(552)));
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(557))
    );
    assert_eq!(session.predictor.work().predicted_ticks, 20);
    assert_eq!(session.sequence, CommandSeq(1));
    assert_eq!(runtime.pending.len(), 1);
    assert_eq!(session.journal.pending_samples(), 0);
    assert_eq!(session.input_deadline, Some(Duration::from_millis(9200)));
    for tick in 538..557 {
        assert!(matches!(
            session
                .predictor
                .replay_input_history(OWNER_GROUP, ServerTick(tick)),
            Some(ReplayInput::Substitute)
        ));
        assert!(
            session
                .predictor
                .command_history(OWNER_GROUP, ServerTick(tick))
                .is_none()
        );
        let hero = session
            .predictor
            .history_state(OWNER_GROUP, ServerTick(tick))
            .unwrap()
            .0
            .hero_view();
        assert!(
            !hero.dashing,
            "future hardware edge leaked into forecast tick {tick}"
        );
        assert_eq!(hero.dash_cooldown, 0.0);
    }
    let command = runtime.pending.front().unwrap().command.clone();
    assert_eq!(command.sequence, CommandSeq(1));
    assert_eq!(command.target, TargetTick(557));
    assert_eq!(command.input.held.movement, [1.0, 0.0]);
    assert_eq!(
        command.actions.as_slice(),
        &[ActionEdge {
            slot: 0,
            action: DreamAction::Dash {
                direction: [1.0, 0.0]
            },
        }]
    );
    let actions = runtime.outcomes.trace(1, 8);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].key, command.action_key(0));
    assert_eq!(actions[0].target_c, ServerTick(557));
    assert_eq!(actions[0].created_at, Duration::from_millis(9200));

    // Real Renet carries only the authored future command. At its physical
    // 50 ms uplink arrival the authoritative inbox has committed tick555.
    let rules = Rules(command.owner);
    while inbox.finalized_through() < ServerTick(555) {
        let prepared = inbox.prepare_next(&rules).unwrap();
        assert!(prepared.actions().is_empty());
        inbox.commit(prepared.tick()).unwrap();
    }
    for packet in runtime.renet.get_packets_to_send() {
        server
            .process_packet_from(&packet, runtime.client_id)
            .unwrap();
    }
    let mut transmitted = Vec::new();
    while let Some(bytes) = server.receive_message(runtime.client_id, INPUT_CHANNEL) {
        let welcome = &runtime.prediction.as_ref().unwrap().model.welcome;
        let bundle =
            decode_commands(&bytes, command.owner.epoch, welcome.frame_limit().unwrap()).unwrap();
        transmitted.extend(bundle.records.into_vec());
    }
    assert_eq!(transmitted, [command.clone()]);
    assert_eq!(
        inbox
            .admit(command.owner.connection, command.clone(), &rules)
            .unwrap(),
        Admission::Accepted { arrival_slack: 2 }
    );
    assert_eq!(
        inbox
            .admit(command.owner.connection, command.clone(), &rules)
            .unwrap(),
        Admission::Duplicate
    );
    let mut consumed_actions = 0;
    while inbox.finalized_through() < ServerTick(558) {
        let prepared = inbox.prepare_next(&rules).unwrap();
        consumed_actions += prepared.actions().len();
        let receipt = inbox.commit(prepared.tick()).unwrap();
        if receipt.tick == ServerTick(557) {
            assert_eq!(receipt.sequence, Some(CommandSeq(1)));
            assert_eq!(receipt.status, FinalizedStatus::Executed);
        }
    }
    assert_eq!(consumed_actions, 1);
    assert_eq!(
        inbox
            .admit(command.owner.connection, command, &rules)
            .unwrap(),
        Admission::Duplicate
    );
}

#[test]
fn frontier_ahead_of_estimate_keeps_contiguous_historical_capture_deadlines() {
    let (mut runtime, _, _) = exact_stale_frontier();
    generate_commands(&mut runtime, Duration::from_millis(9200)).unwrap();
    let session = runtime.prediction.as_mut().unwrap();
    assert_eq!(session.sequence, CommandSeq(1));
    session
        .journal
        .capture(HardwareSample {
            at: Duration::from_millis(9205),
            held: Some(
                DreamInput {
                    movement: [0.0, 1.0],
                    ..Default::default()
                }
                .into(),
            ),
            edges: BoundedVec::new(vec![DreamAction::Dash {
                direction: [0.0, 1.0],
            }])
            .unwrap(),
            mouse_delta: [0.0; 2],
        })
        .unwrap();
    session.begin_frame().unwrap();
    generate_commands(&mut runtime, Duration::from_millis(9250)).unwrap();
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.estimated_simulation_tick, Some(ServerTick(555)));
    assert_eq!(session.predictor.work().predicted_ticks, 3);
    assert_eq!(session.sequence, CommandSeq(4));
    let frames = runtime.pending.iter().skip(1).collect::<Vec<_>>();
    assert_eq!(frames.len(), 3);
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame.command.sequence, CommandSeq(index as u64 + 2));
        assert_eq!(frame.command.target, TargetTick(index as u64 + 558));
        assert_eq!(frame.command.input.held.movement, [0.0, 1.0]);
        assert!(
            session
                .predictor
                .command_history(OWNER_GROUP, ServerTick(frame.command.target.0))
                .is_some()
        );
    }
    assert_eq!(frames[0].command.actions.len(), 1);
    assert!(
        frames[1..]
            .iter()
            .all(|frame| frame.command.actions.is_empty())
    );
    assert_eq!(session.input_deadline, Some(Duration::from_millis(9250)));
}

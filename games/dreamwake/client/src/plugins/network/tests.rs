use super::*;
use dreamwake_sim::replication::{
    CommittedReplication, PublicEnemyView, PublicReplica, Replica, ReplicaState, ReplicationKey,
    ReplicationStamp,
};
use engine_net::{commands::OwnerStream, replication::stream::ServerStream};

const GLOBAL: EntityId = EntityId {
    index: 1,
    generation: 1,
};
const OWNER: EntityId = EntityId {
    index: 2,
    generation: 1,
};

pub(super) fn welcome() -> Welcome {
    Welcome {
        server_instance: 9,
        protocol: PROTOCOL_ID,
        client: ConnectionId(41),
        player: player::PlayerId::new(1).unwrap(),
        stream: OwnerStream {
            connection: ConnectionId(41),
            epoch: ConnectionEpoch(3),
            stream: CommandStream(1),
            owner: OWNER,
            ownership: OwnershipEpoch(1),
        },
        match_epoch: 1,
        tick: ServerTick(10),
        server_time_nanos: 166_666_667,
        content: ContentIdentity::current(SceneRevision(1)),
        global_entity: GLOBAL,
        owner_entity: OWNER,
        application_frame_bytes: live::APPLICATION_FRAME_BYTES as u16,
    }
}

/// The server side of one connection's state stream, for feeding a Session.
pub(super) struct Feed {
    pub stream: ServerStream<EntityId, ReplicaState>,
}
impl Feed {
    pub fn new() -> Self {
        Self {
            stream: ServerStream::new(64),
        }
    }
    /// One packet carrying these replicas whole or against their bases.
    pub fn packet(
        &mut self,
        tick: u64,
        status: OwnerStatus,
        entries: &[(EntityId, Replica)],
        removed: &[EntityId],
    ) -> Vec<u8> {
        let seq = self.stream.begin().unwrap();
        let mut packet = StatePacket {
            seq,
            tick: ServerTick(tick),
            status,
            entries: Vec::new(),
            removed: removed.to_vec(),
        };
        for entity in removed {
            self.stream.remove(*entity);
            self.stream.include_removal(*entity).unwrap();
        }
        for (entity, replica) in entries {
            let state = ReplicaState::capture(replica).unwrap();
            let base = self
                .stream
                .base(*entity)
                .filter(|(_, base)| state.deltas_from(base))
                .map(|(seq, base)| (seq, base.clone()));
            let mut body = Vec::new();
            state
                .encode(base.as_ref().map(|(_, base)| base), &mut body)
                .unwrap();
            packet.entries.push(StateEntry {
                entity: *entity,
                base: base.map(|(seq, _)| seq),
                body,
            });
            self.stream.include(*entity, state).unwrap();
        }
        self.stream.finish().unwrap();
        encode_state(
            &packet,
            welcome().stream.epoch,
            engine_net::codec::FrameLimit::new(MAX_STATE_FRAME_BYTES).unwrap(),
        )
        .unwrap()
    }
}

fn capture(sim: &DreamSimulation, tick: u64) -> CommittedReplication {
    sim.capture_replication(ReplicationStamp {
        match_epoch: 1,
        server_tick: tick,
        gameplay_tick: sim.snapshot().tick,
        scene_revision: 1,
        revision: tick,
    })
    .unwrap()
}
/// The owner, the match summary and every platform, as a server would send
/// them to a connection that can see the whole scene.
pub(super) fn owner_and_global(sim: &DreamSimulation, tick: u64) -> Vec<(EntityId, Replica)> {
    let capture = capture(sim, tick);
    let mut entries = vec![
        (GLOBAL, Replica::Global(capture.global().clone())),
        (
            OWNER,
            Replica::Owner(
                capture
                    .owner_checkpoint(1, 1)
                    .and_then(|owner| {
                        owner.with_input_continuity(ServerTick(tick), Default::default())
                    })
                    .unwrap(),
            ),
        ),
    ];
    for (index, key) in capture.keys().into_iter().enumerate() {
        if let ReplicationKey::Platform(id) = key {
            entries.push((
                EntityId {
                    index: 100 + index as u64,
                    generation: 1,
                },
                Replica::Actor(PublicReplica::Platform(
                    capture.project_platform(id).unwrap(),
                )),
            ));
        }
    }
    entries
}
fn enemy(id: u64, x: f32) -> Replica {
    Replica::Actor(PublicReplica::Enemy(PublicEnemyView {
        id,
        position: [x, 0.0],
        facing: [0.0, -1.0],
        velocity: [0.0, 0.0],
        hp: 10.0,
        max_hp: 10.0,
        kind: dreamwake_sim::EnemyKind::Melee,
        windup: 0.0,
        target: [x, 0.0],
        warn_radius: 0.0,
        phase: 0,
        slowed: false,
        hit_flash: 0.0,
    }))
}

/// A session that received and applied its first owner state.
pub(super) fn fixture() -> (Session, Feed) {
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    let mut session = Session::new(welcome()).unwrap();
    let mut feed = Feed::new();
    let bytes = feed.packet(10, OwnerStatus::default(), &owner_and_global(&sim, 10), &[]);
    assert!(session.receive_state(&bytes).unwrap());
    session.reconcile().unwrap();
    assert!(session.active);
    (session, feed)
}

pub(super) fn receipt_runtime(session: Session) -> Runtime {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let client_id = session.welcome.client.0;
    let transport = Transport::Udp(
        renet_cross::UdpNetcodeClientTransport::new(
            Duration::ZERO,
            renet_cross::ClientAuthentication::Unsecure {
                protocol_id: PROTOCOL_ID,
                client_id,
                server_addr: socket.local_addr().unwrap(),
                user_data: None,
            },
            socket,
        )
        .unwrap(),
    );
    // Only Renet's local outbound queue is used; no packets are transmitted.
    let mut renet = RenetClient::new(connection_config());
    renet.set_connected();
    Runtime {
        renet,
        transport,
        client_id,
        action_seq: CommandSeq(0),
        pending: VecDeque::new(),
        prediction: Some(session),
        last_received: 0.0,
        last_snapshot: None,
        in_flight_action: None,
        outcomes: Default::default(),
    }
}

fn command(session: &Session, sequence: u64, target: u64) -> InputCommand {
    Command {
        owner: session.welcome.stream,
        sequence: CommandSeq(sequence),
        target: TargetTick(target),
        input: DreamInput {
            movement: [1.0, 0.0],
            aim: [1.0, 0.0],
            ..Default::default()
        }
        .into(),
        actions: BoundedVec::default(),
    }
}

#[test]
fn state_deltas_survive_loss_and_only_accepted_packets_are_acknowledged() {
    let (mut session, mut feed) = fixture();
    let first = EntityId {
        index: 50,
        generation: 1,
    };
    let bytes = feed.packet(11, OwnerStatus::default(), &[(first, enemy(50, 1.0))], &[]);
    session.receive_state(&bytes).unwrap();
    feed.stream.acknowledge(session.stream_ack()).unwrap();
    // Lost on the way: the client never sees it.
    feed.packet(12, OwnerStatus::default(), &[(first, enemy(50, 2.0))], &[]);
    // Encoded against the acknowledged state, it still decodes.
    let bytes = feed.packet(13, OwnerStatus::default(), &[(first, enemy(50, 3.0))], &[]);
    assert!(session.receive_state(&bytes).unwrap());
    assert!(matches!(
        session.replica(first),
        Some(Replica::Actor(PublicReplica::Enemy(view))) if view.position == [3.0, 0.0]
    ));
    // A repeat is ignored, and a malformed packet is not acknowledged.
    assert!(!session.receive_state(&bytes).unwrap());
    let ack = session.stream_ack();
    let mut forged = feed.packet(14, OwnerStatus::default(), &[(first, enemy(50, 4.0))], &[]);
    let last = forged.len() - 1;
    forged[last] ^= 0xff;
    assert!(session.receive_state(&forged).is_err());
    assert_eq!(session.stream_ack(), ack);
    // A removal drops the entity.
    feed.stream.acknowledge(session.stream_ack()).unwrap();
    let bytes = feed.packet(15, OwnerStatus::default(), &[], &[first]);
    session.receive_state(&bytes).unwrap();
    assert!(session.replica(first).is_none());
}

#[test]
fn owner_state_replaces_prediction_and_replays_local_input() {
    let (mut session, mut feed) = fixture();
    let dependencies = session.prediction_dependencies(ServerTick(10));
    for (sequence, tick) in (11..=16).enumerate() {
        session
            .predictor
            .predict(
                ServerTick(tick),
                RewindInput::Command(command(&session, sequence as u64 + 1, tick)),
                dependencies.clone(),
            )
            .unwrap();
    }
    let predicted = session.predictor.state().unwrap().hero_view().position;
    assert!(predicted[0] > 0.0, "the hero moved under local input");
    // The server's state for tick 12 with nobody moving: replay 13..=16.
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    let bytes = feed.packet(12, OwnerStatus::default(), &owner_and_global(&sim, 12), &[]);
    session.receive_state(&bytes).unwrap();
    session.reconcile().unwrap();
    assert_eq!(session.predictor.base_tick(), Some(ServerTick(12)));
    assert_eq!(session.predictor.predicted_tick(), Some(ServerTick(16)));
    assert_eq!(session.predictor.work().replayed_ticks, 4);
    // A state past everything predicted becomes the new base.
    let bytes = feed.packet(40, OwnerStatus::default(), &owner_and_global(&sim, 40), &[]);
    session.receive_state(&bytes).unwrap();
    session.reconcile().unwrap();
    assert_eq!(session.predictor.predicted_tick(), Some(ServerTick(40)));
}

#[test]
fn owner_status_is_latest_wins_and_feedback_tunes_lead_once() {
    let (mut session, mut feed) = fixture();
    session.sequence = CommandSeq(5);
    session.record_command_issuance(CommandSeq(5), TargetTick(20), TargetTick(20));
    let status = |finalized, observed, sample| OwnerStatus {
        finalized: ServerTick(finalized),
        menu_applied: None,
        arrival: ArrivalFeedback {
            sample,
            observed_commands: observed,
            late_commands: 0,
        },
    };
    let sample = Some(ArrivalSample {
        sequence: CommandSeq(5),
        target: TargetTick(20),
        arrival_tick: ServerTick(14),
        slack: 6,
    });
    let older = feed.packet(15, status(14, 4, None), &[], &[]);
    let newer = feed.packet(16, status(15, 5, sample), &[], &[]);
    session.receive_state(&newer).unwrap();
    session.receive_state(&older).unwrap();
    assert_eq!(session.status.finalized, ServerTick(15));
    assert_eq!(session.arrival_slack, Some(6));
    let applied = session.metrics().arrival_feedback_applied;
    // The same sample again (a later packet repeating it) changes nothing.
    let repeat = feed.packet(17, status(16, 5, sample), &[], &[]);
    session.receive_state(&repeat).unwrap();
    assert_eq!(session.metrics().arrival_feedback_applied, applied);
    assert_eq!(session.status.finalized, ServerTick(16));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn input_packets_carry_newest_and_oldest_unconsumed_commands_with_the_ack() {
    let (session, _) = fixture();
    let commands: Vec<_> = (1..=10).map(|n| command(&session, n, 10 + n)).collect();
    let mut runtime = receipt_runtime(session);
    for command in &commands {
        runtime.pending.push_back(PendingFrame {
            command: command.clone(),
            sent_at: 0.0,
        });
    }
    send_input(&mut runtime, &commands[9..]).unwrap();
    let session = runtime.prediction.as_ref().unwrap();
    let mut sent = Vec::new();
    for packet in runtime.renet.get_packets_to_send() {
        let mut server = renet::RenetServer::new(connection_config());
        server.add_connection(1);
        server.process_packet_from(&packet, 1).unwrap();
        while let Some(bytes) = server.receive_message(1, INPUT_CHANNEL) {
            sent.push(
                decode_input(
                    &bytes,
                    session.welcome.stream.epoch,
                    session.welcome.frame_limit().unwrap(),
                )
                .unwrap(),
            );
        }
    }
    let [packet] = sent.as_slice() else {
        panic!("one input packet per tick: {}", sent.len());
    };
    assert_eq!(packet.ack, session.stream_ack());
    let sequences: Vec<_> = packet
        .records
        .as_slice()
        .iter()
        .map(|r| r.sequence.0)
        .collect();
    assert_eq!(sequences, vec![10, 9, 8, 1, 2, 3]);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_joining_client_acknowledges_state_without_commands() {
    let mut session = Session::new(welcome()).unwrap();
    let mut feed = Feed::new();
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    let bytes = feed.packet(10, OwnerStatus::default(), &owner_and_global(&sim, 10), &[]);
    session.receive_state(&bytes).unwrap();
    let mut runtime = receipt_runtime(session);
    send_input(&mut runtime, &[]).unwrap();
    assert!(!runtime.renet.get_packets_to_send().is_empty());
}

#[test]
fn cover_markers_render_only_beside_their_present_cover() {
    let (mut session, mut feed) = fixture();
    let mut sim = DreamSimulation::new(42, false);
    sim.continue_run();
    let capture = capture(&sim, 11);
    let cover_id = sim.snapshot().covers[0].id;
    let Some(PublicReplica::Cover(cover)) =
        capture.project(ReplicationKey::Cover(cover_id), &Default::default())
    else {
        panic!("cover");
    };
    let cover_entity = EntityId {
        index: 60,
        generation: 1,
    };
    let marker_entity = EntityId {
        index: 61,
        generation: 1,
    };
    let marker_id = capture
        .keys()
        .into_iter()
        .find_map(|key| match key {
            ReplicationKey::CoverMarker(id) if capture.marker_parent(id) == Some(cover_id) => {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let marker = capture
        .project_cover_marker(marker_id, cover_entity)
        .unwrap();
    let bytes = feed.packet(
        11,
        OwnerStatus::default(),
        &[(
            marker_entity,
            Replica::Actor(PublicReplica::CoverMarker(marker)),
        )],
        &[],
    );
    session.receive_state(&bytes).unwrap();
    let shown = |session: &Session| {
        session
            .presentation(false)
            .unwrap()
            .unwrap()
            .cover_markers
            .len()
    };
    assert_eq!(shown(&session), 0, "waits for its cover");
    let bytes = feed.packet(
        12,
        OwnerStatus::default(),
        &[(cover_entity, Replica::Actor(PublicReplica::Cover(cover)))],
        &[],
    );
    session.receive_state(&bytes).unwrap();
    assert_eq!(shown(&session), 1);
}

#[test]
fn stable_player_identity_is_distinct_from_transport_identity_in_client_view() {
    let mut welcome = welcome();
    welcome.client = ConnectionId(77);
    welcome.stream.connection = ConnectionId(77);
    let session = Session::new(welcome).unwrap();
    let mut connection = crate::DreamConnection::default();
    session.update_connection(&mut connection, 0.0, 1);
    assert_eq!(connection.client_id, 77);
    assert_eq!(connection.player_id, 1);
    assert_eq!(session.predictor.model().expectation().owner, 1);
}

#[test]
fn slow_first_clock_reply_does_not_partially_initialize_prediction() {
    let (mut session, _) = fixture();
    let delayed = TimeExchange {
        client_send: Duration::from_secs(1),
        server_receive: Duration::from_millis(2020),
        server_send: Duration::from_millis(2020),
        client_receive: Duration::from_millis(1900),
    };
    // The delay happened locally after the server replied, so this round trip
    // measures a renderer or asset stall rather than the network. Discard the
    // probe and keep waiting.
    assert_eq!(session.observe_clock(delayed), Ok(false));
    assert_eq!(session.clock.sample_count(), 0);
    assert_eq!(session.lead.lead(), 3);
    session
        .observe_clock(TimeExchange {
            client_send: Duration::from_secs(2),
            server_receive: Duration::from_millis(3020),
            server_send: Duration::from_millis(3020),
            client_receive: Duration::from_millis(2040),
        })
        .unwrap();
    assert_eq!(session.clock.sample_count(), 1);
    // Four seconds without a frame (a background tab) is time that passed,
    // not a fault: the estimate lands where the server now is.
    assert_eq!(
        session.clock.estimate_server_tick(Duration::from_secs(6)),
        Ok(ServerTick(420))
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
    assert_eq!(sixty.iter().map(|(_, edges)| edges.len()).sum::<usize>(), 2);
    assert!(matches!(sixty[0].1.as_slice(), [DreamAction::Dash { .. }]));
    assert!(matches!(sixty[11].1.as_slice(), [DreamAction::Cast { .. }]));
}

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
        assert!(
            report
                .diagnostic
                .contains("application receive timeout after 12.001s")
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn disconnection_cleanup_unlocks_identity_and_retains_outcomes_without_double_counting() {
    for transport_error_recorded in [false, true] {
        let (session, _) = fixture();
        let mut command = command(&session, 7, 11);
        command.actions = BoundedVec::new(vec![ActionEdge {
            slot: 0,
            action: DreamAction::Dash {
                direction: [1.0, 0.0],
            },
        }])
        .unwrap();
        let mut runtime = receipt_runtime(session);
        runtime.last_received = 100.0;
        runtime.outcomes.bind_instance(42);
        runtime
            .outcomes
            .track(1, &command, Duration::from_secs(100))
            .unwrap();
        let report = disconnect_report(&runtime, Duration::from_secs(113), None).unwrap();
        let expected_status = report.status;
        let mut world = World::new();
        let mut profile = crate::identity::TravelerProfile::default();
        profile.lock().unwrap();
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
        assert_eq!(
            world.resource::<diagnostics::Telemetry>().transport_errors,
            if transport_error_recorded { 4 } else { 5 }
        );
        let retained = world.resource::<outcomes::ClientOutcomes>().trace(1, 2);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].key, command.action_key(0));
    }
}

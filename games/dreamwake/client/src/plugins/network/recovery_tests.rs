//! Recovery enqueue and reliable delivery remain live while prediction is stopped.
use super::*;
use renet::RenetServer;

fn runtime_and_server() -> (Runtime, RenetServer) {
    let (mut session, frames) = tests::fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.acknowledge_active(SnapshotId(1), ServerTick(10));
    session.reconcile().unwrap();
    assert!(session.active);
    let mut runtime = tests::receipt_runtime(session);
    runtime.renet.set_connected();
    let mut server = RenetServer::new(connection_config());
    server.add_connection(runtime.client_id);
    (runtime, server)
}

fn controls(runtime: &mut Runtime, server: &mut RenetServer) -> Vec<ClientControl> {
    for packet in runtime.renet.get_packets_to_send() {
        server
            .process_packet_from(&packet, runtime.client_id)
            .unwrap();
    }
    let welcome = &runtime.prediction.as_ref().unwrap().model.welcome;
    let mut controls = Vec::new();
    while let Some(bytes) = server.receive_message(runtime.client_id, CONTROL_CHANNEL) {
        controls.push(
            decode_control(&bytes, welcome.stream.epoch, welcome.frame_limit().unwrap()).unwrap(),
        );
    }
    for packet in server.get_packets_to_send(runtime.client_id).unwrap() {
        runtime.renet.process_packet(&packet);
    }
    controls
}

#[test]
fn recovery_at_250ms_enqueues_its_first_request_immediately() {
    let (mut runtime, mut server) = runtime_and_server();
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    request_resync(
        &mut world,
        &mut runtime,
        Duration::from_millis(250),
        "invalid checkpoint",
        ResyncReason::InvalidCheckpoint,
    );
    assert!(runtime.prediction.as_ref().unwrap().recovering);
    assert!(matches!(
        controls(&mut runtime, &mut server).as_slice(),
        [ClientControl::Resync {
            reason: ResyncReason::InvalidCheckpoint,
            ..
        }]
    ));
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.last_resync, Some(Duration::from_millis(250)));
    assert_eq!(
        session.resync_request,
        session::ResyncRequest::AwaitingWelcome
    );
}

fn polling_world(runtime: Runtime) -> World {
    let mut world = World::new();
    world.init_resource::<Time<Real>>();
    world.init_resource::<diagnostics::Telemetry>();
    world.init_resource::<DreamConnection>();
    world.init_resource::<DreamPreferences>();
    world.init_resource::<CapturedInput>();
    world.init_resource::<crate::identity::TravelerProfile>();
    crate::identity::select(
        &mut world,
        &runtime
            .prediction
            .as_ref()
            .unwrap()
            .model
            .welcome
            .player
            .to_string(),
    )
    .unwrap();
    world.insert_resource(DreamView(crate::offline_presentation(
        DreamSimulation::new(71, false).snapshot(),
    )));
    world.insert_non_send(runtime);
    world
}

fn poll_at(world: &mut World, millis: u64) {
    let mut time = world.resource_mut::<Time<Real>>();
    let delta = Duration::from_millis(millis) - time.elapsed();
    time.advance_by(delta);
    poll_network(world);
    assert!(
        world.get_non_send::<Runtime>().is_some(),
        "poll must retain the live connection"
    );
}

fn deliver_server_packets(world: &mut World, server: &mut RenetServer) {
    let mut runtime = world.get_non_send_mut::<Runtime>().unwrap();
    for packet in server.get_packets_to_send(runtime.client_id).unwrap() {
        runtime.renet.process_packet(&packet);
    }
}

fn receive_snapshot_and_poll(
    world: &mut World,
    server: &mut RenetServer,
    revision: u64,
    millis: u64,
) {
    {
        let runtime = world.get_non_send::<Runtime>().unwrap();
        let session = runtime.prediction.as_ref().unwrap();
        let state = tests::public_full_state(session, 100, revision);
        let bytes = encode_state(
            &StateFrame::Full(state),
            session.model.welcome.stream.epoch,
            revision as u32,
            session.model.welcome.frame_limit().unwrap(),
        )
        .unwrap();
        server.send_message(runtime.client_id, STATE_CHANNEL, bytes);
    }
    deliver_server_packets(world, server);
    poll_at(world, millis);
    assert_eq!(
        world.get_non_send::<Runtime>().unwrap().last_received,
        millis as f64 / 1000.0
    );
}

fn take_controls(world: &mut World, server: &mut RenetServer) -> Vec<ClientControl> {
    controls(&mut world.get_non_send_mut::<Runtime>().unwrap(), server)
}

fn resync_count(controls: &[ClientControl]) -> usize {
    controls
        .iter()
        .filter(|control| matches!(control, ClientControl::Resync { .. }))
        .count()
}

#[test]
fn throttled_recovery_is_serviced_by_poll_despite_continued_valid_snapshots() {
    let (mut runtime, mut server) = runtime_and_server();
    runtime.prediction.as_mut().unwrap().last_resync = Some(Duration::ZERO);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    request_resync(
        &mut world,
        &mut runtime,
        Duration::from_millis(250),
        "recovery inside throttle",
        ResyncReason::InvalidCheckpoint,
    );
    assert_eq!(
        runtime.prediction.as_ref().unwrap().resync_request,
        session::ResyncRequest::PendingEnqueue
    );
    assert_eq!(resync_count(&controls(&mut runtime, &mut server)), 0);
    let mut world = polling_world(runtime);
    for (revision, millis) in [(1, 500), (2, 750), (3, 999)] {
        receive_snapshot_and_poll(&mut world, &mut server, revision, millis);
        assert_eq!(resync_count(&take_controls(&mut world, &mut server)), 0);
        assert!(
            world
                .get_non_send::<Runtime>()
                .unwrap()
                .prediction
                .as_ref()
                .unwrap()
                .recovering
        );
    }
    receive_snapshot_and_poll(&mut world, &mut server, 4, 1000);
    assert_eq!(resync_count(&take_controls(&mut world, &mut server)), 1);
    let session = world
        .get_non_send::<Runtime>()
        .unwrap()
        .prediction
        .as_ref()
        .unwrap();
    assert_eq!(session.last_resync, Some(Duration::from_secs(1)));
    assert_eq!(
        session.resync_request,
        session::ResyncRequest::AwaitingWelcome
    );
    receive_snapshot_and_poll(&mut world, &mut server, 5, 1250);
    assert_eq!(resync_count(&take_controls(&mut world, &mut server)), 0);
}

#[test]
fn failed_encoding_keeps_recovery_pending_without_advancing_the_throttle() {
    let (mut runtime, mut server) = runtime_and_server();
    runtime.prediction.as_mut().unwrap().control_sequence = u32::MAX;
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    request_resync(
        &mut world,
        &mut runtime,
        Duration::from_millis(250),
        "cannot encode control",
        ResyncReason::InvalidCheckpoint,
    );
    let session = runtime.prediction.as_mut().unwrap();
    assert_eq!(session.last_resync, None);
    assert_eq!(
        session.resync_request,
        session::ResyncRequest::PendingEnqueue
    );
    session.control_sequence = 0;
    let mut world = polling_world(runtime);
    receive_snapshot_and_poll(&mut world, &mut server, 1, 300);
    assert_eq!(resync_count(&take_controls(&mut world, &mut server)), 1);
    assert_eq!(
        world
            .get_non_send::<Runtime>()
            .unwrap()
            .prediction
            .as_ref()
            .unwrap()
            .last_resync,
        Some(Duration::from_millis(300))
    );
}

#[test]
fn full_control_queue_retains_resync_until_poll_can_enqueue_it() {
    let (mut runtime, mut server) = runtime_and_server();
    let mut config = connection_config();
    config
        .client_channels_config
        .iter_mut()
        .find(|channel| channel.channel_id == CONTROL_CHANNEL)
        .unwrap()
        .max_memory_usage_bytes = 1024;
    runtime.renet = RenetClient::new(config);
    runtime.renet.set_connected();
    runtime.renet.send_message(CONTROL_CHANNEL, vec![0; 1024]);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    request_resync(
        &mut world,
        &mut runtime,
        Duration::from_millis(250),
        "control backpressure",
        ResyncReason::InvalidCheckpoint,
    );
    assert!(!runtime.renet.is_disconnected());
    assert_eq!(runtime.prediction.as_ref().unwrap().last_resync, None);
    assert_eq!(
        runtime.prediction.as_ref().unwrap().resync_request,
        session::ResyncRequest::PendingEnqueue
    );
    for packet in runtime.renet.get_packets_to_send() {
        server
            .process_packet_from(&packet, runtime.client_id)
            .unwrap();
    }
    assert_eq!(
        server
            .receive_message(runtime.client_id, CONTROL_CHANNEL)
            .unwrap()
            .len(),
        1024
    );
    for packet in server.get_packets_to_send(runtime.client_id).unwrap() {
        runtime.renet.process_packet(&packet);
    }
    assert_eq!(
        runtime.renet.channel_available_memory(CONTROL_CHANNEL),
        1024
    );
    let mut world = polling_world(runtime);
    receive_snapshot_and_poll(&mut world, &mut server, 1, 300);
    assert_eq!(resync_count(&take_controls(&mut world, &mut server)), 1);
}

#[test]
fn queued_resync_uses_reliable_retransmission_then_new_welcome_resumes_prediction() {
    let (mut runtime, mut server) = runtime_and_server();
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    request_resync(
        &mut world,
        &mut runtime,
        Duration::from_millis(250),
        "recover",
        ResyncReason::InvalidCheckpoint,
    );
    let sequence = runtime.prediction.as_ref().unwrap().control_sequence;
    assert!(!runtime.renet.get_packets_to_send().is_empty()); // drop the first transmission
    runtime
        .renet
        .update(engine_net::RELIABLE_RESEND_TIME + Duration::from_millis(20));
    request_resync(
        &mut world,
        &mut runtime,
        Duration::from_secs(2),
        "another fault while awaiting Welcome",
        ResyncReason::InvalidCheckpoint,
    );
    assert_eq!(
        runtime.prediction.as_ref().unwrap().control_sequence,
        sequence
    );
    assert_eq!(resync_count(&controls(&mut runtime, &mut server)), 1);

    let old_welcome = runtime.prediction.as_ref().unwrap().model.welcome.clone();
    let mut world = polling_world(runtime);
    let duplicate = encode_welcome(&old_welcome).unwrap();
    server.send_message(old_welcome.client.0, CONTROL_CHANNEL, duplicate);
    deliver_server_packets(&mut world, &mut server);
    poll_at(&mut world, 250);
    assert!(
        world
            .get_non_send::<Runtime>()
            .unwrap()
            .prediction
            .as_ref()
            .unwrap()
            .recovering
    );

    let mut welcome = old_welcome;
    welcome.stream.epoch = ConnectionEpoch(2);
    server.send_message(
        welcome.client.0,
        CONTROL_CHANNEL,
        encode_welcome(&welcome).unwrap(),
    );
    deliver_server_packets(&mut world, &mut server);
    poll_at(&mut world, 300);
    let session = world
        .get_non_send::<Runtime>()
        .unwrap()
        .prediction
        .as_ref()
        .unwrap();
    assert!(!session.recovering);
    assert!(
        !session.active,
        "replacement still requires decoded owner state and Active proof"
    );
    assert_eq!(session.last_resync, None);
    assert_eq!(session.resync_request, session::ResyncRequest::Idle);

    let (mut source, frames) = tests::fixture();
    for frame in frames {
        source.receive_state(&frame, Duration::ZERO).unwrap();
    }
    let mut group = tests::published_group_chunk(&source);
    group.publication.connection = welcome.stream.epoch;
    for state in &mut group.members {
        state.scope.connection = welcome.stream.epoch;
    }
    group.manifest = group.members.iter().map(|state| state.scope).collect();
    for frame in tests::group_frames(&welcome, group) {
        server.send_message(welcome.client.0, STATE_CHANNEL, frame);
    }
    server.send_message(
        welcome.client.0,
        CONTROL_CHANNEL,
        encode_control(
            &ServerControl::Active {
                owner_snapshot: SnapshotId(1),
                through: ServerTick(10),
            },
            welcome.stream.epoch,
            0,
            welcome.frame_limit().unwrap(),
        )
        .unwrap(),
    );
    deliver_server_packets(&mut world, &mut server);
    poll_at(&mut world, 350);
    let mut runtime = world.get_non_send_mut::<Runtime>().unwrap();
    let session = runtime.prediction.as_mut().unwrap();
    assert!(session.active && !session.recovering);
    session
        .observe_clock(TimeExchange {
            client_send: Duration::from_millis(350),
            server_receive: Duration::from_nanos(welcome.server_time_nanos),
            server_send: Duration::from_nanos(welcome.server_time_nanos),
            client_receive: Duration::from_millis(350),
        })
        .unwrap();
    // This fixture routes Renet packets directly, without a netcode handshake.
    // The real poll's idle UDP transport marks Renet connecting on each update.
    runtime.renet.set_connected();
    generate_commands(&mut runtime, Duration::from_millis(450)).unwrap();
    assert!(runtime.prediction.as_ref().unwrap().sequence.0 > 0);
}

use super::ingress::{DecodedInput, decode_input};
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_worker(
    shared: SharedNet,
    stop: Arc<AtomicBool>,
    mut input: IngressSender,
    ingress_epoch: Arc<AtomicU64>,
    output: Receiver<OutputBatch>,
    context: DebugContext,
    bridge: Option<ServerDebugBridgeHandle>,
    recorder: Option<DebugRecorderHandle>,
    conditioner: renet_cross::server_conditioner::ServerConditionerHandle,
) -> Result<(), String> {
    let mut server = RenetServer::new(renet::ConnectionConfig::default());
    let mut clients = HashMap::<u64, ClientNetState>::new();
    let mut history = ReplicationHistory::default();
    let started = Instant::now();
    let mut last_update = started;
    let mut last_transport_update = started;
    let mut send_clock = PacketSendClock::default();
    let mut pending_output = None;
    let mut ingress_backpressured = false;
    let mut admitted_epoch = 0;
    let mut metrics_epoch = None;
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
            while !stop.load(Ordering::Acquire) {
                let now = Instant::now();
                let dt = now.duration_since(last_update);
                last_update = now;
                server.update(dt);
                let epoch = ingress_epoch.load(Ordering::Acquire);
                refresh_ingress_budgets(&mut clients, &mut admitted_epoch, epoch);
                let available = input.available();
                if available == 0 && !ingress_backpressured {
                    log::warn!(
                        "network ingress ring full; pausing receive polling until ECS makes space"
                    );
                    if let Some(recorder) = &recorder {
                        recorder.event(
                            "network_ingress_backpressure",
                            None,
                            None,
                            format!("capacity={}", shared.max_clients * INPUTS_PER_CLIENT + 1),
                        );
                    }
                }
                ingress_backpressured = available == 0;
                if available > 0 {
                    // Backpressure stops receive polling off the simulation thread.
                    // Socket/Renet buffers remain bounded, including lifecycle events.
                    let mut transport = shared.transport.lock().map_err(|e| e.to_string())?;
                    let transport_dt = now.duration_since(last_transport_update);
                    last_transport_update = now;
                    if let Err(error) = transport.update(transport_dt, &mut server) {
                        log::warn!("transport update error: {error}");
                        if let Some(recorder) = &recorder {
                            recorder.event("transport_error", None, None, error.to_string());
                        }
                    }
                }
                // Leave reliable inputs in Renet's bounded receive channel under
                // backpressure. A slow simulation cannot grow the handoff queue.
                if available > 0 {
                    for _ in 0..available {
                        let Some(event) = server.get_event() else {
                            break;
                        };
                        match event {
                            ServerEvent::ClientConnected { client_id } => {
                                if shared.bootstrap.on_client_connected(client_id).is_ok() {
                                    clients.insert(client_id, ClientNetState::default());
                                    publish_input(&mut input, Ingress::Connected(client_id));
                                    if let Some(r) = &recorder {
                                        r.event(
                                            "client_connected",
                                            Some(client_id),
                                            None,
                                            String::new(),
                                        );
                                    }
                                } else {
                                    server.disconnect(client_id);
                                }
                            }
                            ServerEvent::ClientDisconnected { client_id, reason } => {
                                shared.bootstrap.on_client_disconnected(client_id);
                                clients.remove(&client_id);
                                publish_input(&mut input, Ingress::Disconnected(client_id));
                                if let Some(r) = &recorder {
                                    r.event(
                                        "client_disconnected",
                                        Some(client_id),
                                        None,
                                        reason.to_string(),
                                    );
                                }
                            }
                        }
                    }
                    receive_messages(&mut server, &mut clients, &mut input);
                    // Metrics are sampled once per fixed tick, not once per
                    // millisecond. If the ring is full, try on a later poll.
                    if metrics_epoch != Some(epoch) && input.available() > 0 {
                        let transport = shared.transport.lock().map_err(|e| e.to_string())?;
                        let mut ids = server.clients_id();
                        ids.sort_unstable();
                        let peers = ids
                            .into_iter()
                            .map(|id| {
                                (
                                    id,
                                    if transport.udp().client_addr(id).is_some() {
                                        "UDP"
                                    } else {
                                        "WebRTC"
                                    },
                                    server.network_info(id).ok(),
                                )
                            })
                            .collect();
                        drop(transport);
                        publish_input(&mut input, Ingress::Metrics(peers));
                        metrics_epoch = Some(epoch);
                    }
                }
                let mut produced_output = false;
                if let Some(batch) = coalesce_output(pending_output.take(), &output) {
                    produced_output = true;
                    for message in batch.messages {
                        match message {
                            Outbound::ToClient(id, message) => {
                                send_reliable(&mut server, id, &message)
                            }
                            Outbound::Broadcast(message) => {
                                for id in server.clients_id() {
                                    send_reliable(&mut server, id, &message);
                                }
                            }
                        }
                    }
                    if let Some(world) = batch.world {
                        super::egress::broadcast_world_deltas(
                            &mut server,
                            &world,
                            &mut clients,
                            &mut history,
                        );
                        if bridge.is_some() || recorder.is_some() {
                            let udp_clients = {
                                let transport =
                                    shared.transport.lock().map_err(|e| e.to_string())?;
                                server
                                    .clients_id()
                                    .into_iter()
                                    .filter(|id| transport.udp().client_addr(*id).is_some())
                                    .collect()
                            };
                            let frame = super::egress::build_server_debug_frame(
                                &server,
                                &udp_clients,
                                &world,
                                &clients,
                                &context,
                                batch.step_duration_ms,
                                &conditioner,
                            );
                            if let Some(bridge) = &bridge {
                                bridge.push_frame(frame.clone());
                            }
                            if let Some(recorder) = &recorder {
                                recorder.record_server_frame(frame);
                            }
                        }
                    }
                }
                if send_clock.should_flush(started.elapsed(), produced_output) {
                    shared
                        .transport
                        .lock()
                        .map_err(|e| e.to_string())?
                        .send_packets(&mut server);
                }
                // New ECS output wakes this wait immediately. The timeout keeps
                // receive/handshake polling independent from simulation progress.
                pending_output = match output.recv_timeout(TRANSPORT_POLL_INTERVAL) {
                    Ok(batch) => Some(batch),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
            }
            Ok(())
        }));
    // A transport panic can poison its mutex. Recover it only for teardown;
    // the failure is returned and surfaced by the ECS boundary.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut transport = shared
            .transport
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        transport.disconnect_all(&mut server);
    }));
    for id in clients.keys() {
        shared.bootstrap.on_client_disconnected(*id);
    }
    match result {
        Ok(result) => result,
        Err(payload) => Err(format!(
            "transport worker panicked: {}",
            panic_payload_to_string(payload.as_ref())
        )),
    }
}

/// A worker pause can leave several app frames waiting. Preserve every reliable
/// delivery, but encode only the newest world in this bounded receive pass.
fn coalesce_output(
    first: Option<OutputBatch>,
    output: &Receiver<OutputBatch>,
) -> Option<OutputBatch> {
    let mut pending = PendingOutput::default();
    for batch in first.into_iter().chain(output.try_iter()).take(QUEUE_TICKS) {
        pending.push(batch);
    }
    pending.take()
}

pub(super) fn send_reliable(server: &mut RenetServer, id: u64, message: &ReliableServerMessage) {
    let payload = game_shared::encode(message);
    if !server.can_send_message(id, DefaultChannel::ReliableOrdered, payload.len()) {
        log::warn!("disconnecting client {id}: reliable output retention exhausted");
        server.disconnect(id);
        return;
    }
    server.send_message(id, DefaultChannel::ReliableOrdered, payload);
}

const TRANSPORT_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Simulation output drives state sends. Only idle ACK/retransmit traffic uses
/// a timer; an unrelated packet clock must not hold a completed snapshot.
#[derive(Default)]
pub(crate) struct PacketSendClock {
    last_send: Option<Duration>,
    sent_output: bool,
}
impl PacketSendClock {
    pub(crate) fn should_flush(&mut self, now: Duration, produced_output: bool) -> bool {
        let interval = Duration::from_secs_f64(1.0 / game_shared::SERVER_TICK_HZ as f64);
        // Allow two receive polls of scheduling jitter for the next state. This
        // avoids an ACK-only send just before that state's channel wake-up.
        let grace = if self.sent_output {
            TRANSPORT_POLL_INTERVAL * 2
        } else {
            Duration::ZERO
        };
        if !produced_output
            && self
                .last_send
                .is_some_and(|last| now.saturating_sub(last) < interval + grace)
        {
            return false;
        }
        self.last_send = Some(now);
        self.sent_output = produced_output;
        true
    }
}

pub(crate) fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_owned();
    }

    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }

    "<non-string panic payload>".to_owned()
}

// The caller checks space before taking anything from Renet. There is only
// one producer, so a concurrent consumer can only increase that space.
fn publish_input(input: &mut IngressSender, event: Ingress) {
    assert!(
        input.try_send(event).is_ok(),
        "reserved ingress slot must remain available"
    );
}

fn refresh_ingress_budgets(
    clients: &mut HashMap<u64, ClientNetState>,
    admitted_epoch: &mut u64,
    epoch: u64,
) {
    if epoch != *admitted_epoch {
        for client in clients.values_mut() {
            client.ingress_counts = [0; 2];
        }
        *admitted_epoch = epoch;
    }
}

// Every transport poll can prepare commands. Quotas remain worker-owned and
// reset only when the simulation advances the fixed-tick admission epoch.
fn receive_messages(
    server: &mut RenetServer,
    clients: &mut HashMap<u64, ClientNetState>,
    input: &mut IngressSender,
) {
    // Reserve a finite range once rather than acquiring the consumer's index
    // for every message. Space freed during this pass is used on the next poll.
    let mut available = input.available();
    for id in server.clients_id() {
        let Some(state) = clients.get_mut(&id) else {
            continue;
        };
        'channels: for (reliable, limit) in [
            (true, RELIABLE_INPUT_LIMIT),
            (false, UNRELIABLE_INPUT_LIMIT),
        ] {
            let channel: u8 = if reliable {
                DefaultChannel::ReliableOrdered.into()
            } else {
                DefaultChannel::Unreliable.into()
            };
            let index = usize::from(!reliable);
            while state.ingress_counts[index] < limit {
                if available == 0 {
                    return;
                }
                let Some(bytes) = server.receive_message(id, channel) else {
                    break;
                };
                // Invalid traffic and ACKs also consume the peer's quota.
                state.ingress_counts[index] += 1;
                match decode_input(id, reliable, &bytes) {
                    DecodedInput::Command(command) => {
                        publish_input(input, command);
                        available -= 1;
                    }
                    DecodedInput::Ack(tick) => state.acknowledge(tick),
                    DecodedInput::Disconnect => {
                        server.disconnect(id);
                        break 'channels;
                    }
                    DecodedInput::Ignore => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_backlog_keeps_reliable_order_and_only_the_latest_world() {
        let sim = game_sim::Simulation::new();
        let batch = |tick: u32| {
            let mut world = sim.world_delta();
            world.tick = tick;
            OutputBatch {
                messages: vec![
                    Outbound::Broadcast(ReliableServerMessage::Event(
                        game_shared::ReliableGameEvent::WaveStarted { wave: tick },
                    )),
                    Outbound::ToClient(
                        u64::from(tick),
                        ReliableServerMessage::JoinSnapshot(sim.join_snapshot(u64::from(tick))),
                    ),
                ],
                world: Some(world),
                step_duration_ms: f64::from(tick),
            }
        };
        let (tx, rx) = mpsc::sync_channel(QUEUE_TICKS);
        // The worker already received the first batch before its pause. A full
        // channel can hold eight more, but one pass still handles only eight.
        let first = batch(1);
        for tick in 2..=QUEUE_TICKS as u32 {
            assert!(tx.try_send(batch(tick)).is_ok());
        }
        assert!(
            tx.try_send(OutputBatch {
                messages: vec![Outbound::Broadcast(ReliableServerMessage::Event(
                    game_shared::ReliableGameEvent::Victory,
                ))],
                ..default()
            })
            .is_ok()
        );

        let merged = coalesce_output(Some(first), &rx).unwrap();
        assert_eq!(merged.world.unwrap().tick, QUEUE_TICKS as u32);
        assert_eq!(merged.step_duration_ms, QUEUE_TICKS as f64);
        assert_eq!(merged.messages.len(), QUEUE_TICKS * 2);
        for (index, pair) in merged.messages.chunks_exact(2).enumerate() {
            let tick = index as u32 + 1;
            assert!(matches!(
                pair[0],
                Outbound::Broadcast(ReliableServerMessage::Event(
                    game_shared::ReliableGameEvent::WaveStarted { wave }
                )) if wave == tick
            ));
            assert!(matches!(
                &pair[1],
                Outbound::ToClient(id, ReliableServerMessage::JoinSnapshot(join))
                    if *id == u64::from(tick) && join.you == *id
            ));
        }

        // Reliable-only output survives the bounded pass and still requests a
        // send on the next poll, even though it has no new world snapshot.
        let remaining = coalesce_output(None, &rx).unwrap();
        assert!(remaining.world.is_none());
        assert!(matches!(
            remaining.messages.as_slice(),
            [Outbound::Broadcast(ReliableServerMessage::Event(
                game_shared::ReliableGameEvent::Victory
            ))]
        ));
        assert!(coalesce_output(None, &rx).is_none());
    }

    #[test]
    fn completed_state_does_not_wait_for_an_unrelated_send_slot() {
        let mut clock = PacketSendClock::default();
        assert!(clock.should_flush(Duration::ZERO, false));
        assert!(clock.should_flush(Duration::from_millis(1), true));
        assert!(!clock.should_flush(Duration::from_millis(2), false));
        // ACK/retransmit polling continues even if simulation produces nothing.
        assert!(clock.should_flush(Duration::from_millis(37), false));
        assert!(!clock.should_flush(Duration::from_millis(38), false));
        assert!(clock.should_flush(Duration::from_millis(71), false));
    }

    #[test]
    fn state_cadence_with_poll_jitter_does_not_duplicate_ack_only_sends() {
        let mut clock = PacketSendClock::default();
        let frames: Vec<_> = (0..30)
            .map(|tick| 2 + tick * 1000 / 30 + tick % 2)
            .collect();
        let mut state_sends = 0;
        let mut idle_sends = 0;
        for ms in 0..1000 {
            let state_ready = frames.contains(&ms);
            if clock.should_flush(Duration::from_millis(ms), state_ready) {
                if state_ready {
                    state_sends += 1;
                } else {
                    idle_sends += 1;
                }
            }
        }
        assert_eq!(state_sends, 30);
        assert_eq!(idle_sends, 1, "only the initial pre-state handshake flush");
    }

    fn connected_pair() -> (
        RenetServer,
        renet::RenetClient,
        HashMap<u64, ClientNetState>,
    ) {
        let mut server = RenetServer::new(renet::ConnectionConfig::default());
        server.add_connection(1);
        let mut client = renet::RenetClient::new(renet::ConnectionConfig::default());
        client.set_connected();
        (
            server,
            client,
            HashMap::from([(1, ClientNetState::default())]),
        )
    }

    fn action(seq: u32) -> game_shared::ClientAction {
        game_shared::ClientAction {
            match_epoch: 1,
            command: game_shared::ClientCommand::BasicAttack { seq },
            after_move_seq: None,
            view_tick: None,
        }
    }

    #[test]
    fn draining_the_ring_does_not_reset_fixed_tick_peer_quotas() {
        let (mut server, mut client, mut states) = connected_pair();
        for seq in 0..17 {
            client.send_message(
                DefaultChannel::ReliableOrdered,
                game_shared::encode(&action(seq)),
            );
        }
        for seq in 0..33 {
            client.send_message(
                DefaultChannel::Unreliable,
                game_shared::encode(&game_shared::ClientMoveBundle {
                    match_epoch: 1,
                    moves: vec![(seq, [1.0, 0.0])],
                    actions: vec![],
                }),
            );
        }
        for packet in client.get_packets_to_send() {
            server.process_packet_from(&packet, 1).unwrap();
        }
        let (mut sender, mut receiver) = ingress_channel(INPUTS_PER_CLIENT);
        receive_messages(&mut server, &mut states, &mut sender);
        assert_eq!(
            receiver.pending(),
            RELIABLE_INPUT_LIMIT + UNRELIABLE_INPUT_LIMIT
        );
        for seq in 0..16 {
            assert!(
                matches!(receiver.try_recv(), Some(Ingress::Action(1, a)) if a.command.seq() == seq)
            );
        }
        for _ in 0..32 {
            assert!(matches!(receiver.try_recv(), Some(Ingress::Moves(1, _))));
        }
        let mut epoch = 0;
        refresh_ingress_budgets(&mut states, &mut epoch, 0);
        receive_messages(&mut server, &mut states, &mut sender);
        assert_eq!(receiver.pending(), 0, "an outer frame cannot evade quotas");
        refresh_ingress_budgets(&mut states, &mut epoch, 1);
        let (mut sender, mut receiver) = ingress_channel(1);
        receive_messages(&mut server, &mut states, &mut sender);
        assert_eq!(receiver.pending(), 1, "bounded global capacity");
        assert_eq!(
            states[&1].ingress_counts,
            [1, 0],
            "full ring must leave input in Renet"
        );
        assert!(
            matches!(receiver.try_recv(), Some(Ingress::Action(1, a)) if a.command.seq() == 16)
        );
        receive_messages(&mut server, &mut states, &mut sender);
        assert!(matches!(receiver.try_recv(), Some(Ingress::Moves(1, _))));
    }

    #[test]
    fn invalid_messages_are_charged_to_the_admission_budget() {
        let (mut server, mut client, mut states) = connected_pair();
        for _ in 0..17 {
            client.send_message(DefaultChannel::ReliableOrdered, vec![255]);
        }
        for packet in client.get_packets_to_send() {
            server.process_packet_from(&packet, 1).unwrap();
        }
        let (mut sender, receiver) = ingress_channel(1);
        receive_messages(&mut server, &mut states, &mut sender);
        assert_eq!(receiver.pending(), 0);
        assert_eq!(states[&1].ingress_counts[0], RELIABLE_INPUT_LIMIT);
        assert!(
            server
                .receive_message(1, DefaultChannel::ReliableOrdered)
                .is_some()
        );
    }

    #[test]
    fn ack_stays_on_worker_and_invalid_reliable_move_disconnects() {
        let (mut server, mut client, mut states) = connected_pair();
        let mut history = ReplicationHistory::default();
        let mut sim = game_sim::Simulation::new();
        sim.add_player(1);
        sim.step();
        let world = sim.world_delta();
        history.record(&world);
        states.get_mut(&1).unwrap().sent(history.packet_for(None));
        client.send_message(
            DefaultChannel::Unreliable,
            game_shared::encode(&game_shared::ClientAck { tick: world.tick }),
        );
        for packet in client.get_packets_to_send() {
            server.process_packet_from(&packet, 1).unwrap();
        }
        let (mut sender, receiver) = ingress_channel(1);
        receive_messages(&mut server, &mut states, &mut sender);
        assert_eq!(states[&1].last_acked_tick, Some(world.tick));
        assert_eq!(receiver.pending(), 0);
        let mut invalid = action(1);
        invalid.command = game_shared::ClientCommand::Move {
            seq: 1,
            dir: [0.0, 0.0],
        };
        client.send_message(
            DefaultChannel::ReliableOrdered,
            game_shared::encode(&invalid),
        );
        for packet in client.get_packets_to_send() {
            server.process_packet_from(&packet, 1).unwrap();
        }
        receive_messages(&mut server, &mut states, &mut sender);
        assert!(!server.is_connected(1));
        assert_eq!(receiver.pending(), 0);
    }
}

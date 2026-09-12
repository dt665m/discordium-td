use super::*;
use engine_server::Authority;
use renet::RenetClient;
#[test]
fn receipt_ack_only_retires_exact_sent_endpoints_and_never_commits_commands() {
    let mut authority = DreamAuthority::new(7, false, 1);
    assert!(authority.admit(41));
    let mut server = RenetServer::new(connection_config());
    server.add_connection(41);
    authority
        .peers
        .get_mut(&41)
        .unwrap()
        .finalized_batches
        .issue(ServerTick(2))
        .unwrap();
    authority
        .peers
        .get_mut(&41)
        .unwrap()
        .finalized_batches
        .issue(ServerTick(4))
        .unwrap();
    for through in [ServerTick(3), ServerTick(999)] {
        assert!(
            authority
                .receive_control(
                    41,
                    live::ClientControl::FinalizedAck { through },
                    &mut server
                )
                .is_err()
        );
        assert_eq!(authority.peers[&41].finalized_batches.len(), 2);
    }
    authority
        .receive_control(
            41,
            live::ClientControl::FinalizedAck {
                through: ServerTick(4),
            },
            &mut server,
        )
        .unwrap();
    authority
        .receive_control(
            41,
            live::ClientControl::FinalizedAck {
                through: ServerTick(2),
            },
            &mut server,
        )
        .unwrap();
    assert!(authority.peers[&41].finalized_batches.is_empty());
    assert_eq!(authority.tick, ServerTick(0));
    assert_eq!(
        authority.peers[&41].inbox.finalized_through(),
        ServerTick(0)
    );
}
#[test]
fn sustained_small_control_messages_cannot_starve_finalization_or_outcomes_at_150ms_rtt() {
    let mut authority = DreamAuthority::new(7, false, 1);
    assert!(authority.admit(41));
    {
        let peer = authority.peers.get_mut(&41).unwrap();
        peer.active = true;
        peer.welcome_pending = false;
    }
    let welcome = authority.peers[&41].welcome.clone();
    let mut server = RenetServer::new(connection_config());
    server.add_connection(41);
    let mut client = RenetClient::new(connection_config());
    client.set_connected();
    let mut to_client = VecDeque::<(Duration, Vec<u8>)>::new();
    let mut to_server = VecDeque::<(Duration, Vec<u8>)>::new();
    let mut sequence = 0;
    let mut finalized = ServerTick(0);
    let mut outcome_keys = BTreeSet::new();
    let mut maximum_messages = 0;
    let mut maximum_lag = 0;
    let rate = TickRate::new(dreamwake_sim::TICK_HZ).unwrap();
    let mut previous = Duration::ZERO;
    for tick in 1..=1800 {
        let now = rate.deadline(ServerTick(tick)).unwrap();
        let delta = now - previous;
        previous = now;
        server.update(delta);
        client.update(delta);
        while to_client.front().is_some_and(|(at, _)| *at <= now) {
            client.process_packet(&to_client.pop_front().unwrap().1);
        }
        while to_server.front().is_some_and(|(at, _)| *at <= now) {
            server
                .process_packet_from(&to_server.pop_front().unwrap().1, 41)
                .unwrap();
        }
        let mut ack = None;
        let mut outcomes_ack = None;
        while let Some(bytes) = client.receive_message(wire::CONTROL_CHANNEL) {
            let control: live::ServerControl =
                live::decode_control(&bytes, welcome.stream.epoch, welcome.frame_limit().unwrap())
                    .unwrap();
            match control {
                live::ServerControl::Finalized {
                    through, receipts, ..
                } => {
                    assert!(receipts.as_slice().first().unwrap().tick.0 <= finalized.0 + 1);
                    finalized = finalized.max(through);
                    ack = Some(through);
                }
                live::ServerControl::ActionOutcomes { batch, records } => {
                    outcomes_ack = Some(batch);
                    for record in records.into_vec() {
                        outcome_keys.insert(record.key);
                    }
                }
                _ => {}
            }
        }
        for control in [
            ack.map(|through| live::ClientControl::FinalizedAck { through }),
            outcomes_ack.map(|through| live::ClientControl::OutcomesAck { through }),
        ]
        .into_iter()
        .flatten()
        {
            sequence += 1;
            client.send_message(
                wire::CONTROL_CHANNEL,
                live::encode_control(
                    &control,
                    welcome.stream.epoch,
                    sequence,
                    welcome.frame_limit().unwrap(),
                )
                .unwrap(),
            );
        }
        authority.receive(&mut server, now);
        authority.step(now);
        let key = ActionKey {
            connection: welcome.stream.epoch,
            stream: welcome.stream.stream,
            command: CommandSeq(tick),
            slot: 0,
        };
        authority
            .outcomes
            .normal
            .reserve(welcome.player.get(), &[key])
            .unwrap();
        authority
            .outcomes
            .normal
            .finish(
                welcome.player.get(),
                key,
                ServerTick(tick),
                outcomes::rejection(
                    live::TerminalStatus::Rejected,
                    live::OutcomeReason::Capacity,
                ),
            )
            .unwrap();
        for _ in 0..2 {
            publish::control(
                &mut server,
                41,
                authority.peers.get_mut(&41).unwrap(),
                &live::ServerControl::ClockReply {
                    tick: ServerTick(tick),
                    client_send_nanos: nanos(now),
                    server_receive_nanos: nanos(now),
                    server_send_nanos: nanos(now),
                },
            )
            .unwrap();
        }
        authority.publish(&mut server);
        let queue = server
            .channel_send_queue(41, wire::CONTROL_CHANNEL)
            .unwrap();
        maximum_messages = maximum_messages.max(queue.messages);
        assert!(queue.payload_bytes < 32 * 1024);
        assert!(authority.peers[&41].finalized_batches.len() <= 4);
        assert!(authority.peers[&41].outcome_batches.len() <= 4);
        if tick > 60 {
            maximum_lag = maximum_lag.max(tick - finalized.0);
        }
        for packet in server.get_packets_to_send(41).unwrap() {
            to_client.push_back((now + Duration::from_millis(75), packet));
        }
        for packet in client.get_packets_to_send() {
            to_server.push_back((now + Duration::from_millis(75), packet));
        }
    }
    assert!(
        maximum_messages > 4,
        "test must maintain unrelated controls above the old global gate"
    );
    assert!(
        maximum_lag <= 24,
        "bounded finalization lag, observed {maximum_lag} ticks"
    );
    assert!(finalized.0 > 1770);
    assert!(
        outcome_keys.len() > 1770,
        "outcomes progress independently of small control traffic"
    );
}

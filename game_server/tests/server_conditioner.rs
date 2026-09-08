//! End-to-end startup regression: no client-side conditioner is attached.
use std::{
    net::{TcpListener, UdpSocket},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct RunningServer(Child);
impl Drop for RunningServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn preconfigured_server_delay_affects_native_client_rtt() {
    run_conditioned_connection("100", "0", "0", false, false);
}

#[test]
fn delayed_action_executes_once_under_latency_jitter_loss_and_outage() {
    run_conditioned_connection("150", "25", "5", true, false);
}

#[test]
fn walking_attack_recovers_from_loss_without_reliable_delivery() {
    run_conditioned_connection("150", "25", "5", false, true);
}

fn run_conditioned_connection(
    delay: &str,
    jitter: &str,
    loss: &str,
    test_action: bool,
    walk_attack: bool,
) {
    // Reserve distinct ports together, then release them immediately before spawn.
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let rtc = UdpSocket::bind("127.0.0.1:0").unwrap();
    let http_addr = http.local_addr().unwrap().to_string();
    let udp_addr = udp.local_addr().unwrap().to_string();
    let rtc_addr = rtc.local_addr().unwrap().to_string();
    let base = format!("http://{http_addr}");
    drop((http, udp, rtc));
    let mut server = RunningServer(
        Command::new(env!("CARGO_BIN_EXE_game_server"))
            .args([
                "--http-bind",
                &http_addr,
                "--udp-bind",
                &udp_addr,
                "--webrtc-bind",
                &rtc_addr,
                "--public-udp-addr",
                &udp_addr,
                "--public-webrtc-addr",
                &rtc_addr,
                "--public-http-base",
                &base,
                "--net-delay-ms",
                delay,
                "--net-jitter-ms",
                jitter,
                "--net-loss-percent",
                loss,
            ])
            .env_remove("TD_HTTP_TLS_CERT")
            .env_remove("TD_HTTP_TLS_KEY")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let conditioner = renet_cross::conditioner::ConditionerHandle::default();
    let started = Instant::now();
    let (mut client, mut transport, _) = loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited before bootstrap"
        );
        match renet_cross::connect_via_session_http_blocking(
            &base,
            game_shared::PROTOCOL_ID,
            renet_cross::NativeConnectOptions {
                transport: renet_cross::ClientTransportConfig {
                    conditioner: test_action.then(|| conditioner.clone()),
                },
                ..Default::default()
            },
        ) {
            Ok(connection) => break connection,
            Err(error) => {
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "bootstrap failed: {error}"
                );
                thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let sampling_started = Instant::now();
    let mut last = sampling_started;
    let mut received_gameplay = false;
    let mut samples = Vec::new();
    let mut move_sent = false;
    let mut action_sent = false;
    let mut action_acked = false;
    let mut cast_events = 0;
    let mut expected_stop = None;
    let mut walk_confirmed = false;
    let mut last_input_send = Instant::now();
    while sampling_started.elapsed() < Duration::from_secs(if test_action { 8 } else { 5 }) {
        let now = Instant::now();
        let dt = now.duration_since(last);
        last = now;
        client.update(dt);
        transport.update(dt, &mut client).unwrap();
        while let Some(bytes) = client.receive_message(renet::DefaultChannel::ReliableOrdered) {
            match game_shared::decode(&bytes) {
                Ok(game_shared::ReliableServerMessage::JoinSnapshot(join)) => {
                    received_gameplay = true;
                    if walk_attack {
                        let mut predicted = game_sim::Simulation::from_snapshot(
                            &join.world,
                            &join.world.sim_meta.unwrap(),
                        );
                        // The six-move recovered burst retains the newest three;
                        // two walk ticks precede its stationary attack movement.
                        for seq in 1..game_shared::MAX_MOVEMENT_BACKLOG as u32 {
                            predicted.queue_command(
                                join.you,
                                game_shared::ClientCommand::Move {
                                    seq,
                                    dir: [1.0, 0.0],
                                },
                            );
                            predicted.step();
                        }
                        expected_stop = Some((
                            join.you,
                            predicted
                                .world_delta()
                                .heroes
                                .iter()
                                .find(|h| h.client_id == join.you)
                                .unwrap()
                                .pos,
                        ));
                    }
                }
                Ok(game_shared::ReliableServerMessage::Event(
                    game_shared::ReliableGameEvent::AbilityCast { .. },
                )) => cast_events += 1,
                _ => {}
            }
        }
        if test_action && received_gameplay && !move_sent {
            client.send_message(
                renet::DefaultChannel::Unreliable,
                game_shared::encode(&game_shared::ClientMoveBundle {
                    match_epoch: 0,
                    actions: vec![],
                    moves: vec![(101, [0.0, 0.0])],
                }),
            );
            move_sent = true;
        }
        while let Some(bytes) = client.receive_message(renet::DefaultChannel::Unreliable) {
            if let Ok(game_shared::ServerWorldMessage::Full(world)) = game_shared::decode(&bytes) {
                if test_action
                    && world
                        .heroes
                        .iter()
                        .any(|hero| hero.last_move_seq == Some(101))
                    && !action_sent
                {
                    conditioner.outage(Duration::from_secs(1));
                    let action = game_shared::ClientCommand::CastAbility {
                        seq: 100,
                        ability: game_shared::AbilityId::ArcBurst,
                    };
                    for _ in 0..2 {
                        client.send_message(
                            renet::DefaultChannel::ReliableOrdered,
                            game_shared::encode(&game_shared::ClientAction {
                                view_tick: None,
                                match_epoch: 0,
                                command: action,
                                after_move_seq: None,
                            }),
                        );
                    }
                    action_sent = true;
                }
                action_acked |= world.heroes.iter().any(|h| h.last_action_seq == Some(100));
                if let Some((id, stop)) = expected_stop {
                    if let Some(hero) = world
                        .heroes
                        .iter()
                        .find(|h| h.client_id == id && h.last_attack_seq == Some(6))
                    {
                        assert!(
                            game_shared::distance_sq(hero.pos, stop) < 0.0001,
                            "recovered attack violated bounded movement barrier: {:?} vs {:?}",
                            hero.pos,
                            stop
                        );
                        walk_confirmed = true;
                    }
                }
            }
        }
        // Retransmit the same movement identity until ACKed, as a redundant bundle does.
        if test_action && move_sent && !action_sent {
            client.send_message(
                renet::DefaultChannel::Unreliable,
                game_shared::encode(&game_shared::ClientMoveBundle {
                    match_epoch: 0,
                    actions: vec![],
                    moves: vec![(101, [0.0, 0.0])],
                }),
            );
        }
        if walk_attack
            && expected_stop.is_some()
            && !walk_confirmed
            && last_input_send.elapsed() >= Duration::from_secs_f64(1.0 / 30.0)
        {
            last_input_send = Instant::now();
            let mut moves = (1..=5).map(|seq| (seq, [1.0, 0.0])).collect::<Vec<_>>();
            moves.push((7, [1.0, 0.0]));
            client.send_message(
                renet::DefaultChannel::Unreliable,
                game_shared::encode(&game_shared::ClientMoveBundle {
                    match_epoch: 0,
                    moves,
                    actions: vec![game_shared::ClientAction {
                        view_tick: None,
                        match_epoch: 0,
                        command: game_shared::ClientCommand::BasicAttack { seq: 6 },
                        after_move_seq: Some(5),
                    }],
                }),
            );
        }
        transport.send_packets(&mut client).unwrap();
        if sampling_started.elapsed() > Duration::from_secs(3) && client.is_connected() {
            samples.push(client.rtt() * 1000.0);
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        client.is_connected(),
        "client disconnected: {:?}",
        client.disconnect_reason()
    );
    if walk_attack {
        assert!(walk_confirmed, "walk/attack did not complete");
    }
    if test_action {
        assert!(
            action_sent && action_acked,
            "delayed action was not acknowledged"
        );
        assert_eq!(
            cast_events, 1,
            "duplicated delayed action must execute exactly once"
        );
    }
    assert!(
        received_gameplay,
        "conditioned connection never delivered a join snapshot"
    );
    assert!(samples.len() > 10, "not enough connected RTT samples");
    samples.sort_by(f64::total_cmp);
    let median = samples[samples.len() / 2];
    assert!(
        median >= delay.parse::<f64>().unwrap() * 2.0 - 40.0 && median < 1500.0,
        "{delay} ms each-way server delay measured {median:.1} ms RTT"
    );
}

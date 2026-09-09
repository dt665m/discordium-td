#[path = "diagnostics.rs"]
mod diagnostics;
use bevy_net_debug::ConditionerDebug;
// Renet client, bounded local prediction, and authoritative snapshot reconciliation.
// Hosting launches the same dedicated server entry point; it never bypasses transport.
use super::{CapturedInput, DreamConnection, DreamPreferences, DreamView, Playtest};
use bevy::prelude::*;
use dreamwake_protocol::*;
use dreamwake_sim::{DreamInput, DreamSimulation, DreamSnapshot, RunPhase};
use renet::RenetClient;
use std::{collections::VecDeque, time::Duration};

const PREDICTION_MAX_SNAPSHOT_AGE: f64 = 0.30;

#[cfg(not(target_arch = "wasm32"))]
type Transport = renet_cross::UdpNetcodeClientTransport;
#[cfg(target_arch = "wasm32")]
type Transport = renet_cross::WebRtcNetcodeClientTransport;
type BootstrapResult = Result<(RenetClient, Transport, u64), String>;

#[derive(Resource)]
pub struct NetworkOptions {
    pub seed: u64,
    pub auto_connect: bool,
    pub auto_host: bool,
    pub expected_party: usize,
    #[cfg(not(target_arch = "wasm32"))]
    pub hosted: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub max_clients: u16,
}
#[derive(Clone)]
struct PendingFrame {
    seq: u32,
    sent_at: f64,
    input: DreamInput,
    dash_seq: Option<u32>,
    cast_seq: [Option<u32>; 4],
}
struct Runtime {
    renet: RenetClient,
    transport: Transport,
    client_id: u64,
    epoch: u32,
    revision: u64,
    input_seq: u32,
    action_seq: u32,
    pending: VecDeque<PendingFrame>,
    prediction: Option<DreamSimulation>,
    last_received: f64,
    last_send: f64,
    last_snapshot: Option<DreamSnapshot>,
    in_flight_action: Option<u32>,
}
#[cfg(not(target_arch = "wasm32"))]
struct PendingBootstrap(std::sync::mpsc::Receiver<BootstrapResult>);
#[cfg(target_arch = "wasm32")]
struct PendingBootstrap(std::rc::Rc<std::cell::RefCell<Option<BootstrapResult>>>);

pub struct DreamNetworkPlugin;
impl Plugin for DreamNetworkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<diagnostics::Telemetry>()
            .add_systems(
                Update,
                diagnostics::sample.before(engine_client::network_tools::graphs::update_graphs),
            )
            .add_systems(Startup, auto_connect)
            .add_systems(
                PreUpdate,
                poll_network.in_set(super::DreamInputSystems::Receive),
            )
            .add_systems(FixedUpdate, predict_and_send)
            .add_systems(PostUpdate, flush_network)
            .add_systems(Last, close_connection.after(bevy::window::ExitSystems));
    }
}
fn auto_connect(world: &mut World) {
    let options = world.resource::<NetworkOptions>();
    if options.auto_host {
        host(world);
    } else if options.auto_connect {
        connect(world, 1);
    }
}
pub fn connect(world: &mut World, attempts: u32) {
    disconnect_runtime(world);
    *world.resource_mut::<diagnostics::Telemetry>() = default();
    *world.resource_mut::<engine_client::network_tools::graphs::DiagnosticHistory>() = default();
    let conditioner = world.resource::<ConditionerDebug>().handle().clone();
    let base = world
        .resource::<DreamConnection>()
        .http_base
        .trim_end_matches('/')
        .to_owned();
    world.resource_mut::<DreamConnection>().status = format!("Connecting to {base}…");
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut result = Err("Could not reach server".to_owned());
            for attempt in 0..attempts.max(1) {
                result = renet_cross::connect_via_session_http_blocking(
                    &base,
                    PROTOCOL_ID,
                    renet_cross::NativeConnectOptions {
                        transport: renet_cross::ClientTransportConfig {
                            conditioner: Some(conditioner.clone()),
                        },
                        ..Default::default()
                    },
                )
                .map(|(_, transport, id)| (RenetClient::new(connection_config()), transport, id))
                .map_err(|e| e.to_string());
                if result.is_ok() {
                    break;
                }
                if attempt + 1 < attempts {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            let _ = sender.send(result);
        });
        world.insert_non_send(PendingBootstrap(receiver));
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = attempts;
        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let result_slot = slot.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = renet_cross::connect_via_sdp_http_with_options(
                &base,
                PROTOCOL_ID,
                renet_cross::WebRtcConnectOptions {
                    transport: renet_cross::ClientTransportConfig {
                        conditioner: Some(conditioner),
                    },
                    connection_config: connection_config(),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| e.to_string());
            *result_slot.borrow_mut() = Some(result);
        });
        world.insert_non_send(PendingBootstrap(slot));
    }
}

pub fn host(world: &mut World) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use clap::Parser;
        if !world.resource::<NetworkOptions>().hosted {
            let ip = std::net::UdpSocket::bind("0.0.0.0:0")
                .ok()
                .and_then(|socket| {
                    socket.connect("192.0.2.1:80").ok()?;
                    Some(socket.local_addr().ok()?.ip())
                })
                .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
            let seed = world.resource::<NetworkOptions>().seed;
            let args = dreamwake_server::ServerArgs::parse_from([
                "dreamwake-host".to_owned(),
                "--seed".into(),
                seed.to_string(),
                "--max-clients".into(),
                world.resource::<NetworkOptions>().max_clients.to_string(),
                "--http-bind".into(),
                "0.0.0.0:18082".into(),
                "--udp-bind".into(),
                "0.0.0.0:15002".into(),
                "--webrtc-bind".into(),
                "0.0.0.0:15003".into(),
                "--public-http-base".into(),
                format!("http://{ip}:18082"),
                "--public-udp-addr".into(),
                format!("{ip}:15002"),
                "--public-webrtc-addr".into(),
                format!("{ip}:15003"),
            ]);
            std::thread::Builder::new()
                .name("dreamwake-server".into())
                .spawn(move || {
                    if let Err(error) = dreamwake_server::run(args) {
                        log::error!("Dreamwake host failed: {error}");
                    }
                })
                .expect("could not launch Dreamwake server");
            world.resource_mut::<NetworkOptions>().hosted = true;
            world.resource_mut::<DreamConnection>().share_url = format!("http://{ip}:18082");
        }
        world.resource_mut::<DreamConnection>().http_base = "http://127.0.0.1:18082".into();
        connect(world, 40);
    }
    #[cfg(target_arch = "wasm32")]
    {
        world.resource_mut::<DreamConnection>().status =
            "Run the dedicated server, then join its address.".into();
    }
}
fn disconnect_runtime(world: &mut World) {
    world.remove_non_send::<PendingBootstrap>();
    world.resource_mut::<ConditionerDebug>().report_rtt(None);
    if let Some(mut runtime) = world.remove_non_send::<Runtime>() {
        runtime.renet.disconnect();
        let _ = runtime.transport.send_packets(&mut runtime.renet);
    }
    let mut connection = world.resource_mut::<DreamConnection>();
    connection.connected = false;
    connection.party_size = 0;
    connection.client_id = 0;
}
pub fn disconnect(world: &mut World) {
    disconnect_runtime(world);
    let seed = world.resource::<NetworkOptions>().seed;
    world.resource_mut::<DreamView>().0 = DreamSimulation::new(seed, false).snapshot();
    let mut prefs = world.resource_mut::<DreamPreferences>();
    prefs.paused = false;
    prefs.build_open = false;
    world.resource_mut::<DreamConnection>().status =
        "Disconnected. Host a dream or join your party again.".into();
}
pub fn send_action(world: &mut World, action: DreamAction) {
    if let Some(mut runtime) = world.get_non_send_mut::<Runtime>() {
        if runtime.epoch != 0 && runtime.renet.is_connected() {
            // UI commands wait for one authoritative acknowledgement so double clicks
            // cannot spend two rewards or skip two readiness gates.
            if matches!(action, DreamAction::Pause { .. }) {
                queue_action(&mut runtime, action);
            } else if runtime.in_flight_action.is_none() {
                let seq = queue_action(&mut runtime, action);
                runtime.in_flight_action = Some(seq);
            }
        }
    }
}
fn queue_action(runtime: &mut Runtime, action: DreamAction) -> u32 {
    runtime.action_seq = runtime.action_seq.wrapping_add(1);
    let seq = runtime.action_seq;
    runtime.renet.send_message(
        ACTION_CHANNEL,
        encode(&DreamClientMessage::Action {
            epoch: runtime.epoch,
            seq,
            action,
        }),
    );
    seq
}

fn poll_network(world: &mut World) {
    let result = world
        .get_non_send::<PendingBootstrap>()
        .and_then(|pending| {
            #[cfg(not(target_arch = "wasm32"))]
            {
                pending.0.try_recv().ok()
            }
            #[cfg(target_arch = "wasm32")]
            {
                pending.0.borrow_mut().take()
            }
        });
    if let Some(result) = result {
        world.remove_non_send::<PendingBootstrap>();
        match result {
            Ok((renet, transport, id)) => {
                let now = world.resource::<Time<Real>>().elapsed_secs_f64();
                world.insert_non_send(Runtime {
                    renet,
                    transport,
                    client_id: id,
                    epoch: 0,
                    revision: 0,
                    input_seq: 0,
                    action_seq: 0,
                    pending: VecDeque::new(),
                    prediction: None,
                    last_received: now,
                    last_send: 0.0,
                    last_snapshot: None,
                    in_flight_action: None,
                });
                world.resource_mut::<DreamConnection>().status = "Joining the shared dream…".into();
            }
            Err(error) => {
                world.resource_mut::<DreamConnection>().status =
                    format!("Could not connect: {error}");
            }
        }
    }
    let Some(mut runtime) = world.remove_non_send::<Runtime>() else {
        return;
    };
    let time = world.resource::<Time<Real>>();
    let now = time.elapsed_secs_f64();
    let duration = time.delta().min(Duration::from_millis(100));
    runtime.renet.update(duration);
    if let Err(error) = runtime.transport.update(duration, &mut runtime.renet) {
        world
            .resource_mut::<diagnostics::Telemetry>()
            .transport_errors += 1;
        world.resource_mut::<DreamConnection>().status = format!("Connection interrupted: {error}");
    }
    let mut newest: Option<(u32, u64, Option<u32>, Option<u32>, Box<DreamSnapshot>)> = None;
    for channel in [STATE_CHANNEL, CONTROL_CHANNEL] {
        for _ in 0..128 {
            let Some(bytes) = runtime.renet.receive_message(channel) else {
                break;
            };
            match decode::<DreamServerMessage>(&bytes) {
                Ok(DreamServerMessage::State {
                    epoch,
                    revision,
                    client_id,
                    host_id: _,
                    ack_input,
                    ack_action,
                    snapshot,
                }) => {
                    if client_id != runtime.client_id
                        || !is_new_state(epoch, revision, runtime.epoch, runtime.revision)
                    {
                        continue;
                    }
                    if !has_local_hero(&snapshot, client_id) {
                        continue;
                    }
                    if let Some((old_epoch, old_revision, ..)) = &newest {
                        if !is_new_state(epoch, revision, *old_epoch, *old_revision) {
                            continue;
                        }
                    }
                    newest = Some((epoch, revision, ack_input, ack_action, snapshot));
                }
                Ok(DreamServerMessage::Notice { text }) => {
                    world.resource_mut::<DreamConnection>().status = text
                }
                Err(error) => {
                    world.resource_mut::<diagnostics::Telemetry>().decode_errors += 1;
                    log::warn!("Ignored malformed Dreamwake server message: {error}");
                }
            }
        }
    }
    if let Some((epoch, revision, ack_input, ack_action, snapshot)) = newest {
        let changed_epoch = runtime.epoch != epoch;
        {
            let mut telemetry = world.resource_mut::<diagnostics::Telemetry>();
            if changed_epoch {
                telemetry.reset_samples();
            }
            telemetry.snapshots += 1;
            if !changed_epoch {
                if let Some(frame) = runtime.pending.iter().find(|f| Some(f.seq) == ack_input) {
                    telemetry.acknowledge(frame.seq, frame.sent_at, now);
                }
            }
        }
        if changed_epoch {
            runtime.pending.clear();
            runtime.input_seq = 0;
            runtime.action_seq = 0;
            runtime.in_flight_action = None;
            let mut prefs = world.resource_mut::<DreamPreferences>();
            prefs.paused = false;
            prefs.build_open = false;
        }
        runtime.epoch = epoch;
        runtime.revision = revision;
        runtime.last_received = now;
        if runtime
            .in_flight_action
            .is_some_and(|seq| !newer(seq, ack_action))
        {
            runtime.in_flight_action = None;
        }
        // An acknowledged movement may still contain an unacknowledged reliable cast.
        // Preserve its edge for replay but never replay acknowledged movement twice.
        runtime.pending.retain(|frame| {
            newer(frame.seq, ack_input)
                || frame.dash_seq.is_some_and(|seq| newer(seq, ack_action))
                || frame
                    .cast_seq
                    .iter()
                    .flatten()
                    .any(|seq| newer(*seq, ack_action))
        });
        let mut prediction = DreamSimulation::from_snapshot(&snapshot);
        let client_id = runtime.client_id;
        let mut replay_count = 0;
        engine_client::prediction::replay_bounded(
            &mut prediction,
            runtime.pending.iter(),
            18,
            |prediction, frame| {
                replay_count += 1;
                let mut input = frame.input;
                if !newer(frame.seq, ack_input) {
                    input.movement = [0.0; 2];
                    input.attack = false;
                }
                input.dash = frame.dash_seq.is_some_and(|seq| newer(seq, ack_action));
                input.casts = std::array::from_fn(|i| {
                    frame.cast_seq[i].is_some_and(|seq| newer(seq, ack_action))
                });
                prediction.step_multiplayer(&[(client_id, input)]);
            },
        );
        let displayed = presented_snapshot(prediction.snapshot_for(client_id), &snapshot);
        let shift = diagnostics::reconcile_shift(
            &world.resource::<DreamView>().0,
            &displayed,
            changed_epoch,
            client_id,
        );
        world
            .resource_mut::<diagnostics::Telemetry>()
            .record_reconciliation(replay_count, shift, now);
        runtime.last_snapshot = Some(*snapshot);
        runtime.prediction = Some(prediction);
        let mut conn = world.resource_mut::<DreamConnection>();
        if !conn.connected || conn.status == "Waiting for server — prediction paused" {
            conn.status = "Connected to the shared dream".into();
        }
        conn.connected = true;
        conn.client_id = client_id;
        conn.party_size = displayed.heroes.len();
        conn.rtt_ms = runtime.renet.rtt() * 1000.0;
        world.resource_mut::<DreamView>().0 = displayed;
    }
    if runtime.renet.is_disconnected() || now - runtime.last_received > 12.0 {
        let reason = runtime
            .renet
            .disconnect_reason()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "Server stopped responding".into());
        world.resource_mut::<DreamConnection>().connected = false;
        world.resource_mut::<DreamConnection>().status =
            format!("Disconnected: {reason}. Join again to reconnect.");
        return;
    }
    if now - runtime.last_received > 0.5 && runtime.epoch > 0 {
        world.resource_mut::<DreamConnection>().status =
            "Waiting for server — prediction paused".into();
    }
    world.insert_non_send(runtime);
}

fn predict_and_send(world: &mut World) {
    playtest_actions(world);
    let Some(mut runtime) = world.remove_non_send::<Runtime>() else {
        return;
    };
    if !runtime.renet.is_connected() || runtime.epoch == 0 {
        world.insert_non_send(runtime);
        return;
    }
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    let mut input = world.resource::<CapturedInput>().0;
    let prefs = world.resource::<DreamPreferences>();
    if prefs.paused || prefs.build_open {
        input = DreamInput::default();
    }
    runtime.input_seq = runtime.input_seq.wrapping_add(1);
    let seq = runtime.input_seq;
    let dash_seq = input.dash.then(|| {
        queue_action(
            &mut runtime,
            DreamAction::Dash {
                direction: if input.movement == [0.0; 2] {
                    input.aim
                } else {
                    input.movement
                },
            },
        )
    });
    let cast_seq = std::array::from_fn(|i| {
        input.casts[i].then(|| {
            queue_action(
                &mut runtime,
                DreamAction::Cast {
                    slot: i as u8,
                    aim: input.aim,
                },
            )
        })
    });
    input.action_sequences[0] = dash_seq.unwrap_or_default();
    for i in 0..4 {
        input.action_sequences[i + 1] = cast_seq[i].unwrap_or_default();
    }
    let mut held = input;
    held.dash = false;
    held.casts = [false; 4];
    held.action_sequences = [0; 5];
    runtime.renet.send_message(
        INPUT_CHANNEL,
        encode(&DreamClientMessage::Input {
            epoch: runtime.epoch,
            seq,
            input: held,
        }),
    );
    runtime.pending.push_back(PendingFrame {
        seq,
        sent_at: now,
        input,
        dash_seq,
        cast_seq,
    });
    while runtime.pending.len() > 30 {
        runtime.pending.pop_front();
    }
    let client_id = runtime.client_id;
    // Stop speculative simulation during an outage. Inputs still flow for recovery.
    if now - runtime.last_received <= PREDICTION_MAX_SNAPSHOT_AGE {
        if let Some(prediction) = &mut runtime.prediction {
            prediction.step_multiplayer(&[(client_id, input)]);
            let mut shown = prediction.snapshot_for(client_id);
            if let Some(authority) = &runtime.last_snapshot {
                shown = presented_snapshot(shown, authority);
            }
            world.resource_mut::<DreamView>().0 = shown;
        }
    }
    let mut captured = world.resource_mut::<CapturedInput>();
    captured.0.dash = false;
    captured.0.casts = [false; 4];
    world.insert_non_send(runtime);
}
/// Only the server opens rewards, changes rooms, or declares victory/defeat.
/// Pose, cooldowns and transient effects remain responsive through shared prediction.
fn presented_snapshot(mut predicted: DreamSnapshot, authority: &DreamSnapshot) -> DreamSnapshot {
    predicted.phase = authority.phase;
    predicted.rewards = authority.rewards.clone();
    predicted.ready = authority.ready;
    predicted.awaiting_party = authority.awaiting_party;
    predicted.room = authority.room;
    predicted.realm = authority.realm;
    predicted.encounter_name = authority.encounter_name.clone();
    predicted.cleared = authority.cleared;
    predicted.kills = authority.kills;
    predicted.enemies_remaining = authority.enemies_remaining;
    predicted.message = authority.message.clone();
    predicted.paused = authority.paused;
    predicted.hero.shards = authority.hero.shards;
    predicted.hero.level = authority.hero.level;
    predicted.hero.xp = authority.hero.xp;
    predicted.hero.xp_next = authority.hero.xp_next;
    // Remote teammates are interpolated from authority rather than being driven by
    // our local input buffer. Their authoritative state is already in the rollback world.
    for hero in &mut predicted.heroes {
        if hero.id == predicted.hero.id {
            *hero = predicted.hero.clone();
        } else {
            if let Some(confirmed) = authority.heroes.iter().find(|h| h.id == hero.id) {
                *hero = confirmed.clone();
            }
        }
    }
    predicted
}

fn is_new_state(epoch: u32, revision: u64, old_epoch: u32, old_revision: u64) -> bool {
    if old_epoch == 0 {
        return true;
    }
    if epoch == old_epoch {
        revision != old_revision && revision.wrapping_sub(old_revision) < (1_u64 << 63)
    } else {
        newer(epoch, Some(old_epoch))
    }
}

fn has_local_hero(snapshot: &DreamSnapshot, id: u64) -> bool {
    snapshot.hero.id == id && snapshot.heroes.iter().any(|hero| hero.id == id)
}

fn playtest_actions(world: &mut World) {
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    let test = world.resource::<Playtest>();
    if !test.autoplay || now < test.next_decision {
        return;
    }
    let manual_rewards = test.manual_rewards;
    let connection = world.resource::<DreamConnection>();
    if !connection.connected
        || connection.party_size < world.resource::<NetworkOptions>().expected_party
    {
        return;
    }
    let view = &world.resource::<DreamView>().0;
    let action = match view.phase {
        RunPhase::Intro if !view.ready => Some(DreamAction::Start { lucid: false }),
        RunPhase::Reward | RunPhase::Rest
            if !manual_rewards && !view.awaiting_party && !view.rewards.is_empty() =>
        {
            Some(DreamAction::Choose {
                choice: ((view.room + view.hero.level as usize) % view.rewards.len()) as u8,
                slot: (view.room % 4) as u8,
            })
        }
        RunPhase::Transition if !view.ready => Some(DreamAction::Continue),
        _ => None,
    };
    if let Some(action) = action {
        send_action(world, action);
        world.resource_mut::<Playtest>().next_decision = now + 1.1;
    }
}
fn flush_network(
    time: Res<Time<Real>>,
    runtime: Option<NonSendMut<Runtime>>,
    mut telemetry: ResMut<diagnostics::Telemetry>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };
    if time.elapsed_secs_f64() - runtime.last_send < 1.0 / 60.0 {
        return;
    }
    runtime.last_send = time.elapsed_secs_f64();
    let Runtime {
        transport, renet, ..
    } = &mut *runtime;
    if let Err(error) = transport.send_packets(renet) {
        telemetry.transport_errors += 1;
        log::warn!("Dreamwake packet flush: {error}");
    }
}
fn close_connection(mut exits: MessageReader<AppExit>, runtime: Option<NonSendMut<Runtime>>) {
    if exits.read().next().is_none() {
        return;
    }
    if let Some(mut runtime) = runtime {
        runtime.renet.disconnect();
        let Runtime {
            transport, renet, ..
        } = &mut *runtime;
        let _ = transport.send_packets(renet);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn predicted_combat_cannot_open_rewards_or_replace_authoritative_progress() {
        let mut sim = DreamSimulation::new(13, false);
        sim.continue_run_for(1);
        let authority = sim.snapshot_for(1);
        let mut predicted = authority.clone();
        predicted.phase = RunPhase::Victory;
        predicted.room = 9;
        predicted.hero.position = [4.0, 3.0];
        predicted.hero.shards = 999;
        predicted.hero.level = 99;
        let shown = presented_snapshot(predicted, &authority);
        assert_eq!(shown.phase, RunPhase::Combat);
        assert_eq!(shown.room, authority.room);
        assert_eq!(shown.hero.shards, authority.hero.shards);
        assert_eq!(shown.hero.level, authority.hero.level);
        assert_eq!(shown.hero.position, [4.0, 3.0]);
        assert_eq!(
            shown.heroes.iter().find(|h| h.id == 1).unwrap(),
            &shown.hero
        );
    }
    #[test]
    fn missing_local_prediction_cannot_fall_back_to_another_or_stale_hero() {
        let mut sim = DreamSimulation::new(19, false);
        sim.add_player(2);
        assert!(has_local_hero(&sim.snapshot_for(2), 2));
        sim.remove_player(2);
        assert!(!has_local_hero(&sim.snapshot_for(2), 2));
        assert!(!has_local_hero(&sim.snapshot_for(1), 2));
    }
    #[test]
    fn a_batch_cannot_overwrite_a_new_run_with_an_old_epoch() {
        assert!(is_new_state(2, 1, 1, 900));
        assert!(!is_new_state(1, 901, 2, 1));
        assert!(!is_new_state(2, 1, 2, 1));
        assert!(is_new_state(2, 2, 2, 1));
        assert!(is_new_state(1, 1, u32::MAX, 100));
        assert!(is_new_state(2, 0, 2, u64::MAX));
    }
}

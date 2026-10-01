use crate::plugins::diagnostics::capture as diagnostics;
use bevy_net_debug::ConditionerDebug;
// Renet client, bounded local prediction, and authoritative snapshot reconciliation.
// Hosting launches the same dedicated server entry point; it never bypasses transport.
use crate::plugins::{
    diagnostics::Playtest,
    input::{CapturedActions, CapturedInput},
};
use crate::{DreamConnection, DreamPreferences, DreamView};
use bevy::prelude::*;
use dreamwake_protocol::live::*;
use dreamwake_protocol::*;
use dreamwake_sim::{DreamInput, DreamSimulation, RunPhase, replication::DreamPresentation};
use engine_net::{
    codec::BoundedVec,
    commands::{ActionEdge, Command},
    input::HardwareSample,
    prediction::rewind::RewindInput,
    synchronization::{SyncError, TimeExchange},
    types::*,
};
mod model;
mod outcomes;
pub(crate) mod predicted_events;
mod presentation;
mod session;
use model::InputCommand;
pub(crate) use presentation::RemotePoseMetrics;
use renet::RenetClient;
use session::Session;
pub(crate) use session::SessionMetrics;
use std::{collections::VecDeque, time::Duration};

pub(crate) const PREDICTION_MAX_SNAPSHOT_AGE: f64 = 0.30;

/// Which network path a native client takes to the server. Browsers only have
/// WebRTC; native clients default to UDP but can take the browser's path, so
/// that path can be exercised and diagnosed without a browser.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativeTransportKind {
    #[default]
    Udp,
    WebRtc,
}
#[cfg(not(target_arch = "wasm32"))]
impl std::str::FromStr for NativeTransportKind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "udp" => Ok(Self::Udp),
            "webrtc" => Ok(Self::WebRtc),
            other => Err(format!(
                "unknown transport '{other}'; expected udp or webrtc"
            )),
        }
    }
}
/// Both native paths behind one surface, so the session above never branches
/// on how its bytes travel.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) enum Transport {
    Udp(renet_cross::UdpNetcodeClientTransport),
    WebRtc(renet_cross::NativeWebRtcNetcodeClientTransport),
}
#[cfg(not(target_arch = "wasm32"))]
impl Transport {
    /// Recorded in traces so a run can prove which path it exercised.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Udp(_) => "udp",
            Self::WebRtc(_) => "webrtc",
        }
    }
    pub(crate) fn update(
        &mut self,
        duration: Duration,
        renet: &mut RenetClient,
    ) -> Result<(), renet_cross::NativeClientError> {
        match self {
            Self::Udp(transport) => transport.update(duration, renet),
            Self::WebRtc(transport) => transport.update(duration, renet),
        }
    }
    pub(crate) fn send_packets(
        &mut self,
        renet: &mut RenetClient,
    ) -> Result<(), renet_cross::NativeClientError> {
        match self {
            Self::Udp(transport) => transport.send_packets(renet),
            Self::WebRtc(transport) => transport.send_packets(renet),
        }
    }
}
#[cfg(target_arch = "wasm32")]
type Transport = renet_cross::WebRtcNetcodeClientTransport;
type BootstrapResult = Result<(RenetClient, Transport, u64), String>;

#[derive(Resource, Default)]
struct PendingConnect(Option<u32>);

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
    #[cfg(not(target_arch = "wasm32"))]
    pub transport: NativeTransportKind,
    /// Impairs the WebRTC path's datagrams beneath DTLS and SCTP.
    #[cfg(not(target_arch = "wasm32"))]
    pub link_impairment: Option<renet_cross::LinkImpairment>,
}
/// `DELAY_MS:JITTER_MS:LOSS_PERCENT`, applied in each direction.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_link_impairment(
    value: &str,
    seed: u64,
) -> Result<renet_cross::LinkImpairment, String> {
    let parts: Vec<_> = value.split(':').collect();
    let [delay, jitter, loss] = parts.as_slice() else {
        return Err(format!(
            "link impairment '{value}' must be DELAY_MS:JITTER_MS:LOSS_PERCENT"
        ));
    };
    let millis = |text: &str| {
        text.parse::<u64>()
            .map(Duration::from_millis)
            .map_err(|_| format!("'{text}' is not whole milliseconds"))
    };
    let loss = loss
        .parse::<f64>()
        .ok()
        .filter(|percent| (0.0..=100.0).contains(percent))
        .ok_or_else(|| format!("loss '{loss}' is not a percentage"))?;
    Ok(renet_cross::LinkImpairment {
        delay: millis(delay)?,
        jitter: millis(jitter)?,
        loss: loss / 100.0,
        seed,
    })
}
#[derive(Clone)]
pub(crate) struct PendingFrame {
    command: Command<TickInput, DreamAction>,
    /// First transmission, for acknowledgement latency.
    sent_at: f64,
}
pub(crate) struct Runtime {
    pub(crate) renet: RenetClient,
    pub(crate) transport: Transport,
    client_id: u64,
    action_seq: CommandSeq,
    pub(crate) pending: VecDeque<PendingFrame>,
    pub(crate) prediction: Option<Session>,
    pub(crate) last_received: f64,
    pub(crate) last_snapshot: Option<DreamPresentation>,
    in_flight_action: Option<CommandSeq>,
    pub(crate) outcomes: outcomes::ClientOutcomes,
}
impl Runtime {
    /// Called at the hardware edge, before journaling or command catch-up.
    pub(crate) fn capture_combat_view(&mut self, at: Duration) -> Option<CombatViewStamp> {
        self.prediction.as_mut()?.capture_combat_view(at)
    }
}
#[derive(Debug, Clone)]
pub(crate) struct RuntimeMetrics {
    pub transport_connected: bool,
    pub pending_commands: usize,
    pub last_received_seconds: f64,
    pub session: Option<SessionMetrics>,
}
impl Runtime {
    pub(crate) fn metrics(&self) -> RuntimeMetrics {
        RuntimeMetrics {
            transport_connected: self.renet.is_connected(),
            pending_commands: self.pending.len(),
            last_received_seconds: self.last_received,
            session: self.prediction.as_ref().map(Session::metrics),
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
struct PendingBootstrap(std::sync::mpsc::Receiver<BootstrapResult>);
#[cfg(target_arch = "wasm32")]
struct PendingBootstrap(std::rc::Rc<std::cell::RefCell<Option<BootstrapResult>>>);

pub struct DreamNetworkPlugin;
impl Plugin for DreamNetworkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingConnect>()
            .init_resource::<outcomes::ClientOutcomes>()
            .add_systems(Startup, auto_connect)
            .add_systems(
                PreUpdate,
                (
                    crate::identity::poll,
                    connect_when_profile_ready,
                    poll_network,
                )
                    .chain()
                    .in_set(crate::plugins::input::DreamInputSystems::Receive),
            )
            .add_systems(
                PreUpdate,
                journal_hardware.after(crate::plugins::input::DreamInputSystems::Capture),
            )
            .add_systems(FixedUpdate, predict_and_send)
            .add_systems(PostUpdate, flush_network)
            .add_systems(Last, close_connection.after(bevy::window::ExitSystems));
    }
}
fn connect_when_profile_ready(world: &mut World) {
    if world
        .resource::<crate::identity::TravelerProfile>()
        .is_pending()
    {
        return;
    }
    let Some(attempts) = world.resource_mut::<PendingConnect>().0.take() else {
        return;
    };
    if let Some(error) = world
        .resource::<crate::identity::TravelerProfile>()
        .storage_error()
    {
        let error = error.to_owned();
        world.resource_mut::<DreamConnection>().status = error;
        return;
    }
    connect(world, attempts);
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
    let requested_server = world.resource::<DreamConnection>().http_base.clone();
    if let Err(error) = world
        .resource_mut::<crate::identity::TravelerProfile>()
        .use_server(&requested_server)
    {
        world.resource_mut::<DreamConnection>().status = error;
        return;
    }
    if world
        .resource::<crate::identity::TravelerProfile>()
        .is_pending()
    {
        world.resource_mut::<PendingConnect>().0 = Some(attempts);
        world.resource_mut::<DreamConnection>().status = "Saving your Traveler profile…".into();
        return;
    }
    let credential = match world
        .resource_mut::<crate::identity::TravelerProfile>()
        .lock()
    {
        Ok(credential) => credential,
        Err(error) => {
            world.resource_mut::<DreamConnection>().status = error;
            return;
        }
    };
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
        let kind = world.resource::<NetworkOptions>().transport;
        let link = world.resource::<NetworkOptions>().link_impairment;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut result = Err("Could not reach server".to_owned());
            for attempt in 0..attempts.max(1) {
                let transport = renet_cross::ClientTransportConfig {
                    conditioner: Some(conditioner.clone()),
                };
                result = match kind {
                    NativeTransportKind::Udp => renet_cross::connect_via_session_http_blocking(
                        &base,
                        PROTOCOL_ID,
                        renet_cross::NativeConnectOptions {
                            session_request: player_session_request(credential.clone()),
                            require_secure: true,
                            connection_config: connection_config(),
                            transport,
                            ..Default::default()
                        },
                    )
                    .map(|(renet, transport, id)| (renet, Transport::Udp(transport), id)),
                    NativeTransportKind::WebRtc => {
                        renet_cross::connect_webrtc_via_session_http_blocking(
                            &base,
                            PROTOCOL_ID,
                            renet_cross::NativeWebRtcConnectOptions {
                                session_request: player_session_request(credential.clone()),
                                require_secure: true,
                                connection_config: connection_config(),
                                transport,
                                ..Default::default()
                            },
                        )
                        .map(|(renet, mut transport, id)| {
                            transport.set_link_impairment(link);
                            (renet, Transport::WebRtc(transport), id)
                        })
                    }
                }
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
                    session_request: player_session_request(credential),
                    require_secure: true,
                    transport: renet_cross::ClientTransportConfig {
                        conditioner: Some(conditioner),
                    },
                    connection_config: connection_config(),
                    // Headroom for the burst of channel callbacks that lands
                    // when a blocked main thread resumes, which arrives before
                    // the next frame can drain the inbox. Overflow retains the
                    // freshest packets, so this only widens the window; it is
                    // not a substitute for the transport keeping its own queue.
                    max_inbox_packets: 1024,
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
                "--allow-insecure-bootstrap-http".into(),
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

fn player_session_request(credential: String) -> renet_cross::SessionCreateRequest {
    renet_cross::SessionCreateRequest {
        protocol_id: Some(PROTOCOL_ID),
        service: SESSION_SERVICE.into(),
        match_id: SESSION_MATCH.into(),
        require_secure: true,
        credential,
        ..Default::default()
    }
}
fn disconnect_runtime(world: &mut World) {
    world.remove_non_send::<PendingBootstrap>();
    if let Some(mut pending) = world.get_resource_mut::<PendingConnect>() {
        pending.0 = None;
    }
    if let Some(mut profile) = world.get_resource_mut::<crate::identity::TravelerProfile>() {
        profile.unlock();
    }
    world.resource_mut::<ConditionerDebug>().report_rtt(None);
    if let Some(mut runtime) = world.remove_non_send::<Runtime>() {
        runtime.renet.disconnect();
        let _ = runtime.transport.send_packets(&mut runtime.renet);
        world.insert_resource(runtime.outcomes);
    }
    let mut connection = world.resource_mut::<DreamConnection>();
    connection.connected = false;
    connection.party_size = 0;
    connection.client_id = 0;
    connection.player_id = 0;
}
pub fn disconnect(world: &mut World) {
    disconnect_runtime(world);
    let seed = world.resource::<NetworkOptions>().seed;
    world.resource_mut::<DreamView>().0 =
        crate::offline_presentation(DreamSimulation::new(seed, false).snapshot());
    let mut prefs = world.resource_mut::<DreamPreferences>();
    prefs.paused = false;
    prefs.build_open = false;
    world.resource_mut::<DreamConnection>().status =
        "Disconnected. Host a dream or join your party again.".into();
}
pub fn send_action(world: &mut World, action: DreamAction) {
    if matches!(action, DreamAction::Cast { .. } | DreamAction::Dash { .. }) {
        return;
    }
    let Some(mut runtime) = world.get_non_send_mut::<Runtime>() else {
        return;
    };
    if !runtime.renet.is_connected() || !runtime.prediction.as_ref().is_some_and(|s| s.active) {
        return;
    }
    if runtime.in_flight_action.is_some() && !matches!(action, DreamAction::Pause { .. }) {
        return;
    }
    let Some(sequence) = runtime.action_seq.checked_next() else {
        runtime.renet.disconnect();
        return;
    };
    if send_control(&mut runtime, ClientControl::Menu { sequence, action }).is_ok() {
        runtime.action_seq = sequence;
        runtime.in_flight_action = Some(sequence);
    }
}
fn send_control(runtime: &mut Runtime, message: ClientControl) -> Result<(), String> {
    let bytes = runtime
        .prediction
        .as_mut()
        .ok_or("no session")?
        .control(&message)?;
    runtime.renet.send_message(CONTROL_CHANNEL, bytes);
    Ok(())
}
/// The time network work actually runs, on the frame clock's timeline.
///
/// The frame clock can trail the wall clock by a whole frame: with pipelined
/// rendering Bevy stamps a frame with the previous render's instant, so the
/// first frame after a hitch still reads the time from before it. Commands
/// authored from that clock target ticks the server has already simulated,
/// and are then corrected. Deadlines use when the work runs instead.
pub(crate) fn network_now(time: &Time<Real>) -> Duration {
    time.elapsed()
        + time
            .last_update()
            .map_or(Duration::ZERO, |last| last.elapsed())
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
                let now = network_now(world.resource::<Time<Real>>()).as_secs_f64();
                let outcomes = world
                    .remove_resource::<outcomes::ClientOutcomes>()
                    .unwrap_or_default();
                world.insert_non_send(Runtime {
                    renet,
                    transport,
                    client_id: id,
                    action_seq: CommandSeq(0),
                    pending: VecDeque::new(),
                    prediction: None,
                    last_received: now,
                    last_snapshot: None,
                    in_flight_action: None,
                    outcomes,
                });
                world.resource_mut::<DreamConnection>().status = "Joining the shared dream…".into();
            }
            Err(error) => {
                world
                    .resource_mut::<crate::identity::TravelerProfile>()
                    .unlock();
                world.resource_mut::<DreamConnection>().status =
                    format!("Could not connect: {error}")
            }
        }
    }
    let Some(mut runtime) = world.remove_non_send::<Runtime>() else {
        return;
    };
    let now = network_now(world.resource::<Time<Real>>());
    let duration = world
        .resource::<Time<Real>>()
        .delta()
        .min(Duration::from_millis(100));
    runtime.renet.update(duration);
    let transport_error = runtime
        .transport
        .update(duration, &mut runtime.renet)
        .err()
        .map(|error| error.to_string());
    if let Some(error) = &transport_error {
        world
            .resource_mut::<diagnostics::Telemetry>()
            .transport_errors += 1;
        world
            .resource_mut::<diagnostics::Telemetry>()
            .record_issue(diagnostics::IssueKind::Transport, error);
        world.resource_mut::<DreamConnection>().status = format!("Connection interrupted: {error}");
    }
    if let Some(session) = &mut runtime.prediction {
        session.begin_frame();
    }
    // Reliable controls precede the state lane in this batch.
    for channel in [CONTROL_CHANNEL, STATE_CHANNEL] {
        for _ in 0..256 {
            let Some(bytes) = runtime.renet.receive_message(channel) else {
                break;
            };
            let result = receive_message(&mut runtime, channel, &bytes, now, world);
            if let Err(error) = result {
                let mut telemetry = world.resource_mut::<diagnostics::Telemetry>();
                record_receive_rejection(&mut telemetry, &error);
                log::warn!("Ignored scoped message: {error}");
            }
        }
    }
    let outcome_ack = runtime
        .prediction
        .as_mut()
        .and_then(|session| session.outcomes_ack_pending.take());
    if let Some(through) = outcome_ack {
        if send_control(&mut runtime, ClientControl::OutcomesAck { through }).is_err() {
            if let Some(session) = &mut runtime.prediction {
                session.outcomes_ack_pending = Some(through);
            }
        }
    }
    let result = if let Some(session) = &mut runtime.prediction {
        session.advance_presentation_clock(now);
        session
            .reconcile()
            .and_then(|_| session.presentation(false))
    } else {
        Ok(None)
    };
    match result {
        Ok(Some(authority)) => {
            runtime.last_snapshot = Some(authority.clone());
            let displayed = runtime
                .prediction
                .as_ref()
                .and_then(|s| s.presentation(true).ok().flatten())
                .unwrap_or(authority);
            let shift = diagnostics::reconcile_shift(
                &world.resource::<DreamView>().0,
                &displayed,
                false,
                displayed.hero.id,
            );
            let replay = runtime
                .prediction
                .as_ref()
                .map_or(0, |s| s.predictor.work().replayed_ticks);
            world
                .resource_mut::<diagnostics::Telemetry>()
                .record_reconciliation(replay, shift, now.as_secs_f64());
            if let Some(session) = &runtime.prediction {
                session.update_connection(
                    &mut world.resource_mut::<DreamConnection>(),
                    runtime.renet.rtt() * 1000.0,
                    usize::from(displayed.active_travelers),
                );
            }
            world.resource_mut::<DreamView>().0 = displayed;
        }
        Err(error) => {
            // A state that cannot be applied is skipped; the next owner state
            // supersedes it. Nothing here resets the session.
            log::warn!("Owner state skipped: {error}");
            world
                .resource_mut::<diagnostics::Telemetry>()
                .record_decode_rejection(&error);
        }
        _ => {}
    }
    let mut probe = false;
    if let Some(session) = &mut runtime.prediction {
        if now.saturating_sub(session.last_probe) >= Duration::from_millis(250) {
            session.last_probe = now;
            probe = true;
        }
    }
    if probe {
        let _ = send_control(
            &mut runtime,
            ClientControl::ClockProbe {
                client_send_nanos: now.as_nanos() as u64,
            },
        );
    }
    if let Some(session) = &runtime.prediction {
        if let Some(lookup) = runtime.outcomes.lookup(
            session.welcome.player.get(),
            session.welcome.stream.epoch,
            session.status.finalized,
            now,
        ) {
            send_control(&mut runtime, lookup).ok();
        }
    }
    if let Some(report) = disconnect_report(&runtime, now, transport_error.as_deref()) {
        finish_disconnection(world, runtime, report, transport_error.is_some());
        return;
    }
    world.insert_non_send(runtime);
}
struct DisconnectReport {
    status: &'static str,
    diagnostic: String,
}
fn disconnect_report(
    runtime: &Runtime,
    now: Duration,
    transport_error: Option<&str>,
) -> Option<DisconnectReport> {
    let before_welcome = runtime.prediction.is_none();
    let phase = if before_welcome {
        "before Welcome"
    } else {
        "after Welcome"
    };
    let receive_gap = now.as_secs_f64() - runtime.last_received;
    if runtime.renet.is_disconnected() {
        let reason = runtime
            .renet
            .disconnect_reason()
            .map_or_else(|| "unavailable".into(), |reason| format!("{reason:?}"));
        let mut diagnostic = format!(
            "{phase}: Renet disconnect {reason}; Welcome/state receive gap {receive_gap:.3}s"
        );
        // Renet's Transport variant alone does not preserve netcode's concrete
        // timeout/denial error. Retain this update's cause before Runtime drops.
        if let Some(error) = transport_error {
            diagnostic.push_str(&format!("; transport: {error}"));
        }
        Some(DisconnectReport {
            status: if before_welcome {
                "Connection ended before the server welcome. Try joining again."
            } else {
                "Disconnected. Join again to reconnect with the same Traveler ID."
            },
            diagnostic,
        })
    } else if receive_gap > 12.0 {
        Some(DisconnectReport {
            status: if before_welcome {
                "Timed out waiting for the server welcome. Try joining again."
            } else {
                "Timed out waiting for server updates. Join again to reconnect."
            },
            diagnostic: format!(
                "{phase}: application receive timeout after {receive_gap:.3}s (limit 12.000s)"
            ),
        })
    } else {
        None
    }
}
fn finish_disconnection(
    world: &mut World,
    runtime: Runtime,
    report: DisconnectReport,
    transport_error_recorded: bool,
) {
    {
        let mut telemetry = world.resource_mut::<diagnostics::Telemetry>();
        if !transport_error_recorded {
            telemetry.transport_errors = telemetry.transport_errors.saturating_add(1);
        }
        telemetry.record_issue(diagnostics::IssueKind::Transport, &report.diagnostic);
    }
    log::warn!(
        "Dreamwake transport client {} disconnected: {}",
        runtime.client_id,
        report.diagnostic
    );
    world
        .resource_mut::<crate::identity::TravelerProfile>()
        .unlock();
    world.resource_mut::<DreamConnection>().connected = false;
    world.resource_mut::<DreamConnection>().status = report.status.into();
    world.insert_resource(runtime.outcomes);
}
#[derive(Debug)]
enum ReceiveRejection {
    Scoped(String),
    Clock(SyncError),
}
fn record_receive_rejection(telemetry: &mut diagnostics::Telemetry, error: &ReceiveRejection) {
    match error {
        ReceiveRejection::Scoped(reason) => telemetry.record_decode_rejection(reason),
        ReceiveRejection::Clock(error) => telemetry.record_clock_rejection(*error),
    }
}
impl From<String> for ReceiveRejection {
    fn from(reason: String) -> Self {
        Self::Scoped(reason)
    }
}
impl From<&str> for ReceiveRejection {
    fn from(reason: &str) -> Self {
        Self::Scoped(reason.into())
    }
}
impl From<SyncError> for ReceiveRejection {
    fn from(error: SyncError) -> Self {
        Self::Clock(error)
    }
}
impl std::fmt::Display for ReceiveRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scoped(reason) => f.write_str(reason),
            Self::Clock(error) => error.fmt(f),
        }
    }
}

fn receive_message(
    runtime: &mut Runtime,
    channel: u8,
    bytes: &[u8],
    now: Duration,
    world: &mut World,
) -> Result<(), ReceiveRejection> {
    if channel == CONTROL_CHANNEL && is_welcome(bytes) {
        let welcome = decode_welcome(bytes).map_err(|e| e.to_string())?;
        if !welcome.compatible(ConnectionId(runtime.client_id))
            || welcome.player
                != world
                    .resource::<crate::identity::TravelerProfile>()
                    .selected_id()
        {
            return Err("incompatible welcome".into());
        }
        if runtime.prediction.as_ref().is_some_and(|s| {
            welcome.server_instance == s.welcome.server_instance
                && welcome.stream.epoch <= s.welcome.stream.epoch
        }) {
            return Ok(());
        }
        let content = welcome.content;
        runtime.outcomes.bind_instance(welcome.server_instance);
        let mut session = Session::new(welcome)?;
        session.begin_frame();
        runtime.prediction = Some(session);
        runtime.pending.clear();
        runtime.action_seq = CommandSeq(0);
        runtime.in_flight_action = None;
        runtime.last_snapshot = None;
        world.resource_mut::<CapturedInput>().0 = DreamInput::default();
        world
            .resource_mut::<diagnostics::Telemetry>()
            .reset_samples();
        let mut prefs = world.resource_mut::<DreamPreferences>();
        prefs.paused = false;
        prefs.build_open = false;
        send_control(runtime, ClientControl::Ready { content })?;
        runtime.last_received = now.as_secs_f64();
        return Ok(());
    }
    let session = runtime.prediction.as_mut().ok_or("state before welcome")?;
    if channel == STATE_CHANNEL {
        if session.receive_state(bytes)? {
            let owner = session.welcome.player.get();
            let epoch = session.welcome.stream.epoch;
            let finalized = session.status.finalized;
            let menu_applied = session.status.menu_applied;
            if let Some((tick, _)) = session.authoritative_owner_pose()? {
                runtime.outcomes.observe(owner, epoch, tick, now, false);
            }
            for frame in runtime
                .pending
                .iter()
                .filter(|frame| frame.command.target.0 <= finalized.0)
            {
                // Telemetry's legacy u32 sample key is display-only.
                world.resource_mut::<diagnostics::Telemetry>().acknowledge(
                    frame.command.sequence.0 as u32,
                    frame.sent_at,
                    now.as_secs_f64(),
                );
            }
            runtime
                .pending
                .retain(|frame| frame.command.target.0 > finalized.0);
            if runtime
                .in_flight_action
                .is_some_and(|sequence| menu_applied.is_some_and(|applied| sequence <= applied))
            {
                runtime.in_flight_action = None;
            }
            runtime.last_received = runtime.last_received.max(now.as_secs_f64());
            world.resource_mut::<diagnostics::Telemetry>().snapshots += 1;
        }
        return Ok(());
    }
    let control: ServerControl = decode_control(
        bytes,
        session.welcome.stream.epoch,
        session.welcome.frame_limit().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    match control {
        ServerControl::ActionOutcomes { batch, records } => {
            runtime.outcomes.receive_batch(
                session.welcome.player.get(),
                records.as_slice(),
                now,
            )?;
            for outcome in records.as_slice() {
                session.events.outcome(outcome)?;
            }
            session.outcomes_ack_pending = Some(
                session
                    .outcomes_ack_pending
                    .map_or(batch, |previous| previous.max(batch)),
            );
        }
        ServerControl::OutcomeUnavailable { keys } => {
            // Lookup-window expiry is not a new gameplay rejection.
            runtime
                .outcomes
                .unavailable(session.welcome.player.get(), keys.as_slice());
        }
        ServerControl::ClockReply {
            client_send_nanos,
            server_receive_nanos,
            server_send_nanos,
            tick,
        } => {
            session.observe_clock_reply(
                TimeExchange {
                    client_send: Duration::from_nanos(client_send_nanos),
                    server_receive: Duration::from_nanos(server_receive_nanos),
                    server_send: Duration::from_nanos(server_send_nanos),
                    client_receive: now,
                },
                tick,
            )?;
        }
        ServerControl::Notice { utf8 } => {
            world.resource_mut::<DreamConnection>().status =
                String::from_utf8_lossy(utf8.as_slice()).into_owned()
        }
    }
    Ok(())
}
fn journal_hardware(world: &mut World) {
    let now = network_now(world.resource::<Time<Real>>());
    let mut input = world.resource::<CapturedInput>().0;
    let prefs = world.resource::<DreamPreferences>();
    if prefs.paused || prefs.build_open {
        input = DreamInput::default();
    }
    if world.resource::<Playtest>().autoplay {
        if let Some(mut runtime) = world.get_non_send_mut::<Runtime>() {
            if let Some(session) = &mut runtime.prediction {
                let current = [
                    input.dash,
                    input.casts[0],
                    input.casts[1],
                    input.casts[2],
                    input.casts[3],
                ];
                input.dash &= !session.autoplay_edges[0];
                for slot in 0..4 {
                    input.casts[slot] &= !session.autoplay_edges[slot + 1];
                }
                session.autoplay_edges = current;
            }
        }
    }
    let mut edges = std::mem::take(&mut world.resource_mut::<CapturedActions>().0);
    let mut edge_overflow = false;
    if input.dash {
        edge_overflow |= edges
            .push(DreamAction::Dash {
                direction: if input.movement == [0.0; 2] {
                    input.aim
                } else {
                    input.movement
                },
            })
            .is_err();
    }
    for (slot, pressed) in input.casts.iter().enumerate() {
        if *pressed {
            edge_overflow |= edges
                .push(DreamAction::Cast {
                    slot: slot as u8,
                    aim: input.aim,
                })
                .is_err();
        }
    }
    input.dash = false;
    input.casts = [false; 4];
    input.action_sequences = [0; 5];
    input.charge = engine_core::ChargeCommand::None;
    let beam = world
        .get_resource::<crate::plugins::input::CapturedBeam>()
        .and_then(|beam| beam.0);
    if edge_overflow {
        // More edges than one frame can carry: the extra presses are dropped,
        // as a game would drop input it cannot keep up with.
        log::warn!("Hardware action edge capacity; extra edges dropped this frame");
    }
    if let Some(mut runtime) = world.remove_non_send::<Runtime>() {
        if let Some(session) = &mut runtime.prediction
            && session.active
            && let Err(error) = session.journal.capture(HardwareSample {
                at: now,
                held: Some(TickInput { held: input, beam }),
                edges,
                mouse_delta: [0.0; 2],
            })
        {
            // The journal is full after a long pause in command generation.
            // Start input over; the server already filled the gap.
            log::warn!("Input journal restarted: {error}");
            session.reset_input();
        }
        world.insert_non_send(runtime);
    }
    let mut captured = world.resource_mut::<CapturedInput>();
    captured.0.dash = false;
    captured.0.casts = [false; 4];
}
fn predict_and_send(world: &mut World) {
    playtest_actions(world);
    let Some(mut runtime) = world.remove_non_send::<Runtime>() else {
        return;
    };
    let now = network_now(world.resource::<Time<Real>>());
    match generate_commands(&mut runtime, now) {
        Ok(commands) => {
            if let Err(error) = send_input(&mut runtime, &commands) {
                log::warn!("Could not send input: {error}");
            }
        }
        Err(error) => {
            // Prediction could not advance. Drop local intent and continue
            // from the next owner state; the session stays connected.
            log::warn!("Prediction paused: {error}");
            if let Some(session) = &mut runtime.prediction {
                session.reset_input();
                session.predictor.clear();
            }
        }
    }
    if now.as_secs_f64() - runtime.last_received <= PREDICTION_MAX_SNAPSHOT_AGE {
        if let Some(displayed) = runtime
            .prediction
            .as_ref()
            .and_then(|s| s.presentation(true).ok().flatten())
        {
            if let Some(session) = &runtime.prediction {
                if let Some(tick) = session.predictor.predicted_tick() {
                    runtime.outcomes.observe(
                        session.welcome.player.get(),
                        session.welcome.stream.epoch,
                        tick,
                        now,
                        true,
                    );
                    if let Some((key, id)) = session
                        .predictor
                        .state()
                        .and_then(|owner| owner.beam_graphics_action())
                    {
                        let stream = session.welcome.stream;
                        if key.connection_epoch == stream.epoch.0
                            && key.command_stream == stream.stream.0
                            && key.ownership_epoch == stream.ownership.0
                            && key.match_epoch == session.welcome.match_epoch
                            && displayed
                                .presentations
                                .iter()
                                .any(|graphic| graphic.id == id)
                        {
                            runtime.outcomes.graphics_seen(
                                session.welcome.player.get(),
                                ActionKey {
                                    connection: stream.epoch,
                                    stream: stream.stream,
                                    command: CommandSeq(key.command_sequence),
                                    slot: key.action_slot,
                                },
                                id,
                                tick,
                                now,
                            );
                        }
                    }
                }
            }
            world.resource_mut::<DreamView>().0 = displayed;
        }
    }
    world.insert_non_send(runtime);
}
/// Commands the newest input packet repeats right behind the newest one, so a
/// short burst of loss costs nothing.
const RECENT_RECORDS: usize = 3;
/// Commands the server has not consumed yet, oldest first, repeated in every
/// packet until it confirms them, as Unreal resends its oldest unacknowledged
/// important move.
const OLDEST_RECORDS: usize = 3;

/// One input packet per simulation tick: the acknowledgement of state received
/// so far, the newest commands and the oldest the server has not consumed. A
/// tick without commands, such as while joining, still sends the
/// acknowledgement the server uses to send deltas and to activate the hero.
fn send_input(runtime: &mut Runtime, commands: &[InputCommand]) -> Result<(), String> {
    let Some(session) = &mut runtime.prediction else {
        return Ok(());
    };
    if commands.is_empty() && !session.has_state() {
        return Ok(());
    }
    if !runtime.renet.is_connected() {
        return Ok(());
    }
    let mut records: Vec<InputRecord> = Vec::new();
    let recent = runtime
        .pending
        .iter()
        .rev()
        .take(RECENT_RECORDS)
        .map(|frame| &frame.command);
    let oldest = runtime
        .pending
        .iter()
        .take(OLDEST_RECORDS)
        .map(|frame| &frame.command);
    for command in commands.iter().rev().chain(recent).chain(oldest) {
        if records.len() == MAX_INPUT_RECORDS {
            break;
        }
        if !records
            .iter()
            .any(|record| record.sequence == command.sequence)
        {
            records.push(InputRecord::from_command(command));
        }
    }
    let bytes = session.input(records)?;
    runtime.renet.send_message(INPUT_CHANNEL, bytes);
    Ok(())
}

/// How many ticks toward `target` fit this frame's prediction budget. A gap
/// longer than the budget (the client stopped running for a while) is covered
/// over several frames rather than treated as a fault.
fn prediction_ticks_this_frame(session: &Session, current: ServerTick, target: TargetTick) -> u64 {
    let remaining = session.predictor.remaining_ticks();
    target.0.saturating_sub(current.0).min(remaining as u64)
}
/// Generate this tick's commands and advance prediction through them.
fn generate_commands(runtime: &mut Runtime, now: Duration) -> Result<Vec<InputCommand>, String> {
    let Some(session) = &mut runtime.prediction else {
        return Ok(Vec::new());
    };
    if !session.active
        || !runtime.renet.is_connected()
        || session.clock.sample_count() == 0
        || !session.predictor.is_initialized()
    {
        return Ok(Vec::new());
    }
    let server_now = session
        .clock
        .estimate_server_time(now)
        .map_err(|e| e.to_string())?;
    session.estimated_server_tick = Some(
        session
            .clock
            .estimate_server_tick(now)
            .map_err(|e| e.to_string())?,
    );
    let estimated = session
        .simulation_clock
        .estimate(server_now, session.lead.lead())
        .map_err(|e| e.to_string())?;
    session.estimated_simulation_tick = Some(estimated);
    let Some(target) = session.assign_command_target(estimated)? else {
        return Ok(Vec::new());
    };
    let current = session
        .predictor
        .predicted_tick()
        .ok_or("missing owner prediction")?;
    if target.0 <= current.0 {
        return Ok(Vec::new());
    }
    let reachable = prediction_ticks_this_frame(session, current, target);
    // Bootstrap or a frontier behind estimated simulation time is a forecast
    // gap. Replay server-style substitutes through the fresh head's predecessor;
    // only that head authors intent. An already-ahead frontier keeps every normal
    // contiguous hardware sample. Forecast never drains an edge or allocates a
    // command/action identity.
    let forecast_gap = session.forecast_gap(estimated);
    if reachable < target.0 - current.0 {
        // Catch up by forecasting only; the head is authored once in reach.
        for tick in current.0 + 1..=current.0 + reachable {
            let dependencies = session.prediction_dependencies(ServerTick(tick));
            session
                .predictor
                .predict(ServerTick(tick), RewindInput::Substitute, dependencies)
                .map_err(|e| format!("{e:?}"))?;
        }
        session.events.publish(&session.predictor)?;
        return Ok(Vec::new());
    }
    let mut commands = Vec::new();
    for tick in current.0 + 1..=target.0 {
        if forecast_gap && tick < target.0 {
            let dependencies = session.prediction_dependencies(ServerTick(tick));
            session
                .predictor
                .predict(ServerTick(tick), RewindInput::Substitute, dependencies)
                .map_err(|e| format!("{e:?}"))?;
            continue;
        }
        let sample = session.command_sample(TargetTick(tick), target, now)?;
        session.sequence = session
            .sequence
            .checked_next()
            .ok_or("input sequence exhausted")?;
        let mut actions = BoundedVec::default();
        // Hardware press/release edges retain order across simulation ticks.
        if let Some(action) = session.next_action(sample.edges.as_slice())? {
            actions
                .push(ActionEdge { slot: 0, action })
                .map_err(|e| e.to_string())?;
        }
        let command = Command {
            owner: session.welcome.stream,
            sequence: session.sequence,
            target: TargetTick(tick),
            input: sample.held,
            actions,
        };
        runtime
            .outcomes
            .track(session.welcome.player.get(), &command, now)?;
        let dependencies = session.prediction_dependencies(ServerTick(tick));
        session
            .predictor
            .predict(
                ServerTick(tick),
                RewindInput::Command(command.clone()),
                dependencies,
            )
            .map_err(|e| format!("{e:?}"))?;
        session.record_command_issuance(command.sequence, command.target, target);
        runtime.pending.push_back(PendingFrame {
            command: command.clone(),
            sent_at: now.as_secs_f64(),
        });
        runtime.outcomes.enqueued(
            command
                .actions
                .as_slice()
                .iter()
                .map(|edge| command.action_key(edge.slot)),
            now,
        );
        commands.push(command);
    }
    session.events.publish(&session.predictor)?;
    Ok(commands)
}
pub(crate) fn has_local_hero(snapshot: &DreamPresentation, id: u64) -> bool {
    snapshot.hero.id == id && snapshot.heroes.iter().any(|hero| hero.id == id)
}

fn playtest_actions(world: &mut World) {
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    let test = world.resource::<Playtest>();
    if !test.autoplay
        || now < test.next_decision
        || world
            .get_resource::<crate::ui::identity::TravelerIdentityUi>()
            .is_some_and(|ui| ui.open)
    {
        return;
    }
    let manual_rewards = test.manual_rewards;
    let restart_on_terminal = test.restart_on_terminal;
    let connection = world.resource::<DreamConnection>();
    if !connection.connected
        || connection.party_size < world.resource::<NetworkOptions>().expected_party
    {
        return;
    }
    let view = &world.resource::<DreamView>().0;
    if restart_on_terminal && matches!(view.phase, RunPhase::Victory | RunPhase::Defeat) {
        let mut actions = world.resource_mut::<crate::ui::UiActions>();
        if actions.0.is_empty() {
            actions.0.push(crate::ui::UiAction::Restart);
            world.resource_mut::<Playtest>().next_decision = now + 1.1;
        }
        return;
    }
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
#[cfg(test)]
mod autoplay_restart_tests {
    use super::*;
    #[test]
    fn terminal_restart_is_explicit_paced_and_uses_the_normal_ui_queue() {
        for phase in [RunPhase::Victory, RunPhase::Defeat] {
            let mut world = World::new();
            let mut view = crate::offline_presentation(DreamSimulation::new(1, false).snapshot());
            view.phase = phase;
            world.insert_resource(DreamView(view));
            world.insert_resource(Time::<Real>::default());
            let mut test = Playtest::default();
            test.autoplay = true;
            world.insert_resource(test);
            world.insert_resource(DreamConnection {
                connected: true,
                party_size: 1,
                ..default()
            });
            world.insert_resource(NetworkOptions {
                seed: 1,
                auto_connect: true,
                auto_host: false,
                expected_party: 1,
                #[cfg(not(target_arch = "wasm32"))]
                hosted: false,
                #[cfg(not(target_arch = "wasm32"))]
                max_clients: 8,
                #[cfg(not(target_arch = "wasm32"))]
                transport: NativeTransportKind::Udp,
                #[cfg(not(target_arch = "wasm32"))]
                link_impairment: None,
            });
            world.init_resource::<crate::ui::UiActions>();
            playtest_actions(&mut world);
            assert!(
                world.resource::<crate::ui::UiActions>().0.is_empty(),
                "ordinary autoplay stays terminal"
            );
            world.resource_mut::<Playtest>().restart_on_terminal = true;
            world.resource_mut::<DreamConnection>().connected = false;
            playtest_actions(&mut world);
            assert!(world.resource::<crate::ui::UiActions>().0.is_empty());
            world.resource_mut::<DreamConnection>().connected = true;
            world
                .resource_mut::<crate::ui::UiActions>()
                .0
                .push(crate::ui::UiAction::TogglePause);
            playtest_actions(&mut world);
            assert_eq!(
                world.resource::<crate::ui::UiActions>().0.len(),
                1,
                "manual input is not overtaken"
            );
            world.resource_mut::<crate::ui::UiActions>().0.clear();
            playtest_actions(&mut world);
            assert!(matches!(
                world.resource::<crate::ui::UiActions>().0.as_slice(),
                [crate::ui::UiAction::Restart]
            ));
            world.resource_mut::<crate::ui::UiActions>().0.clear();
            playtest_actions(&mut world);
            assert!(world.resource::<crate::ui::UiActions>().0.is_empty());
            world
                .resource_mut::<Time<Real>>()
                .advance_by(Duration::from_millis(1100));
            playtest_actions(&mut world);
            assert!(matches!(
                world.resource::<crate::ui::UiActions>().0.as_slice(),
                [crate::ui::UiAction::Restart]
            ));
            assert_eq!(world.resource::<Playtest>().next_decision, 2.2);
        }
    }
}

/// Every frame: input must leave in the frame that authored it. Holding it
/// for a send cadence lets a hitch that follows delay it past its tick.
fn flush_network(
    runtime: Option<NonSendMut<Runtime>>,
    mut telemetry: ResMut<diagnostics::Telemetry>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };
    let Runtime {
        transport, renet, ..
    } = &mut *runtime;
    if let Err(error) = transport.send_packets(renet) {
        telemetry.transport_errors += 1;
        telemetry.record_issue(diagnostics::IssueKind::Transport, &error.to_string());
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

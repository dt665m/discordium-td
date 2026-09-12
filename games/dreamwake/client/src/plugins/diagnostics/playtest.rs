use crate::DreamView;
use bevy::prelude::*;
#[cfg(target_arch = "wasm32")]
use dreamwake_sim::RunPhase;
#[derive(Resource, Default)]
pub(crate) struct Playtest {
    pub(crate) autoplay: bool,
    pub(crate) restart_on_terminal: bool,
    pub(crate) spatial: super::SpatialPlaytest,
    pub(crate) manual_rewards: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) capture_dir: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) quit_after: f32,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) next_capture: f32,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) captures: u32,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) trace_path: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) next_trace: f64,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) trace_bytes: usize,
    #[cfg(not(target_arch = "wasm32"))]
    trace_file: Option<engine_net::trace::SegmentedTraceFile>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) trace_options: TraceOptions,
    #[cfg(not(target_arch = "wasm32"))]
    trace_failed: bool,
    pub(crate) next_decision: f64,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct TraceOptions {
    pub(crate) chunks: u16,
    pub(crate) interval_seconds: f64,
}
#[cfg(not(target_arch = "wasm32"))]
impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            chunks: 1,
            interval_seconds: 0.25,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn playtest_capture(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut test: ResMut<Playtest>,
    keys: Res<ButtonInput<KeyCode>>,
    view: Res<DreamView>,
    runtime: Option<NonSend<crate::plugins::network::Runtime>>,
    telemetry: Res<super::capture::Telemetry>,
    mut exit: MessageWriter<AppExit>,
    mut capture: ResMut<super::native_capture::NativeCapture>,
    window: Res<super::native_window::NativeWindowMetrics>,
) {
    if test.trace_path.is_some() && time.elapsed_secs_f64() >= test.next_trace {
        test.next_trace = time.elapsed_secs_f64() + test.trace_options.interval_seconds;
        record_trace(
            &mut test,
            &view.0,
            runtime.as_deref(),
            &telemetry,
            time.elapsed_secs_f64(),
            time.delta_secs_f64(),
            &window,
        );
    }
    if test.trace_failed {
        exit.write(AppExit::error());
        return;
    }
    capture.poll();
    let quitting = test.quit_after > 0.0 && time.elapsed_secs() >= test.quit_after;
    if !quitting
        && (keys.just_pressed(KeyCode::F9)
            || (test.capture_dir.is_some() && time.elapsed_secs() > test.next_capture))
    {
        test.next_capture = time.elapsed_secs() + 8.0;
        let dir = test.capture_dir.as_deref().unwrap_or("/tmp/dreamwake");
        let path =
            std::path::PathBuf::from(dir).join(format!("dreamwake-{:03}.png", test.captures));
        if capture.request(&mut commands, path) {
            info!(
                "Dreamwake capture: room={} phase={:?} hp={:.0} kills={}",
                view.0.room + 1,
                view.0.phase,
                view.0.hero.hp,
                view.0.kills
            );
            test.captures += 1;
        }
    }
    if quitting && capture.ready_to_exit() {
        exit.write(if capture.failed() {
            AppExit::error()
        } else {
            AppExit::Success
        });
    }
}

/// Opt-in local diagnostic output contains only state already authorized for this
/// client. Default is one 8 MiB file; native CLI may opt into a finite number
/// of segments. No credentials or owner checkpoint payloads are included.
#[cfg(not(target_arch = "wasm32"))]
fn record_trace(
    test: &mut Playtest,
    view: &dreamwake_sim::DreamPresentation,
    runtime: Option<&crate::plugins::network::Runtime>,
    telemetry: &super::capture::Telemetry,
    elapsed: f64,
    frame_delta_seconds: f64,
    window: &super::native_window::NativeWindowMetrics,
) {
    let Some(path) = test.trace_path.clone() else {
        return;
    };
    let metrics = runtime.map(|r| r.metrics());
    let session = metrics.as_ref().and_then(|m| m.session.as_ref());
    let scopes = runtime
        .and_then(|r| r.prediction.as_ref())
        .map(|s| s.scope_identities())
        .unwrap_or_default();
    let hero_scopes = runtime
        .and_then(|r| r.prediction.as_ref())
        .map(|s| s.public_hero_scopes())
        .unwrap_or_default();
    let authoritative = runtime
        .and_then(|r| r.prediction.as_ref())
        .map(|s| s.authoritative_owner_pose())
        .transpose();
    let authority_error = authoritative.as_ref().err();
    let authority_pose = authoritative.as_ref().ok().copied().flatten().flatten();
    let mut record = serde_json::json!({
        "format": 1, "pid": std::process::id(), "elapsed": elapsed,
        "frame_delta_seconds": frame_delta_seconds,
        "active_travelers": view.active_travelers,
        "scenario": test.spatial.label(),
        "separated": test.spatial.separated(view.tick),
        "match_epoch": view.stamp.match_epoch, "server_tick": view.stamp.server_tick,
        "phase": format!("{:?}", view.phase), "room": view.room,
        "owner": view.hero.id, "position": view.hero.position,
        "authoritative_owner_position": authority_pose.map(|(_, position)| position),
        "owner_checkpoint_tick": authority_pose.map(|(tick, _)| tick.0),
        "authority_error": authority_error,
        "heroes": view.heroes.iter().map(|h| h.id).collect::<Vec<_>>(),
        "scopes": scopes,
        "hero_scopes": hero_scopes.into_iter().map(|(hero_id, scope, end_tick, position)| serde_json::json!({"hero_id":hero_id, "scope":scope, "end_tick":end_tick.0, "position":position})).collect::<Vec<_>>(),
        "active": session.is_some_and(|s| s.active && !s.recovering),
        "recovering": session.map(|s| s.recovering),
        "connection_epoch": session.map(|s| s.connection_epoch.0),
        "estimated_server_tick": session.and_then(|s| s.estimated_server_tick).map(|t| t.0),
        "lead_ticks": session.map(|s| s.lead_ticks),
        "arrival_slack": session.and_then(|s| s.arrival_slack),
        "pending_commands": metrics.as_ref().map(|m| m.pending_commands),
        "finalized_tick": session.map(|s| s.finalized_tick.0),
        "predicted_tick": session.and_then(|s| s.predicted_tick).map(|t| t.0),
        "history_bytes": session.map(|s| s.prediction_bytes),
        "scope_bytes": session.map(|s| s.scope_bytes),
        "staging_bytes": session.map(|s| s.fragment_bytes + s.group_bytes),
        "baseline_bytes": session.map(|s| s.baseline_bytes),
        "baseline_repairs": session.map(|s| s.baseline_repairs),
        "decode_errors": telemetry.decode_errors,
        "transport_errors": telemetry.transport_errors,
        "decode_issue": telemetry.decode_issue,
        "recovery_issue": telemetry.recovery_issue,
        "transport_issue": telemetry.transport_issue,
        "rtt_ms": runtime.map(|r| r.renet.rtt() * 1000.0),
        "renet_receive_bytes_per_second": runtime.map(|r| r.renet.bytes_received_per_sec()),
    });
    record["estimated_simulation_tick"] = serde_json::json!(
        session
            .and_then(|s| s.estimated_simulation_tick)
            .map(|t| t.0)
    );
    record["committed_server_tick"] = serde_json::json!(session.map(|s| s.committed_server_tick.0));
    record["clock_anchor_tick"] = serde_json::json!(session.map(|s| s.clock_anchor_tick.0));
    record["arrival_feedback"] = serde_json::json!(session.map(|s| s.arrival_feedback));
    record["arrival_feedback_applied"] =
        serde_json::json!(session.map(|s| s.arrival_feedback_applied));
    record["arrival_feedback_ignored"] =
        serde_json::json!(session.map(|s| s.arrival_feedback_ignored));
    record["lead_policy"] = serde_json::json!(session.map(|s| s.lead_policy));
    record["transport_connection"] = serde_json::json!(session.map(|s| s.transport_connection));
    record["stale_epoch_rejections"] = serde_json::json!(telemetry.stale_epoch_rejections);
    record["clock_resync_rejections"] = serde_json::json!(telemetry.clock_resync_rejections);
    record["obsolete_rejections"] = serde_json::json!(telemetry.obsolete_rejections);
    record["decode_failures"] = serde_json::json!(telemetry.decode_failures);
    record["replay_ticks"] = serde_json::json!(session.map(|s| s.replay_ticks));
    record["predicted_ticks_this_frame"] =
        serde_json::json!(session.map(|s| s.predicted_ticks_this_frame));
    record["gameplay_tick"] = serde_json::json!(view.tick);
    record["window"] = serde_json::json!(window);
    record["hp"] = serde_json::json!(view.hero.hp);
    record["enemy_scopes"] = serde_json::json!(
        runtime
            .and_then(|r| r.prediction.as_ref())
            .map(|s| s.public_enemy_scopes())
            .unwrap_or_default()
            .into_iter()
            .map(|(enemy_id, scope, end_tick, position)| serde_json::json!({
                "enemy_id": enemy_id, "scope": scope, "end_tick": end_tick.0,
                "position": position,
            }))
            .collect::<Vec<_>>()
    );
    record["interpolation"] = serde_json::json!(session.map(|s| serde_json::json!({
        "known": s.interpolation.known_entities,
        "active": s.interpolation.active_entities,
        "samples": s.interpolation.retained_samples,
        "bytes": s.interpolation.retained_bytes,
        "cursor_seconds": s.interpolation.cursor.as_secs_f64(),
        "buffering": s.interpolation.buffering,
        "interpolated": s.interpolation.interpolated,
        "extrapolated": s.interpolation.extrapolated,
        "frozen": s.interpolation.frozen,
        "maximum_pose_age_seconds": s.interpolation.maximum_pose_age.as_secs_f64(),
    })));
    record["combat_view_tick"] = serde_json::json!(
        runtime
            .and_then(|runtime| runtime.prediction.as_ref())
            .and_then(|session| session.metrics().combat_view_tick)
            .map(|tick| tick.0)
    );
    record["delivery"] = serde_json::json!(session.map(|s| s.delivery));
    record["server_instance"] = serde_json::json!(session.map(|s| s.server_instance));
    record["action_traces"] = serde_json::json!(runtime.map(|r| r.outcomes.trace(view.hero.id, 8)));
    if test.trace_file.is_none() {
        match engine_net::trace::SegmentedTraceFile::create(
            &path,
            8 * 1024 * 1024,
            test.trace_options.chunks,
        ) {
            Ok(file) => test.trace_file = Some(file),
            Err(error) => {
                trace_failure(test, &path, error);
                return;
            }
        }
    }
    let writer = test.trace_file.as_mut().unwrap();
    let result = writer.write_record(|position| {
        record["trace_sequence"] = serde_json::json!(position.sequence);
        record["trace_segment"] = serde_json::json!(position.segment);
        let mut encoded = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
        encoded.push(b'\n');
        Ok(encoded)
    });
    match result {
        Ok(()) => test.trace_bytes = writer.bytes_written(),
        Err(error) => trace_failure(test, &path, error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn trace_failure(test: &mut Playtest, path: &str, error: std::io::Error) {
    let position = test
        .trace_file
        .as_ref()
        .map(|writer| writer.next_position())
        .unwrap_or(engine_net::trace::TracePosition {
            sequence: 1,
            segment: 0,
        });
    eprintln!(
        "DREAMWAKE_TRACE_ERROR {}",
        serde_json::json!({
            "error": error.to_string(), "kind": format!("{:?}",error.kind()), "path": path,
            "trace_sequence": position.sequence, "trace_segment": position.segment,
        })
    );
    test.trace_failed = true;
    test.trace_path = None;
}

#[cfg(target_arch = "wasm32")]
pub(super) fn playtest_capture(
    test: Res<Playtest>,
    view: Res<DreamView>,
    time: Res<Time<Real>>,
    mut previous: Local<Option<(usize, RunPhase, u8)>>,
    mut sample: Local<(f32, u32)>,
) {
    if !test.autoplay {
        return;
    }
    sample.0 += time.delta_secs();
    sample.1 += 1;
    let boss_phase = view.0.enemies.iter().map(|e| e.phase).max().unwrap_or(0);
    let state = (view.0.room, view.0.phase, boss_phase);
    if previous.as_ref() != Some(&state) {
        info!(
            "Dreamwake browser playtest: room={} phase={:?} enemy_phase={} party={} hp={:.0} kills={} elapsed={:.1}s mean_fps={:.1}",
            view.0.room + 1,
            view.0.phase,
            boss_phase,
            view.0.heroes.len(),
            view.0.hero.hp,
            view.0.kills,
            view.0.elapsed,
            sample.1 as f32 / sample.0.max(0.001)
        );
        *previous = Some(state);
        *sample = (0.0, 0);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod trace_tests {
    use super::*;
    #[test]
    fn trace_records_continuity_and_marks_failure_instead_of_disabling_silently() {
        let path = std::env::temp_dir().join(format!(
            "dreamwake-trace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut test = Playtest {
            trace_path: Some(path.to_string_lossy().into_owned()),
            ..default()
        };
        let view =
            crate::offline_presentation(dreamwake_sim::DreamSimulation::new(1, false).snapshot());
        let mut telemetry = super::super::capture::Telemetry::default();
        telemetry.record_decode_rejection("WrongEpoch");
        telemetry.record_decode_rejection("InvalidState");
        telemetry.record_decode_rejection("WrongEpoch");
        telemetry.record_clock_rejection(engine_net::synchronization::SyncError::ResyncRequired(
            engine_net::synchronization::ResyncReason::Suspension,
        ));
        telemetry
            .record_obsolete_rejection(engine_net::replication::ObsoleteReason::Scope, "snapshot");
        telemetry.reset_samples();
        let window = super::super::native_window::NativeWindowMetrics::default();
        record_trace(&mut test, &view, None, &telemetry, 1.0, 0.016, &window);
        let row: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(row["trace_sequence"], 1);
        assert_eq!(row["trace_segment"], 0);
        assert_eq!(row["elapsed"], 1.0);
        assert_eq!(row["decode_errors"], 5);
        assert_eq!(row["stale_epoch_rejections"], 2);
        assert_eq!(row["clock_resync_rejections"], 1);
        assert_eq!(row["obsolete_rejections"], 1);
        assert_eq!(row["decode_failures"], 1);
        assert_eq!(
            row["decode_issue"],
            format!(
                "obsolete {}: snapshot",
                engine_net::replication::ObsoleteReason::Scope
            )
        );
        assert!(row.as_object().unwrap().contains_key("replay_ticks"));
        assert!(
            row.as_object()
                .unwrap()
                .contains_key("predicted_ticks_this_frame")
        );
        assert!(!test.trace_failed);
        // Force a bounded failure without producing a large trace or runtime.
        test.trace_file = None;
        record_trace(&mut test, &view, None, &telemetry, 2.0, 0.016, &window);
        assert!(test.trace_failed);
        assert!(test.trace_path.is_none());
        let unchanged: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(row, unchanged);
        std::fs::remove_file(path).unwrap();
    }
}

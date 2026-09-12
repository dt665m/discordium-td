//! Dreamwake telemetry adapter for the shared network tools. No UI or transport ownership.
use super::overlay::OverlayMetric;
use crate::plugins::network::{PREDICTION_MAX_SNAPSHOT_AGE, Runtime, has_local_hero};
use crate::{DreamPreferences, DreamView};
use bevy::prelude::*;
use bevy_net_debug::ConditionerDebug;
use dreamwake_sim::{DreamPresentation, RunPhase};
use engine_client::network_tools::graphs::{DebugMetric, DiagnosticHistory};
use engine_client::ui::debug::DebugUiState;
use engine_net::replication::ObsoleteReason;
use std::time::Duration;

#[derive(Resource, Default)]
pub(crate) struct Telemetry {
    pub snapshots: u64,
    pub decode_errors: u64,
    pub stale_epoch_rejections: u64,
    pub clock_resync_rejections: u64,
    pub obsolete_rejections: u64,
    pub decode_failures: u64,
    pub transport_errors: u64,
    pub decode_issue: Option<String>,
    pub recovery_issue: Option<String>,
    pub transport_issue: Option<String>,
    last_ack: Option<u32>,
    ack_ms: Option<f64>,
    ack_at: Option<f64>,
    jitter_ms: f64,
    replay_count: Option<usize>,
    reconcile_shift: Option<(f32, f64)>,
}
pub(crate) enum IssueKind {
    Decode,
    Recovery,
    Transport,
}
impl Telemetry {
    /// Lifetime counters survive reconnect and sample resets. Only the exact
    /// codec rejection is stale; clock and obsolete classifications require typed errors.
    pub fn record_decode_rejection(&mut self, reason: &str) {
        if self.decode_errors < u64::MAX {
            self.decode_errors += 1;
            let counter = if reason == "WrongEpoch" {
                &mut self.stale_epoch_rejections
            } else {
                &mut self.decode_failures
            };
            *counter += 1;
        }
        self.record_issue(IssueKind::Decode, reason);
    }
    pub fn record_obsolete_rejection(&mut self, reason: ObsoleteReason, context: &str) {
        // Freeze all partition counters together at the lifetime counter ceiling.
        if self.decode_errors < u64::MAX {
            self.decode_errors += 1;
            self.obsolete_rejections += 1;
        }
        self.record_issue(IssueKind::Decode, &format!("obsolete {reason}: {context}"));
    }
    pub fn record_clock_rejection(&mut self, error: engine_net::synchronization::SyncError) {
        use engine_net::synchronization::SyncError;
        if matches!(error, SyncError::ResyncRequired(_)) {
            // Freeze the partition together at the lifetime counter ceiling.
            if self.decode_errors < u64::MAX {
                self.decode_errors += 1;
                self.clock_resync_rejections += 1;
            }
            self.record_issue(IssueKind::Decode, &error.to_string());
        } else {
            self.record_decode_rejection(&error.to_string());
        }
    }
    pub fn record_issue(&mut self, kind: IssueKind, reason: &str) {
        // Fixed latest-value slots survive disconnect for browser diagnosis.
        // Wire packets, credentials and unbounded trace histories are not stored.
        let reason = reason
            .chars()
            .filter(|c| !c.is_control())
            .take(160)
            .collect();
        *match kind {
            IssueKind::Decode => &mut self.decode_issue,
            IssueKind::Recovery => &mut self.recovery_issue,
            IssueKind::Transport => &mut self.transport_issue,
        } = Some(reason);
    }
    pub fn reset_samples(&mut self) {
        self.last_ack = None;
        self.ack_ms = None;
        self.ack_at = None;
        self.jitter_ms = 0.0;
        self.replay_count = None;
        self.reconcile_shift = None;
    }
    pub fn record_reconciliation(&mut self, count: usize, shift: Option<f32>, now: f64) {
        self.replay_count = Some(count);
        self.reconcile_shift = shift.map(|distance| (distance, now));
    }
    pub fn acknowledge(&mut self, seq: u32, sent_at: f64, now: f64) {
        if self.last_ack == Some(seq) {
            return;
        }
        let ms = (now - sent_at).max(0.0) * 1000.0;
        if let Some(previous) = self.ack_ms {
            self.jitter_ms += 0.1 * ((ms - previous).abs() - self.jitter_ms);
        }
        self.last_ack = Some(seq);
        self.ack_ms = Some(ms);
        self.ack_at = Some(now);
    }
}

/// Displayed displacement across reconciliation; snapshots can have different ticks.
pub(crate) fn reconcile_shift(
    previous: &DreamPresentation,
    next: &DreamPresentation,
    changed_epoch: bool,
    client_id: u64,
) -> Option<f32> {
    if changed_epoch
        || previous.stamp.match_epoch != next.stamp.match_epoch
        || previous.room != next.room
        || !has_local_hero(previous, client_id)
        || !has_local_hero(next, client_id)
    {
        return None;
    }
    let distance =
        Vec2::from_array(previous.hero.position).distance(Vec2::from_array(next.hero.position));
    distance.is_finite().then_some(distance)
}

fn shift_text(telemetry: &Telemetry, available: bool, now: f64) -> String {
    if available {
        if let Some((distance, at)) = telemetry.reconcile_shift {
            return format!(
                "{distance:.3} wu ({:.0} ms ago)",
                (now - at).max(0.0) * 1000.0
            );
        }
    }
    "--".into()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sample(
    time: Res<Time<Real>>,
    runtime: Option<NonSend<Runtime>>,
    telemetry: Res<Telemetry>,
    mut conditioner: ResMut<ConditionerDebug>,
    mut history: ResMut<DiagnosticHistory>,
    view: Res<DreamView>,
    prefs: Res<DreamPreferences>,
    overlay: Res<DebugUiState>,
    mut next_overlay_update: Local<f64>,
    mut rows: Query<(&DebugMetric, &mut Text), Without<OverlayMetric>>,
    mut overlay_rows: Query<(&OverlayMetric, &mut Text), Without<DebugMetric>>,
) {
    let now = time.elapsed_secs_f64();
    let connected = runtime.as_ref().filter(|r| r.renet.is_connected());
    let fresh = connected.filter(|r| r.last_snapshot.is_some() && now - r.last_received < 1.5);
    conditioner.report_rtt(fresh.map(|r| Duration::from_secs_f64(r.renet.rtt().max(0.0))));
    let ack_fresh = fresh.is_some() && telemetry.ack_at.is_some_and(|t| now - t < 1.5);
    history.record(
        now,
        fresh.map_or([None; 5], |r| {
            [
                Some((r.renet.rtt() * 1000.0) as f32),
                ack_fresh.then_some(telemetry.jitter_ms as f32),
                Some((r.renet.packet_loss() * 100.0) as f32),
                Some((r.renet.bytes_received_per_sec() / 1024.0) as f32),
                Some((r.renet.bytes_sent_per_sec() / 1024.0) as f32),
            ]
        }),
    );
    if !overlay.visible {
        *next_overlay_update = 0.0;
    } else if now >= *next_overlay_update {
        *next_overlay_update = now + 0.25;
        let transport = if cfg!(target_arch = "wasm32") {
            "WebRTC"
        } else {
            "UDP"
        };
        let network_text = connected.map_or_else(
            || format!("NETWORK  {transport} · offline / connecting\nRTT -- · Loss -- · ACK --\nSnapshot: waiting"),
            |r| {
                let age = r.last_snapshot.as_ref().map(|_| (now - r.last_received).max(0.0));
                let snapshot = age.map_or("waiting".into(), |age| format!("{:.0} ms{}", age * 1000.0,
                    if age > PREDICTION_MAX_SNAPSHOT_AGE { " · stale" } else { "" }));
                let ack = if ack_fresh { format!("{:.0} ms", telemetry.ack_ms.unwrap_or_default()) } else { "--".into() };
                format!("NETWORK  {transport} · connected\nRTT {:.0} ms · Loss {:.1}% · Input final {ack}\nState age: {snapshot}", r.renet.rtt() * 1000.0, r.renet.packet_loss() * 100.0)
            },
        );
        let prediction_text = connected.filter(|r| r.last_snapshot.is_some()).map_or_else(
            || "PREDICTION  Waiting for state\nServer -- · View -- · Lead --\nPending -- · Replay --\nReconcile shift --".into(),
            |r| {
                let metrics = r.metrics();
                let Some(session) = metrics.session else { return "PREDICTION  Waiting for session".into(); };
                let tick = |value: Option<engine_net::types::ServerTick>| value.map_or("--".into(), |t| t.0.to_string());
                let status = if session.recovering {
                    "Recovering"
                } else if !session.active {
                    "Waiting for activation"
                } else if now - metrics.last_received_seconds > PREDICTION_MAX_SNAPSHOT_AGE {
                    "STALLED · state >300 ms"
                } else if view.0.paused {
                    "Simulation paused"
                } else if prefs.paused || prefs.build_open {
                    "Active · input paused"
                } else if view.0.phase != RunPhase::Combat {
                    "Active · noncombat"
                } else { "Active" };
                let slack = session.arrival_slack.map_or("--".into(), |n| n.to_string());
                format!(concat!(
                    "PREDICTION  {}\nState {} · Final {} · Predicted {}\n",
                    "Estimate {} · Lead {} · Last slack {}\nPending {} · Replay {} · New {}\n",
                    "ARRIVAL  {} unique · {} late · Policy {}\nFeedback {} applied / {} prior · Command {}@{}\n",
                    "History {:.1} KiB · Game {} · Shift {}\n",
                    "SCOPES  {} live / {} known · {} predicted groups\n",
                    "Staging {} transfers / {} groups · {:.1} KiB\n",
                    "Replicas {:.1} KiB · Epoch {}/{}\n",
                    "REMOTE  {} blended · {} frozen · {} samples\n",
                    "{} active / {} known · {:.1} KiB · t {:.2}s\n",
                    "DELIVERY  {} groups · {} stale · {} superseded\n",
                    "Replay max {} ticks · {:.2} ms"
                ), status, tick(session.checkpoint_server_tick), session.finalized_tick.0, tick(session.predicted_tick),
                    tick(session.estimated_server_tick), session.lead_ticks, slack, metrics.pending_commands,
                    session.replay_ticks, session.predicted_ticks_this_frame,
                    session.arrival_feedback.observed_commands, session.arrival_feedback.late_commands, session.lead_policy,
                    session.arrival_feedback_applied, session.arrival_feedback_ignored,
                    session.arrival_feedback.sample.map_or("--".into(), |sample| sample.sequence.0.to_string()),
                    session.arrival_feedback.sample.map_or("--".into(), |sample| sample.target.0.to_string()),
                    session.prediction_bytes as f64 / 1024.0,
                    session.gameplay_tick.map_or("--".into(), |t| t.to_string()), shift_text(&telemetry, metrics.transport_connected, now), session.live_scopes, session.known_scopes,
                    session.prediction_groups, session.incomplete_fragments, session.incomplete_groups,
                    (session.fragment_bytes + session.group_bytes) as f64 / 1024.0, session.scope_bytes as f64 / 1024.0,
                    session.match_epoch, session.connection_epoch.0, session.interpolation.interpolated,
                    session.interpolation.frozen, session.interpolation.retained_samples, session.interpolation.active_entities,
                    session.interpolation.known_entities, session.interpolation.retained_bytes as f64 / 1024.0, session.interpolation.cursor.as_secs_f64(),
                    session.delivery.groups_completed, session.delivery.fragment_stale, session.delivery.partial_superseded,
                    session.delivery.maximum_replay_requested, session.delivery.maximum_replay_cpu_micros as f64 / 1000.0)
            },
        );
        for (metric, mut text) in &mut overlay_rows {
            let next = match metric {
                OverlayMetric::Network => &network_text,
                OverlayMetric::Prediction => &prediction_text,
            };
            if text.0 != *next {
                text.0.clone_from(next);
            }
        }
    }
    if !conditioner.visible {
        return;
    }
    let metrics = connected.map(|r| r.metrics());
    for (metric, mut text) in &mut rows {
        let next =
            match metric {
                DebugMetric::Connection => if connected.is_some() {
                    if cfg!(target_arch = "wasm32") {
                        "WebRTC / connected"
                    } else {
                        "UDP / connected"
                    }
                } else {
                    "Offline / connecting"
                }
                .into(),
                DebugMetric::Conditioning => if conditioner.handle().is_active() {
                    "ACTIVE"
                } else {
                    "Off"
                }
                .into(),
                DebugMetric::Rtt => {
                    fresh.map_or("--".into(), |r| format!("{:.0} ms", r.renet.rtt() * 1000.0))
                }
                DebugMetric::AckJitter => {
                    if ack_fresh {
                        format!("{:.1} ms", telemetry.jitter_ms)
                    } else {
                        "--".into()
                    }
                }
                DebugMetric::InputAck => {
                    if ack_fresh {
                        format!("{:.0} ms", telemetry.ack_ms.unwrap_or_default())
                    } else {
                        "--".into()
                    }
                }
                DebugMetric::Loss => fresh.map_or("--".into(), |r| {
                    format!("{:.1}%", r.renet.packet_loss() * 100.0)
                }),
                DebugMetric::Sent => fresh.map_or("--".into(), |r| {
                    format!("{:.1} KiB/s", r.renet.bytes_sent_per_sec() / 1024.0)
                }),
                DebugMetric::Received => fresh.map_or("--".into(), |r| {
                    format!("{:.1} KiB/s", r.renet.bytes_received_per_sec() / 1024.0)
                }),
                DebugMetric::SnapshotAge => connected
                    .filter(|r| r.last_snapshot.is_some())
                    .map_or("--".into(), |r| {
                        format!("{:.0} ms", (now - r.last_received).max(0.0) * 1000.0)
                    }),
                DebugMetric::InputBuffer => {
                    connected.map_or("--".into(), |r| r.pending.len().to_string())
                }
                DebugMetric::Messages => format!("{} scoped updates", telemetry.snapshots),
                DebugMetric::DecodeErrors => telemetry.decode_errors.to_string(),
                DebugMetric::DecodeIssue => format!(
                    "Last decode issue: {}",
                    telemetry.decode_issue.as_deref().unwrap_or("--")
                ),
                DebugMetric::RecoveryIssue => format!(
                    "Last recovery issue: {}",
                    telemetry.recovery_issue.as_deref().unwrap_or("--")
                ),
                DebugMetric::TransportIssue => format!(
                    "Last transport issue: {}",
                    telemetry.transport_issue.as_deref().unwrap_or("--")
                ),
                DebugMetric::TransportErrors => telemetry.transport_errors.to_string(),
                DebugMetric::BaselineMisses => metrics
                    .as_ref()
                    .and_then(|m| m.session.as_ref())
                    .map_or("--".into(), |s| {
                        format!(
                            "{} repairs · {} KiB",
                            s.baseline_repairs,
                            s.baseline_bytes / 1024
                        )
                    }),
                DebugMetric::Interpolation => metrics
                    .as_ref()
                    .and_then(|m| m.session.as_ref())
                    .map_or("--".into(), |s| {
                        format!(
                            "{} blended / {} frozen",
                            s.interpolation.interpolated, s.interpolation.frozen
                        )
                    }),
                DebugMetric::Correction => {
                    format!("shift {}", shift_text(&telemetry, connected.is_some(), now))
                }
                DebugMetric::BrowserDrops => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        connected.map_or("--".into(), |r| {
                            let s = r.transport.stats();
                            format!(
                                "{} / {} / {}",
                                s.send_backpressure_drops,
                                s.receive_overflow_drops,
                                s.receive_invalid_size_drops
                            )
                        })
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        "N/A · UDP".into()
                    }
                }
            };
        if text.0 != next {
            text.0 = next;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use dreamwake_sim::DreamSimulation;
    #[test]
    fn reconciliation_measures_world_displacement_and_rejects_context_changes() {
        let previous = crate::offline_presentation(DreamSimulation::new(7, false).snapshot());
        let id = previous.hero.id;
        let mut next = previous.clone();
        next.hero.position = [
            previous.hero.position[0] + 3.0,
            previous.hero.position[1] + 4.0,
        ];
        next.heroes[0] = next.hero.clone();
        next.tick += 3;
        assert_eq!(reconcile_shift(&previous, &next, false, id), Some(5.0));
        assert_eq!(reconcile_shift(&previous, &next, true, id), None);
        assert_eq!(reconcile_shift(&previous, &next, false, id + 1), None);
        for change in 0..3 {
            let mut unrelated = next.clone();
            match change {
                0 => unrelated.room += 1,
                1 => unrelated.stamp.match_epoch += 1,
                _ => unrelated.heroes.clear(),
            }
            assert_eq!(reconcile_shift(&previous, &unrelated, false, id), None);
        }
    }

    #[test]
    fn reconciliation_samples_clear_when_unavailable_or_epoch_resets() {
        let mut telemetry = Telemetry::default();
        assert_eq!(shift_text(&telemetry, true, 1.0), "--");
        telemetry.record_reconciliation(18, Some(0.25), 1.0);
        assert_eq!(telemetry.replay_count, Some(18));
        assert_eq!(shift_text(&telemetry, true, 1.5), "0.250 wu (500 ms ago)");
        assert_eq!(shift_text(&telemetry, false, 1.5), "--");
        telemetry.record_reconciliation(0, None, 2.0);
        assert_eq!(shift_text(&telemetry, true, 2.0), "--");
        telemetry.record_reconciliation(3, Some(1.0), 3.0);
        telemetry.reset_samples();
        assert_eq!(telemetry.replay_count, None);
        assert_eq!(telemetry.reconcile_shift, None);
    }
    #[test]
    fn repeated_ack_does_not_inflate_latency_and_epoch_resets_samples() {
        let mut telemetry = Telemetry::default();
        telemetry.acknowledge(7, 1.0, 1.1);
        telemetry.acknowledge(7, 1.0, 2.0);
        assert!((telemetry.ack_ms.unwrap() - 100.0).abs() < 0.001);
        telemetry.acknowledge(8, 2.0, 2.2);
        assert!((telemetry.jitter_ms - 10.0).abs() < 0.001);
        telemetry.reset_samples();
        assert!(telemetry.ack_ms.is_none());
        telemetry.acknowledge(1, 3.0, 3.05);
        assert_eq!(telemetry.jitter_ms, 0.0);
    }
    #[test]
    fn decode_classification_is_exact_lifetime_and_cannot_hide_a_hard_failure() {
        let mut telemetry = Telemetry::default();
        telemetry.record_decode_rejection("WrongEpoch");
        assert_eq!(
            (
                telemetry.decode_errors,
                telemetry.stale_epoch_rejections,
                telemetry.decode_failures
            ),
            (1, 1, 0)
        );
        telemetry.record_decode_rejection("InvalidState");
        telemetry.record_decode_rejection("WrongEpoch");
        assert_eq!(telemetry.decode_issue.as_deref(), Some("WrongEpoch"));
        assert_eq!(
            (
                telemetry.decode_errors,
                telemetry.stale_epoch_rejections,
                telemetry.decode_failures
            ),
            (3, 2, 1)
        );
        telemetry.reset_samples();
        assert_eq!(
            (
                telemetry.decode_errors,
                telemetry.stale_epoch_rejections,
                telemetry.decode_failures
            ),
            (3, 2, 1)
        );
        for reason in [
            "WrongEpoch ",
            "WrongEpoch\n",
            "Codec(WrongEpoch)",
            "wrong_epoch",
        ] {
            telemetry.record_decode_rejection(reason);
        }
        assert_eq!(telemetry.stale_epoch_rejections, 2);
        assert_eq!(telemetry.decode_failures, 5);
        assert_eq!(
            telemetry.decode_errors,
            telemetry.stale_epoch_rejections
                + telemetry.clock_resync_rejections
                + telemetry.obsolete_rejections
                + telemetry.decode_failures
        );
    }
    #[test]
    fn typed_clock_resync_is_separate_from_malformed_errors_and_survives_reset() {
        use engine_net::synchronization::{ResyncReason, SyncError};
        let mut telemetry = Telemetry::default();
        for reason in [
            ResyncReason::LeadExceeded,
            ResyncReason::Suspension,
            ResyncReason::OffsetJump,
            ResyncReason::TargetDiscontinuity,
        ] {
            telemetry.record_clock_rejection(SyncError::ResyncRequired(reason));
        }
        assert_eq!(telemetry.clock_resync_rejections, 4);
        assert_eq!(telemetry.decode_failures, 0);
        telemetry.reset_samples();
        assert_eq!(telemetry.clock_resync_rejections, 4);
        assert_eq!(
            telemetry.decode_issue.as_deref(),
            Some("clock synchronization: ResyncRequired(TargetDiscontinuity)")
        );
        for error in [
            SyncError::InvalidSample,
            SyncError::InvalidConfiguration,
            SyncError::NotSynchronized,
            SyncError::NonMonotonicTime,
            SyncError::TimeRange,
        ] {
            telemetry.record_clock_rejection(error);
        }
        // A string that resembles a recoverable error has no typed authority.
        telemetry.record_decode_rejection("clock synchronization: ResyncRequired(Suspension)");
        telemetry.record_decode_rejection("WrongEpoch");
        assert_eq!(
            (
                telemetry.decode_errors,
                telemetry.stale_epoch_rejections,
                telemetry.clock_resync_rejections,
                telemetry.decode_failures
            ),
            (11, 1, 4, 6)
        );
    }
    #[test]
    fn typed_obsolete_rejections_are_bounded_lifetime_and_do_not_hide_stale_errors() {
        let mut telemetry = Telemetry::default();
        for reason in [
            ObsoleteReason::Scope,
            ObsoleteReason::Publication,
            ObsoleteReason::RetiredBaseline,
        ] {
            telemetry.record_obsolete_rejection(reason, "snapshot");
        }
        telemetry.reset_samples();
        assert_eq!(telemetry.obsolete_rejections, 3);
        assert_eq!(telemetry.decode_errors, 3);
        assert_eq!(telemetry.decode_failures, 0);
        let reason = ObsoleteReason::Scope;
        telemetry.record_obsolete_rejection(reason, &format!("snapshot\n{}", "x".repeat(300)));
        let issue = telemetry.decode_issue.as_ref().unwrap();
        assert!(issue.starts_with(&format!("obsolete {reason}: snapshot")));
        assert_eq!(issue.chars().count(), 160);
        assert!(!issue.chars().any(char::is_control));
        telemetry.record_decode_rejection("Stale");
        telemetry.record_decode_rejection("Obsolete(Scope)");
        assert_eq!(telemetry.decode_failures, 2);
        assert_eq!(telemetry.obsolete_rejections, 4);
        assert_eq!(telemetry.decode_errors, 6);
    }
    #[test]
    fn obsolete_partition_saturation_keeps_latest_issue_visible() {
        let mut telemetry = Telemetry {
            decode_errors: u64::MAX - 1,
            decode_failures: u64::MAX - 1,
            ..Default::default()
        };
        telemetry.record_obsolete_rejection(ObsoleteReason::Scope, "snapshot");
        telemetry.record_decode_rejection("Stale");
        telemetry.record_obsolete_rejection(ObsoleteReason::Publication, "delta");
        telemetry.reset_samples();
        assert_eq!(telemetry.decode_errors, u64::MAX);
        assert_eq!(telemetry.obsolete_rejections, 1);
        assert_eq!(
            telemetry.decode_errors,
            telemetry.stale_epoch_rejections
                + telemetry.clock_resync_rejections
                + telemetry.obsolete_rejections
                + telemetry.decode_failures
        );
        assert_eq!(
            telemetry.decode_issue,
            Some(format!("obsolete {}: delta", ObsoleteReason::Publication))
        );
    }
    #[test]
    fn lifetime_rejection_partition_freezes_together_but_latest_issue_remains_visible() {
        use engine_net::synchronization::{ResyncReason, SyncError};
        let mut telemetry = Telemetry {
            decode_errors: u64::MAX - 1,
            decode_failures: u64::MAX - 1,
            ..Default::default()
        };
        telemetry.record_clock_rejection(SyncError::ResyncRequired(ResyncReason::Suspension));
        telemetry.record_decode_rejection("WrongEpoch");
        telemetry.record_obsolete_rejection(ObsoleteReason::Scope, "snapshot");
        telemetry.record_clock_rejection(SyncError::InvalidSample);
        telemetry.reset_samples();
        assert_eq!(telemetry.decode_errors, u64::MAX);
        assert_eq!(
            telemetry.decode_errors,
            telemetry.stale_epoch_rejections
                + telemetry.clock_resync_rejections
                + telemetry.obsolete_rejections
                + telemetry.decode_failures
        );
        assert_eq!(telemetry.clock_resync_rejections, 1);
        assert_eq!(telemetry.stale_epoch_rejections, 0);
        assert_eq!(
            telemetry.decode_issue.as_deref(),
            Some("clock synchronization: InvalidSample")
        );
    }
}

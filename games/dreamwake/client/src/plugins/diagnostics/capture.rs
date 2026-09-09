//! Dreamwake telemetry adapter for the shared network tools. No UI or transport ownership.
use super::overlay::{DiagnosticsOverlay, OverlayMetric};
use crate::plugins::network::{PREDICTION_MAX_SNAPSHOT_AGE, Runtime, has_local_hero};
use crate::{DreamPreferences, DreamView};
use bevy::prelude::*;
use bevy_net_debug::ConditionerDebug;
use dreamwake_sim::{DreamSnapshot, RunPhase};
use engine_client::network_tools::graphs::{DebugMetric, DiagnosticHistory};
use std::time::Duration;

#[derive(Resource, Default)]
pub(crate) struct Telemetry {
    pub snapshots: u64,
    pub decode_errors: u64,
    pub transport_errors: u64,
    last_ack: Option<u32>,
    ack_ms: Option<f64>,
    ack_at: Option<f64>,
    jitter_ms: f64,
    replay_count: Option<usize>,
    reconcile_shift: Option<(f32, f64)>,
}
impl Telemetry {
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
    previous: &DreamSnapshot,
    next: &DreamSnapshot,
    changed_epoch: bool,
    client_id: u64,
) -> Option<f32> {
    if changed_epoch
        || previous.seed != next.seed
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
    overlay: Res<DiagnosticsOverlay>,
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
                format!("NETWORK  {transport} · connected\nRTT {:.0} ms · Loss {:.1}% · ACK {ack}\nSnapshot: {snapshot}", r.renet.rtt() * 1000.0, r.renet.packet_loss() * 100.0)
            },
        );
        let prediction_text = connected.filter(|r| r.last_snapshot.is_some()).map_or_else(
            || "PREDICTION  Waiting for state\nServer -- · View -- · Lead --\nPending -- · Replay --\nReconcile shift --".into(),
            |r| {
                let server_tick = r.last_snapshot.as_ref().unwrap().tick;
                let lead = i64::from(view.0.tick) - i64::from(server_tick);
                let status = if now - r.last_received > PREDICTION_MAX_SNAPSHOT_AGE {
                    "STALLED · snapshot >300 ms"
                } else if r.prediction.is_none() {
                    "Waiting for prediction"
                } else if view.0.paused {
                    "Simulation paused"
                } else if prefs.paused || prefs.build_open {
                    "Active · input paused"
                } else if view.0.phase != RunPhase::Combat {
                    "Active · noncombat"
                } else { "Active" };
                let replay = telemetry.replay_count.map_or("--".into(), |count| count.to_string());
                format!("PREDICTION  {status}\nServer {server_tick} · View {} · Lead {lead:+}\nPending {} · Replay {replay}\nReconcile shift {}", view.0.tick, r.pending.len(), shift_text(&telemetry, true, now))
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
    for (metric, mut text) in &mut rows {
        let next = match metric {
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
            DebugMetric::Messages => format!("{} full", telemetry.snapshots),
            DebugMetric::DecodeErrors => telemetry.decode_errors.to_string(),
            DebugMetric::TransportErrors => telemetry.transport_errors.to_string(),
            DebugMetric::BaselineMisses => "N/A · full snapshots".into(),
            DebugMetric::Interpolation => "N/A · prediction/replay".into(),
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
        let previous = DreamSimulation::new(7, false).snapshot();
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
                1 => unrelated.seed += 1,
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
}

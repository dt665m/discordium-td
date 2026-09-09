//! Dreamwake telemetry adapter for the shared network tools. No UI or transport ownership.
use super::*;
use game_client::network_tools::graphs::{DebugMetric, DiagnosticHistory};

#[derive(Resource, Default)]
pub(super) struct Telemetry {
    pub snapshots: u64,
    pub decode_errors: u64,
    pub transport_errors: u64,
    last_ack: Option<u32>,
    ack_ms: Option<f64>,
    ack_at: Option<f64>,
    jitter_ms: f64,
}
impl Telemetry {
    pub fn reset_ack(&mut self) {
        self.last_ack = None;
        self.ack_ms = None;
        self.ack_at = None;
        self.jitter_ms = 0.0;
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
pub(super) fn sample(
    time: Res<Time<Real>>,
    runtime: Option<NonSend<Runtime>>,
    telemetry: Res<Telemetry>,
    mut conditioner: ResMut<ConditionerDebug>,
    mut history: ResMut<DiagnosticHistory>,
    mut rows: Query<(&DebugMetric, &mut Text)>,
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
    if !conditioner.visible {
        return;
    }
    for (metric, mut text) in &mut rows {
        text.0 = match metric {
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
            DebugMetric::Correction => "Not sampled".into(),
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
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_ack_does_not_inflate_latency_and_epoch_resets_samples() {
        let mut telemetry = Telemetry::default();
        telemetry.acknowledge(7, 1.0, 1.1);
        telemetry.acknowledge(7, 1.0, 2.0);
        assert!((telemetry.ack_ms.unwrap() - 100.0).abs() < 0.001);
        telemetry.acknowledge(8, 2.0, 2.2);
        assert!((telemetry.jitter_ms - 10.0).abs() < 0.001);
        telemetry.reset_ack();
        assert!(telemetry.ack_ms.is_none());
        telemetry.acknowledge(1, 3.0, 3.05);
        assert_eq!(telemetry.jitter_ms, 0.0);
    }
}

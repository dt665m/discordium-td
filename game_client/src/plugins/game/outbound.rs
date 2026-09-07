//! Real-time send cadence and bounded redundant input delivery.
use super::*;

/// Wall-clock network cadence, independent of Bevy's simulation clock.
/// Missed slots coalesce into one send of current state, never a catch-up burst.
#[derive(Default)]
pub(super) struct SendCadence {
    next: Option<Duration>,
}

impl SendCadence {
    const PERIOD: Duration = Duration::from_nanos(1_000_000_000 / 30);

    pub(super) fn ready(&mut self, now: Duration) -> bool {
        let next = self.next.unwrap_or(now);
        if now < next {
            return false;
        }
        let remainder = (now - next).as_nanos() % Self::PERIOD.as_nanos();
        self.next = Some(now + Self::PERIOD - Duration::from_nanos(remainder as u64));
        true
    }
}

/// Batch current movement, baseline ACKs and queued reliable actions on the
/// independent network clock. Receive/update still runs every rendered frame.
pub(super) fn network_send(
    time: Res<Time<Real>>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
    snapshot_buffer: Res<SnapshotBuffer>,
    mut debug_bridge: ResMut<ClientDebugBridgeState>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };
    if runtime.renet.disconnect_reason().is_some() || !runtime.send_cadence.ready(time.elapsed()) {
        return;
    }
    if runtime.renet.is_connected() {
        let moves = runtime
            .pending_moves
            .iter()
            .skip(runtime.pending_moves.len().saturating_sub(16))
            .map(|pm| (pm.seq, pm.dir))
            .collect::<Vec<_>>();
        if !moves.is_empty() || !runtime.unacked_actions.is_empty() {
            let match_epoch = runtime.match_epoch;
            let actions = runtime.unacked_actions.iter().copied().collect();
            runtime.renet.send_message(
                DefaultChannel::Unreliable,
                encode(&ClientMoveBundle {
                    moves,
                    actions,
                    match_epoch,
                }),
            );
        }
        // Repeat ACKs to recover loss even if no newer snapshot arrives.
        if let Some(latest) = snapshot_buffer.snapshots.back() {
            runtime.renet.send_message(
                DefaultChannel::Unreliable,
                encode(&game_shared::ClientAck {
                    tick: latest.server_tick,
                }),
            );
        }
    }
    if let Err(err) = runtime.transport_send_packets() {
        debug_bridge.replication.transport_errors += 1;
        log::warn!("client send_packets error: {err}");
    }
}

#[cfg(test)]
mod send_cadence_tests {
    use super::*;

    #[test]
    fn render_rates_do_not_change_network_rate() {
        for fps in [30, 60, 120, 144, 240] {
            let mut cadence = SendCadence::default();
            let sends = (0..fps)
                .filter(|frame| cadence.ready(Duration::from_secs_f64(*frame as f64 / fps as f64)))
                .count();
            assert_eq!(sends, 30, "render FPS {fps}");
        }
    }

    #[test]
    fn stalls_coalesce_without_drifting_or_replaying_send_slots() {
        let mut cadence = SendCadence::default();
        assert!(cadence.ready(Duration::ZERO));
        let late = SendCadence::PERIOD * 10 + Duration::from_millis(5);
        assert!(cadence.ready(late));
        assert!(!cadence.ready(late));
        assert!(!cadence.ready(SendCadence::PERIOD * 11 - Duration::from_nanos(1)));
        assert!(cadence.ready(SendCadence::PERIOD * 11));
    }
}

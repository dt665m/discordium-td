use std::time::Duration;
/// One fixed-step clock owns simulation, publication and the transport send tick.
/// Socket/handshake polling runs more frequently, independently of this clock.
/// This is transport policy (bounded catch-up and coalesced publication), not an
/// application clock. Bevy's app runner/Time<Real> supply elapsed time; its
/// virtual-time FixedUpdate loop does not implement this drop/cadence contract.
pub struct TickClock {
    period: Duration,
    snapshot_interval: u32,
    next_tick: Duration,
    ticks_since_snapshot: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn polling_rate_does_not_change_ticks_or_snapshot_cadence() {
        let mut clock = TickClock::new(60, 3);
        let (mut steps, mut sends, mut snapshots) = (0, 0, 0);
        for ms in 0..3000 {
            let tick = clock.advance(Duration::from_millis(ms), || steps += 1);
            sends += usize::from(tick.send_due);
            snapshots += usize::from(tick.snapshot_due);
        }
        assert_eq!((steps, sends, snapshots), (180, 180, 60));
    }
    #[test]
    fn suspension_bounds_catchup_and_counts_only_executed_ticks() {
        let mut clock = TickClock::new(60, 3);
        let mut steps = 0;
        assert!(!clock.advance(Duration::ZERO, || steps += 1).snapshot_due);
        let resumed = Duration::from_secs(600);
        assert!(clock.advance(resumed, || steps += 1).snapshot_due);
        assert_eq!(steps, 7);
        assert!(!clock.advance(resumed, || steps += 1).send_due);
    }
}

#[derive(Default)]
pub struct TickDecision {
    pub send_due: bool,
    pub snapshot_due: bool,
}

impl TickClock {
    pub fn new(tick_hz: u32, snapshot_interval: u32) -> Self {
        assert!(tick_hz > 0 && tick_hz <= 1_000_000_000 && snapshot_interval > 0);
        Self {
            period: Duration::from_nanos(1_000_000_000 / tick_hz as u64),
            snapshot_interval,
            next_tick: Duration::ZERO,
            ticks_since_snapshot: 0,
        }
    }

    /// Run due steps and publish at most the latest completed state after catch-up.
    pub fn advance(&mut self, now: Duration, mut step: impl FnMut()) -> TickDecision {
        let mut tick = TickDecision::default();
        let mut steps = 0;
        while now >= self.next_tick && steps < 6 {
            step();
            tick.send_due = true;
            self.next_tick += self.period;
            steps += 1;
            self.ticks_since_snapshot += 1;
            if self.ticks_since_snapshot == self.snapshot_interval {
                self.ticks_since_snapshot = 0;
                tick.snapshot_due = true;
            }
        }
        // Preserve the existing bounded catch-up policy after OS suspension.
        // Dropped wall time never counts as executed simulation ticks.
        if now >= self.next_tick {
            self.next_tick = now + self.period;
        }
        tick
    }
}

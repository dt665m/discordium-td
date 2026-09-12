//! Rational fixed-tick dispatch with bounded catch-up and visible scheduling debt.
//! The adapter supplies monotonic elapsed time; the clock never reads wall time.
use crate::types::{ServerTick, TickRate};
use std::time::Duration;

pub struct TickClock {
    rate: TickRate,
    snapshot_interval: u32,
    committed: ServerTick,
    last_observed: Duration,
    catchup_limit: u32,
    last_flush_wall_tick: ServerTick,
    pending_publication: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct TickContext {
    /// The command for this tick advances S[k-1] to end-of-tick S[k].
    pub tick: ServerTick,
    pub deadline: Duration,
    pub observed_now: Duration,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickHealth {
    pub committed: ServerTick,
    /// Authoritative steps still owed; these are never silently skipped.
    pub debt_ticks: u64,
    pub overloaded: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TickDecision {
    pub send_due: bool,
    pub snapshot_due: bool,
    pub executed_ticks: u32,
    pub health: TickHealth,
}

impl TickClock {
    pub fn new(tick_hz: u32, snapshot_interval: u32) -> Self {
        assert!(snapshot_interval > 0);
        Self {
            rate: TickRate::new(tick_hz).expect("invalid simulation tick rate"),
            snapshot_interval,
            committed: ServerTick(0),
            last_observed: Duration::ZERO,
            catchup_limit: 4,
            last_flush_wall_tick: ServerTick(0),
            pending_publication: false,
        }
    }

    pub fn with_catchup_limit(mut self, ticks: u32) -> Self {
        assert!((1..=1024).contains(&ticks));
        self.catchup_limit = ticks;
        self
    }

    pub fn catchup_limit(&self) -> u32 {
        self.catchup_limit
    }

    pub fn health(&self, now: Duration) -> TickHealth {
        let due = self
            .rate
            .elapsed_ticks(now.max(self.last_observed))
            .unwrap_or(ServerTick(u64::MAX));
        let debt_ticks = due.0.saturating_sub(self.committed.0);
        TickHealth {
            committed: self.committed,
            debt_ticks,
            overloaded: debt_ticks > u64::from(self.catchup_limit),
        }
    }

    /// Compatibility callback for callers that do not need typed tick metadata.
    pub fn advance(&mut self, now: Duration, mut step: impl FnMut()) -> TickDecision {
        self.advance_ticked(now, |_| step())
    }

    /// Initialize at S[0], then execute at most one configured burst of due ticks.
    /// Several overdue publications coalesce to the latest complete committed S[k].
    /// Retained debt drives admission shedding instead of silently changing the epoch.
    pub fn advance_ticked(
        &mut self,
        now: Duration,
        mut step: impl FnMut(TickContext),
    ) -> TickDecision {
        let now = now.max(self.last_observed);
        self.last_observed = now;
        let before = self.health(now);
        let mut decision = TickDecision::default();
        for _ in 0..before.debt_ticks.min(u64::from(self.catchup_limit)) {
            let Some(tick) = self.committed.checked_next() else {
                break;
            };
            let Some(deadline) = self.rate.deadline(tick) else {
                break;
            };
            step(TickContext {
                tick,
                deadline,
                observed_now: now,
            });
            self.committed = tick;
            decision.executed_ticks += 1;
            self.pending_publication |= tick.0.is_multiple_of(u64::from(self.snapshot_interval));
        }
        // Catch-up may service several bursts at one observed wall time. Keep
        // network flushes paced and coalesce unsent publications into current S[k].
        let wall_tick = self.rate.elapsed_ticks(now).unwrap_or(ServerTick(u64::MAX));
        if wall_tick > self.last_flush_wall_tick {
            self.last_flush_wall_tick = wall_tick;
            decision.send_due = true;
            decision.snapshot_due = std::mem::take(&mut self.pending_publication);
        }
        decision.health = self.health(now);
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_rate_does_not_change_ticks_or_snapshot_cadence() {
        let mut clock = TickClock::new(60, 3);
        let (mut steps, mut sends, mut snapshots) = (0, 0, 0);
        assert_eq!(
            clock.advance(Duration::ZERO, || steps += 1).executed_ticks,
            0
        );
        for ms in 1..=3000 {
            let tick = clock.advance(Duration::from_millis(ms), || steps += 1);
            sends += usize::from(tick.send_due);
            snapshots += usize::from(tick.snapshot_due);
        }
        assert_eq!((steps, sends, snapshots), (180, 180, 60));
        assert_eq!(clock.health(Duration::from_secs(3)).debt_ticks, 0);
    }

    #[test]
    fn suspension_bounds_each_burst_and_preserves_visible_debt() {
        let mut clock = TickClock::new(60, 3);
        let mut executed = Vec::new();
        let resumed = Duration::from_secs(600);
        let first = clock.advance_ticked(resumed, |context| executed.push(context));
        assert_eq!(first.executed_ticks, 4);
        assert_eq!(first.health.debt_ticks, 36_000 - 4);
        assert!(first.health.overloaded && first.snapshot_due);
        let second = clock.advance_ticked(Duration::ZERO, |context| executed.push(context));
        assert_eq!(second.executed_ticks, 4);
        assert_eq!(second.health.debt_ticks, 36_000 - 8);
        assert_eq!(
            executed.iter().map(|c| c.tick.0).collect::<Vec<_>>(),
            (1..=8).collect::<Vec<_>>()
        );
        assert!(
            executed
                .iter()
                .all(|context| context.observed_now == resumed)
        );
        assert_eq!(executed[2].deadline, Duration::from_millis(50));
    }

    #[test]
    fn rational_deadline_and_complete_catchup_do_not_drop_ticks() {
        let mut clock = TickClock::new(60, 2);
        assert!(
            !clock
                .advance(Duration::from_nanos(16_666_666), || {})
                .send_due
        );
        assert_eq!(
            clock
                .advance(Duration::from_nanos(16_666_667), || {})
                .health
                .committed,
            ServerTick(1)
        );
        let now = Duration::from_secs(1);
        while clock.health(now).debt_ticks != 0 {
            clock.advance(now, || {});
        }
        assert_eq!(
            clock.health(now),
            TickHealth {
                committed: ServerTick(60),
                debt_ticks: 0,
                overloaded: false
            }
        );
        assert!(!clock.advance(now, || {}).send_due);
    }
}

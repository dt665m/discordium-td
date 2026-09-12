//! Bounded instance readiness shared with hosting services, never gameplay state.
use engine_net::{clock::TickHealth, types::TickRate};
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    Starting,
    Ready,
    Overloaded,
    Stopped,
    Unavailable,
}

/// Public operational counters contain no player identities or world contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HealthReport {
    pub status: InstanceStatus,
    pub committed_tick: u64,
    pub debt_ticks: u64,
    pub last_poll_age_ms: Option<u64>,
}
impl HealthReport {
    pub fn ready(self) -> bool {
        self.status == InstanceStatus::Ready
    }
}

#[derive(Clone, Default)]
pub struct InstanceHealth(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    rate: Option<TickRate>,
    catchup_limit: u32,
    observation: Option<Observation>,
    stopped: bool,
}
struct Observation {
    elapsed: Duration,
    started_at: Instant,
    health: TickHealth,
}

impl InstanceHealth {
    pub fn report(&self) -> HealthReport {
        self.report_at(Instant::now())
    }

    pub(crate) fn bind(&self, rate: TickRate, catchup_limit: u32) {
        if let Ok(mut state) = self.0.lock() {
            *state = State {
                rate: Some(rate),
                catchup_limit,
                ..Default::default()
            };
        }
    }

    pub(crate) fn observe(&self, elapsed: Duration, started_at: Instant, health: TickHealth) {
        if let Ok(mut state) = self.0.lock() {
            state.observation = Some(Observation {
                elapsed,
                started_at,
                health,
            });
        }
    }

    pub(crate) fn stop(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.stopped = true;
        }
    }

    fn report_at(&self, now: Instant) -> HealthReport {
        let empty = |status| HealthReport {
            status,
            committed_tick: 0,
            debt_ticks: 0,
            last_poll_age_ms: None,
        };
        let Ok(state) = self.0.lock() else {
            return empty(InstanceStatus::Unavailable);
        };
        let Some(observation) = &state.observation else {
            return empty(if state.stopped {
                InstanceStatus::Stopped
            } else {
                InstanceStatus::Starting
            });
        };
        let age = now.saturating_duration_since(observation.started_at);
        // Extrapolate the same rational clock while the simulation thread is
        // stalled. A cached healthy report must not keep admission open forever.
        // The anchor is poll start, so work spent inside poll is included too.
        let due = state.rate.and_then(|rate| {
            observation
                .elapsed
                .checked_add(age)
                .and_then(|elapsed| rate.elapsed_ticks(elapsed))
        });
        let debt_ticks = due.map_or(u64::MAX, |tick| {
            tick.0.saturating_sub(observation.health.committed.0)
        });
        HealthReport {
            status: if state.stopped {
                InstanceStatus::Stopped
            } else if debt_ticks > u64::from(state.catchup_limit) {
                InstanceStatus::Overloaded
            } else {
                InstanceStatus::Ready
            },
            committed_tick: observation.health.committed.0,
            debt_ticks,
            last_poll_age_ms: Some(age.as_millis().min(u128::from(u64::MAX)) as u64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_net::{clock::TickClock, types::ServerTick};

    #[test]
    fn readiness_detects_stalls_without_another_simulation_poll_and_recovers() {
        let shared = InstanceHealth::default();
        let start = Instant::now();
        assert_eq!(shared.report_at(start).status, InstanceStatus::Starting);
        shared.bind(TickRate::new(60).unwrap(), 4);
        let mut clock = TickClock::new(60, 3);
        shared.observe(Duration::ZERO, start, clock.health(Duration::ZERO));
        assert!(shared.report_at(start).ready());
        assert!(shared.report_at(start + Duration::from_millis(83)).ready());
        let stalled = shared.report_at(start + Duration::from_millis(84));
        assert_eq!(stalled.status, InstanceStatus::Overloaded);
        assert_eq!((stalled.committed_tick, stalled.debt_ticks), (0, 5));
        let now = Duration::from_millis(84);
        let tick = clock.advance(now, || {});
        assert_eq!(tick.executed_ticks, 4);
        shared.observe(now, start + now, tick.health);
        let recovered = shared.report_at(start + now);
        assert_eq!(recovered.committed_tick, 4);
        assert_eq!(recovered.debt_ticks, 1);
        assert!(recovered.ready());
        assert_eq!(clock.health(now).committed, ServerTick(4));
        shared.stop();
        assert_eq!(
            shared.report_at(start + now).status,
            InstanceStatus::Stopped
        );
    }

    #[test]
    fn poll_work_and_fractional_deadlines_are_included_in_debt() {
        let shared = InstanceHealth::default();
        let start = Instant::now();
        shared.bind(TickRate::new(60).unwrap(), 4);
        shared.observe(
            Duration::from_nanos(16_666_666),
            start,
            TickHealth::default(),
        );
        let report = shared.report_at(start + Duration::from_nanos(66_666_668));
        assert_eq!(report.debt_ticks, 5);
        assert!(!report.ready());
    }
}

//! How much a peer's transport may hold before it is queueing.
//!
//! A data channel that keeps up holds about one round trip of traffic in
//! flight; more than that waits in a queue and delays every later message.
//! The limit is therefore the measured drain rate times the base round trip,
//! plus a small allowance, as in BBR's bandwidth-delay product. Measuring it
//! instead of fixing a byte count keeps a fast path from shedding needlessly
//! and a slow, lossy one from queueing for half a second.
use std::{collections::VecDeque, time::Duration};

/// Queueing tolerated above one round trip in flight.
const QUEUE_ALLOWANCE: Duration = Duration::from_millis(50);
/// Round-trip samples older than this no longer describe the path.
const BASE_RTT_WINDOW: Duration = Duration::from_secs(10);
const MIN_LIMIT_BYTES: usize = 8 * 1024;
const MAX_LIMIT_BYTES: usize = 48 * 1024;
// The game sheds its own superseded state before the transport has to hold
// anything back.
const _: () = assert!(MAX_LIMIT_BYTES < renet_cross::MAX_PEER_BACKLOG_BYTES);
/// Used until the first drain measurement exists.
const INITIAL_LIMIT_BYTES: usize = 24 * 1024;

#[derive(Debug, Default)]
pub(crate) struct PathEstimate {
    last: Option<(Duration, u64, usize)>,
    drain_bytes_per_second: Option<f64>,
    rtt: VecDeque<(Duration, f64)>,
}

impl PathEstimate {
    /// `written` is the cumulative byte count handed to the transport and
    /// `backlog` what it still holds; `rtt` is the current smoothed round trip.
    pub(crate) fn observe(&mut self, now: Duration, written: u64, backlog: usize, rtt: f64) {
        if rtt.is_finite() && rtt > 0.0 {
            while self
                .rtt
                .front()
                .is_some_and(|(at, _)| now.saturating_sub(*at) > BASE_RTT_WINDOW)
            {
                self.rtt.pop_front();
            }
            // Only a new minimum or a sample at least a second newer is kept,
            // so the window stays small while tracking the running minimum.
            let keep = self.rtt.back().is_none_or(|(at, value)| {
                rtt < *value || now.saturating_sub(*at) >= Duration::from_secs(1)
            });
            if keep {
                self.rtt.push_back((now, rtt));
            }
        }
        if let Some((at, previous_written, previous_backlog)) = self.last {
            let elapsed = now.saturating_sub(at).as_secs_f64();
            if elapsed >= 0.04 {
                let left = written.saturating_sub(previous_written) as f64
                    - (backlog as f64 - previous_backlog as f64);
                let rate = (left.max(0.0)) / elapsed;
                self.drain_bytes_per_second = Some(
                    self.drain_bytes_per_second
                        .map_or(rate, |old| old * 0.8 + rate * 0.2),
                );
                self.last = Some((now, written, backlog));
            }
        } else {
            self.last = Some((now, written, backlog));
        }
    }

    /// Backlog above which superseded state should wait.
    pub(crate) fn limit_bytes(&self) -> usize {
        let base_rtt = self
            .rtt
            .iter()
            .map(|(_, value)| *value)
            .fold(f64::INFINITY, f64::min);
        match self.drain_bytes_per_second {
            Some(rate) if base_rtt.is_finite() => {
                let limit = rate * (base_rtt + QUEUE_ALLOWANCE.as_secs_f64());
                (limit as usize).clamp(MIN_LIMIT_BYTES, MAX_LIMIT_BYTES)
            }
            _ => INITIAL_LIMIT_BYTES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rate: f64, rtt: f64) -> PathEstimate {
        let mut path = PathEstimate::default();
        let mut written = 0u64;
        for step in 0..100u64 {
            let now = Duration::from_millis(step * 50);
            written += (rate * 0.05) as u64;
            // A path that keeps up holds one round trip in flight.
            path.observe(now, written, (rate * rtt) as usize, rtt);
        }
        path
    }

    #[test]
    fn limit_is_one_round_trip_in_flight_plus_a_short_queue() {
        // 60 KB/s over 150 ms: 9 KB in flight, limit 12 KB.
        let lossy = run(60_000.0, 0.15);
        assert!(
            (11_500..=12_500).contains(&lossy.limit_bytes()),
            "{}",
            lossy.limit_bytes()
        );
        // A fast path may hold more before it counts as queueing.
        let fast = run(200_000.0, 0.15);
        assert!(fast.limit_bytes() > lossy.limit_bytes());
        // Bounded both ways.
        assert_eq!(run(1_000.0, 0.02).limit_bytes(), MIN_LIMIT_BYTES);
        assert_eq!(run(2_000_000.0, 0.3).limit_bytes(), MAX_LIMIT_BYTES);
        assert_eq!(PathEstimate::default().limit_bytes(), INITIAL_LIMIT_BYTES);
    }

    #[test]
    fn base_round_trip_ignores_queueing_inflation() {
        let mut path = run(60_000.0, 0.15);
        let before = path.limit_bytes();
        // The smoothed round trip grows while a queue forms; the base does not.
        for step in 100..140u64 {
            path.observe(
                Duration::from_millis(step * 50),
                300_000 + step * 3000,
                9000,
                1.2,
            );
        }
        assert!(path.limit_bytes() <= before + 1000);
    }
}

//! Monotonic clock estimation, bounded prediction lead and presentation time.
//!
//! Four timestamps remove server processing from network RTT. Offset still assumes
//! path symmetry; arrival-slack feedback corrects useful command lead without
//! claiming an exact uplink measurement. These clocks schedule fixed simulation
//! steps; they never alter simulation dt or rewrite an already assigned target.
use crate::types::{ServerTick, TargetTick, TickRate};
use std::{cmp::Reverse, collections::VecDeque, time::Duration};

mod committed;
pub use committed::CommittedClock;
#[cfg(test)]
mod committed_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeExchange {
    pub client_send: Duration,
    pub server_receive: Duration,
    pub server_send: Duration,
    pub client_receive: Duration,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSample {
    pub client_send: Duration,
    pub network_round_trip: Duration,
    pub server_processing: Duration,
    pub offset_nanos: i128,
    pub client_receive: Duration,
}
impl ClockSample {
    pub fn from_exchange(exchange: TimeExchange) -> Result<Self, SyncError> {
        let local = exchange
            .client_receive
            .checked_sub(exchange.client_send)
            .ok_or(SyncError::InvalidSample)?;
        let processing = exchange
            .server_send
            .checked_sub(exchange.server_receive)
            .ok_or(SyncError::InvalidSample)?;
        let network_round_trip = local
            .checked_sub(processing)
            .ok_or(SyncError::InvalidSample)?;
        let offset_nanos = ((nanos(exchange.server_receive) - nanos(exchange.client_send))
            + (nanos(exchange.server_send) - nanos(exchange.client_receive)))
            / 2;
        Ok(Self {
            client_send: exchange.client_send,
            network_round_trip,
            server_processing: processing,
            offset_nanos,
            client_receive: exchange.client_receive,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResyncReason {
    Suspension,
    OffsetJump,
    LeadExceeded,
    TargetDiscontinuity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncError {
    InvalidConfiguration,
    InvalidSample,
    NotSynchronized,
    NonMonotonicTime,
    TimeRange,
    ResyncRequired(ResyncReason),
}
impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "clock synchronization: {self:?}")
    }
}
impl std::error::Error for SyncError {}

#[derive(Debug, Clone, Copy)]
pub struct ClockConfig {
    pub sample_window: usize,
    pub maximum_round_trip: Duration,
    pub offset_jump: Duration,
    pub suspension: Duration,
    /// 20,000 ppm = maximum 0.98–1.02 logical-clock slew. Fixed dt is unchanged.
    pub slew_parts_per_million: u32,
}
impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            sample_window: 32,
            maximum_round_trip: Duration::from_secs(1),
            offset_jump: Duration::from_millis(250),
            suspension: Duration::from_secs(2),
            slew_parts_per_million: 20_000,
        }
    }
}
pub struct ClockEstimator {
    config: ClockConfig,
    rate: TickRate,
    server_epoch: Duration,
    samples: VecDeque<ClockSample>,
    offset: Option<i128>,
    last_estimate: Option<(Duration, Duration)>,
    resync: Option<ResyncReason>,
}
impl ClockEstimator {
    pub fn new(
        rate: TickRate,
        server_epoch: Duration,
        config: ClockConfig,
    ) -> Result<Self, SyncError> {
        if config.sample_window == 0
            || config.sample_window > 256
            || config.maximum_round_trip.is_zero()
            || config.offset_jump.is_zero()
            || config.suspension.is_zero()
            || config.slew_parts_per_million > 20_000
        {
            return Err(SyncError::InvalidConfiguration);
        }
        Ok(Self {
            config,
            rate,
            server_epoch,
            samples: VecDeque::new(),
            offset: None,
            last_estimate: None,
            resync: None,
        })
    }
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
    pub fn target_offset_nanos(&self) -> Option<i128> {
        self.offset
    }
    pub fn best_network_round_trip(&self) -> Option<Duration> {
        self.samples.iter().map(|s| s.network_round_trip).min()
    }
    pub fn round_trip_spread(&self) -> Option<Duration> {
        Some(
            self.samples.iter().map(|s| s.network_round_trip).max()?
                - self.best_network_round_trip()?,
        )
    }
    pub fn needs_resynchronization(&self) -> Option<ResyncReason> {
        self.resync
    }
    fn active(&self) -> Result<(), SyncError> {
        self.resync
            .map_or(Ok(()), |reason| Err(SyncError::ResyncRequired(reason)))
    }
    pub fn observe(&mut self, exchange: TimeExchange) -> Result<ClockSample, SyncError> {
        self.active()?;
        let sample = ClockSample::from_exchange(exchange)?;
        if sample.network_round_trip > self.config.maximum_round_trip {
            return Err(SyncError::InvalidSample);
        }
        if self.samples.back().is_some_and(|old| {
            sample.client_receive < old.client_receive || sample.client_send <= old.client_send
        }) {
            return Err(SyncError::NonMonotonicTime);
        }
        // Choose the best sample in the prospective bounded window. On equal RTT,
        // use the newest sample so slow drift does not wait for old entries to age out.
        let skip = usize::from(self.samples.len() == self.config.sample_window);
        let best = self
            .samples
            .iter()
            .skip(skip)
            .chain(std::iter::once(&sample))
            .min_by_key(|s| (s.network_round_trip, Reverse(s.client_receive)))
            .expect("new sample exists");
        if self
            .offset
            .is_some_and(|old| (best.offset_nanos - old).abs() > nanos(self.config.offset_jump))
        {
            self.resync = Some(ResyncReason::OffsetJump);
            return Err(SyncError::ResyncRequired(ResyncReason::OffsetJump));
        }
        self.offset = Some(best.offset_nanos);
        if self.samples.len() == self.config.sample_window {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        Ok(sample)
    }
    /// Local monotonic time only. A discontinuity latches resync and stops new
    /// estimates until the owner installs a fresh checkpoint/command incarnation.
    pub fn estimate_server_time(&mut self, local_now: Duration) -> Result<Duration, SyncError> {
        self.active()?;
        let offset = self.offset.ok_or(SyncError::NotSynchronized)?;
        let target = nanos(local_now) + offset;
        let estimate = if let Some((last_local, last_server)) = self.last_estimate {
            let elapsed = local_now
                .checked_sub(last_local)
                .ok_or(SyncError::NonMonotonicTime)?;
            if elapsed > self.config.suspension {
                self.resync = Some(ResyncReason::Suspension);
                return Err(SyncError::ResyncRequired(ResyncReason::Suspension));
            }
            let free_running = nanos(last_server) + nanos(elapsed);
            let max_adjustment =
                nanos(elapsed) * i128::from(self.config.slew_parts_per_million) / 1_000_000;
            free_running + (target - free_running).clamp(-max_adjustment, max_adjustment)
        } else {
            target
        };
        let estimate = duration(estimate)?;
        self.last_estimate = Some((local_now, estimate));
        Ok(estimate)
    }
    /// Estimated time relative to this match's tick-zero epoch, including phase.
    pub fn estimate_server_elapsed(&mut self, local_now: Duration) -> Result<Duration, SyncError> {
        self.estimate_server_time(local_now)?
            .checked_sub(self.server_epoch)
            .ok_or(SyncError::TimeRange)
    }
    pub fn estimate_server_tick(&mut self, local_now: Duration) -> Result<ServerTick, SyncError> {
        let elapsed = self.estimate_server_elapsed(local_now)?;
        self.rate.elapsed_ticks(elapsed).ok_or(SyncError::TimeRange)
    }
    /// This does not resolve old action outcomes or authorize starting input. The
    /// application must complete its explicit resynchronization lifecycle first.
    pub fn reset(&mut self, server_epoch: Duration) {
        self.server_epoch = server_epoch;
        self.samples.clear();
        self.offset = None;
        self.last_estimate = None;
        self.resync = None;
    }
}
fn nanos(duration: Duration) -> i128 {
    duration.as_nanos() as i128
}
fn duration(nanoseconds: i128) -> Result<Duration, SyncError> {
    if nanoseconds < 0 {
        return Err(SyncError::TimeRange);
    }
    let seconds = u64::try_from(nanoseconds / 1_000_000_000).map_err(|_| SyncError::TimeRange)?;
    Ok(Duration::new(seconds, (nanoseconds % 1_000_000_000) as u32))
}

pub struct LeadController {
    lead: u8,
    samples: VecDeque<i32>,
    last_target: Option<TargetTick>,
    resync: Option<ResyncReason>,
}
impl LeadController {
    pub fn new(lead: u8) -> Result<Self, SyncError> {
        if !(1..=12).contains(&lead) {
            return Err(SyncError::InvalidConfiguration);
        }
        Ok(Self {
            lead,
            samples: VecDeque::new(),
            last_target: None,
            resync: None,
        })
    }
    /// Half RTT is only a bootstrap estimate. Processing time must already be
    /// removed; use ClockSample::network_round_trip, not raw request elapsed time.
    pub fn bootstrap(
        rate: TickRate,
        network_round_trip: Duration,
        jitter_and_queue_margin: Duration,
    ) -> Result<Self, SyncError> {
        let estimate = (network_round_trip / 2)
            .checked_add(jitter_and_queue_margin)
            .ok_or(SyncError::TimeRange)?;
        let ticks = (estimate.as_nanos() * u128::from(rate.hz()))
            .div_ceil(1_000_000_000)
            .max(1);
        if ticks > 12 {
            return Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded));
        }
        Self::new(ticks as u8)
    }
    pub fn lead(&self) -> u8 {
        self.lead
    }
    pub fn needs_resynchronization(&self) -> Option<ResyncReason> {
        self.resync
    }
    /// Slack is target_tick - authoritative_tick_at_arrival. Increase promptly on
    /// missed deadlines; decrease only after eight consistently overbuffered samples.
    pub fn observe_arrival_slack(&mut self, slack: i32) -> Result<u8, SyncError> {
        if let Some(reason) = self.resync {
            return Err(SyncError::ResyncRequired(reason));
        }
        if !(-12..=12).contains(&slack) {
            return Err(SyncError::InvalidSample);
        }
        if slack <= 0 {
            if self.lead == 12 {
                self.resync = Some(ResyncReason::LeadExceeded);
                return Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded));
            }
            self.lead += 1;
            self.samples.clear();
        } else {
            if self.samples.len() == 32 {
                self.samples.pop_front();
            }
            self.samples.push_back(slack);
            if self.samples.len() >= 8 {
                let mut sorted: Vec<_> = self.samples.iter().copied().collect();
                sorted.sort_unstable();
                if sorted[sorted.len() / 4] > 2 && self.lead > 1 {
                    self.lead -= 1;
                    self.samples.clear();
                }
            }
        }
        Ok(self.lead)
    }
    /// A lead reduction waits for the server clock to catch up. It never reassigns
    /// a previously issued tick, and repeated polling cannot create a command burst.
    pub fn assign_target(
        &mut self,
        estimated_server_tick: ServerTick,
    ) -> Result<Option<TargetTick>, SyncError> {
        if let Some(reason) = self.resync {
            return Err(SyncError::ResyncRequired(reason));
        }
        let target = TargetTick(
            estimated_server_tick
                .0
                .checked_add(u64::from(self.lead))
                .ok_or(SyncError::TimeRange)?,
        );
        if let Some(previous) = self.last_target {
            if target <= previous {
                return Ok(None);
            }
            if target.0 - previous.0 > 12 {
                self.resync = Some(ResyncReason::TargetDiscontinuity);
                return Err(SyncError::ResyncRequired(ResyncReason::TargetDiscontinuity));
            }
        }
        self.last_target = Some(target);
        Ok(Some(target))
    }
}

#[derive(Debug, Default)]
pub struct PresentationCursor {
    last: Option<Duration>,
    segment: u64,
}
impl PresentationCursor {
    /// `total_age` includes publication/downlink age plus chosen jitter slack; do
    /// not add snapshot network age twice. Increasing buffering may pause this
    /// cursor, but normal playback never moves backward.
    pub fn advance(&mut self, estimated_server_now: Duration, total_age: Duration) -> Duration {
        let candidate = estimated_server_now.saturating_sub(total_age);
        let next = self.last.map_or(candidate, |last| last.max(candidate));
        self.last = Some(next);
        next
    }
    pub fn segment(&self) -> u64 {
        self.segment
    }
    pub fn discontinuity(&mut self, at: Duration) -> Result<(), SyncError> {
        self.segment = self.segment.checked_add(1).ok_or(SyncError::TimeRange)?;
        self.last = Some(at);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }
    fn exchange(client_send: u64, offset: u64, one_way: u64, processing: u64) -> TimeExchange {
        TimeExchange {
            client_send: ms(client_send),
            server_receive: ms(client_send + one_way + offset),
            server_send: ms(client_send + one_way + offset + processing),
            client_receive: ms(client_send + 2 * one_way + processing),
        }
    }
    #[test]
    fn four_timestamps_remove_processing_and_use_monotonic_epoch_offset() {
        let sample = ClockSample::from_exchange(exchange(1000, 500, 20, 150)).unwrap();
        assert_eq!(sample.network_round_trip, ms(40));
        assert_eq!(sample.server_processing, ms(150));
        assert_eq!(sample.offset_nanos, 500_000_000);
        let mut bad = exchange(1000, 500, 20, 150);
        bad.client_receive = ms(1001);
        assert_eq!(
            ClockSample::from_exchange(bad),
            Err(SyncError::InvalidSample)
        );
    }
    #[test]
    fn batched_replies_share_receive_time_but_probes_must_advance() {
        let mut clock = ClockEstimator::new(
            TickRate::new(60).unwrap(),
            Duration::ZERO,
            ClockConfig::default(),
        )
        .unwrap();
        let first = TimeExchange {
            client_send: ms(1000),
            server_receive: ms(1520),
            server_send: ms(1520),
            client_receive: ms(1160),
        };
        let second = TimeExchange {
            client_send: ms(1100),
            server_receive: ms(1620),
            server_send: ms(1620),
            client_receive: ms(1160),
        };
        clock.observe(first).unwrap();
        clock.observe(second).unwrap();
        assert_eq!(clock.sample_count(), 2);
        assert_eq!(clock.best_network_round_trip(), Some(ms(60)));
        assert_eq!(clock.target_offset_nanos(), Some(490_000_000));
        assert_eq!(clock.observe(second), Err(SyncError::NonMonotonicTime));
        assert_eq!(clock.observe(first), Err(SyncError::NonMonotonicTime));
        assert_eq!(clock.sample_count(), 2);
    }
    #[test]
    fn low_delay_window_and_bounded_slew_follow_drift_without_changing_tick_rate() {
        let rate = TickRate::new(60).unwrap();
        let mut clock = ClockEstimator::new(
            rate,
            Duration::ZERO,
            ClockConfig {
                sample_window: 4,
                ..Default::default()
            },
        )
        .unwrap();
        clock.observe(exchange(1000, 500, 20, 0)).unwrap();
        assert_eq!(clock.estimate_server_time(ms(1040)).unwrap(), ms(1540));
        clock.observe(exchange(1100, 600, 100, 0)).unwrap(); // slower biased sample ignored
        assert_eq!(clock.target_offset_nanos(), Some(500_000_000));
        clock.observe(exchange(1400, 520, 20, 0)).unwrap(); // equal low delay: slow drift target
        let estimate = clock.estimate_server_time(ms(1440)).unwrap();
        assert_eq!(estimate, ms(1948)); // 400 ms + at most 2% phase correction
        for index in 0..10 {
            clock
                .observe(exchange(1500 + index * 100, 520, 20, 0))
                .unwrap();
        }
        assert_eq!(clock.sample_count(), 4);
        assert!(clock.estimate_server_tick(ms(2440)).unwrap().0 >= 177);
    }
    #[test]
    fn suspension_and_offset_discontinuity_require_explicit_resync() {
        let mut clock = ClockEstimator::new(
            TickRate::new(60).unwrap(),
            Duration::ZERO,
            ClockConfig::default(),
        )
        .unwrap();
        clock.observe(exchange(1000, 500, 20, 0)).unwrap();
        clock.estimate_server_time(ms(1040)).unwrap();
        assert_eq!(
            clock.estimate_server_time(ms(10000)),
            Err(SyncError::ResyncRequired(ResyncReason::Suspension))
        );
        assert!(clock.observe(exchange(11000, 500, 20, 0)).is_err());
        clock.reset(Duration::ZERO);
        clock.observe(exchange(11000, 500, 20, 0)).unwrap();
        assert_eq!(
            clock.observe(exchange(12000, 1500, 10, 0)),
            Err(SyncError::ResyncRequired(ResyncReason::OffsetJump))
        );
    }
    #[test]
    fn uplink_slack_tunes_lead_and_decrease_never_rewrites_sent_targets() {
        let mut lead =
            LeadController::bootstrap(TickRate::new(60).unwrap(), ms(40), ms(10)).unwrap();
        assert_eq!(lead.lead(), 2);
        assert_eq!(lead.observe_arrival_slack(0).unwrap(), 3);
        assert_eq!(
            lead.assign_target(ServerTick(10)).unwrap(),
            Some(TargetTick(13))
        );
        for _ in 0..8 {
            lead.observe_arrival_slack(4).unwrap();
        }
        assert_eq!(lead.lead(), 2);
        assert_eq!(lead.assign_target(ServerTick(10)).unwrap(), None);
        assert_eq!(lead.assign_target(ServerTick(11)).unwrap(), None);
        assert_eq!(
            lead.assign_target(ServerTick(12)).unwrap(),
            Some(TargetTick(14))
        );
        assert_eq!(lead.assign_target(ServerTick(12)).unwrap(), None);
        assert_eq!(
            lead.assign_target(ServerTick(1000)),
            Err(SyncError::ResyncRequired(ResyncReason::TargetDiscontinuity))
        );
    }
    #[test]
    fn lead_capacity_and_presentation_monotonicity_are_explicit() {
        assert!(matches!(
            LeadController::bootstrap(TickRate::new(60).unwrap(), ms(600), ms(0)),
            Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded))
        ));
        let mut lead = LeadController::new(12).unwrap();
        assert_eq!(
            lead.observe_arrival_slack(-1),
            Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded))
        );
        let mut cursor = PresentationCursor::default();
        assert_eq!(cursor.advance(ms(1000), ms(100)), ms(900));
        assert_eq!(cursor.advance(ms(1010), ms(200)), ms(900));
        assert_eq!(cursor.advance(ms(1200), ms(100)), ms(1100));
        cursor.discontinuity(ms(700)).unwrap();
        assert_eq!(cursor.segment(), 1);
        assert_eq!(cursor.advance(ms(900), ms(150)), ms(750));
    }
}

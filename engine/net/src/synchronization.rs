//! Monotonic clock estimation, bounded prediction lead and presentation time.
//!
//! Four timestamps remove server processing from network RTT. Offset still assumes
//! path symmetry; arrival-slack feedback corrects useful command lead without
//! claiming an exact uplink measurement. These clocks schedule fixed simulation
//! steps; they never alter simulation dt or rewrite an already assigned target.
use crate::types::{MAX_COMMAND_LEAD_TICKS, ServerTick, TargetTick, TickRate};
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
/// Only bootstrap can refuse: a first sample past the lead horizon is reported so
/// the caller can wait for a cleaner probe. Once running, every clock
/// discontinuity re-anchors in place; none of them ends the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResyncReason {
    LeadExceeded,
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
    pub fn observe(&mut self, exchange: TimeExchange) -> Result<ClockSample, SyncError> {
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
        let best_offset = best.offset_nanos;
        if self
            .offset
            .is_some_and(|old| (best_offset - old).abs() > nanos(self.config.offset_jump))
        {
            // A step this large is a host clock change or a new path, not drift,
            // and slewing at 2% would take many seconds to close it. Re-anchor
            // on the new sample: the older window describes a clock that no
            // longer exists.
            self.samples.clear();
            self.last_estimate = None;
        }
        self.offset = Some(best_offset);
        if self.samples.len() == self.config.sample_window {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        Ok(sample)
    }
    /// Local monotonic time only. After a suspension (a background tab, a
    /// debugger, a long hitch) the estimate jumps to the target instead of
    /// slewing, so the caller sees the time that actually passed.
    pub fn estimate_server_time(&mut self, local_now: Duration) -> Result<Duration, SyncError> {
        let offset = self.offset.ok_or(SyncError::NotSynchronized)?;
        let target = nanos(local_now) + offset;
        let continuing = match self.last_estimate {
            Some((last_local, _)) => {
                let elapsed = local_now
                    .checked_sub(last_local)
                    .ok_or(SyncError::NonMonotonicTime)?;
                (elapsed <= self.config.suspension).then_some(elapsed)
            }
            None => None,
        };
        let estimate =
            if let (Some(elapsed), Some((_, last_server))) = (continuing, self.last_estimate) {
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
    /// Start over for a new match epoch. This does not resolve old action
    /// outcomes; the caller's session lifecycle owns those.
    pub fn reset(&mut self, server_epoch: Duration) {
        self.server_epoch = server_epoch;
        self.samples.clear();
        self.offset = None;
        self.last_estimate = None;
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

/// Arrival samples a lead decrease must cover: about ten seconds at one
/// sample per 20 Hz publication.
pub const LEAD_DECAY_SAMPLES: usize = 200;
/// Spare ticks every command keeps at its tightest arrival in a decay window.
pub const LEAD_MARGIN_TICKS: i32 = 2;

#[derive(Debug, Clone)]
pub struct LeadController {
    lead: u8,
    samples: VecDeque<i32>,
    last_target: Option<TargetTick>,
}
impl LeadController {
    pub fn new(lead: u8) -> Result<Self, SyncError> {
        if !(1..=MAX_COMMAND_LEAD_TICKS).contains(&lead) {
            return Err(SyncError::InvalidConfiguration);
        }
        Ok(Self {
            lead,
            samples: VecDeque::new(),
            last_target: None,
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
        // Beyond the horizon this controller can cover. Report it so the
        // caller can prefer a cleaner probe: the first sample is timestamped
        // when the client processes the reply, so a local stall reads as an
        // enormous round trip that no lead would actually fix.
        if ticks > u128::from(MAX_COMMAND_LEAD_TICKS) {
            return Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded));
        }
        Self::new(ticks as u8)
    }
    pub fn lead(&self) -> u8 {
        self.lead
    }
    /// Slack is target_tick - authoritative_tick_at_arrival. A late or
    /// just-in-time sample raises lead at once by enough to have covered it.
    /// Lead falls only once a full decay window of samples all kept more than
    /// [`LEAD_MARGIN_TICKS`] spare, and then by as much as the tightest of them
    /// allows. Lateness that recurs within the window, as delay spikes and
    /// jitter bursts on wireless links do, therefore stays covered instead of
    /// being forgotten between occurrences. A late command is always a
    /// misprediction; a larger lead only costs replay work.
    pub fn observe_arrival_slack(&mut self, slack: i32) -> Result<u8, SyncError> {
        let bound = i32::from(MAX_COMMAND_LEAD_TICKS);
        if !(-bound..=bound).contains(&slack) {
            return Err(SyncError::InvalidSample);
        }
        if slack <= 0 {
            // Saturate rather than fail. A round trip this controller cannot
            // cover makes commands arrive late, which is a quality outcome the
            // player can see and play through. Ending the session instead
            // fixes nothing: starting over cannot shorten the path.
            let raised = i32::from(self.lead) + 1 - slack;
            self.lead = raised.min(bound) as u8;
            self.samples.clear();
        } else {
            if self.samples.len() == LEAD_DECAY_SAMPLES {
                self.samples.pop_front();
            }
            self.samples.push_back(slack);
            if self.samples.len() == LEAD_DECAY_SAMPLES {
                let tightest = self.samples.iter().copied().min().expect("full window");
                let spare = tightest - LEAD_MARGIN_TICKS;
                if spare > 0 && self.lead > 1 {
                    self.lead -= spare.min(i32::from(self.lead) - 1) as u8;
                    self.samples.clear();
                }
            }
        }
        Ok(self.lead)
    }
    /// A lead reduction waits for the server clock to catch up. It never reassigns
    /// a previously issued tick, and repeated polling cannot create a command burst.
    /// A forward jump (the client stopped running for a while) simply moves the
    /// head: the skipped ticks had no player input, and the server fills them
    /// with the same substitutes the client forecasts.
    pub fn assign_target(
        &mut self,
        estimated_server_tick: ServerTick,
    ) -> Result<Option<TargetTick>, SyncError> {
        let target = TargetTick(
            estimated_server_tick
                .0
                .checked_add(u64::from(self.lead))
                .ok_or(SyncError::TimeRange)?,
        );
        if self.last_target.is_some_and(|previous| target <= previous) {
            return Ok(None);
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
    fn suspension_and_offset_steps_re_anchor_instead_of_stopping() {
        let mut clock = ClockEstimator::new(
            TickRate::new(60).unwrap(),
            Duration::ZERO,
            ClockConfig::default(),
        )
        .unwrap();
        clock.observe(exchange(1000, 500, 20, 0)).unwrap();
        clock.estimate_server_time(ms(1040)).unwrap();
        // Ten seconds suspended: the estimate reports the time that passed
        // rather than slewing toward it at 2%.
        assert_eq!(clock.estimate_server_time(ms(10000)).unwrap(), ms(10500));
        clock.observe(exchange(11000, 500, 20, 0)).unwrap();
        // A one-second clock step replaces the window instead of ending the
        // session, and the next estimate follows the new offset at once.
        clock.observe(exchange(12000, 1500, 10, 0)).unwrap();
        assert_eq!(clock.sample_count(), 1);
        assert_eq!(clock.target_offset_nanos(), Some(1_500_000_000));
        assert_eq!(clock.estimate_server_time(ms(12100)).unwrap(), ms(13600));
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
        // Seven ticks late raises lead enough to have covered it at once.
        let mut late = lead.clone();
        assert_eq!(late.observe_arrival_slack(-7).unwrap(), 11);
        // A decrease needs a whole window with spare slack, and then keeps the
        // margin at the tightest arrival.
        for _ in 0..LEAD_DECAY_SAMPLES - 1 {
            lead.observe_arrival_slack(3).unwrap();
        }
        assert_eq!(lead.lead(), 3);
        lead.observe_arrival_slack(3).unwrap();
        assert_eq!(lead.lead(), 2);
        assert_eq!(lead.assign_target(ServerTick(10)).unwrap(), None);
        assert_eq!(lead.assign_target(ServerTick(11)).unwrap(), None);
        assert_eq!(
            lead.assign_target(ServerTick(12)).unwrap(),
            Some(TargetTick(14))
        );
        assert_eq!(lead.assign_target(ServerTick(12)).unwrap(), None);
        // After the client stops running for a while, the head moves forward.
        assert_eq!(
            lead.assign_target(ServerTick(1000)).unwrap(),
            Some(TargetTick(1002))
        );
    }
    #[test]
    fn lead_capacity_and_presentation_monotonicity_are_explicit() {
        let rate = TickRate::new(60).unwrap();
        // An ordinary intercontinental round trip is covered, not refused.
        assert_eq!(
            LeadController::bootstrap(rate, ms(600), ms(20))
                .unwrap()
                .lead(),
            20
        );
        // Past the horizon the controller can cover, it reports rather than
        // guesses, so the caller can prefer a cleaner probe.
        assert!(matches!(
            LeadController::bootstrap(rate, ms(1200), ms(0)),
            Err(SyncError::ResyncRequired(ResyncReason::LeadExceeded))
        ));
        // Saturating lead is a quality outcome, never a failed session: a slow
        // link keeps playing with late commands instead of being disconnected.
        let mut lead = LeadController::new(MAX_COMMAND_LEAD_TICKS).unwrap();
        assert_eq!(lead.observe_arrival_slack(-1), Ok(MAX_COMMAND_LEAD_TICKS));
        assert_eq!(lead.observe_arrival_slack(-1), Ok(MAX_COMMAND_LEAD_TICKS));
        let mut cursor = PresentationCursor::default();
        assert_eq!(cursor.advance(ms(1000), ms(100)), ms(900));
        assert_eq!(cursor.advance(ms(1010), ms(200)), ms(900));
        assert_eq!(cursor.advance(ms(1200), ms(100)), ms(1100));
        cursor.discontinuity(ms(700)).unwrap();
        assert_eq!(cursor.segment(), 1);
        assert_eq!(cursor.advance(ms(900), ms(150)), ms(750));
    }
}

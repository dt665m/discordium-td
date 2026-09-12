//! Simulation progress is distinct from synchronized server wall time.
use super::SyncError;
use crate::types::{ServerTick, TickRate};
use std::time::Duration;

/// Estimate committed simulation progress from a tick paired with its server
/// timestamp. A delayed untimestamped checkpoint proves a lower bound only.
///
/// The game supplies a total speculative budget, including command lead. This
/// bounds exposure to an unobserved authority stall; it cannot promise an exact
/// future-admission horizon at the remote instant a command eventually arrives.
#[derive(Debug, Clone, Copy)]
pub struct CommittedClock {
    rate: TickRate,
    anchor_tick: ServerTick,
    anchor_time: Duration,
    phase_offset_nanos: i64,
    committed: ServerTick,
    maximum_ahead: u32,
}
impl CommittedClock {
    pub fn new(
        rate: TickRate,
        tick: ServerTick,
        server_time: Duration,
        maximum_ahead: u32,
    ) -> Result<Self, SyncError> {
        if maximum_ahead == 0 {
            return Err(SyncError::InvalidConfiguration);
        }
        Ok(Self {
            rate,
            anchor_tick: tick,
            anchor_time: server_time,
            phase_offset_nanos: 0,
            committed: tick,
            maximum_ahead,
        })
    }
    pub fn committed(&self) -> ServerTick {
        self.committed
    }
    pub fn anchor(&self) -> (ServerTick, Duration) {
        (self.anchor_tick, self.anchor_time)
    }
    /// Only authenticated complete publications or finalized command receipts
    /// may advance this floor. Receiving them does not make their tick current.
    pub fn commit(&mut self, tick: ServerTick) {
        self.committed = self.committed.max(tick);
    }
    /// A newer timestamp with the same tick records retained authority debt.
    /// It does not replenish the total speculative budget.
    pub fn observe(&mut self, tick: ServerTick, server_time: Duration) -> Result<(), SyncError> {
        if server_time < self.anchor_time || tick < self.anchor_tick {
            return Err(SyncError::NonMonotonicTime);
        }
        // A reply reports an integer committed tick at an arbitrary point in
        // its interval. Preserve the inferred phase when advancing observations
        // agree within one tick; re-anchoring every reply would pause or jump
        // fractional playback as probe timing changes. Same-tick progress debt
        // and larger discrepancies still replace the old estimate immediately.
        let phase_offset_nanos = if tick > self.anchor_tick && server_time > self.anchor_time {
            let preceding = self.rate.deadline(ServerTick(tick.0 - 1));
            let following = tick
                .checked_next()
                .and_then(|tick| self.rate.deadline(tick));
            self.extrapolated_elapsed(server_time)
                .ok()
                .zip(self.rate.deadline(tick))
                .filter(|(previous, _)| {
                    // Use neighboring rational deadlines: a rounded nominal
                    // period can mistake a complete tick for fractional phase.
                    preceding.is_some_and(|bound| *previous > bound)
                        && following.is_some_and(|bound| *previous < bound)
                })
                .and_then(|(previous, reported)| {
                    let offset = previous.as_nanos() as i128 - reported.as_nanos() as i128;
                    i64::try_from(offset).ok()
                })
                .unwrap_or(0)
        } else if tick == self.anchor_tick && server_time == self.anchor_time {
            self.phase_offset_nanos
        } else {
            0
        };
        self.anchor_tick = tick;
        self.anchor_time = server_time;
        self.phase_offset_nanos = phase_offset_nanos;
        self.commit(tick);
        Ok(())
    }
    fn extrapolated_elapsed(&self, server_now: Duration) -> Result<Duration, SyncError> {
        let phase = Duration::from_nanos(self.phase_offset_nanos.unsigned_abs());
        self.rate
            .deadline(self.anchor_tick)
            .and_then(|anchor| {
                if self.phase_offset_nanos < 0 {
                    anchor.checked_sub(phase)
                } else {
                    anchor.checked_add(phase)
                }
            })
            .and_then(|time| time.checked_add(server_now.saturating_sub(self.anchor_time)))
            .ok_or(SyncError::TimeRange)
    }
    /// Reserve command lead inside the game's unchanged total replay budget.
    /// Repeated estimates and same-tick replies cannot advance the ceiling.
    pub fn estimate(
        &self,
        server_now: Duration,
        reserved_headroom: u8,
    ) -> Result<ServerTick, SyncError> {
        self.rate
            .elapsed_ticks(self.estimate_elapsed(server_now, reserved_headroom)?)
            .ok_or(SyncError::TimeRange)
    }
    /// The same bounded simulation clock including fractional tick phase. Use
    /// it for presentation and timestamped input that share the command domain.
    pub fn estimate_elapsed(
        &self,
        server_now: Duration,
        reserved_headroom: u8,
    ) -> Result<Duration, SyncError> {
        let available = self
            .maximum_ahead
            .checked_sub(u32::from(reserved_headroom))
            .ok_or(SyncError::InvalidConfiguration)?;
        let estimated = self.extrapolated_elapsed(server_now)?;
        let ceiling = self
            .committed
            .0
            .checked_add(u64::from(available))
            .ok_or(SyncError::TimeRange)?;
        let floor = self
            .rate
            .deadline(self.committed)
            .ok_or(SyncError::TimeRange)?;
        let ceiling = self
            .rate
            .deadline(ServerTick(ceiling))
            .ok_or(SyncError::TimeRange)?;
        Ok(estimated.max(floor).min(ceiling))
    }
}

//! Portable protocol identities. Each counter has one meaning and one lifetime.
//!
//! Wide counters never wrap inside an incarnation. Exhaustion requires a new
//! authenticated session; it must not make stale state appear current again.
use serde::{Deserialize, Serialize};
use std::time::Duration;

macro_rules! identity {
    ($($name:ident($repr:ty);)+) => {$ (
        #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $repr);

        impl $name {
            pub const fn checked_next(self) -> Option<Self> {
                match self.0.checked_add(1) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }
        }
    )+};
}

identity! {
    ServerTick(u64);
    CommandSeq(u64);
    TargetTick(u64);
    SnapshotId(u64);
    BaselineId(u64);
    BaselineGeneration(u64);
    ConnectionEpoch(u64);
    ConnectionId(u64);
    CommandStream(u32);
    OwnershipEpoch(u32);
    ScopeEpoch(u64);
    RepresentationRevision(u32);
    StateVersion(u64);
    ReplicationFrame(u64);
    SceneRevision(u32);
    PolicyRevision(u64);
    GroupId(u32);
    GroupRevision(u64);
    SchemaId(u32);
}

/// Stable simulation identity, independent of an ECS slot or a connection scope.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct EntityId {
    pub index: u64,
    pub generation: u32,
}

/// Full fence for a connection-local representation. A scope exit is not death.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScopeIdentity {
    pub connection: ConnectionEpoch,
    pub entity: EntityId,
    pub scope: ScopeEpoch,
    pub representation: RepresentationRevision,
}

/// Stable action identity survives redundant delivery and simulation replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ActionKey {
    pub connection: ConnectionEpoch,
    pub stream: CommandStream,
    pub command: CommandSeq,
    pub slot: u8,
}

/// Ordering for truncated wire counters is valid only inside a half-range window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialOrdering {
    Older,
    Equal,
    Newer,
    Ambiguous,
}

pub const fn compare_serial32(value: u32, previous: u32) -> SerialOrdering {
    match value.wrapping_sub(previous) {
        0 => SerialOrdering::Equal,
        0x8000_0000 => SerialOrdering::Ambiguous,
        delta if delta < 0x8000_0000 => SerialOrdering::Newer,
        _ => SerialOrdering::Older,
    }
}

/// A rational fixed rate. Derive deadlines from the epoch, never by repeatedly
/// adding a rounded period. This value schedules work; it does not run gameplay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickRate(u32);

impl TickRate {
    pub const fn new(hz: u32) -> Option<Self> {
        if hz == 0 || hz > 1_000_000_000 {
            None
        } else {
            Some(Self(hz))
        }
    }

    pub const fn hz(self) -> u32 {
        self.0
    }

    /// Deadline of end-of-tick state S[k] relative to the initialized S[0].
    /// Round upward: a tick must not execute before its rational deadline.
    pub fn deadline(self, tick: ServerTick) -> Option<Duration> {
        let nanos = (u128::from(tick.0) * 1_000_000_000).div_ceil(u128::from(self.0));
        let seconds = u64::try_from(nanos / 1_000_000_000).ok()?;
        Some(Duration::new(seconds, (nanos % 1_000_000_000) as u32))
    }

    /// Quantize one elapsed instant downward to a fixed tick and a bounded phase.
    /// Power-of-two phase units preserve identical wire/replay rounding.
    pub fn elapsed_tick_phase(self, elapsed: Duration) -> Option<(ServerTick, u16)> {
        let scaled = elapsed.as_nanos().checked_mul(u128::from(self.0))?;
        let tick = ServerTick(u64::try_from(scaled / 1_000_000_000).ok()?);
        let fraction = ((scaled % 1_000_000_000) * 65_536 / 1_000_000_000) as u16;
        Some((tick, fraction))
    }

    pub fn elapsed_ticks(self, elapsed: Duration) -> Option<ServerTick> {
        u64::try_from(elapsed.as_nanos() * u128::from(self.0) / 1_000_000_000)
            .ok()
            .map(ServerTick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_wrap_and_half_range_are_explicit() {
        assert_eq!(compare_serial32(0, u32::MAX), SerialOrdering::Newer);
        assert_eq!(compare_serial32(u32::MAX, 0), SerialOrdering::Older);
        assert_eq!(compare_serial32(7, 7), SerialOrdering::Equal);
        assert_eq!(compare_serial32(1 << 31, 0), SerialOrdering::Ambiguous);
        assert_eq!(ScopeEpoch(u64::MAX).checked_next(), None);
    }

    #[test]
    fn rational_sixty_hz_has_no_day_long_rounding_drift() {
        let rate = TickRate::new(60).unwrap();
        assert_eq!(
            rate.deadline(ServerTick(60 * 86_400)),
            Some(Duration::from_secs(86_400))
        );
        for tick in [1, 2, 59, 60, 61, 60 * 86_400] {
            let deadline = rate.deadline(ServerTick(tick)).unwrap();
            assert_eq!(rate.elapsed_ticks(deadline), Some(ServerTick(tick)));
            assert_eq!(
                rate.elapsed_ticks(deadline - Duration::from_nanos(1)),
                Some(ServerTick(tick - 1))
            );
        }
        assert!(TickRate::new(0).is_none());
        assert!(TickRate::new(u32::MAX).is_none());
        assert!(
            TickRate::new(1)
                .unwrap()
                .elapsed_ticks(Duration::MAX)
                .is_some()
        );
        assert!(rate.elapsed_ticks(Duration::MAX).is_none());
    }
    #[test]
    fn fractional_tick_quantization_is_bounded_and_rounds_down() {
        let rate = TickRate::new(60).unwrap();
        assert_eq!(
            rate.elapsed_tick_phase(Duration::from_millis(1025)),
            Some((ServerTick(61), 32768))
        );
        for tick in 0..100 {
            assert_eq!(
                rate.elapsed_tick_phase(rate.deadline(ServerTick(tick)).unwrap()),
                Some((ServerTick(tick), 0))
            );
        }
        assert_eq!(
            rate.elapsed_tick_phase(Duration::from_nanos(16_666_666)),
            Some((ServerTick(0), u16::MAX))
        );
    }
}

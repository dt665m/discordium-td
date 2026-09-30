//! Opt-in main-thread stalls for native qualification runs.
//!
//! Browsers routinely block the page's main thread: garbage collection, shader
//! and pipeline compilation, a background tab, a slow frame. While blocked,
//! nothing renders, reads input or pumps the network. A native client does not
//! do this on its own, which is why native runs can pass while browsers fail.
//! Blocking the whole frame here reproduces that condition deterministically.
use bevy::prelude::*;
use std::time::{Duration, Instant};

#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InjectedStall {
    pub(crate) duration: Duration,
    pub(crate) every: Duration,
}

impl std::str::FromStr for InjectedStall {
    type Err = String;
    /// `MILLIS@SECONDS`, for example `500@5` stalls 500 ms every 5 seconds.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (duration, every) = value
            .split_once('@')
            .ok_or_else(|| format!("stall '{value}' must be MILLIS@SECONDS"))?;
        let duration = duration
            .parse::<u64>()
            .map_err(|_| format!("stall duration '{duration}' is not whole milliseconds"))?;
        let every = every
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .ok_or_else(|| format!("stall interval '{every}' is not positive seconds"))?;
        let stall = Self {
            duration: Duration::from_millis(duration),
            every: Duration::from_secs_f64(every),
        };
        // A stall at least as long as its interval never lets a frame run.
        if stall.duration.is_zero() || stall.duration >= stall.every {
            return Err(format!(
                "stall of {duration} ms must be nonzero and shorter than its interval"
            ));
        }
        Ok(stall)
    }
}

#[derive(Resource, Debug)]
pub(crate) struct StallClock {
    next: Instant,
    pub(crate) injected: u32,
}

impl StallClock {
    pub(crate) fn new(stall: InjectedStall) -> Self {
        Self {
            next: Instant::now() + stall.every,
            injected: 0,
        }
    }
}

/// Runs first in the frame so the stall shows up exactly as a hitch does:
/// the whole frame, and everything scheduled in it, arrives late.
pub(super) fn inject_stall(stall: Res<InjectedStall>, mut clock: ResMut<StallClock>) {
    let now = Instant::now();
    if now < clock.next {
        return;
    }
    std::thread::sleep(stall.duration);
    clock.injected += 1;
    // Schedule from the end of this stall so they cannot bunch up after a
    // frame that was already slow for unrelated reasons.
    clock.next = Instant::now() + stall.every;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalls_parse_and_reject_intervals_they_would_never_leave() {
        assert_eq!(
            "500@5".parse(),
            Ok(InjectedStall {
                duration: Duration::from_millis(500),
                every: Duration::from_secs(5),
            })
        );
        assert!("500@0.25".parse::<InjectedStall>().is_err());
        assert!("0@5".parse::<InjectedStall>().is_err());
        assert!("500".parse::<InjectedStall>().is_err());
        assert!("500@-1".parse::<InjectedStall>().is_err());
        assert!("500@nan".parse::<InjectedStall>().is_err());
    }
}

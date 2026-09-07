//! Server-owned geometry history for future bounded lag-compensated hit queries.
//! This is not an API for accepting unvalidated client timestamps or rolling back
//! gameplay. Callers must first map and validate the client's viewed server tick.
use bevy::prelude::Resource;
use game_shared::{WorldDelta, distance_sq};
use std::collections::VecDeque;

/// At 30 Hz, retains approximately one second including the current frame.
const HISTORY_FRAMES: usize = 31;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistoricalActor {
    pub id: u64,
    pub pos: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct HistoricalFrame {
    pub match_epoch: u32,
    pub tick: u32,
    pub heroes: Vec<HistoricalActor>,
    pub enemies: Vec<HistoricalActor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryUnavailable {
    WrongRound,
    TickNotRetained,
}

#[derive(Resource, Default)]
pub struct StateHistory {
    frames: VecDeque<HistoricalFrame>,
}

impl StateHistory {
    pub fn record(&mut self, world: &WorldDelta) {
        let Some(meta) = world.sim_meta else {
            return;
        };
        if self
            .frames
            .back()
            .is_some_and(|f| f.match_epoch != meta.match_epoch)
        {
            self.frames.clear();
        }
        if self.frames.back().is_some_and(|f| f.tick == world.tick) {
            self.frames.pop_back();
        }
        self.frames.push_back(HistoricalFrame {
            match_epoch: meta.match_epoch,
            tick: world.tick,
            heroes: world
                .heroes
                .iter()
                .map(|h| HistoricalActor {
                    id: h.client_id,
                    pos: h.pos,
                })
                .collect(),
            enemies: world
                .enemies
                .iter()
                .map(|e| HistoricalActor {
                    id: e.id,
                    pos: e.pos,
                })
                .collect(),
        });
        while self.frames.len() > HISTORY_FRAMES {
            self.frames.pop_front();
        }
    }

    /// Exact retained tick only; never silently use the oldest/current state.
    pub fn at_tick(
        &self,
        match_epoch: u32,
        tick: u32,
    ) -> Result<&HistoricalFrame, HistoryUnavailable> {
        if self
            .frames
            .back()
            .is_some_and(|f| f.match_epoch != match_epoch)
        {
            return Err(HistoryUnavailable::WrongRound);
        }
        self.frames
            .iter()
            .find(|f| f.tick == tick)
            .ok_or(HistoryUnavailable::TickNotRetained)
    }
}

impl HistoricalFrame {
    /// Same center-distance test as current Arc Burst. Returns historical IDs,
    /// not damage: callers must check current existence/eligibility and apply once.
    pub fn enemies_in_radius(&self, origin: [f32; 2], radius: f32) -> Vec<u64> {
        if !origin.iter().all(|v| v.is_finite()) || !radius.is_finite() || radius < 0.0 {
            return Vec::new();
        }
        self.enemies
            .iter()
            .filter(|e| distance_sq(e.pos, origin) <= radius * radius)
            .map(|e| e.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_is_bounded_and_handles_tick_wrap_and_round_reset() {
        let sim = game_sim::Simulation::new();
        let mut world = sim.world_delta_for(0);
        let mut history = StateHistory::default();
        for i in 0..40u32 {
            world.tick = (u32::MAX - 20).wrapping_add(i);
            history.record(&world);
        }
        let epoch = world.sim_meta.unwrap().match_epoch;
        assert_eq!(history.frames.len(), HISTORY_FRAMES);
        assert!(history.at_tick(epoch, 0).is_ok());
        assert_eq!(
            history.at_tick(epoch, u32::MAX - 20).unwrap_err(),
            HistoryUnavailable::TickNotRetained
        );
        assert_eq!(
            history.at_tick(epoch, world.tick + 1).unwrap_err(),
            HistoryUnavailable::TickNotRetained
        );
        world.sim_meta.as_mut().unwrap().match_epoch += 1;
        history.record(&world);
        assert_eq!(history.frames.len(), 1);
        assert_eq!(
            history.at_tick(epoch, world.tick).unwrap_err(),
            HistoryUnavailable::WrongRound
        );
    }

    #[test]
    fn historical_query_is_read_only_and_preserves_target_identity() {
        let frame = HistoricalFrame {
            match_epoch: 1,
            tick: 7,
            heroes: vec![],
            enemies: vec![
                HistoricalActor {
                    id: 9,
                    pos: [1.0, 0.0],
                },
                HistoricalActor {
                    id: 10,
                    pos: [4.0, 0.0],
                },
            ],
        };
        assert_eq!(frame.enemies_in_radius([0.0, 0.0], 2.0), vec![9]);
        assert!(frame.enemies_in_radius([f32::NAN, 0.0], 2.0).is_empty());
        assert_eq!(frame.enemies[0].pos, [1.0, 0.0]);
    }
}

/// First-pass policy: predicted-view queries, without an extra interpolation
/// subtraction (our enemies are predicted, not rendered from a past snapshot).
pub const MAX_REWIND_TICKS: u32 = 6; // 200 ms at 30 Hz

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindFallback {
    MissingViewTick,
    FutureTick,
    OutsideWindow,
    LatencyMismatch,
    MissingHistory,
    CasterDiscontinuity,
}

pub fn validate_view_tick(
    now: u32,
    requested: Option<u32>,
    rtt_seconds: f64,
) -> Result<u32, RewindFallback> {
    let requested = requested.ok_or(RewindFallback::MissingViewTick)?;
    if game_shared::is_newer_input_seq(requested, now) {
        return Err(RewindFallback::FutureTick);
    }
    let age = now.wrapping_sub(requested);
    if age > MAX_REWIND_TICKS {
        return Err(RewindFallback::OutsideWindow);
    }
    // Two ticks allow command sampling and send scheduling quantization. Never
    // grant a larger window from a client-supplied ping or timestamp.
    let latency_ticks = if rtt_seconds.is_finite() && rtt_seconds >= 0.0 {
        (rtt_seconds * 0.5 / game_shared::FIXED_DT_SECONDS as f64)
            .ceil()
            .min(MAX_REWIND_TICKS as f64) as u32
    } else {
        0
    };
    if age > (latency_ticks + 2).min(MAX_REWIND_TICKS) {
        return Err(RewindFallback::LatencyMismatch);
    }
    Ok(requested)
}

impl StateHistory {
    /// Use only continuous identities. Origin is the current server-simulated
    /// caster position after its preceding movement, never a client claim.
    pub fn validated_targets(
        &self,
        epoch: u32,
        tick: u32,
        now: u32,
        caster: u64,
        origin: [f32; 2],
    ) -> Result<Vec<(u64, [f32; 2])>, RewindFallback> {
        let first = self
            .at_tick(epoch, tick)
            .map_err(|_| RewindFallback::MissingHistory)?;
        let age = now.wrapping_sub(tick);
        if age > MAX_REWIND_TICKS {
            return Err(RewindFallback::OutsideWindow);
        }
        let caster_pos = first
            .heroes
            .iter()
            .find(|h| h.id == caster)
            .ok_or(RewindFallback::CasterDiscontinuity)?
            .pos;
        let reach = game_shared::HERO_SPEED * game_shared::FIXED_DT_SECONDS * age as f32 + 0.5;
        if distance_sq(caster_pos, origin) > reach * reach {
            return Err(RewindFallback::CasterDiscontinuity);
        }
        let mut previous = first;
        let mut eligible = first.enemies.clone();
        for offset in 1..=age {
            let frame = self
                .at_tick(epoch, tick.wrapping_add(offset))
                .map_err(|_| RewindFallback::MissingHistory)?;
            let before = previous
                .heroes
                .iter()
                .find(|h| h.id == caster)
                .ok_or(RewindFallback::CasterDiscontinuity)?;
            let after = frame
                .heroes
                .iter()
                .find(|h| h.id == caster)
                .ok_or(RewindFallback::CasterDiscontinuity)?;
            let max_step = game_shared::HERO_SPEED * game_shared::FIXED_DT_SECONDS + 0.5;
            if distance_sq(before.pos, after.pos) > max_step * max_step {
                return Err(RewindFallback::CasterDiscontinuity);
            }
            // A generous per-tick discontinuity guard, not enemy speed policing.
            eligible.retain(|enemy| {
                let a = previous.enemies.iter().find(|e| e.id == enemy.id);
                let b = frame.enemies.iter().find(|e| e.id == enemy.id);
                matches!((a, b), (Some(a), Some(b)) if distance_sq(a.pos, b.pos) <= 4.0)
            });
            previous = frame;
        }
        Ok(eligible.into_iter().map(|e| (e.id, e.pos)).collect())
    }
}

#[cfg(test)]
mod rewind_policy_tests {
    use super::*;
    #[test]
    fn timestamps_are_bounded_by_server_latency_and_wrap_safely() {
        assert_eq!(validate_view_tick(100, Some(95), 0.300), Ok(95));
        assert_eq!(
            validate_view_tick(100, Some(93), 5.0),
            Err(RewindFallback::OutsideWindow)
        );
        assert_eq!(
            validate_view_tick(100, Some(95), 0.0),
            Err(RewindFallback::LatencyMismatch)
        );
        assert_eq!(
            validate_view_tick(100, Some(101), 0.3),
            Err(RewindFallback::FutureTick)
        );
        assert_eq!(
            validate_view_tick(100, None, 0.3),
            Err(RewindFallback::MissingViewTick)
        );
        assert_eq!(validate_view_tick(1, Some(u32::MAX), 0.3), Ok(u32::MAX));
    }

    #[test]
    fn discontinuities_and_disappeared_targets_cannot_be_rewound() {
        let mut history = StateHistory::default();
        history.frames.push_back(HistoricalFrame {
            match_epoch: 0,
            tick: 1,
            heroes: vec![HistoricalActor {
                id: 1,
                pos: [0.0, 0.0],
            }],
            enemies: vec![
                HistoricalActor {
                    id: 10,
                    pos: [1.0, 0.0],
                },
                HistoricalActor {
                    id: 11,
                    pos: [1.0, 0.0],
                },
            ],
        });
        history.frames.push_back(HistoricalFrame {
            match_epoch: 0,
            tick: 2,
            heroes: vec![HistoricalActor {
                id: 1,
                pos: [0.1, 0.0],
            }],
            enemies: vec![HistoricalActor {
                id: 10,
                pos: [1.2, 0.0],
            }],
        });
        assert_eq!(
            history.validated_targets(0, 1, 2, 1, [0.1, 0.0]).unwrap(),
            vec![(10, [1.0, 0.0])]
        );
        assert_eq!(
            history.validated_targets(0, 1, 2, 1, [30.0, 0.0]),
            Err(RewindFallback::CasterDiscontinuity)
        );
        history.frames.back_mut().unwrap().enemies[0].pos = [20.0, 0.0];
        assert!(
            history
                .validated_targets(0, 1, 2, 1, [0.1, 0.0])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            history.validated_targets(1, 1, 2, 1, [0.1, 0.0]),
            Err(RewindFallback::MissingHistory)
        );
    }
}

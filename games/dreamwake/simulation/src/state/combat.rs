//! Saved game combat lifecycles and non-reusable defensive credit.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub(crate) const COMBAT_HISTORY_TICKS: u32 = 32;
pub(crate) const MAX_SHIELD_EPISODES: usize = 64;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CombatIdentity {
    pub generation: u32,
    pub segment: u64,
    pub pose_revision: u64,
    pub tick: u32,
}
impl Default for CombatIdentity {
    fn default() -> Self {
        Self {
            generation: 1,
            segment: 1,
            pose_revision: 1,
            tick: 0,
        }
    }
}
impl CombatIdentity {
    pub fn valid(&self) -> bool {
        self.generation > 0 && self.segment > 0 && self.pose_revision > 0
    }
    pub fn advance(&mut self, tick: u32) -> Result<(), &'static str> {
        if tick < self.tick {
            return Err("combat clock moved backwards");
        }
        self.pose_revision = self
            .pose_revision
            .checked_add(1)
            .ok_or("combat pose exhausted")?;
        self.tick = tick;
        Ok(())
    }
    pub fn discontinuity(&mut self, respawn: bool) -> Result<(), &'static str> {
        let segment = self
            .segment
            .checked_add(1)
            .ok_or("combat segment exhausted")?;
        let generation = if respawn {
            self.generation
                .checked_add(1)
                .ok_or("combat lifecycle exhausted")?
        } else {
            self.generation
        };
        self.segment = segment;
        self.generation = generation;
        Ok(())
    }
}

/// Each grant adds only new shield credit. Refreshing an existing shield extends
/// its old credits, so a historical hit cannot spend inherited capacity twice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShieldEpisode {
    pub id: u64,
    pub activated_tick: u32,
    pub expires_tick: u32,
    pub granted: f32,
    pub spent: f32,
}
impl ShieldEpisode {
    fn remaining(&self) -> f32 {
        (self.granted - self.spent).max(0.0)
    }
    fn active_at(&self, tick: u32) -> bool {
        self.activated_tick <= tick && tick < self.expires_tick
    }
}
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DefenseEpisodes {
    pub next_episode: u64,
    pub episodes: Vec<ShieldEpisode>,
}
impl Default for DefenseEpisodes {
    fn default() -> Self {
        Self {
            next_episode: 1,
            episodes: Vec::new(),
        }
    }
}
impl DefenseEpisodes {
    pub fn valid(&self, tick: u32) -> bool {
        self.next_episode > 0
            && self.episodes.len() <= MAX_SHIELD_EPISODES
            && self.episodes.windows(2).all(|p| p[0].id < p[1].id)
            && self.episodes.iter().all(|e| {
                e.id > 0
                    && e.id < self.next_episode
                    && e.activated_tick <= tick
                    && e.expires_tick > e.activated_tick
                    && e.granted.is_finite()
                    && e.granted > 0.0
                    && e.spent.is_finite()
                    && e.spent >= 0.0
                    && e.spent <= e.granted
            })
    }
    pub fn cutoff(&self) -> u64 {
        self.next_episode - 1
    }
    #[cfg(test)]
    pub fn remaining_at(&self, tick: u32) -> f32 {
        self.episodes
            .iter()
            .filter(|e| e.active_at(tick))
            .map(ShieldEpisode::remaining)
            .sum()
    }
    pub fn reconcile(
        &mut self,
        combat: &engine_core::CombatState,
        tick: u32,
    ) -> Result<(), &'static str> {
        let mut next = self.clone();
        next.reconcile_inner(combat, tick)?;
        *self = next;
        Ok(())
    }
    fn reconcile_inner(
        &mut self,
        combat: &engine_core::CombatState,
        tick: u32,
    ) -> Result<(), &'static str> {
        self.episodes
            .retain(|e| e.expires_tick.saturating_add(COMBAT_HISTORY_TICKS) >= tick);
        // Engine timers decide expiry. A refresh extends credits that were still
        // active on the previous tick; credits already expired stay historical.
        let duration = (combat.shield_remaining / crate::DT).ceil().max(0.0) as u32;
        let expires = tick.checked_add(duration).ok_or("shield expiry overflow")?;
        if combat.shield <= 0.0 || duration == 0 {
            for e in &mut self.episodes {
                if e.expires_tick > tick {
                    e.expires_tick = tick.max(e.activated_tick + 1);
                }
            }
            return Ok(());
        }
        let previous = self
            .episodes
            .iter()
            .filter(|e| e.expires_tick >= tick)
            .map(ShieldEpisode::remaining)
            .sum::<f32>();
        for e in &mut self.episodes {
            if e.expires_tick >= tick && e.remaining() > 0.0 {
                e.expires_tick = expires;
            }
        }
        let delta = combat.shield - previous;
        if delta > 0.0 {
            if self.episodes.len() >= MAX_SHIELD_EPISODES {
                return Err("shield episode budget");
            }
            let next = self
                .next_episode
                .checked_add(1)
                .ok_or("shield episode identity exhausted")?;
            self.episodes.push(ShieldEpisode {
                id: self.next_episode,
                activated_tick: tick,
                expires_tick: expires,
                granted: delta,
                spent: 0.0,
            });
            self.next_episode = next;
        } else if delta < 0.0 {
            // Current-state damage from an external game operation consumes
            // credits instead of minting a replacement episode.
            self.absorb(tick, self.cutoff(), -delta, -delta, tick);
        }
        Ok(())
    }
    /// Charge historical credit monotonically. Return total absorption and the
    /// part also removed from today's shield; newer credits are never charged.
    pub fn absorb(
        &mut self,
        query_tick: u32,
        cutoff: u64,
        recorded_shield: f32,
        incoming: f32,
        current_tick: u32,
    ) -> (f32, f32) {
        let mut remaining = incoming.min(recorded_shield).max(0.0);
        let mut absorbed = 0.0;
        let mut current = 0.0;
        for e in &mut self.episodes {
            if e.id > cutoff || !e.active_at(query_tick) {
                continue;
            }
            let amount = remaining.min(e.remaining());
            e.spent = (e.spent + amount).min(e.granted);
            absorbed += amount;
            if e.active_at(current_tick) {
                current += amount;
            }
            remaining -= amount;
            if remaining <= 0.0 {
                break;
            }
        }
        (absorbed, current)
    }
}

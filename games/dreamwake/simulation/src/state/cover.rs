//! Veil shutters are game-owned moving cover: they stop shots, while travelers
//! and enemies may cross their magical surface. Their phase is authoritative.
use super::*;
pub(crate) const COVER_HALF_EXTENTS: [f32; 3] = [1.5, 1.0, 0.15];
pub(crate) const COVER_PERIOD_TICKS: u32 = 120;
pub(crate) const COVER_TRAVEL_TICKS: u32 = 30;
pub(crate) const COVER_CLOSED_HEIGHT: f32 = 1.0;
pub(crate) const COVER_OPEN_HEIGHT: f32 = 3.4;
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cover {
    pub id: u64,
    pub marker_id: u64,
    pub ground_position: [f32; 2],
    pub present: bool,
    /// Resting endpoint before the final travel interval of this phase.
    pub open: bool,
    pub next_transition_tick: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedCover {
    pub id: u64,
    pub marker_id: u64,
    pub ground_position: [f32; 2],
    pub present: bool,
    pub identity: CombatIdentity,
    pub open: bool,
    pub next_transition_tick: u32,
}
impl Cover {
    /// Travel occupies the end of the saved phase, meeting the next resting
    /// endpoint continuously when authority flips `open`.
    pub fn position(&self, tick: u32) -> [f32; 3] {
        let remaining = self.next_transition_tick.saturating_sub(tick);
        let progress = (COVER_TRAVEL_TICKS - remaining.min(COVER_TRAVEL_TICKS)) as f32
            / COVER_TRAVEL_TICKS as f32;
        let travel = (COVER_OPEN_HEIGHT - COVER_CLOSED_HEIGHT) * progress;
        [
            self.ground_position[0],
            if self.open {
                COVER_OPEN_HEIGHT - travel
            } else {
                COVER_CLOSED_HEIGHT + travel
            },
            self.ground_position[1],
        ]
    }
    pub fn presentation(&self, identity: &CombatIdentity) -> crate::replication::PublicCoverView {
        crate::replication::PublicCoverView {
            id: self.id,
            revision: identity.segment,
            present: self.present,
            position: self.position(identity.tick),
            half_extents: COVER_HALF_EXTENTS,
            open: self.open,
        }
    }
}
impl SavedCover {
    pub fn actor(&self) -> Cover {
        Cover {
            id: self.id,
            marker_id: self.marker_id,
            ground_position: self.ground_position,
            present: self.present,
            open: self.open,
            next_transition_tick: self.next_transition_tick,
        }
    }
    pub fn valid(&self, tick: u32) -> bool {
        self.id > 0
            && self.marker_id > 0
            && self.marker_id != self.id
            && DEMO_COVER_POSITIONS.contains(&self.ground_position)
            && self.identity.valid()
            && self.identity.tick <= tick
            && (!self.present
                || (self.next_transition_tick > tick
                    && self.next_transition_tick - tick <= COVER_PERIOD_TICKS))
    }
}

impl crate::DreamSimulation {
    /// Authority-side map mutation. Retain the region descriptor so later
    /// observers receive current absence instead of reconstructing a map default.
    /// Apply before the next authoritative step and publish its committed capture.
    /// There is deliberately no client command or input binding for this API.
    pub fn remove_cover(&mut self, id: u64) -> Result<bool, &'static str> {
        let mut query = self.world.query::<(&mut Cover, &mut CombatIdentity)>();
        for (mut cover, mut identity) in query.iter_mut(&mut self.world) {
            if cover.id == id && cover.present {
                identity.discontinuity(false)?;
                cover.present = false;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

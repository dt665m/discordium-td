//! Stable support identity and integer motion phase; no render transform owns motion.
use super::*;
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Platform {
    pub id: u64,
    pub collider: engine_core::ColliderKey,
    pub motion_revision: u32,
    pub origin_gameplay_tick: u32,
    pub gameplay_tick: u32,
    pub phase: u16,
}
pub(crate) type SavedPlatform = Platform;
impl Platform {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            collider: engine_core::ColliderKey {
                index: id,
                generation: 1,
            },
            motion_revision: 1,
            origin_gameplay_tick: 0,
            gameplay_tick: 0,
            phase: 0,
        }
    }
    pub fn presentation(&self, scene_revision: u64) -> crate::replication::PublicPlatformView {
        crate::replication::PublicPlatformView {
            id: self.id,
            collider: self.collider,
            scene_revision,
            motion_revision: self.motion_revision,
            origin_gameplay_tick: self.origin_gameplay_tick,
            gameplay_tick: self.gameplay_tick,
            phase: self.phase,
            pose: crate::platform::pose(self.phase),
            half_extents: crate::platform::PLATFORM_HALF_EXTENTS,
        }
    }
    pub fn advance(&mut self, tick: u32) -> Result<(), crate::replication::ReplicationError> {
        if tick < self.gameplay_tick || tick < self.origin_gameplay_tick {
            return Err(crate::replication::ReplicationError::InvalidState);
        }
        self.gameplay_tick = tick;
        self.phase = ((tick - self.origin_gameplay_tick)
            % u32::from(crate::platform::PLATFORM_PERIOD_TICKS)) as u16;
        Ok(())
    }
    pub fn valid(&self, tick: u32, scene_revision: u64) -> bool {
        self.phase < crate::platform::PLATFORM_PERIOD_TICKS
            && self.gameplay_tick == tick
            && self.presentation(scene_revision).valid()
    }
}

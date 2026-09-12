//! Immutable game combat poses retained in complete authoritative checkpoints.
use bevy::prelude::*;
use engine_core::{ColliderKey, CombatPose, HistoryLimits, HitHistory, HitMetadata};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_COMBAT_POSES: usize = 256;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedCombatPose {
    pub id: u64,
    pub generation: u32,
    pub segment: u64,
    pub pose_revision: u64,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub shape: engine_core::CollisionShape,
    pub faction: u8,
    pub alive: bool,
    pub damageable: bool,
    pub invulnerable: bool,
    pub shield_cutoff: u64,
    pub shield: f32,
}
impl SavedCombatPose {
    pub fn prepared(&self) -> CombatPose {
        CombatPose {
            entity: ColliderKey {
                index: self.id,
                generation: self.generation,
            },
            segment: self.segment,
            pose_revision: self.pose_revision,
            position: Vec3::from_array(self.position),
            rotation: Quat::from_array(self.rotation),
            shape: self.shape,
            metadata: HitMetadata([
                u64::from(self.faction)
                    | (u64::from(self.alive) << 8)
                    | (u64::from(self.damageable) << 9)
                    | (u64::from(self.invulnerable) << 10),
                self.shield_cutoff,
                u64::from(self.shield.to_bits()),
                0,
            ]),
        }
    }
    fn valid(&self) -> bool {
        self.id > 0
            && self.generation > 0
            && self.segment > 0
            && self.pose_revision > 0
            && self.faction <= 2
            && self.shield.is_finite()
            && self.shield >= 0.0
            && (!self.damageable || self.alive)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedCombatFrame {
    pub tick: u32,
    pub scene: u64,
    pub poses: Vec<SavedCombatPose>,
}
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CombatArchive {
    pub frames: Vec<SavedCombatFrame>,
}
#[derive(Resource, Debug)]
pub(crate) struct CombatHistory {
    pub archive: CombatArchive,
    pub prepared: HitHistory,
}
impl Default for CombatHistory {
    fn default() -> Self {
        Self::restore(CombatArchive::default()).expect("empty combat history")
    }
}
impl CombatHistory {
    pub fn restore(archive: CombatArchive) -> Result<Self, &'static str> {
        if archive.frames.len() > crate::COMBAT_HISTORY_TICKS as usize {
            return Err("combat history frame budget");
        }
        let mut result = Self {
            archive: CombatArchive::default(),
            prepared: HitHistory::new(HistoryLimits {
                frames: crate::COMBAT_HISTORY_TICKS as usize,
                poses_per_frame: MAX_COMBAT_POSES,
                total_poses: crate::COMBAT_HISTORY_TICKS as usize * MAX_COMBAT_POSES,
            })
            .map_err(|_| "combat history limits")?,
        };
        for frame in archive.frames {
            if frame.poses.windows(2).any(|p| p[0].id >= p[1].id) {
                return Err("unordered combat history poses");
            }
            result.capture(frame)?;
        }
        Ok(result)
    }
    pub fn capture(&mut self, mut frame: SavedCombatFrame) -> Result<(), &'static str> {
        if frame.tick == 0
            || frame.poses.len() > MAX_COMBAT_POSES
            || frame.poses.iter().any(|p| !p.valid())
        {
            return Err("invalid combat history frame");
        }
        frame.poses.sort_by_key(|p| p.id);
        let poses = frame
            .poses
            .iter()
            .map(SavedCombatPose::prepared)
            .collect::<Vec<_>>();
        self.prepared
            .capture(u64::from(frame.tick), frame.scene, &poses)
            .map_err(|_| "invalid combat history poses")?;
        if self.archive.frames.len() == crate::COMBAT_HISTORY_TICKS as usize {
            self.archive.frames.remove(0);
        }
        self.archive.frames.push(frame);
        Ok(())
    }
    pub fn logical_frame(&self, tick: u32, scene: u64) -> Option<&SavedCombatFrame> {
        self.archive
            .frames
            .iter()
            .find(|f| f.tick == tick && f.scene == scene)
    }
}

//! Bounded immutable combat poses, independent of rendering and live ECS.
//!
//! Games capture actual collision poses at a declared simulation phase and own
//! faction, defense, blocker and damage policy. Fractional queries require exact
//! adjacent endpoints; absent ticks, lifecycle, scene or segment changes reject.
//! This kernel does not authorize damage or implement a game's hitscan action.
mod interpolation;
mod query;
#[cfg(test)]
mod tests;
use crate::physics::kinematic::{ColliderKey, CollisionShape, StaticCollider, prepare_collider};
use bevy::prelude::*;
pub use query::{Hit, HitQuery, QueryBudget};
use std::collections::VecDeque;

/// Fixed-size game-owned flags/defense data; no hidden heap allocation per pose.
/// Games encode shield episodes, damageability, factions and blocker categories.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitMetadata(pub [u64; 4]);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombatPose {
    pub entity: ColliderKey,
    pub segment: u64,
    pub pose_revision: u64,
    pub position: Vec3,
    pub rotation: Quat,
    pub shape: CollisionShape,
    pub metadata: HitMetadata,
}
impl CombatPose {
    pub(super) fn collider(self) -> StaticCollider {
        StaticCollider {
            key: self.entity,
            position: self.position.to_array(),
            rotation: self.rotation.to_array(),
            shape: self.shape,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct HistoryLimits {
    pub frames: usize,
    pub poses_per_frame: usize,
    pub total_poses: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryError {
    InvalidLimits,
    InvalidPose,
    InvalidFrame,
    Capacity,
    MissingHistory,
    WrongScene,
    MissingEntity,
    WrongLifecycle,
    Discontinuity,
    InvalidQuery,
    QueryBudget,
    ResultBudget,
    UnsupportedQuery,
}
/// Owns copied descriptors only, never references to Bevy actors or render poses.
#[derive(Debug)]
pub struct HitFrame {
    tick: u64,
    scene: u64,
    poses: Box<[CombatPose]>,
}
impl HitFrame {
    pub fn tick(&self) -> u64 {
        self.tick
    }
    pub fn scene(&self) -> u64 {
        self.scene
    }
    pub fn poses(&self) -> &[CombatPose] {
        &self.poses
    }
    pub fn pose(&self, entity: ColliderKey, segment: u64) -> Result<&CombatPose, HistoryError> {
        let index = self
            .poses
            .binary_search_by_key(&entity.index, |p| p.entity.index)
            .map_err(|_| HistoryError::MissingEntity)?;
        let pose = &self.poses[index];
        if pose.entity != entity {
            return Err(HistoryError::WrongLifecycle);
        }
        if pose.segment != segment {
            return Err(HistoryError::Discontinuity);
        }
        Ok(pose)
    }
}
#[derive(Resource, Debug)]
pub struct HitHistory {
    limits: HistoryLimits,
    frames: VecDeque<HitFrame>,
    total_poses: usize,
}
impl HitHistory {
    pub fn new(limits: HistoryLimits) -> Result<Self, HistoryError> {
        // Hard ceilings bound configuration itself. Primitive descriptors and
        // metadata are fixed-size; no meshes or arbitrary user payloads retained.
        if !(1..=512).contains(&limits.frames)
            || !(1..=4096).contains(&limits.poses_per_frame)
            || limits.total_poses < limits.poses_per_frame
            || limits.total_poses > 131_072
        {
            return Err(HistoryError::InvalidLimits);
        }
        Ok(Self {
            limits,
            frames: VecDeque::new(),
            total_poses: 0,
        })
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn retained_poses(&self) -> usize {
        self.total_poses
    }
    /// Validate completely before evicting old frames or publishing new state.
    /// Oldest frames are retired until both independent retention caps fit.
    pub fn capture(
        &mut self,
        tick: u64,
        scene: u64,
        poses: &[CombatPose],
    ) -> Result<(), HistoryError> {
        if scene == 0 || self.frames.back().is_some_and(|f| tick <= f.tick) {
            return Err(HistoryError::InvalidFrame);
        }
        if poses.len() > self.limits.poses_per_frame {
            return Err(HistoryError::Capacity);
        }
        let mut next = poses.to_vec();
        next.sort_by_key(|p| p.entity);
        let mut previous = None;
        for pose in &mut next {
            if pose.segment == 0 || pose.pose_revision == 0 || previous == Some(pose.entity.index) {
                return Err(HistoryError::InvalidPose);
            }
            previous = Some(pose.entity.index);
            prepare_collider(pose.collider()).map_err(|_| HistoryError::InvalidPose)?;
            pose.rotation = pose.rotation.normalize();
        }
        while self.frames.len() >= self.limits.frames
            || self.total_poses + next.len() > self.limits.total_poses
        {
            let old = self
                .frames
                .pop_front()
                .expect("validated nonzero retention limits");
            self.total_poses -= old.poses.len();
        }
        self.total_poses += next.len();
        self.frames.push_back(HitFrame {
            tick,
            scene,
            poses: next.into_boxed_slice(),
        });
        Ok(())
    }
    pub fn frame(&self, tick: u64, scene: u64) -> Result<&HitFrame, HistoryError> {
        let frame = self
            .frames
            .iter()
            .find(|f| f.tick == tick)
            .ok_or(HistoryError::MissingHistory)?;
        if frame.scene != scene {
            return Err(HistoryError::WrongScene);
        }
        Ok(frame)
    }
}

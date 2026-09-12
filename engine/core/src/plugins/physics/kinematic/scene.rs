mod sweep;
pub use sweep::*;
#[cfg(test)]
mod sweep_tests;
use super::model::*;
use bevy::prelude::*;
use parry3d::{
    math::{Pose, Rotation, Vector},
    query::{self, ShapeCastOptions, ShapeCastStatus},
    shape::{Capsule, SharedShape},
};
use serde::{Deserialize, Serialize};

pub const MAX_COLLIDERS: usize = 1024;
pub(crate) const MAX_COORDINATE: f32 = 100_000.0;
const MAX_EXTENT: f32 = 10_000.0;
/// Bounded convex primitives supported by the first static query adapter.
/// Geometry is delegated to Parry; meshes and dynamic solvers are not modeled.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum CollisionShape {
    Box { half_extents: [f32; 3] },
    Ball { radius: f32 },
    Capsule { half_segment: f32, radius: f32 },
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StaticCollider {
    pub key: ColliderKey,
    pub position: [f32; 3],
    /// Unit quaternion XYZW; scene construction normalizes accepted roundoff.
    pub rotation: [f32; 4],
    pub shape: CollisionShape,
}
impl StaticCollider {
    pub fn cuboid(key: ColliderKey, position: [f32; 3], half_extents: [f32; 3]) -> Self {
        Self {
            key,
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            shape: CollisionShape::Box { half_extents },
        }
    }
}
#[derive(Clone)]
struct PreparedCollider {
    descriptor: StaticCollider,
    pose: Pose,
    shape: SharedShape,
}
/// Frozen simulation-space collision data, sorted by stable identity. Replace
/// the whole resource after a versioned scene transition; never mutate render
/// transforms or retain a reference to a render replica for replay.
#[derive(Resource, Clone)]
pub struct CollisionScene {
    revision: u64,
    colliders: Vec<PreparedCollider>,
}
impl CollisionScene {
    pub fn new(revision: u64, mut colliders: Vec<StaticCollider>) -> Result<Self, KinematicError> {
        if revision == 0 || colliders.len() > MAX_COLLIDERS {
            return Err(KinematicError::InvalidScene);
        }
        colliders.sort_by_key(|v| v.key);
        let mut prepared = Vec::with_capacity(colliders.len());
        let mut previous = None;
        for collider in colliders {
            if previous == Some(collider.key.index) {
                return Err(KinematicError::InvalidScene);
            }
            previous = Some(collider.key.index);
            let (pose, shape) = prepare_collider(collider)?;
            prepared.push(PreparedCollider {
                descriptor: collider,
                pose,
                shape,
            });
        }
        Ok(Self {
            revision,
            colliders: prepared,
        })
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn len(&self) -> usize {
        self.colliders.len()
    }
    pub fn is_empty(&self) -> bool {
        self.colliders.is_empty()
    }
    pub fn collider(&self, key: ColliderKey) -> Option<&StaticCollider> {
        self.find(key).map(|v| &v.descriptor)
    }
    fn find(&self, key: ColliderKey) -> Option<&PreparedCollider> {
        self.colliders
            .binary_search_by_key(&key, |v| v.descriptor.key)
            .ok()
            .map(|i| &self.colliders[i])
    }
    pub(crate) fn ground(&self, hit: Hit, position: Vec3) -> GroundContact {
        let collider = self
            .find(hit.collider)
            .expect("query hit is from immutable scene");
        GroundContact {
            collider: hit.collider,
            scene_revision: self.revision,
            normal: hit.normal.to_array(),
            local_position: collider
                .pose
                .inverse_transform_point(Vector::from_array(position.to_array()))
                .to_array(),
        }
    }
}
/// Shared bounded convex descriptor validation for immutable query worlds.
pub(crate) fn prepare_collider(
    collider: StaticCollider,
) -> Result<(Pose, SharedShape), KinematicError> {
    if collider.key.index == 0
        || collider.key.generation == 0
        || !bounded(collider.position, MAX_COORDINATE)
        || !bounded(collider.rotation, 1.001)
    {
        return Err(KinematicError::InvalidScene);
    }
    let rotation = Rotation::from_array(collider.rotation);
    if (rotation.length_squared() - 1.0).abs() > 0.001 {
        return Err(KinematicError::InvalidScene);
    }
    let positive = |v: f32| v.is_finite() && v > 0.0 && v <= MAX_EXTENT;
    let shape = match collider.shape {
        CollisionShape::Box { half_extents } if half_extents.into_iter().all(positive) => {
            SharedShape::cuboid(half_extents[0], half_extents[1], half_extents[2])
        }
        CollisionShape::Ball { radius } if positive(radius) => SharedShape::ball(radius),
        CollisionShape::Capsule {
            half_segment,
            radius,
        } if positive(radius)
            && half_segment.is_finite()
            && (0.0..=MAX_EXTENT).contains(&half_segment) =>
        {
            SharedShape::capsule_y(half_segment, radius)
        }
        _ => return Err(KinematicError::InvalidScene),
    };
    Ok((
        Pose::from_parts(Vector::from_array(collider.position), rotation.normalize()),
        shape,
    ))
}
pub(crate) fn bounded<const N: usize>(values: [f32; N], limit: f32) -> bool {
    values
        .into_iter()
        .all(|v| v.is_finite() && v.abs() <= limit)
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct Hit {
    pub collider: ColliderKey,
    pub fraction: f32,
    pub normal: Vec3,
    pub point: Vec3,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct Overlap {
    pub collider: ColliderKey,
    pub depth: f32,
    pub normal: Vec3,
}
/// One budget covers movement, stance clearance, steps, ground and overlap tests.
pub(crate) struct Queries<'a> {
    scene: &'a CollisionScene,
    limit: u32,
    pub used: u32,
}
impl<'a> Queries<'a> {
    pub fn new(scene: &'a CollisionScene, limit: u32) -> Self {
        Self {
            scene,
            limit,
            used: 0,
        }
    }
    fn charge(&mut self) -> Result<(), KinematicError> {
        if self.used >= self.limit {
            return Err(KinematicError::QueryBudgetExceeded);
        }
        self.used += 1;
        Ok(())
    }
    pub fn cast(
        &mut self,
        foot: Vec3,
        height: f32,
        radius: f32,
        movement: Vec3,
        skin: f32,
    ) -> Result<Option<Hit>, KinematicError> {
        self.cast_excluding(foot, height, radius, movement, skin, None)
    }
    pub(crate) fn cast_excluding(
        &mut self,
        foot: Vec3,
        height: f32,
        radius: f32,
        movement: Vec3,
        skin: f32,
        excluded: Option<ColliderKey>,
    ) -> Result<Option<Hit>, KinematicError> {
        let shape = Capsule::new_y(height * 0.5 - radius, radius);
        let pose = Pose::from_translation(Vector::from_array(
            (foot + Vec3::Y * (height * 0.5)).to_array(),
        ));
        let options = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: skin,
            stop_at_penetration: false,
            compute_impact_geometry_on_penetration: true,
        };
        let mut best: Option<Hit> = None;
        for collider in &self.scene.colliders {
            if Some(collider.descriptor.key) == excluded {
                continue;
            }
            self.charge()?;
            let Some(hit) = query::cast_shapes(
                &pose,
                Vector::from_array(movement.to_array()),
                &shape,
                &collider.pose,
                Vector::ZERO,
                collider.shape.as_ref(),
                options,
            )
            .map_err(|_| KinematicError::UnsupportedQuery)?
            else {
                continue;
            };
            if hit.status == ShapeCastStatus::Failed {
                return Err(KinematicError::InvalidQueryResult);
            }
            let normal = Vec3::from_array((collider.pose.rotation * hit.normal2).to_array());
            if !normal.is_finite()
                || !(0.5..=1.5).contains(&normal.length_squared())
                || !hit.time_of_impact.is_finite()
                || !(-0.00001..=1.00001).contains(&hit.time_of_impact)
            {
                return Err(KinematicError::InvalidQueryResult);
            }
            let normal = normal.normalize();
            // Tangential/separating zero-time support cannot mask a later wall.
            if movement.dot(normal) >= -0.000001 {
                continue;
            }
            let point = Vec3::from_array((collider.pose * hit.witness2).to_array());
            if !point.is_finite() {
                return Err(KinematicError::InvalidQueryResult);
            }
            let hit = Hit {
                collider: collider.descriptor.key,
                fraction: hit.time_of_impact.clamp(0.0, 1.0),
                normal,
                point,
            };
            if best.is_none_or(|current| {
                hit.fraction
                    .total_cmp(&current.fraction)
                    .then(hit.collider.cmp(&current.collider))
                    .is_lt()
            }) {
                best = Some(hit);
            }
        }
        Ok(best)
    }
    pub fn support_near(
        &mut self,
        foot: Vec3,
        height: f32,
        radius: f32,
        skin: f32,
        walkable: f32,
    ) -> Result<Option<(Hit, f32)>, KinematicError> {
        let shape = Capsule::new_y(height * 0.5 - radius, radius);
        let pose = Pose::from_translation(Vector::from_array(
            (foot + Vec3::Y * (height * 0.5)).to_array(),
        ));
        let mut best: Option<(Hit, f32)> = None;
        for collider in &self.scene.colliders {
            self.charge()?;
            let Some(contact) = query::contact(
                &pose,
                &shape,
                &collider.pose,
                collider.shape.as_ref(),
                skin * 1.5,
            )
            .map_err(|_| KinematicError::UnsupportedQuery)?
            else {
                continue;
            };
            let normal = Vec3::from_array(contact.normal2.to_array());
            if !contact.dist.is_finite()
                || !normal.is_finite()
                || !(0.5..=1.5).contains(&normal.length_squared())
            {
                return Err(KinematicError::InvalidQueryResult);
            }
            let normal = normal.normalize();
            if normal.y < walkable || contact.dist < -0.00001 || contact.dist > skin * 1.5 {
                continue;
            }
            let point = Vec3::from_array(contact.point2.to_array());
            if !point.is_finite() {
                return Err(KinematicError::InvalidQueryResult);
            }
            let hit = Hit {
                collider: collider.descriptor.key,
                fraction: 0.0,
                normal,
                point,
            };
            if best.is_none_or(|(old, dist)| {
                contact
                    .dist
                    .total_cmp(&dist)
                    .then(hit.collider.cmp(&old.collider))
                    .is_lt()
            }) {
                best = Some((hit, contact.dist));
            }
        }
        Ok(best)
    }
    pub fn overlap(
        &mut self,
        foot: Vec3,
        height: f32,
        radius: f32,
        tolerance: f32,
    ) -> Result<Option<Overlap>, KinematicError> {
        let shape = Capsule::new_y(height * 0.5 - radius, radius);
        let pose = Pose::from_translation(Vector::from_array(
            (foot + Vec3::Y * (height * 0.5)).to_array(),
        ));
        let mut deepest: Option<Overlap> = None;
        for collider in &self.scene.colliders {
            self.charge()?;
            let Some(contact) =
                query::contact(&pose, &shape, &collider.pose, collider.shape.as_ref(), 0.0)
                    .map_err(|_| KinematicError::UnsupportedQuery)?
            else {
                continue;
            };
            let normal = Vec3::from_array(contact.normal2.to_array());
            if !contact.dist.is_finite()
                || !normal.is_finite()
                || !(0.5..=1.5).contains(&normal.length_squared())
            {
                return Err(KinematicError::InvalidQueryResult);
            }
            if contact.dist >= -tolerance {
                continue;
            }
            let hit = Overlap {
                collider: collider.descriptor.key,
                depth: -contact.dist,
                normal: normal.normalize(),
            };
            if deepest.is_none_or(|current| {
                hit.depth
                    .total_cmp(&current.depth)
                    .reverse()
                    .then(hit.collider.cmp(&current.collider))
                    .is_lt()
            }) {
                deepest = Some(hit);
            }
        }
        Ok(deepest)
    }
}

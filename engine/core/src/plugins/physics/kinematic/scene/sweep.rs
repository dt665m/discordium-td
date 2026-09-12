use super::{CollisionScene, MAX_COORDINATE, MAX_EXTENT, bounded};
use crate::plugins::physics::kinematic::{ColliderKey, KinematicError};
use bevy::prelude::Vec3;
use parry3d::{
    math::{Pose, Vector},
    query::{self, ShapeCastOptions, ShapeCastStatus},
    shape::Ball,
};

pub const MAX_SCENE_QUERY_TESTS: u32 = 1_048_576;
pub const MAX_SCENE_QUERY_RESULTS: u32 = 65_536;

/// Aggregate work allowance shared by a caller's entire query batch. One pair
/// test means one Parry contact or shape cast against one prepared collider.
/// A returned nearest hit consumes one result; misses consume no result slots.
/// Failed queries retain their consumed work and never return a partial answer.
#[derive(Debug, PartialEq, Eq)]
pub struct SceneQueryBudget {
    test_limit: u32,
    result_limit: u32,
    tests: u32,
    results: u32,
}
impl SceneQueryBudget {
    pub fn new(pair_tests: u32, results: u32) -> Result<Self, KinematicError> {
        if !(1..=MAX_SCENE_QUERY_TESTS).contains(&pair_tests)
            || !(1..=MAX_SCENE_QUERY_RESULTS).contains(&results)
        {
            return Err(KinematicError::InvalidConfig);
        }
        Ok(Self {
            test_limit: pair_tests,
            result_limit: results,
            tests: 0,
            results: 0,
        })
    }
    pub fn pair_tests_used(&self) -> u32 {
        self.tests
    }
    pub fn results_used(&self) -> u32 {
        self.results
    }
    pub fn pair_tests_remaining(&self) -> u32 {
        self.test_limit - self.tests
    }
    pub fn results_remaining(&self) -> u32 {
        self.result_limit - self.results
    }
    fn preflight(&self, tests: u32) -> Result<(), KinematicError> {
        if tests > self.pair_tests_remaining() {
            Err(KinematicError::QueryBudgetExceeded)
        } else {
            Ok(())
        }
    }
    fn charge_test(&mut self) {
        self.tests += 1;
    }
    fn charge_result(&mut self) -> Result<(), KinematicError> {
        if self.results == self.result_limit {
            return Err(KinematicError::QueryBudgetExceeded);
        }
        self.results += 1;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneCastHit {
    pub collider: ColliderKey,
    /// Fraction of the supplied displacement, including both endpoints.
    pub toi: f32,
    /// Contact point on the collider in canonical world space.
    pub point: Vec3,
    /// Unit outward normal on the collider in canonical world space.
    pub normal: Vec3,
}

impl CollisionScene {
    /// Sweep a sphere center from `start` to `start + displacement` against this
    /// immutable scene. No movement/controller state or gameplay is advanced.
    ///
    /// Initial overlap or touching returns `toi == 0`, including stationary or
    /// separating motion. Thus a projectile spawned inside a blocker hits it
    /// immediately; the caller must explicitly handle that result. Multiple
    /// contacts choose the smallest TOI, then the smallest stable `ColliderKey`.
    ///
    /// Start, displacement and end coordinates must be finite and at most
    /// 100,000 in absolute value; radius must be positive and at most 10,000.
    /// The complete scan requires an allowance of `3 * len()` pair tests before
    /// it starts (initial contact, sweep and terminal contact), and charges only tests performed.
    /// At most one hit is returned. Insufficient work or invalid Parry output
    /// returns an error, never a nearest hit from a partial scan.
    pub fn sphere_cast(
        &self,
        start: Vec3,
        displacement: Vec3,
        radius: f32,
        budget: &mut SceneQueryBudget,
    ) -> Result<Option<SceneCastHit>, KinematicError> {
        if !bounded(start.to_array(), MAX_COORDINATE)
            || !bounded(displacement.to_array(), MAX_COORDINATE)
            || !bounded((start + displacement).to_array(), MAX_COORDINATE)
            || !radius.is_finite()
            || radius <= 0.0
            || radius > MAX_EXTENT
        {
            return Err(KinematicError::InvalidInput);
        }
        budget.preflight((self.colliders.len() * 3) as u32)?;
        let sphere = Ball::new(radius);
        let pose = Pose::from_translation(Vector::from_array(start.to_array()));
        let options = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        let mut nearest: Option<SceneCastHit> = None;
        for collider in &self.colliders {
            budget.charge_test();
            let contact =
                query::contact(&pose, &sphere, &collider.pose, collider.shape.as_ref(), 0.0)
                    .map_err(|_| KinematicError::UnsupportedQuery)?;
            let (toi, point, normal) = if let Some(contact) = contact {
                if !contact.dist.is_finite() || contact.dist > 0.0 {
                    return Err(KinematicError::InvalidQueryResult);
                }
                // Contact results are already in world space. This also gives a
                // defined normal for completely concentric initial overlaps.
                (
                    0.0,
                    Vec3::from_array(contact.point2.to_array()),
                    Vec3::from_array(contact.normal2.to_array()),
                )
            } else {
                budget.charge_test();
                let hit = query::cast_shapes(
                    &pose,
                    Vector::from_array(displacement.to_array()),
                    &sphere,
                    &collider.pose,
                    Vector::ZERO,
                    collider.shape.as_ref(),
                    options,
                )
                .map_err(|_| KinematicError::UnsupportedQuery)?;
                if let Some(hit) = hit {
                    if matches!(
                        hit.status,
                        ShapeCastStatus::Failed | ShapeCastStatus::OutOfIterations
                    ) {
                        return Err(KinematicError::InvalidQueryResult);
                    }
                    (
                        hit.time_of_impact,
                        Vec3::from_array((collider.pose * hit.witness2).to_array()),
                        Vec3::from_array((collider.pose.rotation * hit.normal2).to_array()),
                    )
                } else {
                    // Some convex casts exclude an exact terminal touch. Query
                    // that endpoint directly instead of extending the segment.
                    budget.charge_test();
                    let end = Pose::from_translation(Vector::from_array(
                        (start + displacement).to_array(),
                    ));
                    let Some(contact) =
                        query::contact(&end, &sphere, &collider.pose, collider.shape.as_ref(), 0.0)
                            .map_err(|_| KinematicError::UnsupportedQuery)?
                    else {
                        continue;
                    };
                    if !contact.dist.is_finite() || contact.dist > 0.0 {
                        return Err(KinematicError::InvalidQueryResult);
                    }
                    (
                        1.0,
                        Vec3::from_array(contact.point2.to_array()),
                        Vec3::from_array(contact.normal2.to_array()),
                    )
                }
            };
            if !toi.is_finite()
                || !(-0.00001..=1.00001).contains(&toi)
                || !point.is_finite()
                || !normal.is_finite()
                || !(0.5..=1.5).contains(&normal.length_squared())
            {
                return Err(KinematicError::InvalidQueryResult);
            }
            let hit = SceneCastHit {
                collider: collider.descriptor.key,
                toi: toi.clamp(0.0, 1.0),
                point,
                normal: normal.normalize(),
            };
            if nearest.is_none_or(|old| {
                hit.toi
                    .total_cmp(&old.toi)
                    .then(hit.collider.cmp(&old.collider))
                    .is_lt()
            }) {
                nearest = Some(hit);
            }
        }
        if nearest.is_some() {
            budget.charge_result()?;
        }
        Ok(nearest)
    }
}

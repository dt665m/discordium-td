use super::*;
use parry3d::{
    math::{Pose, Rotation, Vector},
    query::{self, Ray},
    shape::SharedShape,
};

#[derive(Clone, Copy, Debug)]
pub enum HitQuery {
    Ray {
        ray: Ray3d,
        distance: f32,
    },
    Sphere {
        center: Vec3,
        radius: f32,
    },
    /// Finite cone with apex at origin, extending along the unit direction.
    Cone {
        origin: Vec3,
        direction: Dir3,
        length: f32,
        end_radius: f32,
    },
    /// A circular volume in the plane normal to `normal`, with explicit thickness.
    /// The game chooses thickness for its collision rig; this is not an XZ flip.
    Circle {
        center: Vec3,
        normal: Dir3,
        radius: f32,
        half_thickness: f32,
    },
}
#[derive(Debug)]
pub struct QueryBudget {
    remaining_tests: usize,
    remaining_results: usize,
    used_tests: usize,
}
impl QueryBudget {
    pub fn new(maximum_tests: usize, maximum_results: usize) -> Result<Self, HistoryError> {
        if maximum_tests == 0
            || maximum_tests > 1_048_576
            || maximum_results == 0
            || maximum_results > 4096
        {
            return Err(HistoryError::InvalidLimits);
        }
        Ok(Self {
            remaining_tests: maximum_tests,
            remaining_results: maximum_results,
            used_tests: 0,
        })
    }
    pub fn used_tests(&self) -> usize {
        self.used_tests
    }
    pub(super) fn charge(&mut self) -> Result<(), HistoryError> {
        if self.remaining_tests == 0 {
            return Err(HistoryError::QueryBudget);
        }
        self.remaining_tests -= 1;
        self.used_tests += 1;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit<'a> {
    pub pose: &'a CombatPose,
    /// Ray distance, or center distance for overlap queries. Stable entity breaks ties.
    pub distance: f32,
}
enum PreparedQuery {
    Ray(Ray, f32),
    Volume(Pose, SharedShape, Vec3),
}
impl HitQuery {
    fn prepare(self) -> Result<PreparedQuery, HistoryError> {
        let point = |v: Vec3| v.is_finite() && v.abs().max_element() <= 100_000.0;
        let extent = |v: f32| v.is_finite() && v > 0.0 && v <= 10_000.0;
        let unit = |v: Vec3| v.is_finite() && (v.length_squared() - 1.0).abs() <= 0.001;
        let at = |center: Vec3, rotation: Quat| {
            Pose::from_parts(
                Vector::from_array(center.to_array()),
                Rotation::from_array(rotation.to_array()),
            )
        };
        match self {
            Self::Ray { ray, distance }
                if point(ray.origin) && unit(*ray.direction) && extent(distance) =>
            {
                Ok(PreparedQuery::Ray(
                    Ray::new(
                        Vector::from_array(ray.origin.to_array()),
                        Vector::from_array(ray.direction.to_array()),
                    ),
                    distance,
                ))
            }
            Self::Sphere { center, radius } if point(center) && extent(radius) => {
                Ok(PreparedQuery::Volume(
                    at(center, Quat::IDENTITY),
                    SharedShape::ball(radius),
                    center,
                ))
            }
            Self::Cone {
                origin,
                direction,
                length,
                end_radius,
            } if point(origin) && unit(*direction) && extent(length) && extent(end_radius) => {
                // Parry's cone apex is +Y and its base is -Y.
                let center = origin + *direction * (length * 0.5);
                let rotation = Quat::from_rotation_arc(Vec3::NEG_Y, *direction);
                Ok(PreparedQuery::Volume(
                    at(center, rotation),
                    SharedShape::cone(length * 0.5, end_radius),
                    origin,
                ))
            }
            Self::Circle {
                center,
                normal,
                radius,
                half_thickness,
            } if point(center) && unit(*normal) && extent(radius) && extent(half_thickness) => {
                Ok(PreparedQuery::Volume(
                    at(center, Quat::from_rotation_arc(Vec3::Y, *normal)),
                    SharedShape::cylinder(half_thickness, radius),
                    center,
                ))
            }
            _ => Err(HistoryError::InvalidQuery),
        }
    }
}
impl HitFrame {
    /// Tests a bounded frame in stable order. Filtering is game policy, not an
    /// authorization grant. On budget failure no partial hit list is returned.
    /// Share one budget across the complete action's rays/volumes/samples.
    pub fn query(
        &self,
        query: HitQuery,
        budget: &mut QueryBudget,
        mut include: impl FnMut(&CombatPose) -> bool,
    ) -> Result<Vec<Hit<'_>>, HistoryError> {
        let prepared = query.prepare()?;
        let mut hits = Vec::with_capacity(self.poses.len().min(budget.remaining_results));
        for pose in &self.poses {
            budget.charge()?;
            if !include(pose) {
                continue;
            }
            let (transform, shape) =
                prepare_collider(pose.collider()).map_err(|_| HistoryError::InvalidPose)?;
            let distance = match &prepared {
                PreparedQuery::Ray(ray, max) => shape.cast_ray(&transform, ray, *max, true),
                PreparedQuery::Volume(query_pose, query_shape, origin) => {
                    if query::intersection_test(
                        query_pose,
                        query_shape.as_ref(),
                        &transform,
                        shape.as_ref(),
                    )
                    .map_err(|_| HistoryError::UnsupportedQuery)?
                    {
                        Some(origin.distance(pose.position))
                    } else {
                        None
                    }
                }
            };
            if let Some(distance) = distance {
                if !distance.is_finite() || distance < 0.0 {
                    return Err(HistoryError::InvalidQuery);
                }
                if budget.remaining_results == 0 {
                    return Err(HistoryError::ResultBudget);
                }
                budget.remaining_results -= 1;
                hits.push(Hit { pose, distance });
            }
        }
        hits.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.pose.entity.cmp(&b.pose.entity))
        });
        Ok(hits)
    }
}

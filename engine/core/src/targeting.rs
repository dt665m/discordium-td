//! Stable, renderer-independent spatial target selection.
use crate::spatial::{distance, normalize, sub};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A collision snapshot refreshed from authoritative actors before targeting.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollisionTarget {
    pub id: u64,
    pub faction: u8,
    pub position: [f32; 2],
    pub radius: f32,
    pub active: bool,
}

/// Derived spatial index; rebuild from authoritative actors before each phase.
#[derive(Resource, Default)]
pub struct TargetIndex(pub Vec<CollisionTarget>);

/// Strict circle containment; points on the edge are outside.
pub fn contains_circle(origin: [f32; 2], radius: f32, point: [f32; 2]) -> bool {
    distance(origin, point) < radius
}

/// Strict cone containment. Supply a unit facing direction and the cosine of
/// the half angle. Both the radial edge and the angular edge are outside.
pub fn contains_cone(
    origin: [f32; 2],
    direction: [f32; 2],
    radius: f32,
    minimum_dot: f32,
    point: [f32; 2],
) -> bool {
    let toward = normalize(sub(point, origin));
    contains_circle(origin, radius, point)
        && toward[0] * direction[0] + toward[1] * direction[1] > minimum_dot
}

pub fn segment_distance(point: [f32; 2], from: [f32; 2], to: [f32; 2]) -> f32 {
    let from = Vec2::from_array(from);
    let to = Vec2::from_array(to);
    // Keep the simulation's tiny-sweep policy, delegating geometry to Bevy.
    let closest = if from.distance_squared(to) < 0.00001 {
        from
    } else {
        Segment2d::new(from, to).closest_point(Vec2::from_array(point))
    };
    distance(point, closest.to_array())
}

/// Results use distance from the origin, then stable identity for ties.
pub fn radial_targets(
    origin: [f32; 2],
    range: f32,
    faction: u8,
    targets: impl IntoIterator<Item = CollisionTarget>,
) -> Vec<CollisionTarget> {
    let mut result: Vec<_> = targets
        .into_iter()
        .filter(|target| {
            target.active
                && target.faction != faction
                && contains_circle(origin, range, target.position)
        })
        .collect();
    result.sort_by(|a, b| {
        distance(origin, a.position)
            .total_cmp(&distance(origin, b.position))
            .then(a.id.cmp(&b.id))
    });
    result
}

pub fn nearest_target(
    origin: [f32; 2],
    range: f32,
    faction: u8,
    targets: impl IntoIterator<Item = CollisionTarget>,
) -> Option<CollisionTarget> {
    targets
        .into_iter()
        .filter(|target| {
            target.active
                && target.faction != faction
                && contains_circle(origin, range, target.position)
        })
        .min_by(|a, b| {
            distance(origin, a.position)
                .total_cmp(&distance(origin, b.position))
                .then(a.id.cmp(&b.id))
        })
}

/// `minimum_dot` is the cosine of the cone's half angle.
pub fn cone_targets(
    origin: [f32; 2],
    direction: [f32; 2],
    range: f32,
    minimum_dot: f32,
    faction: u8,
    targets: impl IntoIterator<Item = CollisionTarget>,
) -> Vec<CollisionTarget> {
    let direction = normalize(direction);
    radial_targets(origin, range, faction, targets)
        .into_iter()
        .filter(|target| {
            let offset = normalize(sub(target.position, origin));
            offset[0] * direction[0] + offset[1] * direction[1] >= minimum_dot
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bevy_segment_projection_retains_tiny_sweep_and_endpoint_policy() {
        assert_eq!(segment_distance([2.0, 3.0], [2.0, 1.0], [2.0, 1.0]), 2.0);
        assert_eq!(segment_distance([2.0, 3.0], [2.0, 1.0], [2.0, 1.001]), 2.0);
        assert_eq!(segment_distance([1.0, 1.0], [2.0, 1.0], [5.0, 1.0]), 1.0);
        assert_eq!(segment_distance([6.0, 1.0], [2.0, 1.0], [5.0, 1.0]), 1.0);
        assert_eq!(segment_distance([3.0, 4.0], [2.0, 1.0], [5.0, 1.0]), 3.0);
    }
    #[test]
    fn stable_selection_and_swept_distance() {
        let target = |id, position| CollisionTarget {
            id,
            position,
            faction: 2,
            radius: 0.5,
            active: true,
        };
        let targets = [target(3, [1.0, 0.0]), target(2, [-1.0, 0.0])];
        assert_eq!(nearest_target([0.0; 2], 2.0, 1, targets).unwrap().id, 2);
        assert_eq!(
            cone_targets([0.0; 2], [1.0, 0.0], 2.0, 0.5, 1, targets)[0].id,
            3
        );
        assert_eq!(segment_distance([5.0, 0.2], [0.0; 2], [10.0, 0.0]), 0.2);
    }

    #[test]
    fn strict_shapes_exclude_radial_and_angular_boundaries() {
        assert!(contains_circle([0.0; 2], 2.0, [1.999, 0.0]));
        assert!(!contains_circle([0.0; 2], 2.0, [2.0, 0.0]));
        assert!(contains_cone([0.0; 2], [1.0, 0.0], 2.0, 0.0, [1.0, 0.0]));
        assert!(!contains_cone([0.0; 2], [1.0, 0.0], 2.0, 0.0, [0.0, 1.0]));
        assert!(!contains_cone([0.0; 2], [1.0, 0.0], 2.0, 0.0, [2.0, 0.0]));
        let targets = [CollisionTarget {
            id: 1,
            faction: 2,
            position: [2.0, 0.0],
            radius: 0.0,
            active: true,
        }];
        assert!(nearest_target([0.0; 2], 2.0, 1, targets).is_none());
    }
}

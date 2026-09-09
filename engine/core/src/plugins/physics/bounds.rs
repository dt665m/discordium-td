use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CircleBounds {
    pub center: [f32; 2],
    pub radius: f32,
}
impl CircleBounds {
    pub fn clamp(self, position: [f32; 2]) -> [f32; 2] {
        let center = Vec2::from_array(self.center);
        (center
            + Circle::new(self.radius.max(0.0)).closest_point(Vec2::from_array(position) - center))
        .to_array()
    }
}

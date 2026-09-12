//! Array/Bevy math adapters for canonical X/Z snapshot coordinates.
use bevy::prelude::Vec2;

pub fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    (Vec2::from_array(a) + Vec2::from_array(b)).to_array()
}
pub fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    (Vec2::from_array(a) - Vec2::from_array(b)).to_array()
}
pub fn scale(a: [f32; 2], s: f32) -> [f32; 2] {
    (Vec2::from_array(a) * s).to_array()
}
pub fn length(a: [f32; 2]) -> f32 {
    // Use Bevy's fixed multiply/add/sqrt path. Platform hypot implementations
    // differ by an ULP between native and WASM, changing replay state.
    Vec2::from_array(a).length()
}
pub fn normalize(a: [f32; 2]) -> [f32; 2] {
    let len = length(a);
    // This dead zone is the simulation input policy, not a replacement Vec2.
    if len > 0.0001 && len.is_finite() {
        scale(a, 1.0 / len)
    } else {
        [0.0; 2]
    }
}
pub fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    length(sub(a, b))
}
pub fn integrate(position: [f32; 2], velocity: [f32; 2], dt: f32) -> [f32; 2] {
    add(position, scale(velocity, dt))
}

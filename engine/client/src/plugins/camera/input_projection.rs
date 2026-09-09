use bevy::prelude::*;

/// Map screen axes (right, down) once to canonical XZ movement, preserving magnitude.
pub fn screen_axes_to_world(transform: &GlobalTransform, axes: Vec2) -> Vec2 {
    let right = transform.right();
    let forward = transform.forward();
    let right = Vec2::new(right.x, right.z).normalize_or_zero();
    let forward = Vec2::new(forward.x, forward.z).normalize_or_zero();
    (right * axes.x - forward * axes.y).clamp_length_max(axes.length())
}

/// Project cursor input once into canonical XZ world space (+Y up).
pub fn cursor_on_ground(
    camera: &Camera,
    transform: &GlobalTransform,
    cursor: Vec2,
) -> Option<Vec2> {
    let ray = camera.viewport_to_world(transform, cursor).ok()?;
    let distance = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))?;
    let point = ray.get_point(distance);
    Some(Vec2::new(point.x, point.z))
}

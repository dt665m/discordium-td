use bevy::prelude::*;
use engine_core::GraphicsId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VisualId {
    Entity { namespace: u32, id: u64 },
    Effect(GraphicsId),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    Sphere,
    Box,
    Ring,
    Arrow { direction: Vec3 },
}
#[derive(Clone, Debug)]
pub struct Visual {
    pub id: VisualId,
    pub primitive: Primitive,
    pub position: Vec3,
    /// Box dimensions; X is radius for spheres/rings and length for arrows.
    pub scale: Vec3,
    pub color: Color,
}
#[derive(Resource, Default)]
pub struct GraphicsFrame {
    pub visuals: Vec<Visual>,
}

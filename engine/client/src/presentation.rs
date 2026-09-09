//! Renderer-neutral frame contract. Games map authoritative/predicted state to primitives.
use bevy::prelude::*;
use engine_core::PresentationId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VisualId {
    Entity { namespace: u32, id: u64 },
    Effect(PresentationId),
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
pub struct PresentationFrame {
    pub visuals: Vec<Visual>,
}
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresentationSet {
    Adapt,
    Render,
}
pub struct PresentationPlugin;
impl Plugin for PresentationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PresentationFrame>().configure_sets(
            Update,
            (PresentationSet::Adapt, PresentationSet::Render).chain(),
        );
    }
}
/// Simple asset-free renderer suitable for prototypes. Can be replaced without changing input or simulation.
pub struct PrototypeRendererPlugin;
#[derive(Resource)]
pub struct PrototypeSettings {
    pub grid_cells: Option<UVec2>,
    pub grid_spacing: Vec2,
    pub grid_color: Color,
}
impl Default for PrototypeSettings {
    fn default() -> Self {
        Self {
            grid_cells: Some(UVec2::splat(36)),
            grid_spacing: Vec2::ONE,
            grid_color: Color::srgb(0.18, 0.2, 0.23),
        }
    }
}
impl Plugin for PrototypeRendererPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.035, 0.045, 0.055)));
        app.init_resource::<PrototypeSettings>()
            .add_systems(Update, draw.in_set(PresentationSet::Render));
    }
}
fn draw(frame: Res<PresentationFrame>, settings: Res<PrototypeSettings>, mut gizmos: Gizmos) {
    if let Some(cells) = settings.grid_cells {
        gizmos.grid(
            Isometry3d::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            cells,
            settings.grid_spacing,
            settings.grid_color,
        );
    }
    for v in &frame.visuals {
        match v.primitive {
            Primitive::Arrow { direction } => {
                gizmos.arrow(
                    v.position,
                    v.position + direction.normalize_or_zero() * v.scale.x,
                    v.color,
                );
            }
            Primitive::Sphere => {
                gizmos.sphere(Isometry3d::from_translation(v.position), v.scale.x, v.color);
            }
            Primitive::Box => {
                gizmos.cube(
                    Transform::from_translation(v.position).with_scale(v.scale),
                    v.color,
                );
            }
            Primitive::Ring => {
                gizmos.circle(
                    Isometry3d::new(
                        v.position,
                        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                    ),
                    v.scale.x,
                    v.color,
                );
            }
        }
    }
}

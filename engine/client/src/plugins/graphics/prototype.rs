use super::{GraphicsFrame, GraphicsSet, Primitive};
use bevy::prelude::*;

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
            .add_systems(Update, draw_gizmos.in_set(GraphicsSet::Render));
    }
}
/// Draw presentation primitives as gizmos; callers may gate this system at runtime.
pub fn draw_gizmos(
    frame: Res<GraphicsFrame>,
    settings: Res<PrototypeSettings>,
    mut gizmos: Gizmos,
) {
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

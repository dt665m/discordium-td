//! Camera ownership is independent of the renderer; input projects through this rig.
use bevy::{camera::ScalingMode, prelude::*};

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

#[derive(Resource)]
pub struct CameraSettings {
    pub zoom: f32,
    pub target: Vec3,
    pub offset: Vec3,
    pub up: Vec3,
    pub viewport_height: f32,
    /// Exponential response rate in inverse seconds; zero follows immediately.
    pub smoothing: f32,
    pub zoom_bounds: [f32; 2],
}
impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            target: Vec3::ZERO,
            offset: Vec3::new(0.0, 30.0, 23.0),
            up: Vec3::Y,
            viewport_height: 30.0,
            smoothing: 4.5,
            zoom_bounds: [0.75, 1.4],
        }
    }
}
#[derive(Component, Default)]
pub struct CameraRig {
    focus: Vec3,
}
pub struct CameraPlugin;
impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraSettings>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                follow.after(crate::presentation::PresentationSet::Adapt),
            );
    }
}
fn setup(mut commands: Commands, settings: Res<CameraSettings>) {
    commands.spawn((
        Camera3d::default(),
        CameraRig {
            focus: settings.target,
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: settings.viewport_height,
            },
            far: 220.0,
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_translation(settings.target + settings.offset)
            .looking_at(settings.target, settings.up),
    ));
}
fn follow(
    time: Res<Time>,
    settings: Res<CameraSettings>,
    mut cameras: Query<(&mut CameraRig, &mut Transform, &mut Projection)>,
) {
    for (mut rig, mut transform, mut projection) in &mut cameras {
        let blend = if settings.smoothing <= 0.0 {
            1.0
        } else {
            1.0 - (-time.delta_secs() * settings.smoothing).exp()
        };
        rig.focus = rig.focus.lerp(settings.target, blend);
        *transform = Transform::from_translation(rig.focus + settings.offset)
            .looking_at(rig.focus, settings.up);
        if let Projection::Orthographic(p) = &mut *projection {
            p.scale = settings
                .zoom
                .clamp(settings.zoom_bounds[0], settings.zoom_bounds[1]);
            p.scaling_mode = ScalingMode::FixedVertical {
                viewport_height: settings.viewport_height,
            };
        }
    }
}

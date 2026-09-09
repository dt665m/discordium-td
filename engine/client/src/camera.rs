//! Camera ownership is independent of the renderer; input projects through this rig.
use bevy::{camera::ScalingMode, math::StableInterpolate, prelude::*};

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

#[derive(Resource)]
pub struct CameraSettings {
    pub zoom: f32,
    pub target: Vec3,
    pub offset: Vec3,
    pub up: Vec3,
    pub viewport_height: f32,
    /// Exponential response rate in inverse seconds; zero follows immediately.
    pub smoothing: f32,
    /// Exponential zoom response in inverse seconds; zero changes immediately.
    pub zoom_smoothing: f32,
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
            zoom_smoothing: 12.0,
            zoom_bounds: [0.75, 1.4],
        }
    }
}
#[derive(Component, Default)]
pub struct CameraRig {
    focus: Vec3,
}
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraSystems {
    Follow,
}
pub struct CameraPlugin;
impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraSettings>()
            .configure_sets(
                Update,
                CameraSystems::Follow
                    .after(crate::presentation::PresentationSet::Adapt)
                    .before(crate::presentation::PresentationSet::Render),
            )
            .add_systems(Startup, setup)
            .add_systems(Update, follow.in_set(CameraSystems::Follow));
    }
}
fn setup(mut commands: Commands, settings: Res<CameraSettings>) {
    commands.spawn((
        Camera3d::default(),
        CameraRig {
            focus: settings.target,
        },
        Projection::Orthographic(OrthographicProjection {
            scale: settings
                .zoom
                .clamp(settings.zoom_bounds[0], settings.zoom_bounds[1]),
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
        let mut focus = rig.focus;
        if settings.smoothing <= 0.0 {
            focus = settings.target;
        } else {
            focus.smooth_nudge(&settings.target, settings.smoothing, time.delta_secs());
        }
        if rig.focus != focus {
            rig.focus = focus;
        }
        transform.set_if_neq(
            Transform::from_translation(focus + settings.offset).looking_at(focus, settings.up),
        );
        if let Projection::Orthographic(p) = &*projection {
            let target = settings
                .zoom
                .clamp(settings.zoom_bounds[0], settings.zoom_bounds[1]);
            let mut scale = p.scale;
            if settings.zoom_smoothing <= 0.0 {
                scale = target;
            } else {
                scale.smooth_nudge(&target, settings.zoom_smoothing, time.delta_secs());
            }
            let scaling_mode = ScalingMode::FixedVertical {
                viewport_height: settings.viewport_height,
            };
            let same_height = matches!(p.scaling_mode,
                ScalingMode::FixedVertical { viewport_height }
                    if viewport_height == settings.viewport_height);
            if p.scale != scale || !same_height {
                if let Projection::Orthographic(p) = &mut *projection {
                    p.scale = scale;
                    p.scaling_mode = scaling_mode;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn screen_movement_tracks_camera_yaw_without_diagonal_speed_boost() {
        for offset in [Vec3::new(0.0, 30.0, 23.0), Vec3::new(18.0, 32.0, 26.0)] {
            let camera = GlobalTransform::from(
                Transform::from_translation(offset).looking_at(Vec3::ZERO, Vec3::Y),
            );
            let down = Vec2::new(offset.x, offset.z).normalize();
            let right = Vec2::new(down.y, -down.x);
            assert!(screen_axes_to_world(&camera, Vec2::X).abs_diff_eq(right, 1e-6));
            assert!(screen_axes_to_world(&camera, -Vec2::Y).abs_diff_eq(-down, 1e-6));
            let diagonal = screen_axes_to_world(&camera, Vec2::ONE.normalize());
            assert!((diagonal.length() - 1.0).abs() < 1e-6);
            assert!(diagonal.abs_diff_eq((right + down).normalize(), 1e-6));
        }
    }

    fn camera_app() -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<CameraSettings>()
            .add_systems(Update, follow);
        let entity = app
            .world_mut()
            .spawn((
                CameraRig::default(),
                Transform::default(),
                Projection::Orthographic(OrthographicProjection::default_3d()),
            ))
            .id();
        (app, entity)
    }

    fn step(app: &mut App, hz: u32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f64(1.0 / hz as f64));
        app.update();
    }

    #[test]
    fn focus_and_zoom_are_frame_rate_independent_and_zoom_is_bounded() {
        let mut results = Vec::new();
        for hz in [30, 120] {
            let (mut app, entity) = camera_app();
            {
                let mut settings = app.world_mut().resource_mut::<CameraSettings>();
                settings.target = Vec3::new(4.0, 0.0, -3.0);
                settings.zoom = 5.0;
            }
            step(&mut app, hz);
            let Projection::Orthographic(projection) =
                app.world().get::<Projection>(entity).unwrap()
            else {
                panic!()
            };
            assert!(projection.scale > 1.0 && projection.scale < 1.4);
            for _ in 1..hz {
                step(&mut app, hz);
            }
            let focus = app.world().get::<CameraRig>(entity).unwrap().focus;
            let Projection::Orthographic(projection) =
                app.world().get::<Projection>(entity).unwrap()
            else {
                panic!()
            };
            assert!((projection.scale - 1.4).abs() < 1e-5);
            results.push((focus, projection.scale));
        }
        assert!(results[0].0.abs_diff_eq(results[1].0, 1e-5));
        assert!((results[0].1 - results[1].1).abs() < 1e-5);
    }

    #[test]
    fn disabled_smoothing_follows_immediately() {
        let (mut app, entity) = camera_app();
        {
            let mut settings = app.world_mut().resource_mut::<CameraSettings>();
            settings.target = Vec3::X;
            settings.smoothing = 0.0;
            settings.zoom_smoothing = 0.0;
            settings.zoom = 0.1;
        }
        step(&mut app, 60);
        assert_eq!(app.world().get::<CameraRig>(entity).unwrap().focus, Vec3::X);
        let Projection::Orthographic(projection) = app.world().get::<Projection>(entity).unwrap()
        else {
            panic!()
        };
        assert_eq!(projection.scale, 0.75);
        app.world_mut().clear_trackers();
        step(&mut app, 60);
        let mut changed = app
            .world_mut()
            .query_filtered::<Entity, Or<(Changed<Transform>, Changed<Projection>)>>();
        assert_eq!(changed.iter(app.world()).count(), 0);
    }
}

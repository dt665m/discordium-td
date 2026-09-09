use super::super::{CameraRig, CameraSettings};
use bevy::{camera::ScalingMode, prelude::*};

pub(crate) fn setup(mut commands: Commands, settings: Res<CameraSettings>) {
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

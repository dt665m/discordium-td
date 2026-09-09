use super::super::{CameraRig, CameraSettings};
use bevy::{camera::ScalingMode, math::StableInterpolate, prelude::*};

pub(crate) fn zoom(
    time: Res<Time>,
    settings: Res<CameraSettings>,
    mut cameras: Query<&mut Projection, (With<CameraRig>, With<Transform>)>,
) {
    for mut projection in &mut cameras {
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

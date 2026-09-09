use super::super::{CameraRig, CameraSettings};
use bevy::{math::StableInterpolate, prelude::*};

pub(crate) fn follow(
    time: Res<Time>,
    settings: Res<CameraSettings>,
    mut cameras: Query<(&mut CameraRig, &mut Transform), With<Projection>>,
) {
    for (mut rig, mut transform) in &mut cameras {
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
    }
}

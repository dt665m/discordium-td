//! Keep the two opt-in native spatial fixtures visible on one monitor. Ordinary
//! player windows retain their existing size, position, and resize constraints.
use super::{Playtest, SpatialPlaytest};
use bevy::{
    prelude::*,
    window::{Monitor, PrimaryMonitor, PrimaryWindow, WindowOccluded, WindowPosition},
};

/// OS window state accompanies opt-in traces so a hidden or occluded native
/// surface can be distinguished from a running, visible rendered client.
#[derive(Resource, Default, PartialEq, serde::Serialize)]
pub(super) struct NativeWindowMetrics {
    focused: bool,
    visible: bool,
    occluded: Option<bool>,
    physical_size: [u32; 2],
}

pub(super) fn sample_window(
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut occlusion: MessageReader<WindowOccluded>,
    mut metrics: ResMut<NativeWindowMetrics>,
) {
    let Ok((entity, window)) = windows.single() else {
        occlusion.clear();
        return;
    };
    let mut occluded = metrics.occluded;
    for event in occlusion.read() {
        if event.window == entity {
            occluded = Some(event.occluded);
        }
    }
    metrics.set_if_neq(NativeWindowMetrics {
        focused: window.focused,
        visible: window.visible,
        occluded,
        physical_size: [
            window.resolution.physical_width(),
            window.resolution.physical_height(),
        ],
    });
}

// The existing game UI scales its 1160×820 reference layout down to 0.5.
const MIN_LOGICAL: Vec2 = Vec2::new(580.0, 410.0);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    position: IVec2,
    physical_size: UVec2,
}

fn placement(monitor: &Monitor, side: SpatialPlaytest) -> Option<Placement> {
    let right = match side {
        SpatialPlaytest::Left => false,
        SpatialPlaytest::Right => true,
        SpatialPlaytest::Off => return None,
    };
    let scale = monitor.scale_factor;
    if !scale.is_finite() || !(0.25..=8.0).contains(&scale) {
        return None;
    }
    let physical = |logical: f64| (logical * scale).ceil() as u32;
    let margin = physical(16.0);
    let gap = physical(16.0);
    // Monitor exposes its complete bounds, not OS work-area reservations. Leave
    // logical insets for the menu, title bars, borders, and a typical dock/taskbar.
    let top = physical(48.0);
    let bottom = physical(80.0);
    let width = monitor.physical_width.checked_sub(margin * 2 + gap)? / 2;
    let height = monitor.physical_height.checked_sub(top + bottom)?;
    if width < physical(MIN_LOGICAL.x as f64) || height < physical(MIN_LOGICAL.y as f64) {
        return None;
    }
    let x = margin + if right { width + gap } else { 0 };
    let position = IVec2::new(
        monitor
            .physical_position
            .x
            .checked_add(i32::try_from(x).ok()?)?,
        monitor
            .physical_position
            .y
            .checked_add(i32::try_from(top).ok()?)?,
    );
    Some(Placement {
        position,
        physical_size: UVec2::new(width, height),
    })
}

#[derive(Default)]
pub(super) struct PlacementState {
    applied: Option<SpatialPlaytest>,
    waiting_since: Option<f64>,
}

pub(super) fn place_window(
    test: Res<Playtest>,
    time: Res<Time<Real>>,
    monitors: Query<&Monitor, With<PrimaryMonitor>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut state: Local<PlacementState>,
) {
    if state.applied == Some(test.spatial) {
        return;
    }
    if test.spatial == SpatialPlaytest::Off {
        state.applied = Some(test.spatial);
        state.waiting_since = None;
        return;
    }
    let Ok(monitor) = monitors.single() else {
        let now = time.elapsed_secs_f64();
        let since = *state.waiting_since.get_or_insert(now);
        if now - since >= 10.0 {
            warn!(
                "Spatial playtest window placement unavailable: no unique primary monitor after 10 seconds"
            );
            state.applied = Some(test.spatial);
        }
        return;
    };
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    state.applied = Some(test.spatial);
    state.waiting_since = None;
    let Some(placement) = placement(monitor, test.spatial) else {
        warn!(
            "Spatial playtest windows cannot fit the UI minimum on primary monitor {:?} at scale {}",
            monitor.physical_size(),
            monitor.scale_factor
        );
        return;
    };
    window.position = WindowPosition::At(placement.position);
    window
        .resolution
        .set_physical_resolution(placement.physical_size.x, placement.physical_size.y);
    window.resize_constraints.min_width = MIN_LOGICAL.x;
    window.resize_constraints.min_height = MIN_LOGICAL.y;
    window.title = format!("DREAMWAKE — {}", test.spatial.label());
    info!(
        "Spatial playtest window: side={} position={:?} physical_size={:?} monitor={:?} scale={}",
        test.spatial.label(),
        placement.position,
        placement.physical_size,
        monitor.physical_size(),
        monitor.scale_factor
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(width: u32, height: u32, scale: f64, origin: IVec2) -> Monitor {
        Monitor {
            name: None,
            physical_width: width,
            physical_height: height,
            physical_position: origin,
            scale_factor: scale,
            refresh_rate_millihertz: None,
            video_modes: vec![],
        }
    }

    #[test]
    fn physical_placement_respects_dpi_monitor_origin_and_nonoverlap() {
        for (width, height, scale, origin) in [
            (1920, 1080, 1.0, IVec2::ZERO),
            (3456, 2234, 2.0, IVec2::new(-3456, 120)),
            (3840, 2160, 1.5, IVec2::new(1920, -300)),
        ] {
            let monitor = monitor(width, height, scale, origin);
            let left = placement(&monitor, SpatialPlaytest::Left).unwrap();
            let right = placement(&monitor, SpatialPlaytest::Right).unwrap();
            assert_eq!(left.physical_size, right.physical_size);
            assert!(left.position.x >= origin.x && left.position.y >= origin.y);
            assert!(left.position.x + (left.physical_size.x as i32) < right.position.x);
            assert!(right.position.x + (right.physical_size.x as i32) <= origin.x + width as i32);
            assert!(right.position.y + (right.physical_size.y as i32) <= origin.y + height as i32);
            assert!(left.physical_size.x as f64 / scale >= MIN_LOGICAL.x as f64);
            assert!(left.physical_size.y as f64 / scale >= MIN_LOGICAL.y as f64);
        }
    }

    #[test]
    fn ordinary_windows_and_insufficient_monitors_are_not_resized() {
        assert!(placement(&monitor(1920, 1080, 1.0, IVec2::ZERO), SpatialPlaytest::Off).is_none());
        for (width, height, scale) in [(1024, 768, 1.0), (1920, 400, 1.0), (1920, 1080, f64::NAN)] {
            assert!(
                placement(
                    &monitor(width, height, scale, IVec2::ZERO),
                    SpatialPlaytest::Left
                )
                .is_none()
            );
        }
    }
}

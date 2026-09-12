use bevy::{prelude::*, ui::FocusPolicy};

/// Normalized meter value on the fill node. Non-finite values render empty.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterFraction(pub f32);

#[derive(Component, Clone, Copy, Default)]
/// Marker used by `meter`; `meter_with_marker` uses the caller's marker instead.
pub struct MeterFill;

/// Unlit, passive text; games supply typography sizes and palette colors.
pub fn label(value: impl Into<String>, size: f32, color: Color) -> impl Scene {
    let value = value.into();
    bsn! {
        Text(value)
        TextFont { font_size: px(size) }
        TextColor(color)
        template_value(FocusPolicy::Pass)
        template_value(Pickable::IGNORE)
    }
}

pub fn meter(width: Val, height: f32, fraction: f32, fill: Color, background: Color) -> impl Scene {
    meter_with_marker(width, height, fraction, fill, background, MeterFill)
}

/// A clipped track and left-aligned fill. The supplied marker and
/// `MeterFraction` live on the child so game adapters can select each meter.
pub fn meter_with_marker<M: Component + Clone + Default + Unpin>(
    width: Val,
    height: f32,
    fraction: f32,
    fill: Color,
    background: Color,
    marker: M,
) -> impl Scene {
    bsn! {
        Node { width, height: px(height), overflow: Overflow::clip() }
        BackgroundColor(background)
        template_value(FocusPolicy::Pass)
        template_value(Pickable::IGNORE)
        Children [(
            template_value(marker)
            MeterFraction(fraction)
            Node { width: percent(normalized(fraction) * 100.), height: percent(100), flex_shrink: 0. }
            BackgroundColor(fill)
            template_value(FocusPolicy::Pass)
            template_value(Pickable::IGNORE)
        )]
    }
}

fn normalized(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0., 1.)
    } else {
        0.
    }
}

pub(super) fn sync_meters(mut meters: Query<(&MeterFraction, &mut Node), Changed<MeterFraction>>) {
    for (fraction, mut node) in &mut meters {
        let width = percent(normalized(fraction.0) * 100.);
        if node.width != width {
            node.width = width;
        }
    }
}

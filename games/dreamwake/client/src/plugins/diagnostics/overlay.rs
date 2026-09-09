//! Passive F3 diagnostics. Bevy owns frame sampling; this panel only presents it.
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    input::InputSystems,
    picking::Pickable,
    prelude::*,
    ui::FocusPolicy,
};
use dreamwake_sim::TOTAL_ROOMS;

use crate::DreamView;

pub struct DreamDiagnosticsPlugin;

#[derive(Resource, Default)]
pub(super) struct DiagnosticsOverlay {
    pub visible: bool,
}

/// Rows owned by the network adapter, separate from the performance/game query.
#[derive(Component, Clone, Copy, Default)]
pub(super) enum OverlayMetric {
    #[default]
    Network,
    Prediction,
}

#[derive(Component, Clone, Copy, Default)]
enum PerformanceMetric {
    #[default]
    Headline,
    Window,
    Game,
}

#[derive(Component, Clone, Default)]
struct OverlayPanel;

struct RefreshClock(Timer);
impl Default for RefreshClock {
    fn default() -> Self {
        Self(Timer::from_seconds(0.25, TimerMode::Repeating))
    }
}

impl Plugin for DreamDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(FrameTimeDiagnosticsPlugin::new(120));
        }
        app.init_resource::<DiagnosticsOverlay>()
            .add_systems(Startup, spawn_overlay)
            .add_systems(PreUpdate, toggle_overlay.after(InputSystems))
            .add_systems(Update, update_metrics);
    }
}

fn metric_text(value: &str, size: f32, color: Color) -> impl Scene {
    let value = value.to_owned();
    bsn! {
        Text(value)
        TextFont { font_size: px(size) }
        TextColor(color)
        template_value(FocusPolicy::Pass)
        template_value(Pickable::IGNORE)
    }
}

fn spawn_overlay(mut commands: Commands) {
    let white = Color::srgb(0.91, 0.95, 0.95);
    let muted = Color::srgb(0.64, 0.74, 0.79);
    commands.spawn_scene(bsn! {
        OverlayPanel
        Node {
            display: Display::None,
            position_type: PositionType::Absolute,
            left: px(12), top: px(165), width: px(350), max_width: percent(95),
            padding: px(12), row_gap: px(7),
            flex_direction: FlexDirection::Column,
            border: px(1), border_radius: BorderRadius::all(px(6)),
        }
        BackgroundColor(Color::srgba(0.025, 0.041, 0.071, 0.94))
        BorderColor::all(Color::srgba(0.39, 0.64, 0.70, 0.30))
        GlobalZIndex(900)
        template_value(FocusPolicy::Pass)
        template_value(Pickable::IGNORE)
        Children [
            metric_text("METRICS  ·  F3 to hide", 11.0, muted),
            (metric_text("— FPS  ·  — ms", 22.0, white) template_value(PerformanceMetric::Headline)),
            (metric_text("Waiting for frame samples", 14.0, muted) template_value(PerformanceMetric::Window)),
            (metric_text("GAME", 14.0, white) template_value(PerformanceMetric::Game)),
            (metric_text("NETWORK\nWaiting for connection", 14.0, muted) template_value(OverlayMetric::Network)),
            (metric_text("PREDICTION\nWaiting for samples", 14.0, muted) template_value(OverlayMetric::Prediction)),
        ]
    });
}

fn toggle_overlay(
    keys: Res<ButtonInput<KeyCode>>,
    mut overlay: ResMut<DiagnosticsOverlay>,
    mut panels: Query<&mut Node, With<OverlayPanel>>,
) {
    if !keys.just_pressed(KeyCode::F3) {
        return;
    }
    overlay.visible = !overlay.visible;
    for mut panel in &mut panels {
        panel.display = if overlay.visible {
            Display::Flex
        } else {
            Display::None
        };
    }
}

fn update_metrics(
    time: Res<Time<Real>>,
    overlay: Res<DiagnosticsOverlay>,
    diagnostics: Res<DiagnosticsStore>,
    view: Res<DreamView>,
    mut refresh: Local<RefreshClock>,
    mut rows: Query<(&PerformanceMetric, &mut Text), Without<OverlayMetric>>,
) {
    if !overlay.visible {
        return;
    }
    refresh.0.tick(time.delta());
    if !overlay.is_changed() && !refresh.0.just_finished() {
        return;
    }
    if overlay.is_changed() {
        refresh.0.reset();
    }
    let frames = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME);
    let frame_ms = frames
        .and_then(|d| d.smoothed())
        .filter(|value| value.is_finite() && *value > 0.0);
    // Average frame duration first: averaging instantaneous FPS can overstate
    // throughput when short event-driven frames alternate with longer frames.
    let fps = frame_ms
        .map(|value| 1000.0 / value)
        .filter(|value| value.is_finite());
    let headline = format!(
        "{} FPS  ·  {} ms",
        fps.map_or_else(|| "—".into(), |v| format!("{v:.0}")),
        frame_ms.map_or_else(|| "—".into(), |v| format!("{v:.1}")),
    );
    let window = if let Some(frames) = frames.filter(|d| d.history_len() > 0) {
        let count = frames.history_len().min(120);
        let (sum, worst) = frames
            .values()
            .skip(frames.history_len() - count)
            .fold((0.0_f64, 0.0_f64), |(sum, worst), value| {
                (sum + value, worst.max(*value))
            });
        format!(
            "Mean {:.1} ms · worst {worst:.1} ms\nLast {count} frames",
            sum / count as f64
        )
    } else {
        "Waiting for frame samples".into()
    };
    let snap = &view.0;
    let game = format!(
        "GAME  {}/{} · {:?} · tick {}\nHeroes {} · enemies {} · shots {} · FX {}\nLocal ({:.1}, {:.1}) · {:.1} units/s",
        snap.room + 1,
        TOTAL_ROOMS,
        snap.phase,
        snap.tick,
        snap.heroes.len(),
        snap.enemies.len(),
        snap.projectiles.len(),
        snap.presentations.len(),
        snap.hero.position[0],
        snap.hero.position[1],
        Vec2::from_array(snap.hero.velocity).length(),
    );
    for (metric, mut text) in &mut rows {
        let value = match metric {
            PerformanceMetric::Headline => &headline,
            PerformanceMetric::Window => &window,
            PerformanceMetric::Game => &game,
        };
        if text.0 != *value {
            text.0.clone_from(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_net_debug::ConditionerDebug;
    use dreamwake_sim::DreamSimulation;
    use engine_client::network_tools::NetworkToolsPlugin;

    fn overlay_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            bevy::diagnostic::DiagnosticsPlugin,
        ))
        .init_resource::<ButtonInput<KeyCode>>()
        .insert_resource(DreamView(DreamSimulation::new(7, false).snapshot()))
        .add_plugins(DreamDiagnosticsPlugin);
        app
    }

    #[test]
    fn overlay_starts_hidden_and_every_node_passes_pointer_input() {
        let mut app = overlay_app();
        app.update();
        assert!(!app.world().resource::<DiagnosticsOverlay>().visible);
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<&Node, With<OverlayPanel>>()
                .single(world)
                .unwrap()
                .display,
            Display::None
        );
        let mut nodes = world.query_filtered::<(
            Option<&Interaction>,
            Option<&FocusPolicy>,
            Option<&Pickable>,
        ), With<Node>>();
        assert!(nodes.iter(world).count() > 1);
        for (interaction, focus, pickable) in nodes.iter(world) {
            assert!(interaction.is_none());
            assert_eq!(focus, Some(&FocusPolicy::Pass));
            let pickable = pickable.expect("Every overlay node must ignore pointer picking");
            assert!(!pickable.should_block_lower && !pickable.is_hoverable);
        }
    }

    #[test]
    fn f3_and_the_real_f6_menu_toggle_independently() {
        let mut app = overlay_app();
        app.add_plugins(NetworkToolsPlugin);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F3);
        app.update();
        assert!(app.world().resource::<DiagnosticsOverlay>().visible);
        assert!(!app.world().resource::<ConditionerDebug>().visible);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F6);
        app.update();
        assert!(app.world().resource::<DiagnosticsOverlay>().visible);
        assert!(app.world().resource::<ConditionerDebug>().visible);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F3);
        app.update();
        assert!(!app.world().resource::<DiagnosticsOverlay>().visible);
        assert!(app.world().resource::<ConditionerDebug>().visible);
    }
}

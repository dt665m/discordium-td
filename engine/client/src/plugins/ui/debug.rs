//! Passive F3 diagnostics. Bevy owns frame sampling; this panel only presents it.
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    input::InputSystems,
    picking::Pickable,
    prelude::*,
    ui::FocusPolicy,
};

/// Presentation settings supplied before installing the plugin.
#[derive(Resource)]
pub struct DebugUiSettings {
    pub toggle_key: KeyCode,
    pub background: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
}
impl Default for DebugUiSettings {
    fn default() -> Self {
        Self {
            toggle_key: KeyCode::F3,
            background: Color::srgba(0.04, 0.04, 0.04, 0.94),
            border: Color::srgba(0.6, 0.6, 0.6, 0.3),
            text: Color::WHITE,
            muted: Color::srgb(0.7, 0.7, 0.7),
        }
    }
}

/// Passive frame diagnostics with an extensible UI root.
pub struct DebugUiPlugin;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugUiSystems {
    Toggle,
    Metrics,
}

#[derive(Resource, Default)]
pub struct DebugUiState {
    pub visible: bool,
}

#[derive(Component, Clone, Copy, Default)]
enum PerformanceMetric {
    #[default]
    Headline,
    Window,
}

#[derive(Component, Clone, Default)]
/// Attach passive application-specific rows beneath this entity.
pub struct DebugUiRoot;

struct RefreshClock(Timer);
impl Default for RefreshClock {
    fn default() -> Self {
        Self(Timer::from_seconds(0.25, TimerMode::Repeating))
    }
}

impl Plugin for DebugUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(FrameTimeDiagnosticsPlugin::new(120));
        }
        app.init_resource::<DebugUiState>()
            .init_resource::<DebugUiSettings>()
            .add_systems(Startup, spawn_overlay)
            .configure_sets(PreUpdate, DebugUiSystems::Toggle.after(InputSystems))
            .add_systems(PreUpdate, toggle_overlay.in_set(DebugUiSystems::Toggle))
            .add_systems(Update, update_metrics.in_set(DebugUiSystems::Metrics));
    }
}

fn spawn_overlay(mut commands: Commands, settings: Res<DebugUiSettings>) {
    let white = settings.text;
    let muted = settings.muted;
    let background = settings.background;
    let border = settings.border;
    let heading = format!("METRICS  ·  {:?} to hide", settings.toggle_key);
    commands.spawn_scene(bsn! {
        DebugUiRoot
        Node {
            display: Display::None,
            position_type: PositionType::Absolute,
            left: px(12), top: px(165), width: px(350), max_width: percent(95),
            padding: px(12), row_gap: px(7),
            flex_direction: FlexDirection::Column,
            border: px(1), border_radius: BorderRadius::all(px(6)),
        }
        BackgroundColor(background)
        BorderColor::all(border)
        GlobalZIndex(900)
        template_value(FocusPolicy::Pass)
        template_value(Pickable::IGNORE)
        Children [
            super::label(heading, 11.0, muted),
            (super::label("— FPS  ·  — ms", 22.0, white) template_value(PerformanceMetric::Headline)),
            (super::label("Waiting for frame samples", 14.0, muted) template_value(PerformanceMetric::Window)),
        ]
    });
}

fn toggle_overlay(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<DebugUiSettings>,
    mut overlay: ResMut<DebugUiState>,
    mut panels: Query<&mut Node, With<DebugUiRoot>>,
) {
    if keys.just_pressed(settings.toggle_key) {
        overlay.visible = !overlay.visible;
    }
    for mut panel in &mut panels {
        let display = if overlay.visible {
            Display::Flex
        } else {
            Display::None
        };
        if panel.display != display {
            panel.display = display;
        }
    }
}

fn update_metrics(
    time: Res<Time<Real>>,
    overlay: Res<DebugUiState>,
    diagnostics: Res<DiagnosticsStore>,
    mut refresh: Local<RefreshClock>,
    mut rows: Query<(&PerformanceMetric, &mut Text)>,
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
    for (metric, mut text) in &mut rows {
        let value = match metric {
            PerformanceMetric::Headline => &headline,
            PerformanceMetric::Window => &window,
        };
        if text.0 != *value {
            text.0.clone_from(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn debug_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            bevy::diagnostic::DiagnosticsPlugin,
        ))
        .init_resource::<ButtonInput<KeyCode>>()
        .add_plugins(DebugUiPlugin);
        app
    }

    #[test]
    fn passive_overlay_works_without_game_resources() {
        let mut app = debug_app();
        app.update();
        assert!(!app.world().resource::<DebugUiState>().visible);
        let world = app.world_mut();
        for (interaction, focus, pickable) in world
            .query_filtered::<(Option<&Interaction>, &FocusPolicy, &Pickable), With<Node>>()
            .iter(world)
        {
            assert!(interaction.is_none());
            assert_eq!(*focus, FocusPolicy::Pass);
            assert!(!pickable.should_block_lower && !pickable.is_hoverable);
        }
        world.resource_mut::<DebugUiState>().visible = true;
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<&Node, With<DebugUiRoot>>()
                .single(world)
                .unwrap()
                .display,
            Display::Flex
        );
        assert!(
            world
                .query::<&Text>()
                .iter(world)
                .any(|text| text.0.contains("FPS"))
        );
    }

    #[test]
    fn configured_key_controls_visibility() {
        let mut app = debug_app();
        app.world_mut().resource_mut::<DebugUiSettings>().toggle_key = KeyCode::F4;
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F3);
        app.update();
        assert!(!app.world().resource::<DebugUiState>().visible);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F4);
        app.update();
        assert!(app.world().resource::<DebugUiState>().visible);
    }
}

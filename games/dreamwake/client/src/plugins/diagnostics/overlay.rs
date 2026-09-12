//! Game and network rows extend the engine's passive diagnostics panel.
use crate::DreamView;
use bevy::{picking::Pickable, prelude::*, ui::FocusPolicy};
use dreamwake_sim::TOTAL_ROOMS;
use engine_client::ui::debug::{DebugUiPlugin, DebugUiRoot, DebugUiSettings, DebugUiState};

pub struct DreamDiagnosticsPlugin;

#[derive(Component, Clone, Copy, Default)]
pub(super) enum OverlayMetric {
    #[default]
    Network,
    Prediction,
}
#[derive(Component, Clone, Default)]
struct GameMetric;
#[derive(Component, Clone, Default)]
struct ActionMetric;

impl Plugin for DreamDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DebugUiSettings {
            background: Color::srgba(0.025, 0.041, 0.071, 0.94),
            border: Color::srgba(0.39, 0.64, 0.70, 0.30),
            text: Color::srgb(0.91, 0.95, 0.95),
            muted: Color::srgb(0.64, 0.74, 0.79),
            ..default()
        })
        .add_plugins(DebugUiPlugin)
        .add_systems(PostStartup, spawn_rows)
        .add_systems(Update, (update_game_metrics, update_action_metrics));
    }
}

fn spawn_rows(
    mut commands: Commands,
    root: Single<Entity, With<DebugUiRoot>>,
    settings: Res<DebugUiSettings>,
) {
    let parent = *root;
    let white = settings.text;
    let muted = settings.muted;
    commands.spawn_scene(bsn! {
        Node { flex_direction: FlexDirection::Column, row_gap: px(7) }
        template_value(FocusPolicy::Pass)
        template_value(Pickable::IGNORE)
        Children [
            (engine_client::ui::label("GAME", 14.0, white) GameMetric),
            (engine_client::ui::label("NETWORK\nWaiting for connection", 14.0, muted) template_value(OverlayMetric::Network)),
            (engine_client::ui::label("PREDICTION\nWaiting for samples", 14.0, muted) template_value(OverlayMetric::Prediction)),
            (engine_client::ui::label("ACTION\nWaiting for an action", 12.0, muted) ActionMetric),
        ]
    }).insert(ChildOf(parent));
}

fn update_action_metrics(
    time: Res<Time<Real>>,
    overlay: Res<DebugUiState>,
    view: Res<DreamView>,
    runtime: Option<NonSend<crate::plugins::network::Runtime>>,
    mut next_update: Local<f64>,
    mut rows: Query<&mut Text, With<ActionMetric>>,
) {
    if !overlay.visible {
        *next_update = 0.0;
        return;
    }
    let now = time.elapsed_secs_f64();
    if now < *next_update && !overlay.is_changed() {
        return;
    }
    *next_update = now + 0.25;
    let value = runtime
        .as_ref()
        .and_then(|r| r.outcomes.latest_trace(view.0.hero.id));
    let content = value.map_or_else(|| "ACTION\nWaiting for an action".to_string(), |trace| {
        let tick = |value: Option<engine_net::types::ServerTick>| value.map_or_else(|| "—".to_string(), |t| t.0.to_string());
        let verdict = trace.outcome.as_ref().map_or_else(
            || if trace.lookup_unavailable { "Lookup unavailable".into() } else { "Pending verdict".into() },
            |outcome| format!("{:?}: {:?} · {:.0} damage", outcome.data.status, outcome.data.reason, outcome.data.damage),
        );
        format!(
            "ACTION {} · {}:{}:{}:{}\nSample E {} · view R {} · target C {}\nQuery {} · executed {}\nCheckpoint {} · presented timeline {}\n{}",
            trace.kind, trace.key.connection.0, trace.key.stream.0, trace.key.command.0, trace.key.slot,
            tick(trace.view.map(|v| v.sampled_at)), tick(trace.view.map(|v| v.viewed_at)), trace.target_c.0,
            tick(trace.outcome.as_ref().and_then(|o| o.data.query_tick)), tick(trace.outcome.as_ref().map(|o| o.execution_tick)),
            tick(trace.checkpoint.map(|o| o.tick)), tick(trace.presented_timeline.map(|o| o.tick)), verdict,
        )
    });
    for mut text in &mut rows {
        if text.0 != content {
            text.0.clone_from(&content);
        }
    }
}

fn update_game_metrics(
    time: Res<Time<Real>>,
    overlay: Res<DebugUiState>,
    view: Res<DreamView>,
    mut next_update: Local<f64>,
    mut rows: Query<&mut Text, With<GameMetric>>,
) {
    if !overlay.visible {
        *next_update = 0.0;
        return;
    }
    let now = time.elapsed_secs_f64();
    if now < *next_update && !overlay.is_changed() {
        return;
    }
    *next_update = now + 0.25;
    let snap = &view.0;
    let cell_size = engine_net::interest::Limits::default().cell_size as f32;
    let cell = snap
        .hero
        .position
        .map(|axis| (axis / cell_size).floor() as i32);
    let game = format!(
        "GAME  {}/{} · {:?} · tick {}\nHeroes {} · enemies {} · props {} · shots {} · FX {}\nLocal ({:.1}, {:.1}) · cell ({}, {}) · {:.1} units/s",
        snap.room + 1,
        TOTAL_ROOMS,
        snap.phase,
        snap.tick,
        snap.heroes.len(),
        snap.enemies.len(),
        snap.covers.len(),
        snap.projectiles.len(),
        snap.presentations.len(),
        snap.hero.position[0],
        snap.hero.position[1],
        cell[0],
        cell[1],
        Vec2::from_array(snap.hero.velocity).length(),
    );
    for mut text in &mut rows {
        if text.0 != game {
            text.0.clone_from(&game);
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
        .insert_resource(DreamView(crate::offline_presentation(
            DreamSimulation::new(7, false).snapshot(),
        )))
        .add_plugins(DreamDiagnosticsPlugin);
        app
    }

    #[test]
    fn overlay_starts_hidden_and_every_node_passes_pointer_input() {
        let mut app = overlay_app();
        app.update();
        assert!(!app.world().resource::<DebugUiState>().visible);
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<&Node, With<DebugUiRoot>>()
                .single(world)
                .unwrap()
                .display,
            Display::None
        );
        assert_eq!(world.query::<&GameMetric>().iter(world).count(), 1);
        assert_eq!(world.query::<&OverlayMetric>().iter(world).count(), 2);
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
        assert!(app.world().resource::<DebugUiState>().visible);
        assert!(!app.world().resource::<ConditionerDebug>().visible);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F6);
        app.update();
        assert!(app.world().resource::<DebugUiState>().visible);
        assert!(app.world().resource::<ConditionerDebug>().visible);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F3);
        app.update();
        assert!(!app.world().resource::<DebugUiState>().visible);
        assert!(app.world().resource::<ConditionerDebug>().visible);
    }
}

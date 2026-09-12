//! Development controls live in the shared F6 menu, never in launch URLs.
use bevy::prelude::*;
use engine_client::{
    graphics::{GraphicsSet, PrototypeSettings, draw_gizmos},
    network_tools::NetworkToolsRoot,
};

use crate::plugins::{
    diagnostics::Playtest, graphics::scene::SceneGraphic, input::DreamInputSystems,
};

pub struct DreamDebugPlugin;

#[derive(Resource, Default)]
struct DebugGraphics {
    gizmos: bool,
    cell_grid: bool,
    aggro_ranges: bool,
}

#[derive(Component, Clone, Default)]
struct DebugControls;

#[derive(Component, Clone, Copy, Default)]
enum DebugAction {
    #[default]
    Gizmos,
    CellGrid,
    AggroRanges,
    Autoplay,
    Spatial,
}

impl Plugin for DreamDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugGraphics>()
            .init_resource::<PrototypeSettings>()
            .add_systems(Startup, spawn_controls)
            .add_systems(Update, (attach_controls, sync_control_labels))
            .add_systems(
                PostUpdate,
                sync_graphics_visibility
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            )
            .add_systems(PreUpdate, route_controls.in_set(DreamInputSystems::Buttons))
            .add_systems(
                Update,
                draw_gizmos
                    .in_set(GraphicsSet::Render)
                    .run_if(|settings: Res<DebugGraphics>| settings.gizmos),
            )
            .add_systems(
                Update,
                draw_cell_grid
                    .in_set(GraphicsSet::Render)
                    .run_if(|settings: Res<DebugGraphics>| settings.cell_grid),
            )
            .add_systems(
                Update,
                draw_aggro_ranges
                    .in_set(GraphicsSet::Render)
                    .run_if(|settings: Res<DebugGraphics>| settings.aggro_ranges),
            );
    }
}

fn draw_aggro_ranges(view: Res<crate::DreamView>, mut gizmos: Gizmos) {
    for enemy in &view.0.enemies {
        gizmos.circle(
            Isometry3d::new(
                Vec3::new(enemy.position[0], 0.08, enemy.position[1]),
                Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            ),
            dreamwake_sim::ENEMY_AGGRO_RANGE,
            Color::srgba(1.0, 0.55, 0.2, 0.45),
        );
    }
}

fn draw_cell_grid(mut gizmos: Gizmos) {
    let cell = engine_net::interest::Limits::default().cell_size as f32;
    let radius = dreamwake_sim::ARENA_RADIUS;
    let count = (radius / cell).floor() as i32;
    let color = Color::srgba(0.45, 0.9, 0.85, 0.65);
    for index in -count..=count {
        let axis = index as f32 * cell;
        let extent = (radius * radius - axis * axis).max(0.0).sqrt();
        gizmos.line(
            Vec3::new(axis, 0.06, -extent),
            Vec3::new(axis, 0.06, extent),
            color,
        );
        gizmos.line(
            Vec3::new(-extent, 0.06, axis),
            Vec3::new(extent, 0.06, axis),
            color,
        );
    }
}

// Keep scene lifetimes and animation current while hidden so switching back is
// immediate, including actors/effects spawned while gizmos were selected.
fn sync_graphics_visibility(
    graphics: Res<DebugGraphics>,
    mut roots: Query<&mut Visibility, With<SceneGraphic>>,
) {
    let visibility = if graphics.gizmos {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut current in &mut roots {
        current.set_if_neq(visibility);
    }
}

fn debug_button(action: DebugAction, text: &'static str) -> impl Scene {
    let text = text.to_owned();
    bsn! {
        Button
        template_value(action)
        Node { padding: UiRect::all(px(10)), min_height: px(40) }
        BackgroundColor(Color::srgb(0.10, 0.18, 0.22))
        Children [(
            Text(text)
            TextFont { font_size: px(16) }
            TextColor(Color::WHITE)
        )]
    }
}

fn spawn_controls(mut commands: Commands) {
    commands.spawn_scene(bsn! {
        DebugControls
        template_value(Visibility::Hidden)
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(4),
            padding: UiRect::all(px(8)),
            flex_shrink: 0.0,
        }
        Children [
            debug_button(DebugAction::Gizmos, "Gizmos: Off"),
            debug_button(DebugAction::CellGrid, "Graph cells: Off"),
            debug_button(DebugAction::AggroRanges, "Enemy aggro: Off"),
            debug_button(DebugAction::Autoplay, "Playtest autoplay: Off"),
            debug_button(DebugAction::Spatial, "Spatial playtest: Off"),
        ]
    });
}

fn attach_controls(
    mut commands: Commands,
    roots: Query<Entity, With<NetworkToolsRoot>>,
    controls: Query<Entity, (With<DebugControls>, Without<ChildOf>)>,
) {
    let Ok(root) = roots.single() else { return };
    for panel in &controls {
        commands.entity(panel).insert(Visibility::Inherited);
        commands.entity(root).insert_children(0, &[panel]);
    }
}

fn route_controls(
    buttons: Query<(&Interaction, &DebugAction), Changed<Interaction>>,
    mut graphics: ResMut<DebugGraphics>,
    mut playtest: ResMut<Playtest>,
) {
    for (interaction, action) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            DebugAction::Gizmos => graphics.gizmos = !graphics.gizmos,
            DebugAction::CellGrid => graphics.cell_grid = !graphics.cell_grid,
            DebugAction::AggroRanges => graphics.aggro_ranges = !graphics.aggro_ranges,
            DebugAction::Autoplay => playtest.autoplay = !playtest.autoplay,
            DebugAction::Spatial => {
                playtest.spatial = playtest.spatial.next();
                if playtest.spatial != super::SpatialPlaytest::Off {
                    playtest.autoplay = true;
                }
            }
        }
    }
}

fn sync_control_labels(
    buttons: Query<(&DebugAction, &Children)>,
    mut labels: Query<&mut Text>,
    graphics: Res<DebugGraphics>,
    playtest: Res<Playtest>,
) {
    for (action, children) in &buttons {
        let label = match action {
            DebugAction::Gizmos => {
                format!("Gizmos: {}", if graphics.gizmos { "On" } else { "Off" })
            }
            DebugAction::CellGrid => {
                format!(
                    "Graph cells: {}",
                    if graphics.cell_grid { "On" } else { "Off" }
                )
            }
            DebugAction::Autoplay => format!(
                "Playtest autoplay: {}",
                if playtest.autoplay { "On" } else { "Off" }
            ),
            DebugAction::AggroRanges => format!(
                "Enemy aggro: {}",
                if graphics.aggro_ranges { "On" } else { "Off" }
            ),
            DebugAction::Spatial => format!("Spatial playtest: {}", playtest.spatial.label()),
        };
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child)
                && text.0 != label
            {
                text.0.clone_from(&label);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_controls_attach_and_toggle_gizmos_at_runtime() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_resource::<DebugGraphics>()
        .init_resource::<Playtest>()
        .add_systems(Startup, spawn_controls)
        .add_systems(
            Update,
            (attach_controls, route_controls, sync_control_labels).chain(),
        )
        .add_systems(PostUpdate, sync_graphics_visibility);
        let art = app
            .world_mut()
            .spawn((SceneGraphic, Visibility::Inherited))
            .id();
        let hud = app.world_mut().spawn(Visibility::Inherited).id();
        let root = app.world_mut().spawn(NetworkToolsRoot).id();
        app.update();
        assert!(!app.world().resource::<DebugGraphics>().gizmos);
        let world = app.world_mut();
        let parent = world
            .query_filtered::<&ChildOf, With<DebugControls>>()
            .single(world)
            .unwrap();
        assert_eq!(parent.parent(), root);
        let button = world
            .query::<(Entity, &DebugAction)>()
            .iter(world)
            .find(|(_, action)| matches!(action, DebugAction::Gizmos))
            .unwrap()
            .0;
        for expected in [true, false] {
            app.world_mut()
                .entity_mut(button)
                .insert(Interaction::Pressed);
            app.update();
            assert_eq!(app.world().resource::<DebugGraphics>().gizmos, expected);
            assert_eq!(
                *app.world().get::<Visibility>(art).unwrap(),
                if expected {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                }
            );
            assert_eq!(
                *app.world().get::<Visibility>(hud).unwrap(),
                Visibility::Inherited
            );
            let label = if expected {
                "Gizmos: On"
            } else {
                "Gizmos: Off"
            };
            let world = app.world_mut();
            assert!(
                world
                    .query::<&Text>()
                    .iter(world)
                    .any(|text| text.0 == label)
            );
            app.world_mut().entity_mut(button).insert(Interaction::None);
            let spawned = app
                .world_mut()
                .spawn((SceneGraphic, Visibility::Inherited))
                .id();
            app.update();
            assert_eq!(
                *app.world().get::<Visibility>(spawned).unwrap(),
                if expected {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                }
            );
        }
    }
}

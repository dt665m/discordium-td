//! Development controls live in the shared F6 menu, never in launch URLs.
use bevy::prelude::*;
use engine_client::{
    network_tools::NetworkToolsRoot,
    presentation::{PresentationSet, PrototypeSettings, draw_gizmos},
};

use super::{DreamInputSystems, Playtest, scene::SceneGraphic};

pub struct DreamDebugPlugin;

#[derive(Resource, Default)]
struct DebugGraphics {
    gizmos: bool,
}

#[derive(Component, Clone, Default)]
struct DebugControls;

#[derive(Component, Clone, Copy, Default)]
enum DebugAction {
    #[default]
    Gizmos,
    Autoplay,
}

impl Plugin for DreamDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugGraphics>()
            .init_resource::<PrototypeSettings>()
            .add_systems(Startup, spawn_controls)
            .add_systems(Update, attach_controls)
            .add_systems(
                PostUpdate,
                sync_graphics_visibility
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            )
            .add_systems(PreUpdate, route_controls.in_set(DreamInputSystems::Buttons))
            .add_systems(
                Update,
                draw_gizmos
                    .in_set(PresentationSet::Render)
                    .run_if(|settings: Res<DebugGraphics>| settings.gizmos),
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
            debug_button(DebugAction::Autoplay, "Playtest autoplay: Off"),
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
    buttons: Query<(&Interaction, &DebugAction, &Children), Changed<Interaction>>,
    mut labels: Query<&mut Text>,
    mut graphics: ResMut<DebugGraphics>,
    mut playtest: ResMut<Playtest>,
) {
    for (interaction, action, children) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let (name, enabled) = match action {
            DebugAction::Gizmos => {
                graphics.gizmos = !graphics.gizmos;
                ("Gizmos", graphics.gizmos)
            }
            DebugAction::Autoplay => {
                playtest.autoplay = !playtest.autoplay;
                ("Playtest autoplay", playtest.autoplay)
            }
        };
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child) {
                text.0 = format!("{name}: {}", if enabled { "On" } else { "Off" });
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
        .add_systems(Update, (attach_controls, route_controls))
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

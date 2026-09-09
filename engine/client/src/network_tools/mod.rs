//! Shared F6 composition. Game adapters supply telemetry; this plugin owns only UI.
pub mod graphs;

use bevy::{
    input::InputSystems,
    input::mouse::{MouseScrollUnit, MouseWheel},
    prelude::*,
};
use bevy_net_debug::{ConditionerDebug, ConditionerDebugPlugin, ConditionerPanel};
use renet_cross::conditioner::ConditionerHandle;

pub struct NetworkToolsPlugin;
#[derive(Component)]
/// Root of the F6 debug menu; game plugins can attach additional controls.
pub struct NetworkToolsRoot;
#[derive(Component)]
struct ImpairmentNotice;

impl Plugin for NetworkToolsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ConditionerDebugPlugin::new(ConditionerHandle::default()))
            .init_resource::<graphs::DiagnosticHistory>()
            .add_systems(Startup, spawn_graphs)
            .add_systems(PostStartup, compose)
            .add_systems(PreUpdate, toggle.after(InputSystems))
            .add_systems(Update, (scroll, graphs::update_graphs, impairment_notice));
        app.world_mut().resource_mut::<ConditionerDebug>().visible = false;
    }
}
fn spawn_graphs(mut commands: Commands) {
    graphs::spawn(&mut commands);
}
fn compose(
    mut commands: Commands,
    mut graphs: Single<(Entity, &mut Node), With<graphs::DebugPanel>>,
    mut controls: Single<
        (Entity, &mut Node),
        (With<ConditionerPanel>, Without<graphs::DebugPanel>),
    >,
) {
    let root = commands
        .spawn((
            NetworkToolsRoot,
            Interaction::None,
            Visibility::Hidden,
            GlobalZIndex(1000),
            Node {
                position_type: PositionType::Absolute,
                right: px(12),
                top: px(12),
                width: px(640),
                max_width: percent(95),
                max_height: vh(92),
                overflow: Overflow::scroll_y(),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.045, 0.065)),
        ))
        .id();
    embed_panel(&mut commands, root, graphs.0, &mut graphs.1);
    embed_panel(&mut commands, root, controls.0, &mut controls.1);
    commands.spawn((
        ImpairmentNotice,
        Visibility::Hidden,
        GlobalZIndex(1001),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(8),
            right: px(12),
            padding: UiRect::all(px(6)),
            ..default()
        },
        BackgroundColor(Color::srgb(0.25, 0.08, 0.02)),
        Text::new("Network impairment active · F6 to adjust / turn Off"),
        TextFont {
            font_size: FontSize::Px(12.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.8, 0.45)),
    ));
}
fn embed_panel(commands: &mut Commands, root: Entity, entity: Entity, node: &mut Node) {
    node.position_type = PositionType::Relative;
    node.top = Val::Auto;
    node.left = Val::Auto;
    node.right = Val::Auto;
    node.width = percent(100);
    node.max_width = percent(100);
    node.max_height = Val::Auto;
    node.overflow = Overflow::visible();
    node.flex_shrink = 0.0;
    commands
        .entity(entity)
        .remove::<GlobalZIndex>()
        .insert(ChildOf(root));
}
fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<ConditionerDebug>,
    mut roots: Query<&mut Visibility, Or<(With<NetworkToolsRoot>, With<graphs::DebugPanel>)>>,
) {
    if keys.just_pressed(KeyCode::F6) {
        state.visible = !state.visible;
    }
    for mut visibility in &mut roots {
        *visibility = if state.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
fn impairment_notice(
    state: Res<ConditionerDebug>,
    mut notice: Single<&mut Visibility, With<ImpairmentNotice>>,
) {
    **notice = if state.handle().is_active() && !state.visible {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
}
fn scroll(
    mut wheel: MessageReader<MouseWheel>,
    mut root: Single<
        (
            &Visibility,
            &Interaction,
            &ComputedNode,
            &mut ScrollPosition,
        ),
        With<NetworkToolsRoot>,
    >,
) {
    let delta: f32 = wheel
        .read()
        .map(|event| {
            event.y
                * if event.unit == MouseScrollUnit::Line {
                    24.0
                } else {
                    1.0
                }
        })
        .sum();
    let (visibility, interaction, node, position) = &mut *root;
    if **visibility != Visibility::Hidden && **interaction != Interaction::None {
        let max = ((node.content_size().y - node.size().y) * node.inverse_scale_factor()).max(0.0);
        position.0.y = (position.0.y - delta).clamp(0.0, max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn f6_composes_both_panels_and_hiding_preserves_impairment() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ButtonInput<KeyCode>>()
            .add_plugins(NetworkToolsPlugin);
        app.update();
        let world = app.world_mut();
        let root = world
            .query_filtered::<Entity, With<NetworkToolsRoot>>()
            .single(world)
            .unwrap();
        for parent in world
            .query_filtered::<&ChildOf, Or<(With<graphs::DebugPanel>, With<ConditionerPanel>)>>()
            .iter(world)
        {
            assert_eq!(parent.parent(), root);
        }
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F3);
        app.update();
        assert!(!app.world().resource::<ConditionerDebug>().visible);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F6);
        app.update();
        assert!(app.world().resource::<ConditionerDebug>().visible);
        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::Inherited
        );
        let handle = app.world().resource::<ConditionerDebug>().handle().clone();
        let mut config = handle.config();
        config.enabled = true;
        config.latency = std::time::Duration::from_millis(100);
        handle.configure(config).unwrap();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset(KeyCode::F6);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F6);
        app.update();
        assert!(!app.world().resource::<ConditionerDebug>().visible);
        assert!(handle.is_active());
        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::Hidden
        );
    }
}

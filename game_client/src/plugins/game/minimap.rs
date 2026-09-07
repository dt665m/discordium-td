//! A 10 Hz UI map: no second camera, render texture, or duplicate scene rendering.
use super::*;
use bevy::ui::FocusPolicy;
use targeting::{PlayerVisual, player_color};

const SIZE: f32 = 164.0;
const HALF_RANGE: f32 = 12.0;
const INSET: f32 = 10.0;

pub(super) struct MinimapPlugin;
impl Plugin for MinimapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_minimap)
            .add_systems(Update, update_minimap.in_set(ClientUpdateSet::Ui));
    }
}

#[derive(Component)]
struct MinimapRoot;
#[derive(Component)]
struct MinimapSurface;
#[derive(Component)]
struct MapMarker(Entity);
#[derive(Component)]
struct BaseMarker;

fn spawn_minimap(mut commands: Commands) {
    commands
        .spawn((
            MinimapRoot,
            Node {
                position_type: PositionType::Absolute,
                right: px(16),
                bottom: px(16),
                padding: UiRect::all(px(8)),
                row_gap: px(4),
                flex_direction: FlexDirection::Column,
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.025, 0.045, 0.065, 0.88)),
            FocusPolicy::Pass,
            Visibility::Hidden,
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("NEARBY / 24 units"),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::srgb(0.7, 0.79, 0.85)),
                FocusPolicy::Pass,
            ));
            panel
                .spawn((
                    MinimapSurface,
                    Node {
                        width: px(SIZE),
                        height: px(SIZE),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.09, 0.14, 0.16)),
                    FocusPolicy::Pass,
                ))
                .with_children(|map| {
                    for (width, height, left, top) in
                        [(SIZE, 1.0, 0.0, SIZE / 2.0), (1.0, SIZE, SIZE / 2.0, 0.0)]
                    {
                        map.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                width: px(width),
                                height: px(height),
                                left: px(left),
                                top: px(top),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.5, 0.65, 0.7, 0.12)),
                            FocusPolicy::Pass,
                        ));
                    }
                    map.spawn((
                        BaseMarker,
                        Text::new("+"),
                        TextFont {
                            font_size: FontSize::Px(12.0),
                            ..default()
                        },
                        TextColor(Color::srgb(1.0, 0.76, 0.4)),
                        marker_node(),
                        FocusPolicy::Pass,
                    ));
                });
            panel.spawn((
                Text::new("You: white ring | Team: colors"),
                TextFont {
                    font_size: FontSize::Px(9.0),
                    ..default()
                },
                TextColor(Color::srgb(0.7, 0.79, 0.85)),
                FocusPolicy::Pass,
            ));
        });
}

fn marker_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        width: px(14),
        height: px(14),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    }
}

/// Preserve bearing when clipping outside teammates to the square map's edge.
fn map_point(offset: Vec2) -> (Vec2, bool) {
    let normalized = offset / HALF_RANGE;
    let outside = normalized.abs().max_element() > 1.0;
    let clipped = normalized / normalized.abs().max_element().max(1.0);
    (
        Vec2::splat(SIZE / 2.0) + clipped * (SIZE / 2.0 - INSET),
        outside,
    )
}

fn map_offset(position: Vec3, center: Vec3, camera: &Transform) -> Vec2 {
    let offset = position - center;
    let right = camera.right();
    let up = camera.up();
    Vec2::new(
        offset.x * right.x + offset.z * right.z,
        -(offset.x * up.x + offset.z * up.z),
    )
}

#[allow(clippy::too_many_arguments)]
fn update_minimap(
    mut commands: Commands,
    time: Res<Time>,
    mut elapsed: Local<f32>,
    world: Res<WorldView>,
    cameras: Query<&Transform, With<camera::GameCamera>>,
    roots: Query<Entity, With<MinimapRoot>>,
    surfaces: Query<Entity, With<MinimapSurface>>,
    actors: Query<
        (
            Entity,
            &Transform,
            Option<&PlayerVisual>,
            Option<&EnemyActor>,
        ),
        With<DynamicActor>,
    >,
    mut markers: Query<
        (
            Entity,
            &MapMarker,
            &mut Node,
            &mut Text,
            &mut TextColor,
            &mut BackgroundColor,
            &mut UiTransform,
            &mut Visibility,
        ),
        Without<BaseMarker>,
    >,
    mut bases: Query<(&mut Node, &mut Visibility), (With<BaseMarker>, Without<MapMarker>)>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < 0.1 {
        return;
    }
    *elapsed %= 0.1;
    let Ok(camera) = cameras.single() else { return };
    let Some((_, hero, _, _)) = actors
        .iter()
        .find(|(_, _, player, _)| player.is_some_and(|p| Some(p.id) == world.you))
    else {
        for entity in &roots {
            commands.entity(entity).insert(Visibility::Hidden);
        }
        for (entity, ..) in &markers {
            commands.entity(entity).despawn();
        }
        return;
    };
    for entity in &roots {
        commands.entity(entity).insert(Visibility::Inherited);
    }
    let center = hero.translation;
    let Ok(surface) = surfaces.single() else {
        return;
    };
    let mut existing = HashSet::new();
    for (
        entity,
        marker,
        mut node,
        mut text,
        mut color,
        mut background,
        mut transform,
        mut visibility,
    ) in &mut markers
    {
        let Ok((_, actor, player, enemy)) = actors.get(marker.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        existing.insert(marker.0);
        let offset = map_offset(actor.translation, center, camera);
        let (point, outside) = map_point(offset);
        *visibility = if outside && player.is_none() {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let diameter = if outside {
            14.0
        } else if player.is_some() {
            8.0
        } else {
            4.0
        };
        node.width = px(diameter);
        node.height = px(diameter);
        node.left = px(point.x - diameter / 2.0);
        node.top = px(point.y - diameter / 2.0);
        let mine = player.is_some_and(|p| Some(p.id) == world.you);
        let symbol = if outside { ">" } else { "" };
        if text.0 != symbol {
            text.0 = symbol.into();
        }
        color.0 = player.map_or(
            if enemy.is_some() {
                Color::srgb(0.95, 0.4, 0.36)
            } else {
                Color::srgb(0.9, 0.78, 0.3)
            },
            |p| player_color(p.id),
        );
        background.0 = if outside { Color::NONE } else { color.0 };
        // The local marker's white border is independent of the player's hue.
        node.border = UiRect::all(px(if mine { 1.0 } else { 0.0 }));
        node.border_radius = BorderRadius::all(px(7));
        transform.rotation = if outside {
            Rot2::radians(offset.y.atan2(offset.x))
        } else {
            Rot2::IDENTITY
        };
    }
    for (entity, _, _, _) in &actors {
        if existing.contains(&entity) {
            continue;
        }
        commands.spawn((
            MapMarker(entity),
            marker_node(),
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(14.0),
                ..default()
            },
            TextColor(Color::WHITE),
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::WHITE),
            UiTransform::default(),
            Visibility::Hidden,
            FocusPolicy::Pass,
            ChildOf(surface),
        ));
    }
    let (base, outside) = map_point(map_offset(
        world_to_translation(BASE_POSITION, 0.0),
        center,
        camera,
    ));
    for (mut node, mut visibility) in &mut bases {
        node.left = px(base.x - 7.0);
        node.top = px(base.y - 7.0);
        *visibility = if outside {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edge_arrows_preserve_bearing_and_stay_inside_map() {
        for offset in [
            Vec2::new(40.0, 10.0),
            Vec2::new(-20.0, -40.0),
            Vec2::new(0.0, 100.0),
        ] {
            let (point, outside) = map_point(offset);
            assert!(outside);
            let relative = point - Vec2::splat(SIZE / 2.0);
            assert!((relative.normalize() - offset.normalize()).length() < 0.0001);
            assert!(point.min_element() >= INSET && point.max_element() <= SIZE - INSET);
        }
        assert_eq!(map_point(Vec2::ZERO), (Vec2::splat(SIZE / 2.0), false));
        assert!(!map_point(Vec2::splat(HALF_RANGE)).1);
    }
    #[test]
    fn teammate_marker_enters_map_and_disappears_on_disconnect() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(WorldView {
                you: Some(1),
                ..default()
            })
            .add_systems(Update, update_minimap);
        let root = app
            .world_mut()
            .spawn((MinimapRoot, Visibility::Hidden))
            .id();
        app.world_mut().spawn(MinimapSurface);
        app.world_mut().spawn((
            camera::GameCamera::default(),
            Transform::from_xyz(0.0, 42.0, 0.01).looking_at(Vec3::ZERO, Vec3::Z),
        ));
        let local = app
            .world_mut()
            .spawn((
                DynamicActor,
                Transform::default(),
                PlayerVisual {
                    id: 1,
                    target: None,
                },
            ))
            .id();
        let remote = app
            .world_mut()
            .spawn((
                DynamicActor,
                Transform::from_xyz(30.0, 0.0, 0.0),
                PlayerVisual {
                    id: 2,
                    target: None,
                },
            ))
            .id();
        fn tick(app: &mut App) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(110));
            app.update();
        }
        tick(&mut app);
        tick(&mut app);
        let marker = app
            .world_mut()
            .query::<(Entity, &MapMarker)>()
            .iter(app.world())
            .find(|(_, marker)| marker.0 == remote)
            .unwrap()
            .0;
        assert_eq!(app.world().get::<Text>(marker).unwrap().0, ">");
        assert_eq!(
            *app.world().get::<Visibility>(marker).unwrap(),
            Visibility::Inherited
        );
        app.world_mut()
            .get_mut::<Transform>(remote)
            .unwrap()
            .translation
            .x = 3.0;
        tick(&mut app);
        assert_eq!(app.world().get::<Text>(marker).unwrap().0, "");
        assert_eq!(app.world().get::<Node>(marker).unwrap().width, px(8));
        app.world_mut().despawn(remote);
        tick(&mut app);
        assert!(app.world().get_entity(marker).is_err());
        app.world_mut().despawn(local);
        tick(&mut app);
        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::Hidden
        );
        assert_eq!(
            app.world_mut()
                .query::<&MapMarker>()
                .iter(app.world())
                .count(),
            0
        );
    }
}

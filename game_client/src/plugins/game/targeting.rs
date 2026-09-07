//! Continuous indicators derived from actor state, never network-event effects.
use super::*;

pub(super) struct TargetingPlugin;
impl Plugin for TargetingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_arrow_assets).add_systems(
            Update,
            (style_players, update_target_arrows)
                .chain()
                .in_set(ClientUpdateSet::Ui),
        );
    }
}

#[derive(Component)]
struct PlayerTint;

#[derive(Component)]
struct TargetArrow {
    owner: Entity,
}

#[derive(Resource)]
struct ArrowAssets {
    mesh: Handle<Mesh>,
    outline: Handle<StandardMaterial>,
}

fn setup_arrow_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(ArrowAssets {
        mesh: meshes.add(Triangle2d::new(
            Vec2::new(-0.5, 0.4),
            Vec2::new(0.0, -0.5),
            Vec2::new(0.5, 0.4),
        )),
        outline: materials.add(StandardMaterial {
            base_color: Color::srgb(0.025, 0.03, 0.04),
            unlit: true,
            cull_mode: None,
            ..default()
        }),
    });
}

pub(super) fn player_color(id: u64) -> Color {
    // Stable across clients and reconnecting observers; unrelated to local/remote status.
    let hash = id.wrapping_mul(0x9e3779b97f4a7c15);
    Color::hsl((hash >> 32) as f32 / u32::MAX as f32 * 360.0, 0.78, 0.60)
}

#[derive(Component, Clone, Copy, PartialEq)]
pub(super) struct PlayerVisual {
    pub id: u64,
    pub target: Option<u64>,
}

fn style_players(
    mut commands: Commands,
    assets: Res<ArrowAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    players: Query<(Entity, &PlayerVisual), Without<PlayerTint>>,
) {
    for (entity, player) in &players {
        let color = player_color(player.id);
        commands.entity(entity).insert((
            PlayerTint,
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                perceptual_roughness: 0.72,
                ..default()
            })),
        ));
        let fill = materials.add(StandardMaterial {
            base_color: color,
            unlit: true,
            cull_mode: None,
            ..default()
        });
        commands
            .spawn((
                TargetArrow { owner: entity },
                Mesh3d(assets.mesh.clone()),
                MeshMaterial3d(assets.outline.clone()),
                Transform::default(),
                Visibility::Hidden,
                NotShadowCaster,
            ))
            .with_children(|arrow| {
                arrow.spawn((
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(fill),
                    Transform::from_xyz(0.0, 0.015, 0.02).with_scale(Vec3::splat(0.72)),
                    NotShadowCaster,
                ));
            });
    }
}

fn update_target_arrows(
    mut commands: Commands,
    index: Res<RenderIndex>,
    players: Query<(Entity, &PlayerVisual)>,
    targets: Query<
        (&Transform, Option<&HealthStat>),
        (
            With<DynamicActor>,
            Without<TargetArrow>,
            Without<camera::GameCamera>,
        ),
    >,
    cameras: Query<
        (&Camera, &Projection, &Transform),
        (
            With<camera::GameCamera>,
            Without<TargetArrow>,
            Without<DynamicActor>,
        ),
    >,
    mut arrows: Query<
        (Entity, &TargetArrow, &mut Transform, &mut Visibility),
        (Without<DynamicActor>, Without<camera::GameCamera>),
    >,
) {
    let Ok((camera, projection, camera_transform)) = cameras.single() else {
        return;
    };
    let mut heroes: Vec<_> = players.iter().map(|(_, player)| *player).collect();
    heroes.sort_by_key(|hero| hero.id);
    for (entity, arrow, mut transform, mut visibility) in &mut arrows {
        let Ok((_, player)) = players.get(arrow.owner) else {
            commands.entity(entity).despawn();
            continue;
        };
        let target = player
            .target
            .and_then(|id| index.by_id.get(&ActorKey::World(id)))
            .and_then(|entity| targets.get(*entity).ok())
            .filter(|(_, health)| health.is_none_or(|health| health.current > 0.0));
        let Some((target, _)) = target else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let peers = heroes
            .iter()
            .filter(|other| other.target == player.target)
            .count();
        let preceding = heroes
            .iter()
            .filter(|other| other.target == player.target && other.id < player.id)
            .count();
        let column = preceding as f32 - (peers - 1) as f32 * 0.5;
        // Keep the solid pointer about 18 logical pixels wide at every zoom level.
        let viewport_height = camera
            .logical_viewport_size()
            .map_or(720.0, |size| size.y)
            .max(1.0);
        let depth = (target.translation - camera_transform.translation)
            .dot(*camera_transform.forward())
            .max(0.1);
        let view_height = match projection {
            Projection::Perspective(p) => 2.0 * depth * (p.fov * 0.5).tan(),
            Projection::Orthographic(p) => p.area.height(),
            _ => 35.0,
        };
        let size = 18.0 * view_height / viewport_height;
        transform.translation = target.translation
            + Vec3::Y * 0.65
            + *camera_transform.up() * (0.9 + size * 0.65)
            + *camera_transform.right() * column * size * 1.1;
        transform.rotation = camera_transform.rotation;
        transform.scale = Vec3::splat(size);
        *visibility = Visibility::Inherited;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arrows_follow_lock_state_and_clean_up_with_owner() {
        let mut app = App::new();
        app.init_resource::<RenderIndex>()
            .add_systems(Update, update_target_arrows);
        app.world_mut().spawn((
            camera::GameCamera::default(),
            Camera::default(),
            Projection::Perspective(PerspectiveProjection::default()),
            Transform::from_xyz(0.0, 42.0, 0.01).looking_at(Vec3::ZERO, Vec3::Z),
        ));
        let target = app
            .world_mut()
            .spawn((
                DynamicActor,
                Transform::default(),
                HealthStat {
                    current: 10.0,
                    max: 10.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<RenderIndex>()
            .by_id
            .insert(ActorKey::World(10), target);
        let owner = app
            .world_mut()
            .spawn(PlayerVisual {
                id: 1,
                target: Some(10),
            })
            .id();
        let teammate = app
            .world_mut()
            .spawn(PlayerVisual {
                id: 2,
                target: Some(10),
            })
            .id();
        let arrow = app
            .world_mut()
            .spawn((
                TargetArrow { owner },
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
        let other = app
            .world_mut()
            .spawn((
                TargetArrow { owner: teammate },
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(arrow).unwrap(),
            Visibility::Inherited
        );
        assert_ne!(
            app.world().get::<Transform>(arrow).unwrap().translation,
            app.world().get::<Transform>(other).unwrap().translation
        );
        app.world_mut()
            .get_mut::<PlayerVisual>(owner)
            .unwrap()
            .target = None;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(arrow).unwrap(),
            Visibility::Hidden
        );
        app.world_mut()
            .get_mut::<PlayerVisual>(owner)
            .unwrap()
            .target = Some(10);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(arrow).unwrap(),
            Visibility::Inherited
        );
        app.world_mut()
            .get_mut::<HealthStat>(target)
            .unwrap()
            .current = 0.0;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(arrow).unwrap(),
            Visibility::Hidden
        );
        app.world_mut().despawn(target);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(other).unwrap(),
            Visibility::Hidden
        );
        app.world_mut().despawn(owner);
        app.update();
        assert!(app.world().get_entity(arrow).is_err());
    }
}

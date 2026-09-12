//! Game-owned overhead meters using renderer-independent UI projection.
use crate::DreamView;
use bevy::prelude::*;
use dreamwake_sim::{EnemyKind, RunPhase};
use engine_client::{
    camera::CameraRig as DreamCameraRig,
    ui::{MeterFraction, WorldUi, WorldUiAnchor, meter_with_marker},
};
use std::collections::HashSet;

/// Optional rendered pose for overhead UI. Alternate renderers can annotate their roots;
/// without one, meters follow the authoritative snapshot position.
#[derive(Component)]
pub struct OverheadAnchor(pub u64);

fn world(position: [f32; 2], height: f32) -> Vec3 {
    Vec3::new(position[0], height, position[1])
}

/// Snapshot-owned UI identities outlive mesh replacements, but never their actors.
#[derive(Component, Clone, Default)]
pub(super) struct HealthBar(u64);

#[derive(Component, Clone, Default)]
pub(super) struct HealthFill(u64);

pub(super) fn sync_health_bars(
    mut commands: Commands,
    view: Res<DreamView>,
    cameras: Query<Entity, With<DreamCameraRig>>,
    actors: Query<(Entity, &OverheadAnchor)>,
    mut bars: Query<(
        Entity,
        &HealthBar,
        &mut WorldUiAnchor,
        &mut UiTargetCamera,
        &mut Node,
    )>,
    mut fills: Query<(&HealthFill, &mut MeterFraction, &mut BackgroundColor)>,
) {
    let camera = cameras.single().ok();
    let mut desired = Vec::new();
    if view.0.phase != RunPhase::Intro && camera.is_some() {
        for hero in &view.0.heroes {
            desired.push((
                hero.id,
                world(hero.position, hero.elevation),
                if hero.hp > 0.0 {
                    if hero.crouched { 2.3 } else { 2.6 }
                } else {
                    0.75
                },
                hero.hp / hero.max_hp.max(1.0),
                50.0,
                Color::srgb(0.30, 0.90, 0.74),
            ));
        }
        for enemy in &view.0.enemies {
            let (height, width) = match enemy.kind {
                EnemyKind::Boss => (6.5, 88.0),
                EnemyKind::Elite => (3.9, 56.0),
                EnemyKind::Ambusher => (2.55, 44.0),
                _ => (3.0, 48.0),
            };
            desired.push((
                enemy.id,
                world(enemy.position, 0.0),
                height,
                enemy.hp / enemy.max_hp.max(1.0),
                width,
                Color::srgb(1.0, 0.30, 0.32),
            ));
        }
    }
    let anchor_for = |id, position, height| {
        actors
            .iter()
            .find(|(_, actor)| actor.0 == id)
            .map(|(entity, _)| WorldUiAnchor::Entity {
                entity,
                offset: Vec3::Y * height,
            })
            .unwrap_or_else(|| WorldUiAnchor::World(position + Vec3::Y * height))
    };
    let mut existing = HashSet::new();
    for (entity, bar, mut anchor, mut target, mut node) in &mut bars {
        if let Some((id, position, height, _, width, _)) =
            desired.iter().find(|entry| entry.0 == bar.0)
        {
            existing.insert(*id);
            anchor.set_if_neq(anchor_for(*id, *position, *height));
            target.set_if_neq(UiTargetCamera(camera.unwrap()));
            if node.width != px(*width) {
                node.width = px(*width);
            }
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (fill, mut fraction, mut background) in &mut fills {
        if let Some((_, _, _, hp, _, color)) = desired.iter().find(|entry| entry.0 == fill.0) {
            fraction.set_if_neq(MeterFraction(*hp));
            background.set_if_neq(BackgroundColor(*color));
        }
    }
    for (id, position, height, hp, width, color) in desired {
        if existing.contains(&id) {
            continue;
        }
        let anchor = anchor_for(id, position, height);
        let camera = camera.unwrap();
        commands
            .spawn_scene(bsn! {
                HealthBar(id)
                WorldUi
                template_value(anchor)
                GlobalZIndex(3)
                Node { position_type: PositionType::Absolute, width: px(width), height: px(4) }
                Children [meter_with_marker(percent(100), 4.0, hp, color,
                    Color::srgba(0.025, 0.035, 0.055, 0.88), HealthFill(id))]
            })
            .insert(UiTargetCamera(camera));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dreamwake_sim::DreamSimulation;
    #[test]
    fn overhead_health_tracks_snapshot_values_replacements_and_cleanup_without_meshes() {
        let mut snapshot = crate::offline_presentation(DreamSimulation::new(7, false).snapshot());
        snapshot.phase = RunPhase::Combat;
        snapshot.heroes[0].elevation = 2.0;
        snapshot.heroes[0].crouched = true;
        let hero_id = snapshot.heroes[0].id;
        let hero_position = snapshot.heroes[0].position;
        snapshot.enemies.clear();
        let enemy_id = 1 << 63;
        snapshot.enemies.push(dreamwake_sim::EnemyView {
            id: enemy_id,
            position: [3.0, 4.0],
            facing: [1.0, 0.0],
            hp: 100.0,
            max_hp: 100.0,
            kind: EnemyKind::Ranged,
            windup: 0.0,
            target: [0.0; 2],
            warn_radius: 0.0,
            phase: 0,
            slowed: false,
            hit_flash: 0.0,
        });
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .add_plugins(engine_client::ui::EngineUiPlugin)
        .insert_resource(DreamView(snapshot))
        .add_systems(Update, sync_health_bars);
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(800, 600),
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view =
            Mat4::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 0.1, 100.0);
        app.world_mut().spawn((
            DreamCameraRig::default(),
            camera,
            Transform::from_xyz(0.0, 0.0, 20.0),
        ));
        app.update();
        let root = {
            let world = app.world_mut();
            assert_eq!(world.query::<&HealthBar>().iter(world).count(), 2);
            let (_, hero_anchor) = world
                .query::<(&HealthBar, &WorldUiAnchor)>()
                .iter(world)
                .find(|(bar, _)| bar.0 == hero_id)
                .unwrap();
            assert_eq!(
                *hero_anchor,
                WorldUiAnchor::World(super::world(hero_position, 4.3))
            );
            let (_, fraction, color, node) = world
                .query::<(&HealthFill, &MeterFraction, &BackgroundColor, &Node)>()
                .iter(world)
                .find(|(fill, _, _, _)| fill.0 == enemy_id)
                .unwrap();
            assert_eq!(fraction.0, 1.0);
            assert_eq!(node.width, percent(100));
            assert_eq!(color.0, Color::srgb(1.0, 0.30, 0.32));
            world
                .query::<(Entity, &HealthBar)>()
                .iter(world)
                .find(|(_, bar)| bar.0 == enemy_id)
                .unwrap()
                .0
        };
        {
            let mut view = app.world_mut().resource_mut::<DreamView>();
            view.0.enemies[0].hp = 25.0;
            view.0.enemies[0].kind = EnemyKind::Boss;
            view.0.enemies[0].facing = [-1.0, 0.0];
        }
        app.update();
        {
            let world = app.world_mut();
            assert_eq!(world.get::<Node>(root).unwrap().width, px(88));
            assert_eq!(
                *world.get::<WorldUiAnchor>(root).unwrap(),
                WorldUiAnchor::World(Vec3::new(3.0, 6.5, 4.0))
            );
            assert_eq!(
                *world.get::<Visibility>(root).unwrap(),
                Visibility::Inherited
            );
            let (_, fraction, node) = world
                .query::<(&HealthFill, &MeterFraction, &Node)>()
                .iter(world)
                .find(|(fill, _, _)| fill.0 == enemy_id)
                .unwrap();
            assert_eq!(fraction.0, 0.25);
            assert_eq!(node.width, percent(25));
        }
        app.world_mut()
            .resource_mut::<DreamView>()
            .0
            .enemies
            .clear();
        app.update();
        {
            let world = app.world_mut();
            assert!(world.get_entity(root).is_err());
            assert_eq!(world.query::<&HealthFill>().iter(world).count(), 1);
        }
        app.world_mut().resource_mut::<DreamView>().0.phase = RunPhase::Intro;
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&HealthBar>().iter(world).count(), 0);
        assert_eq!(world.query::<&HealthFill>().iter(world).count(), 0);
    }
}

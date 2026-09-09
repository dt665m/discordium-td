//! Game-specific mapping; engine renderers never see a DreamSnapshot or game enum.
use super::DreamView;
use bevy::prelude::*;
use engine_client::{camera::CameraSettings, presentation::*};

pub struct DreamPresentationPlugin;
impl Plugin for DreamPresentationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CameraSettings {
            offset: Vec3::new(18.0, 32.0, 26.0),
            viewport_height: 40.0,
            smoothing: 5.0,
            ..default()
        })
        .add_systems(Update, adapt.in_set(PresentationSet::Adapt));
    }
}
fn point(p: [f32; 2], y: f32) -> Vec3 {
    Vec3::new(p[0], y, p[1])
}
fn adapt(
    view: Res<DreamView>,
    mut frame: ResMut<PresentationFrame>,
    mut camera: ResMut<CameraSettings>,
) {
    let snap = &view.0;
    let target = (point(snap.hero.position, 0.0) * 0.30)
        .clamp(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 0.0, 5.0));
    if camera.target != target {
        camera.target = target;
    }
    frame.visuals.clear();
    let mut actor = |namespace, id, position, primitive, scale, color| {
        frame.visuals.push(Visual {
            id: VisualId::Entity { namespace, id },
            position,
            primitive,
            scale,
            color,
        })
    };
    for hero in &snap.heroes {
        actor(
            0,
            hero.id,
            point(hero.position, 0.7),
            Primitive::Box,
            Vec3::new(0.8, 1.4, 0.8),
            if hero.hit_flash > 0.0 {
                Color::WHITE
            } else {
                Color::srgb(0.3, 0.85, 1.0)
            },
        );
        actor(
            5,
            hero.id,
            point(hero.position, 0.2),
            Primitive::Arrow {
                direction: point(hero.facing, 0.0),
            },
            Vec3::splat(if hero.attack_flash > 0.0 { 3.0 } else { 1.8 }),
            if hero.attack_flash > 0.0 {
                Color::srgb(1.0, 0.9, 0.3)
            } else {
                Color::srgb(0.6, 0.95, 1.0)
            },
        );
    }
    for enemy in &snap.enemies {
        actor(
            1,
            enemy.id,
            point(enemy.position, 0.7),
            Primitive::Sphere,
            Vec3::splat(0.65),
            if enemy.hit_flash > 0.0 {
                Color::WHITE
            } else {
                Color::srgb(1.0, 0.35, 0.3)
            },
        );
        if enemy.windup > 0.0 {
            actor(
                2,
                enemy.id,
                point(enemy.target, 0.08),
                Primitive::Ring,
                Vec3::splat(enemy.warn_radius),
                Color::srgb(1.0, 0.55, 0.15),
            );
        }
    }
    for shot in &snap.projectiles {
        actor(
            3,
            shot.id,
            point(shot.position, 0.4),
            Primitive::Sphere,
            Vec3::splat(shot.radius.max(0.15)),
            if shot.friendly {
                Color::srgb(0.4, 1.0, 0.7)
            } else {
                Color::srgb(1.0, 0.25, 0.2)
            },
        );
    }
    for wisp in &snap.wisps {
        actor(
            4,
            wisp.id,
            point(wisp.position, 1.0),
            Primitive::Sphere,
            Vec3::splat(0.3),
            Color::srgb(0.75, 0.6, 1.0),
        );
    }
    for effect in &snap.presentations {
        frame.visuals.push(Visual {
            id: VisualId::Effect(effect.id),
            primitive: Primitive::Ring,
            position: point(effect.pos, 0.12),
            scale: Vec3::splat(effect.radius),
            color: Color::srgb(0.9, 0.85, 1.0),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stationary_aim_does_not_move_the_camera() {
        let mut snapshot = dreamwake_sim::DreamSimulation::new(7, false).snapshot();
        snapshot.hero.position = [8.0, -4.0];
        let mut app = App::new();
        app.insert_resource(DreamView(snapshot))
            .add_plugins((PresentationPlugin, DreamPresentationPlugin));
        app.update();
        let target = app.world().resource::<CameraSettings>().target;
        assert_eq!(target, Vec3::new(2.4, 0.0, -1.2));
        app.world_mut().resource_mut::<DreamView>().0.hero.facing = [-1.0, 0.0];
        app.update();
        assert_eq!(app.world().resource::<CameraSettings>().target, target);
    }
    #[derive(Resource, Default)]
    struct Observed(Vec<VisualId>);
    struct AlternateRenderer;
    impl Plugin for AlternateRenderer {
        fn build(&self, app: &mut App) {
            app.init_resource::<Observed>().add_systems(
                Update,
                (|frame: Res<PresentationFrame>, mut seen: ResMut<Observed>| {
                    seen.0 = frame.visuals.iter().map(|v| v.id).collect();
                })
                .in_set(PresentationSet::Render),
            );
        }
    }
    #[test]
    fn alternate_renderer_consumes_same_game_state_without_scene_or_camera_entities() {
        let snapshot = dreamwake_sim::DreamSimulation::new(7, false).snapshot();
        let expected = snapshot.heroes.len() * 2;
        let mut app = App::new();
        app.insert_resource(DreamView(snapshot))
            .init_resource::<CameraSettings>()
            .add_plugins((
                PresentationPlugin,
                DreamPresentationPlugin,
                AlternateRenderer,
            ));
        app.update();
        assert_eq!(app.world().resource::<Observed>().0.len(), expected);
        assert_eq!(app.world().resource::<DreamView>().0.tick, 0);
        let identity = app.world().resource::<Observed>().0[0];
        app.world_mut().resource_mut::<DreamView>().0.heroes[0].position = [3.0, -2.0];
        app.update();
        let frame = app.world().resource::<PresentationFrame>();
        assert_eq!(frame.visuals[0].id, identity);
        assert_eq!(frame.visuals[0].position, Vec3::new(3.0, 0.7, -2.0));
        app.world_mut().resource_mut::<DreamView>().0.heroes.clear();
        app.update();
        assert!(app.world().resource::<Observed>().0.is_empty());
    }
}

//! Game-specific mapping; engine renderers never see a DreamPresentation or game enum.
use crate::DreamView;
use bevy::prelude::*;
use engine_client::graphics::*;

fn point(p: [f32; 2], y: f32) -> Vec3 {
    Vec3::new(p[0], y, p[1])
}
pub(super) fn adapt(view: Res<DreamView>, mut frame: ResMut<GraphicsFrame>) {
    let snap = &view.0;
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
        let height = if hero.crouched { 1.2 } else { 1.4 };
        actor(
            0,
            hero.id,
            point(hero.position, hero.elevation + height * 0.5),
            Primitive::Box,
            Vec3::new(0.8, height, 0.8),
            if hero.hit_flash > 0.0 {
                Color::WHITE
            } else {
                Color::srgb(0.3, 0.85, 1.0)
            },
        );
        actor(
            5,
            hero.id,
            point(hero.position, hero.elevation + 0.2),
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
    for platform in &snap.platforms {
        frame.visuals.push(Visual {
            id: VisualId::Entity {
                namespace: 7,
                id: platform.id,
            },
            primitive: Primitive::OrientedBox {
                rotation: Quat::from_array(platform.pose.rotation),
            },
            position: Vec3::from_array(platform.pose.position),
            scale: Vec3::from_array(platform.half_extents) * 2.0,
            color: Color::srgb(0.75, 0.6, 1.0),
        });
    }
    for cover in &snap.covers {
        frame.visuals.push(Visual {
            id: VisualId::Entity {
                namespace: 6,
                id: cover.id,
            },
            primitive: Primitive::Box,
            position: Vec3::from_array(cover.position),
            scale: Vec3::from_array(cover.half_extents) * 2.0,
            color: Color::srgba(0.4, 0.75, 1.0, 0.45),
        });
    }
    for marker in &snap.cover_markers {
        frame.visuals.push(Visual {
            id: VisualId::Entity {
                namespace: 8,
                id: marker.id,
            },
            primitive: Primitive::Sphere,
            position: Vec3::from_array(marker.position),
            scale: Vec3::splat(0.14),
            color: if marker.open {
                Color::srgb(1.0, 0.85, 0.4)
            } else {
                Color::srgb(0.4, 0.75, 1.0)
            },
        });
    }
    for effect in &snap.presentations {
        let (primitive, position, scale) = match effect.kind {
            engine_core::GraphicsKind::Orb { elevation } => (
                Primitive::Sphere,
                point(effect.pos, elevation),
                Vec3::splat(effect.radius),
            ),
            engine_core::GraphicsKind::RadialPulse => (
                Primitive::Ring,
                point(effect.pos, 0.12),
                Vec3::splat(effect.radius),
            ),
            engine_core::GraphicsKind::Beam {
                direction,
                elevation,
                length,
            } => (
                Primitive::Beam {
                    direction: Vec3::from_array(direction),
                },
                point(effect.pos, elevation),
                Vec3::new(effect.radius * 2.0, effect.radius * 2.0, length),
            ),
        };
        frame.visuals.push(Visual {
            id: VisualId::Effect(effect.id),
            primitive,
            position,
            scale,
            color: Color::srgb(0.9, 0.85, 1.0),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::graphics::DreamGraphicsPlugin;
    #[derive(Resource, Default)]
    struct Observed(Vec<VisualId>);
    struct AlternateRenderer;
    impl Plugin for AlternateRenderer {
        fn build(&self, app: &mut App) {
            app.init_resource::<Observed>().add_systems(
                Update,
                (|frame: Res<GraphicsFrame>, mut seen: ResMut<Observed>| {
                    seen.0 = frame.visuals.iter().map(|v| v.id).collect();
                })
                .in_set(GraphicsSet::Render),
            );
        }
    }
    #[test]
    fn alternate_renderer_consumes_same_game_state_without_scene_or_camera_entities() {
        let snapshot =
            crate::offline_presentation(dreamwake_sim::DreamSimulation::new(7, false).snapshot());
        let expected = snapshot.heroes.len() * 2 + snapshot.covers.len() + snapshot.platforms.len();
        let mut app = App::new();
        app.insert_resource(DreamView(snapshot)).add_plugins((
            GraphicsPlugin,
            DreamGraphicsPlugin,
            AlternateRenderer,
        ));
        app.update();
        assert_eq!(app.world().resource::<Observed>().0.len(), expected);
        assert_eq!(app.world().resource::<DreamView>().0.tick, 0);
        let identity = app.world().resource::<Observed>().0[0];
        app.world_mut().resource_mut::<DreamView>().0.heroes[0].position = [3.0, -2.0];
        app.update();
        let frame = app.world().resource::<GraphicsFrame>();
        assert_eq!(frame.visuals[0].id, identity);
        assert_eq!(frame.visuals[0].position, Vec3::new(3.0, 0.7, -2.0));
        {
            let mut view = app.world_mut().resource_mut::<DreamView>();
            view.0.heroes[0].elevation = 2.0;
            view.0.heroes[0].crouched = true;
        }
        app.update();
        let frame = app.world().resource::<GraphicsFrame>();
        assert_eq!(frame.visuals[0].id, identity);
        assert_eq!(frame.visuals[0].position, Vec3::new(3.0, 2.6, -2.0));
        assert_eq!(frame.visuals[0].scale, Vec3::new(0.8, 1.2, 0.8));
        assert_eq!(frame.visuals[1].position, Vec3::new(3.0, 2.2, -2.0));
        app.world_mut().resource_mut::<DreamView>().0.heroes.clear();
        app.world_mut().resource_mut::<DreamView>().0.covers.clear();
        app.world_mut()
            .resource_mut::<DreamView>()
            .0
            .platforms
            .clear();
        app.update();
        assert!(app.world().resource::<Observed>().0.is_empty());
    }

    #[test]
    fn beam_primitive_preserves_simulation_identity_origin_and_dimensions() {
        let mut snapshot =
            crate::offline_presentation(dreamwake_sim::DreamSimulation::new(7, false).snapshot());
        let id = engine_core::GraphicsId {
            scope: None,
            match_epoch: 2,
            owner: 1,
            action_seq: 7,
            slot: 0,
        };
        snapshot.presentations.push(engine_core::GraphicsInstance {
            id,
            kind: engine_core::GraphicsKind::Beam {
                direction: [0.0, 0.6, -0.8],
                elevation: 1.25,
                length: 6.0,
            },
            pos: [2.0, -3.0],
            radius: 0.15,
            age_ticks: 1,
            duration_ticks: 120,
        });
        let mut app = App::new();
        app.insert_resource(DreamView(snapshot))
            .add_plugins((GraphicsPlugin, DreamGraphicsPlugin));
        app.update();
        let frame = app.world().resource::<GraphicsFrame>();
        let beam = frame
            .visuals
            .iter()
            .find(|v| v.id == VisualId::Effect(id))
            .unwrap();
        assert_eq!(beam.position, Vec3::new(2.0, 1.25, -3.0));
        assert_eq!(beam.scale, Vec3::new(0.3, 0.3, 6.0));
        assert_eq!(
            beam.primitive,
            Primitive::Beam {
                direction: Vec3::new(0.0, 0.6, -0.8)
            }
        );
        app.world_mut()
            .resource_mut::<DreamView>()
            .0
            .presentations
            .clear();
        app.update();
        assert!(
            !app.world()
                .resource::<GraphicsFrame>()
                .visuals
                .iter()
                .any(|v| v.id == VisualId::Effect(id))
        );
    }

    #[test]
    fn orbs_keep_control_identity_and_platforms_keep_declared_rotation() {
        let mut snapshot =
            crate::offline_presentation(dreamwake_sim::DreamSimulation::new(7, false).snapshot());
        let platform = &mut snapshot.platforms[0];
        platform.pose = platform.pose_at(128).unwrap();
        platform.gameplay_tick = 128;
        platform.phase = 128;
        let platform = *platform;
        let id = engine_core::GraphicsId {
            scope: Some(engine_core::GraphicsScope {
                source_epoch: 3,
                stream: 1,
                control_epoch: 2,
                generation: 1,
            }),
            match_epoch: 2,
            owner: 1,
            action_seq: 7,
            slot: 61000,
        };
        let next_id = engine_core::GraphicsId {
            scope: Some(engine_core::GraphicsScope {
                control_epoch: 3,
                ..id.scope.unwrap()
            }),
            ..id
        };
        for id in [id, next_id] {
            snapshot.presentations.push(engine_core::GraphicsInstance {
                id,
                kind: engine_core::GraphicsKind::Orb { elevation: 0.9 },
                pos: [2.0, -3.0],
                radius: 0.34,
                age_ticks: 1,
                duration_ticks: 120,
            });
        }
        let mut app = App::new();
        app.insert_resource(DreamView(snapshot))
            .add_plugins((GraphicsPlugin, DreamGraphicsPlugin));
        app.update();
        let frame = app.world().resource::<GraphicsFrame>();
        for id in [id, next_id] {
            let orb = frame
                .visuals
                .iter()
                .find(|v| v.id == VisualId::Effect(id))
                .unwrap();
            assert_eq!(orb.primitive, Primitive::Sphere);
            assert_eq!(orb.position, Vec3::new(2.0, 0.9, -3.0));
            assert_eq!(orb.scale, Vec3::splat(0.34));
        }
        let support = frame
            .visuals
            .iter()
            .find(|v| {
                v.id == VisualId::Entity {
                    namespace: 7,
                    id: platform.id,
                }
            })
            .unwrap();
        assert_eq!(support.position, Vec3::from_array(platform.pose.position));
        assert_eq!(support.scale, Vec3::from_array(platform.half_extents) * 2.0);
        assert_eq!(
            support.primitive,
            Primitive::OrientedBox {
                rotation: Quat::from_array(platform.pose.rotation),
            }
        );
    }
}

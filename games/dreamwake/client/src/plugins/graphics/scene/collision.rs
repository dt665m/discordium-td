//! Visible boundary geometry follows the admitted collision manifest.
use super::{SceneArt, SceneGraphic, piece};
use crate::DreamView;
use bevy::prelude::*;
use dreamwake_sim::collision::CollisionManifest;
use engine_core::CollisionShape;
use std::sync::Arc;

#[derive(Component)]
pub(super) struct CollisionVisual(Arc<CollisionManifest>);

#[derive(Component)]
pub(super) struct CoverVisual(u64);

#[derive(Component)]
pub(super) struct CoverMarkerVisual(u64);

#[derive(Component)]
pub(super) struct PlatformVisual(u64);

pub(super) fn sync_cover_markers(
    mut commands: Commands,
    view: Res<DreamView>,
    art: Res<SceneArt>,
    mut existing: Query<(
        Entity,
        &CoverMarkerVisual,
        &mut Transform,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let appearance = |marker: &dreamwake_sim::replication::CoverMarkerPresentation| {
        (
            Transform::from_translation(Vec3::from_array(marker.position))
                .with_scale(Vec3::splat(0.14)),
            if marker.open {
                art.gold_glow.clone()
            } else {
                art.ice_glow.clone()
            },
        )
    };
    for (entity, marker, mut transform, mut material) in &mut existing {
        if let Some(state) = view
            .0
            .cover_markers
            .iter()
            .find(|state| state.id == marker.0)
        {
            let (next, color) = appearance(state);
            if *transform != next {
                *transform = next;
            }
            if material.0 != color {
                material.0 = color;
            }
        } else {
            commands.entity(entity).despawn();
        }
    }
    for state in &view.0.cover_markers {
        if existing
            .iter()
            .any(|(_, marker, _, _)| marker.0 == state.id)
        {
            continue;
        }
        let (transform, material) = appearance(state);
        commands.spawn((
            SceneGraphic,
            CoverMarkerVisual(state.id),
            Mesh3d(art.sphere.clone()),
            MeshMaterial3d(material),
            transform,
            Visibility::Inherited,
        ));
    }
}

pub(super) fn sync_platforms(
    mut commands: Commands,
    view: Res<DreamView>,
    art: Res<SceneArt>,
    mut existing: Query<(Entity, &PlatformVisual, &mut Transform)>,
) {
    let transform = |platform: &dreamwake_sim::replication::PublicPlatformView| {
        Transform::from_translation(Vec3::from_array(platform.pose.position))
            .with_rotation(Quat::from_array(platform.pose.rotation))
            .with_scale(Vec3::from_array(platform.half_extents) * 2.0)
    };
    for (entity, platform, mut current) in &mut existing {
        if let Some(state) = view.0.platforms.iter().find(|state| state.id == platform.0) {
            let next = transform(state);
            if *current != next {
                *current = next;
            }
        } else {
            commands.entity(entity).despawn();
        }
    }
    for state in &view.0.platforms {
        if existing
            .iter()
            .any(|(_, platform, _)| platform.0 == state.id)
        {
            continue;
        }
        let parent = commands
            .spawn((
                SceneGraphic,
                PlatformVisual(state.id),
                Mesh3d(art.cube.clone()),
                MeshMaterial3d(art.friendly_fill.clone()),
                transform(state),
                Visibility::Inherited,
            ))
            .id();
        let stripe = piece(
            &mut commands,
            &art.cube,
            &art.gold_glow,
            Vec3::new(0.0, 0.51, 0.0),
            Vec3::new(0.96, 0.035, 0.08),
            Quat::IDENTITY,
        );
        commands.entity(stripe).insert(ChildOf(parent));
    }
}

pub(super) fn sync_covers(
    mut commands: Commands,
    view: Res<DreamView>,
    art: Res<SceneArt>,
    mut existing: Query<(Entity, &CoverVisual, &mut Transform)>,
) {
    for (entity, cover, mut transform) in &mut existing {
        if let Some(state) = view.0.covers.iter().find(|state| state.id == cover.0) {
            let next = cover_transform(state);
            if *transform != next {
                *transform = next;
            }
        } else {
            commands.entity(entity).despawn();
        }
    }
    for state in &view.0.covers {
        if existing.iter().any(|(_, cover, _)| cover.0 == state.id) {
            continue;
        }
        commands.spawn((
            SceneGraphic,
            CoverVisual(state.id),
            Mesh3d(art.cube.clone()),
            MeshMaterial3d(art.friendly_fill.clone()),
            cover_transform(state),
            Visibility::Inherited,
        ));
    }
}

fn cover_transform(state: &dreamwake_sim::replication::PublicCoverView) -> Transform {
    Transform::from_translation(Vec3::from_array(state.position))
        .with_scale(Vec3::from_array(state.half_extents) * 2.0)
}

pub(super) fn sync(
    mut commands: Commands,
    view: Res<DreamView>,
    art: Res<SceneArt>,
    mut meshes: ResMut<Assets<Mesh>>,
    existing: Query<(Entity, &CollisionVisual)>,
) {
    let manifest = view.0.collision.as_ref();
    if existing.iter().count() == usize::from(manifest.is_some())
        && existing
            .iter()
            .all(|(_, current)| Some(current.0.as_ref()) == manifest.map(Arc::as_ref))
    {
        return;
    }
    for (entity, _) in &existing {
        commands.entity(entity).despawn();
    }
    let Some(manifest) = manifest else { return };
    let root = commands
        .spawn((
            SceneGraphic,
            CollisionVisual(manifest.clone()),
            Transform::default(),
            Visibility::Inherited,
        ))
        .id();
    for collider in manifest.colliders() {
        let position = Vec3::from_array(collider.position);
        let rotation = Quat::from_array(collider.rotation).normalize();
        let (mesh, scale) = match collider.shape {
            CollisionShape::Box { half_extents } => {
                let half = Vec3::from_array(half_extents);
                // The arena's plinth already renders the horizontal floor.
                // Raised geometry remains visible, including future platforms.
                if rotation == Quat::IDENTITY && position.y + half.y <= 0.001 {
                    continue;
                }
                let trim = piece(
                    &mut commands,
                    &art.cube,
                    &art.gold_glow,
                    position + rotation * Vec3::Y * (-half.y + 0.04),
                    Vec3::new(half.x * 2.0, 0.08, half.z * 2.0),
                    rotation,
                );
                commands.entity(trim).insert(ChildOf(root));
                (art.cube.clone(), half * 2.0)
            }
            CollisionShape::Ball { radius } => (art.sphere.clone(), Vec3::splat(radius)),
            CollisionShape::Capsule {
                half_segment,
                radius,
            } => (
                meshes.add(Capsule3d::new(radius, half_segment * 2.0)),
                Vec3::ONE,
            ),
        };
        let surface = piece(
            &mut commands,
            &mesh,
            &art.friendly_fill,
            position,
            scale,
            rotation,
        );
        commands.entity(surface).insert(ChildOf(root));
    }
}

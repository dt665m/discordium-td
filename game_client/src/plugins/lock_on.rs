use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use game_shared::distance_sq;

const DEFAULT_MARKER_Y_OFFSET: f32 = 1.34;
const DEFAULT_MARKER_SCALE: f32 = 0.26;

#[derive(Component, Clone, Copy, Default)]
pub struct LockTarget {
    pub target_id: Option<u64>,
}

#[derive(Component, Clone, Copy, Default)]
pub struct LockMode {
    pub active: bool,
}

#[derive(Component, Clone, Copy)]
pub struct LockableTarget {
    pub id: u64,
}

#[derive(Component, Clone, Copy)]
pub struct LockMarkerSource;

#[derive(Component, Clone, Copy)]
pub struct LockMarkerConfig {
    pub y_offset: f32,
    pub scale: f32,
}

impl Default for LockMarkerConfig {
    fn default() -> Self {
        Self {
            y_offset: DEFAULT_MARKER_Y_OFFSET,
            scale: DEFAULT_MARKER_SCALE,
        }
    }
}

#[derive(Component, Clone, Copy)]
pub(crate) struct LockMarkerVisual;

#[derive(Resource)]
pub(crate) struct LockOnAssets {
    marker_mesh: Handle<Mesh>,
    marker_material: Handle<StandardMaterial>,
}

#[derive(Resource, Default)]
pub(crate) struct LockMarkerRegistry {
    by_source: HashMap<Entity, Entity>,
}

pub struct LockOnPlugin;

impl Plugin for LockOnPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LockMarkerRegistry>()
            .add_systems(Startup, setup_lock_on_assets);
    }
}

fn setup_lock_on_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let marker_mesh = meshes.add(Cuboid::new(1.0, 0.2, 1.0));
    let marker_material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.9, 0.2, 0.92),
        emissive: LinearRgba::new(0.35, 0.28, 0.02, 0.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..Default::default()
    });
    commands.insert_resource(LockOnAssets {
        marker_mesh,
        marker_material,
    });
}

pub fn sync_lock_markers(
    mut commands: Commands,
    assets: Option<Res<LockOnAssets>>,
    mut registry: ResMut<LockMarkerRegistry>,
    sources: Query<
        (
            Entity,
            &LockTarget,
            Option<&LockMode>,
            Option<&LockMarkerConfig>,
        ),
        With<LockMarkerSource>,
    >,
    targets: Query<(&LockableTarget, &Transform), Without<LockMarkerVisual>>,
    mut marker_transforms: Query<&mut Transform, (With<LockMarkerVisual>, Without<LockableTarget>)>,
) {
    let Some(assets) = assets else {
        return;
    };

    let target_positions: HashMap<u64, Vec3> = targets
        .iter()
        .map(|(target, transform)| (target.id, transform.translation))
        .collect();

    let mut active_sources = HashSet::new();
    for (source_entity, lock_target, lock_mode, marker_config) in &sources {
        active_sources.insert(source_entity);
        let lock_mode_active = lock_mode.map(|mode| mode.active).unwrap_or(true);
        let target_translation = if lock_mode_active {
            lock_target
                .target_id
                .and_then(|target_id| target_positions.get(&target_id).copied())
        } else {
            None
        };

        let config = marker_config.copied().unwrap_or_default();

        if let Some(marker_entity) = registry.by_source.get(&source_entity).copied() {
            if let Ok(mut marker_transform) = marker_transforms.get_mut(marker_entity) {
                if let Some(target_translation) = target_translation {
                    marker_transform.translation =
                        target_translation + Vec3::new(0.0, config.y_offset, 0.0);
                    marker_transform.rotation = Quat::IDENTITY;
                    marker_transform.scale = Vec3::splat(config.scale);
                    continue;
                }

                commands.entity(marker_entity).despawn();
                registry.by_source.remove(&source_entity);
                continue;
            }

            registry.by_source.remove(&source_entity);
        }

        let Some(target_translation) = target_translation else {
            continue;
        };

        let marker_entity = commands
            .spawn((
                Mesh3d(assets.marker_mesh.clone()),
                MeshMaterial3d(assets.marker_material.clone()),
                Transform::from_translation(
                    target_translation + Vec3::new(0.0, config.y_offset, 0.0),
                )
                .with_scale(Vec3::splat(config.scale)),
                LockMarkerVisual,
            ))
            .id();
        registry.by_source.insert(source_entity, marker_entity);
    }

    let stale_sources: Vec<Entity> = registry
        .by_source
        .keys()
        .copied()
        .filter(|source| !active_sources.contains(source))
        .collect();

    for stale_source in stale_sources {
        if let Some(marker_entity) = registry.by_source.remove(&stale_source) {
            commands.entity(marker_entity).despawn();
        }
    }
}

pub fn select_next_lock_target(
    current_target_id: Option<u64>,
    source_pos: [f32; 2],
    candidates: impl IntoIterator<Item = (u64, [f32; 2])>,
) -> Option<u64> {
    let mut sorted_candidates: Vec<(u64, f32)> = candidates
        .into_iter()
        .map(|(id, pos)| (id, distance_sq(source_pos, pos)))
        .collect();
    sorted_candidates.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    if sorted_candidates.is_empty() {
        return None;
    }

    if let Some(current_target_id) = current_target_id {
        if let Some(index) = sorted_candidates
            .iter()
            .position(|(candidate_id, _)| *candidate_id == current_target_id)
        {
            // Pressing cycle should snap to the nearest other target, not rotate through all.
            if sorted_candidates.len() == 1 {
                return Some(sorted_candidates[index].0);
            }
            if let Some((target_id, _)) = sorted_candidates
                .iter()
                .find(|(candidate_id, _)| *candidate_id != current_target_id)
            {
                return Some(*target_id);
            }
            return Some(sorted_candidates[index].0);
        }
    }

    Some(sorted_candidates[0].0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_next_lock_target_picks_nearest_then_id() {
        let selected = select_next_lock_target(
            None,
            [0.0, 0.0],
            vec![(30, [3.0, 0.0]), (10, [1.0, 0.0]), (20, [1.0, 0.0])],
        );
        assert_eq!(selected, Some(10));
    }

    #[test]
    fn select_next_lock_target_prefers_nearest_other_target() {
        let candidates = vec![(10, [1.0, 0.0]), (20, [2.0, 0.0]), (30, [3.0, 0.0])];
        assert_eq!(
            select_next_lock_target(Some(20), [0.0, 0.0], candidates.clone()),
            Some(10)
        );
        assert_eq!(
            select_next_lock_target(Some(10), [0.0, 0.0], candidates),
            Some(20)
        );
    }

    #[test]
    fn select_next_lock_target_uses_nearest_when_current_missing() {
        let selected = select_next_lock_target(
            Some(999),
            [0.0, 0.0],
            vec![(7, [4.0, 0.0]), (2, [1.0, 0.0])],
        );
        assert_eq!(selected, Some(2));
    }

    #[test]
    fn select_next_lock_target_keeps_only_target_when_single_candidate() {
        let selected = select_next_lock_target(Some(5), [0.0, 0.0], vec![(5, [2.0, 0.0])]);
        assert_eq!(selected, Some(5));
    }
}

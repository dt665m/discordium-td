//! The single reconciliation-aware lifecycle for simulation-owned visuals.
//! Mechanics add simulation state plus a renderer here, never a receive handler.
use super::*;
use game_shared::{PresentationId, PresentationKind};

#[derive(Component)]
pub(super) struct PresentedInstance {
    id: PresentationId,
    absent_seconds: f32,
}

pub(super) fn sync_simulation_presentations(
    mut commands: Commands,
    time: Res<Time>,
    local: Res<LocalSimulation>,
    runtime: Option<NonSend<NetworkRuntime>>,
    assets: Res<SceneAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut visuals: Query<(
        Entity,
        &mut PresentedInstance,
        &mut AbilityEffectVisual,
        &mut Transform,
    )>,
) {
    let Some(runtime) = runtime.filter(|_| local.initialized) else {
        for (entity, ..) in &visuals {
            commands.entity(entity).despawn();
        }
        return;
    };
    let instances = local.sim.presentations();
    for instance in instances {
        if !visuals
            .iter()
            .any(|(_, visual, ..)| visual.id == instance.id)
        {
            // Exhaustive match makes a new primitive require a deliberate renderer.
            let entity = match instance.kind {
                PresentationKind::ArcBurst => {
                    spawn_ability_effect(&mut commands, &mut materials, &assets, instance)
                }
            };
            commands.entity(entity).insert(PresentedInstance {
                id: instance.id,
                absent_seconds: 0.0,
            });
        }
    }
    for (entity, mut visual, mut effect, mut transform) in &mut visuals {
        if visual.id.match_epoch != runtime.match_epoch {
            commands.entity(entity).despawn();
            continue;
        }
        effect.age_seconds += time.delta_secs();
        let instance = instances.iter().find(|i| i.id == visual.id);
        if let Some(instance) = instance {
            visual.absent_seconds = 0.0;
            // Replaying the same ID must not restart an already presented effect.
            effect.age_seconds = effect
                .age_seconds
                .max(instance.age_ticks as f32 * FIXED_DT_SECONDS);
            effect.duration_seconds = instance.duration_ticks as f32 * FIXED_DT_SECONDS;
            effect.max_radius = instance.radius;
            let target = world_to_translation(instance.pos, 0.12);
            transform.translation = transform
                .translation
                .lerp(target, 1.0 - (-18.0 * time.delta_secs()).exp());
        } else {
            visual.absent_seconds += time.delta_secs();
        }
        let t = (effect.age_seconds / effect.duration_seconds).clamp(0.0, 1.0);
        let radius =
            ABILITY_EFFECT_START_RADIUS + (effect.max_radius - ABILITY_EFFECT_START_RADIUS) * t;
        transform.scale = Vec3::new(radius, 1.0, radius);
        transform.translation.y = 0.12 + 0.12 * t;
        let correction_fade = (1.0 - visual.absent_seconds / 0.12).clamp(0.0, 1.0);
        if let Some(mut material) = materials.get_mut(&effect.material) {
            material.base_color = Color::srgba(0.25, 0.88, 1.0, 0.46 * (1.0 - t) * correction_fade);
            material.emissive =
                LinearRgba::new(0.08, 0.4, 0.6, 0.0) * ((1.0 - t) * correction_fade);
        }
        // Briefly retain an invisible completed instance across corrections.
        if visual.absent_seconds > 1.0 {
            commands.entity(entity).despawn();
        }
    }
}

#[derive(Component)]
pub(super) struct AbilityEffectVisual {
    age_seconds: f32,
    duration_seconds: f32,
    max_radius: f32,
    material: Handle<StandardMaterial>,
}

fn spawn_ability_effect(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    scene_assets: &SceneAssets,
    instance: &game_shared::PresentationInstance,
) -> Entity {
    let age = instance.age_ticks as f32 * FIXED_DT_SECONDS;
    let duration = instance.duration_ticks as f32 * FIXED_DT_SECONDS;
    let t = (age / duration).clamp(0.0, 1.0);
    let radius = ABILITY_EFFECT_START_RADIUS + (instance.radius - ABILITY_EFFECT_START_RADIUS) * t;
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(0.25, 0.88, 1.0, 0.46 * (1.0 - t)),
        emissive: LinearRgba::new(0.08, 0.4, 0.6, 0.0) * (1.0 - t),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..Default::default()
    });

    commands
        .spawn((
            Mesh3d(scene_assets.ability_effect_mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(world_to_translation(instance.pos, 0.12 + 0.12 * t))
                .with_scale(Vec3::new(radius, 1.0, radius)),
            AbilityEffectVisual {
                age_seconds: age,
                duration_seconds: duration,
                max_radius: instance.radius,
                material,
            },
        ))
        .id()
}

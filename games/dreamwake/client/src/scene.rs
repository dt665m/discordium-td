//! Procedural game graphics for Dreamwake. Gameplay lifetimes remain in the simulation.
use std::{
    collections::HashSet,
    f32::consts::{FRAC_PI_2, PI, TAU},
    time::Duration,
};

use bevy::{
    asset::RenderAssetUsages,
    math::StableInterpolate,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    transform::helper::TransformHelper,
};

use super::DreamView;
use dreamwake_sim::{DreamSnapshot, EnemyKind, EssenceKind, RunPhase, TICK_HZ};
use engine_core::PresentationId;

/// Roots owned by the game renderer; hiding them also hides their mesh children.
#[derive(Component, Clone, Default)]
pub(super) struct SceneGraphic;

pub struct DreamScenePlugin;
impl Plugin for DreamScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_scene).add_systems(
            Update,
            (sync_scene, animate_scene)
                .chain()
                .in_set(engine_client::presentation::PresentationSet::Render),
        );
        add_label_placement_systems(app);
    }
}

fn add_label_placement_systems(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (place_damage_numbers, place_traveler_labels)
            .chain()
            // UI layout precedes transform propagation in Bevy 0.19.
            // Compute current transforms locally so Node/Text changes
            // reach this frame's layout, using the updated projection.
            .after(bevy::camera::CameraUpdateSystems)
            .before(bevy::ui::UiSystems::Content),
    );
}

use engine_client::camera::CameraRig as DreamCameraRig;

#[derive(Resource)]
struct SceneArt {
    cube: Handle<Mesh>,
    sphere: Handle<Mesh>,
    cone: Handle<Mesh>,
    cylinder: Handle<Mesh>,
    ring: Handle<Mesh>,
    disk: Handle<Mesh>,
    crystal: Handle<Mesh>,
    crescent: Handle<Mesh>,
    ground: Handle<StandardMaterial>,
    stone: Handle<StandardMaterial>,
    cliff: Handle<StandardMaterial>,
    tile: Handle<StandardMaterial>,
    gold: Handle<StandardMaterial>,
    ivory: Handle<StandardMaterial>,
    teal: Handle<StandardMaterial>,
    dark: Handle<StandardMaterial>,
    red: Handle<StandardMaterial>,
    violet: Handle<StandardMaterial>,
    glow: Handle<StandardMaterial>,
    gold_glow: Handle<StandardMaterial>,
    coral: Handle<StandardMaterial>,
    purple_glow: Handle<StandardMaterial>,
    ice_glow: Handle<StandardMaterial>,
    healing_glow: Handle<StandardMaterial>,
    flora_glow: Handle<StandardMaterial>,
    flora_leaf: Handle<StandardMaterial>,
    shadow: Handle<StandardMaterial>,
    warning_fill: Handle<StandardMaterial>,
    friendly_fill: Handle<StandardMaterial>,
    realm: usize,
}

#[derive(Component)]
struct Drift {
    base: Vec3,
    phase: f32,
    amplitude: f32,
    speed: f32,
    spin: f32,
}

#[derive(Component)]
struct GateRing;

fn material(
    materials: &mut Assets<StandardMaterial>,
    color: [f32; 3],
    metal: f32,
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::srgb(color[0], color[1], color[2]),
        metallic: metal,
        perceptual_roughness: 0.72,
        ..default()
    })
}

fn glow_material(
    materials: &mut Assets<StandardMaterial>,
    color: [f32; 3],
    strength: f32,
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::srgb(color[0], color[1], color[2]),
        emissive: LinearRgba::rgb(
            color[0] * strength,
            color[1] * strength,
            color[2] * strength,
        ),
        unlit: true,
        ..default()
    })
}

fn transparent(
    materials: &mut Assets<StandardMaterial>,
    color: [f32; 4],
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::srgba(color[0], color[1], color[2], color[3]),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    })
}

/// Static pieces use BSN scene composition; actor/effect ownership uses ECS below.
fn stone_piece(
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    transform: Transform,
) -> impl Scene {
    bsn! { SceneGraphic Mesh3d(mesh) MeshMaterial3d::<StandardMaterial>(material) Transform {
        translation: {transform.translation}, rotation: {transform.rotation}, scale: {transform.scale}
    } }
}

fn piece(
    commands: &mut Commands,
    mesh: &Handle<Mesh>,
    mat: &Handle<StandardMaterial>,
    pos: Vec3,
    scale: Vec3,
    rot: Quat,
) -> Entity {
    commands
        .spawn_scene(stone_piece(
            mesh.clone(),
            mat.clone(),
            Transform::from_translation(pos)
                .with_scale(scale)
                .with_rotation(rot),
        ))
        .id()
}

fn ground_ring(
    commands: &mut Commands,
    art: &SceneArt,
    pos: Vec3,
    radius: f32,
    mat: &Handle<StandardMaterial>,
) -> Entity {
    piece(
        commands,
        &art.ring,
        mat,
        pos,
        Vec3::splat(radius),
        Quat::from_rotation_x(-FRAC_PI_2),
    )
}

fn crescent_mesh() -> Mesh {
    let mut p = Vec::new();
    let mut n = Vec::new();
    let mut uv = Vec::new();
    let mut indices = Vec::new();
    for i in 0..=32 {
        let a = -2.1 + i as f32 / 32.0 * 4.2;
        let outer = Vec3::new(a.cos(), a.sin(), 0.0);
        let inner = Vec3::new(a.cos() * 0.75 + 0.16, a.sin() * 0.82, 0.0);
        p.extend([outer.to_array(), inner.to_array()]);
        n.extend([[0.0, 0.0, 1.0]; 2]);
        uv.extend([[0.0, 0.0]; 2]);
        if i < 32 {
            let j = i * 2;
            indices.extend([j, j + 2, j + 1, j + 1, j + 2, j + 3]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, p)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, n)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
    .with_inserted_indices(Indices::U32(indices))
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let art = SceneArt {
        cube: meshes.add(Cuboid::default()),
        sphere: meshes.add(Sphere::new(1.0).mesh().ico(2).unwrap()),
        cone: meshes.add(Cone::new(1.0, 1.0).mesh().resolution(8)),
        cylinder: meshes.add(Cylinder::new(1.0, 1.0).mesh().resolution(64)),
        ring: meshes.add(Annulus::new(0.975, 1.0).mesh().resolution(64)),
        disk: meshes.add(Circle::new(1.0).mesh().resolution(48)),
        crystal: meshes.add(Sphere::new(1.0).mesh().ico(0).unwrap()),
        crescent: meshes.add(crescent_mesh()),
        ground: material(&mut materials, [0.075, 0.19, 0.20], 0.12),
        stone: material(&mut materials, [0.19, 0.31, 0.32], 0.1),
        cliff: material(&mut materials, [0.055, 0.10, 0.15], 0.2),
        tile: material(&mut materials, [0.105, 0.23, 0.24], 0.18),
        gold: material(&mut materials, [0.62, 0.46, 0.23], 0.7),
        ivory: material(&mut materials, [0.96, 0.89, 0.73], 0.2),
        teal: material(&mut materials, [0.035, 0.50, 0.47], 0.2),
        dark: material(&mut materials, [0.018, 0.025, 0.048], 0.15),
        red: material(&mut materials, [0.54, 0.13, 0.18], 0.22),
        violet: material(&mut materials, [0.28, 0.12, 0.41], 0.3),
        glow: glow_material(&mut materials, [0.24, 0.95, 0.79], 2.0),
        gold_glow: glow_material(&mut materials, [1.0, 0.69, 0.24], 1.5),
        coral: glow_material(&mut materials, [1.0, 0.19, 0.22], 1.4),
        purple_glow: glow_material(&mut materials, [0.73, 0.36, 1.0], 1.5),
        ice_glow: glow_material(&mut materials, [0.35, 0.72, 1.0], 1.8),
        healing_glow: glow_material(&mut materials, [0.56, 1.0, 0.34], 1.5),
        flora_glow: glow_material(&mut materials, [0.24, 0.95, 0.79], 1.6),
        flora_leaf: material(&mut materials, [0.04, 0.40, 0.36], 0.1),
        shadow: transparent(&mut materials, [0.012, 0.021, 0.034, 0.44]),
        warning_fill: transparent(&mut materials, [0.95, 0.07, 0.12, 0.18]),
        friendly_fill: transparent(&mut materials, [0.15, 0.95, 0.72, 0.12]),
        realm: usize::MAX,
    };
    commands.insert_resource(ClearColor(Color::srgb(0.016, 0.025, 0.055)));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.57, 0.72, 0.91),
        brightness: 430.0,
        ..default()
    });
    commands.spawn((
        SceneGraphic,
        DirectionalLight {
            color: Color::srgb(0.77, 0.88, 1.0),
            illuminance: 6500.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-14.0, 25.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        SceneGraphic,
        PointLight {
            color: Color::srgb(0.13, 0.95, 0.74),
            intensity: 2_800_000.0,
            range: 26.0,
            ..default()
        },
        Transform::from_xyz(-10.0, 7.0, -10.0),
    ));
    commands.spawn((
        SceneGraphic,
        PointLight {
            color: Color::srgb(0.86, 0.4, 0.62),
            intensity: 1_900_000.0,
            range: 25.0,
            ..default()
        },
        Transform::from_xyz(13.0, 7.0, 2.0),
    ));

    // Layered plinth and deliberately uneven cliff silhouette.
    piece(
        &mut commands,
        &art.cylinder,
        &art.ground,
        Vec3::new(0.0, -0.42, 0.0),
        Vec3::new(20.5, 0.85, 20.5),
        Quat::IDENTITY,
    );
    piece(
        &mut commands,
        &art.cylinder,
        &art.cliff,
        Vec3::new(0.0, -1.3, 0.0),
        Vec3::new(19.9, 1.4, 19.9),
        Quat::IDENTITY,
    );
    piece(
        &mut commands,
        &art.cone,
        &art.cliff,
        Vec3::new(0.0, -4.0, 0.0),
        Vec3::new(19.3, 5.0, 19.3),
        Quat::from_rotation_z(PI),
    );
    for i in 0..56 {
        let a = i as f32 / 56.0 * TAU;
        let radial = Vec3::new(a.cos(), 0.0, a.sin());
        let height = 1.2 + noise(i as u32 * 9) * 2.0;
        piece(
            &mut commands,
            &art.cube,
            &art.stone,
            radial * 20.0 + Vec3::Y * -0.5,
            Vec3::new(1.8, 1.0, 0.75),
            Quat::from_rotation_y(-a + FRAC_PI_2),
        );
        piece(
            &mut commands,
            &art.crystal,
            &art.cliff,
            radial * 19.5 + Vec3::Y * -2.0,
            Vec3::new(2.3, height, 1.8),
            Quat::from_rotation_y(a),
        );
    }
    for r in [4.4, 9.0, 14.0, 19.1] {
        ground_ring(&mut commands, &art, Vec3::Y * 0.012, r, &art.gold);
    }
    for r in [4.65, 14.24, 19.35] {
        ground_ring(&mut commands, &art, Vec3::Y * 0.016, r, &art.tile);
    }
    for i in 0..48 {
        let a = i as f32 / 48.0 * TAU;
        let r = if i % 4 == 0 { 13.65 } else { 18.65 };
        piece(
            &mut commands,
            &art.cube,
            &art.gold,
            Vec3::new(a.cos() * r, 0.022, a.sin() * r),
            Vec3::new(0.045, 0.026, if i % 4 == 0 { 0.65 } else { 0.28 }),
            Quat::from_rotation_y(-a + FRAC_PI_2),
        );
    }
    // Broad radial slabs articulate the arena without obstructing movement.
    for spoke in 0..8 {
        let a = spoke as f32 * TAU / 8.0;
        for j in 0..5 {
            let r = 6.1 + j as f32 * 2.35;
            piece(
                &mut commands,
                &art.cube,
                &art.tile,
                Vec3::new(a.sin() * r, 0.015, a.cos() * r),
                Vec3::new(1.75, 0.022, 1.96),
                Quat::from_rotation_y(a + 0.03 * (j % 2) as f32),
            );
        }
    }
    ground_ring(&mut commands, &art, Vec3::Y * 0.035, 1.9, &art.gold);
    for i in 0..8 {
        let a = i as f32 * TAU / 8.0;
        piece(
            &mut commands,
            &art.cube,
            &art.gold,
            Vec3::new(a.cos() * 2.7, 0.03, a.sin() * 2.7),
            Vec3::new(0.07, 0.035, 0.6),
            Quat::from_rotation_y(-a + FRAC_PI_2),
        );
    }

    // Impossible moon gate: a gilded broken halo over the far edge of the island.
    for side in [-1.0, 1.0] {
        let base = Vec3::new(side * 5.4, 0.0, -15.8);
        for k in 0..3 {
            piece(
                &mut commands,
                &art.cube,
                &art.stone,
                base + Vec3::Y * (0.27 + k as f32 * 0.30),
                Vec3::new(2.7 - k as f32 * 0.3, 0.3, 2.7 - k as f32 * 0.3),
                Quat::IDENTITY,
            );
        }
        piece(
            &mut commands,
            &art.cube,
            &art.stone,
            base + Vec3::Y * 3.1,
            Vec3::new(1.05, 4.8, 1.05),
            Quat::IDENTITY,
        );
        for dy in [1.2, 4.8, 5.4] {
            piece(
                &mut commands,
                &art.cube,
                &art.gold,
                base + Vec3::Y * dy,
                Vec3::new(1.2, 0.14, 1.2),
                Quat::IDENTITY,
            );
        }
        piece(
            &mut commands,
            &art.crystal,
            &art.glow,
            base + Vec3::Y * 6.0,
            Vec3::new(0.32, 0.7, 0.32),
            Quat::IDENTITY,
        );
    }
    for i in 0..25 {
        if i == 5 || i == 18 {
            continue;
        }
        let a = i as f32 / 24.0 * PI;
        let center = Vec3::new(a.cos() * 5.45, 4.35 + a.sin() * 4.6, -15.8);
        let id = piece(
            &mut commands,
            &art.cube,
            if i % 3 == 0 { &art.gold } else { &art.stone },
            center,
            Vec3::new(0.74, 0.84, 0.85),
            Quat::from_rotation_z(a - FRAC_PI_2),
        );
        if i == 4 || i == 17 {
            commands.entity(id).insert(Drift {
                base: center,
                phase: a,
                amplitude: 0.12,
                speed: 0.8,
                spin: 0.015,
            });
        }
    }
    piece(
        &mut commands,
        &art.crescent,
        &art.gold_glow,
        Vec3::new(0.0, 5.1, -16.1),
        Vec3::splat(2.0),
        Quat::from_rotation_z(0.7),
    );
    let gate = piece(
        &mut commands,
        &art.ring,
        &art.glow,
        Vec3::new(0.0, 4.8, -16.2),
        Vec3::splat(3.1),
        Quat::IDENTITY,
    );
    commands.entity(gate).insert(GateRing);

    for i in 0..14 {
        let a = i as f32 / 14.0 * TAU + 0.11;
        let r = 18.0 + noise(i * 47) * 1.2;
        crystal_garden(
            &mut commands,
            &art,
            Vec3::new(a.cos() * r, 0.0, a.sin() * r),
            i,
        );
    }
    // Wind-sculpted dream trees stay at the boundary, outside the playable radius.
    for (i, a) in [-2.8_f32, -0.4, 0.45, 2.50].into_iter().enumerate() {
        let base = Vec3::new(a.cos() * 19.1, 0.0, a.sin() * 19.1);
        piece(
            &mut commands,
            &art.cone,
            &art.dark,
            base + Vec3::Y * 1.25,
            Vec3::new(0.26, 2.6, 0.22),
            Quat::from_rotation_z(0.18 * a.sin()),
        );
        for branch in 0..4 {
            let b = a + branch as f32 * 1.70;
            let center =
                base + Vec3::new(b.cos() * 0.74, 1.8 + branch as f32 * 0.39, b.sin() * 0.74);
            piece(
                &mut commands,
                &art.cube,
                &art.dark,
                center - Vec3::Y * 0.25,
                Vec3::new(0.10, 0.10, 1.35),
                Quat::from_euler(EulerRot::XYZ, -0.5, -b + FRAC_PI_2, 0.0),
            );
            let id = piece(
                &mut commands,
                &art.crystal,
                &art.flora_leaf,
                center,
                Vec3::new(1.0, 0.31, 0.65),
                Quat::from_rotation_y(b),
            );
            commands.entity(id).insert(Drift {
                base: center,
                phase: b,
                amplitude: 0.035,
                speed: 1.4,
                spin: 0.0,
            });
            for leaf in 0..3 {
                let offset = b + leaf as f32 * 2.1;
                let pos = center
                    + Vec3::new(
                        offset.cos() * 0.65,
                        -0.22 - leaf as f32 * 0.13,
                        offset.sin() * 0.38,
                    );
                let id = piece(
                    &mut commands,
                    &art.crystal,
                    &art.flora_glow,
                    pos,
                    Vec3::new(0.055, 0.18, 0.055),
                    Quat::IDENTITY,
                );
                commands.entity(id).insert(Drift {
                    base: pos,
                    phase: offset + i as f32,
                    amplitude: 0.08,
                    speed: 1.2,
                    spin: 0.1,
                });
            }
        }
    }
    // Fragmented walkways and distant relics frame the playing surface.
    for i in 0..24 {
        let a = i as f32 * 2.399_963;
        let r = 24.0 + noise(i * 13) * 10.0;
        let pos = Vec3::new(a.cos() * r, -0.5 - noise(i * 11) * 4.0, a.sin() * r);
        let size = 0.7 + noise(i * 7) * 2.7;
        let id = piece(
            &mut commands,
            &art.cube,
            &art.stone,
            pos,
            Vec3::new(size, 0.4, size * 0.8),
            Quat::from_rotation_y(a),
        );
        commands.entity(id).insert(Drift {
            base: pos,
            phase: a,
            amplitude: 0.2,
            speed: 0.5,
            spin: 0.008,
        });
        piece(
            &mut commands,
            &art.crystal,
            &art.cliff,
            pos - Vec3::Y * 0.9,
            Vec3::new(size * 0.72, 1.2, size * 0.6),
            Quat::from_rotation_y(a),
        );
    }
    // Soft cloud banks, low in the void so they never cover combat.
    let mist = transparent(&mut materials, [0.13, 0.26, 0.33, 0.075]);
    for i in 0..14 {
        let a = i as f32 * 2.4;
        let pos = Vec3::new(a.cos() * 29.0, -7.0 - noise(i) * 4.0, a.sin() * 29.0);
        let id = piece(
            &mut commands,
            &art.sphere,
            &mist,
            pos,
            Vec3::new(12.0, 0.3, 6.0),
            Quat::from_rotation_y(a),
        );
        commands.entity(id).insert(Drift {
            base: pos,
            phase: a,
            amplitude: 0.5,
            speed: 0.14,
            spin: 0.005,
        });
    }
    for i in 0..180 {
        let a = i as f32 * 2.399_963;
        let r = 8.0 + noise(i * 31) * 44.0;
        let pos = Vec3::new(
            a.cos() * r,
            if r < 19.0 {
                0.3 + noise(i * 5) * 2.0
            } else {
                -5.0 + noise(i * 7) * 8.0
            },
            a.sin() * r,
        );
        let s = 0.018 + noise(i * 3) * 0.036;
        let id = piece(
            &mut commands,
            &art.sphere,
            if i % 5 == 0 {
                &art.gold_glow
            } else {
                &art.flora_glow
            },
            pos,
            Vec3::splat(s),
            Quat::IDENTITY,
        );
        commands.entity(id).insert(Drift {
            base: pos,
            phase: a,
            amplitude: 0.3,
            speed: 0.3 + noise(i) * 0.7,
            spin: 0.0,
        });
    }
    commands.insert_resource(art);
}

fn noise(seed: u32) -> f32 {
    let n = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let n = ((n >> ((n >> 28) + 4)) ^ n).wrapping_mul(277_803_737);
    ((n >> 22) ^ n) as f32 / u32::MAX as f32
}

fn crystal_garden(commands: &mut Commands, art: &SceneArt, base: Vec3, seed: u32) {
    piece(
        commands,
        &art.sphere,
        &art.dark,
        base + Vec3::Y * 0.04,
        Vec3::new(1.5, 0.13, 1.0),
        Quat::IDENTITY,
    );
    for i in 0..6 {
        let a = noise(seed * 37 + i) * TAU;
        let h = 0.5 + noise(seed * 91 + i * 3) * 1.5;
        let pos = base + Vec3::new(a.cos() * 0.65, h * 0.53, a.sin() * 0.65);
        piece(
            commands,
            &art.crystal,
            if i % 3 == 0 {
                &art.purple_glow
            } else {
                &art.glow
            },
            pos,
            Vec3::new(0.22, h * 0.63, 0.22),
            Quat::from_euler(EulerRot::XYZ, 0.18 * a.sin(), a, 0.2 * a.cos()),
        );
        for leaf in 0..3 {
            let b = a + leaf as f32 * 2.1;
            piece(
                commands,
                &art.cone,
                &art.flora_leaf,
                base + Vec3::new(b.cos() * 0.8, 0.3, b.sin() * 0.8),
                Vec3::new(0.16, 0.7, 0.07),
                Quat::from_euler(EulerRot::XYZ, 0.5 * b.sin(), b, 0.5 * b.cos()),
            );
        }
    }
}

const ACTOR_SAMPLE_GRACE_SECS: f32 = 0.15;

#[derive(Component)]
struct ActorVisual {
    id: u64,
    kind: Option<EnemyKind>,
    local: bool,
    motion: f32,
    gait_phase: f32,
    target_motion: f32,
    last_position: Vec3,
    last_tick: u32,
    sample_age: Timer,
    seed: u64,
    room: usize,
    windup: f32,
    hit: f32,
    dash: f32,
    attack: f32,
    shield: bool,
    slowed: bool,
    hp: f32,
}

impl ActorVisual {
    /// Sample simulation motion, never the renderer's remaining correction distance.
    fn sample_target(
        &mut self,
        position: Vec3,
        seed: u64,
        room: usize,
        tick: u32,
        dt: Duration,
    ) -> bool {
        let reset = self.seed != seed
            || self.room != room
            || (tick == 0 && self.last_tick > 0)
            || self.last_position.distance_squared(position) > 16.0;
        if reset || tick != self.last_tick {
            self.sample_age.reset();
        } else {
            self.sample_age.tick(dt);
        }
        if reset {
            self.motion = 0.0;
            self.target_motion = 0.0;
            self.gait_phase = 0.0;
            self.attack = 0.0;
            self.hit = 0.0;
            self.windup = 0.0;
            self.dash = 0.0;
        } else if tick < self.last_tick {
            // Reconciliation can move back a few ticks without changing rooms.
            // Rebase the velocity sample, but retain the continuous visual pose.
            self.target_motion = 0.0;
        } else if tick > self.last_tick {
            self.target_motion = self.last_position.distance(position) * TICK_HZ as f32
                / (tick - self.last_tick) as f32;
        }
        if self.sample_age.is_finished() {
            self.target_motion = 0.0;
        }
        self.last_position = position;
        self.last_tick = tick;
        self.seed = seed;
        self.room = room;
        reset
    }

    fn advance_pose(
        &mut self,
        motion: f32,
        attack: f32,
        windup: f32,
        hit: f32,
        dash: f32,
        dt: f32,
    ) {
        self.motion.smooth_nudge(&motion.clamp(0.0, 14.0), 18.0, dt);
        // The phase belongs to this actor and stops with it instead of jumping to a
        // global clock's unrelated footfall when movement resumes.
        if self.motion < 0.001 && motion == 0.0 {
            self.motion = 0.0;
        }
        self.gait_phase = (self.gait_phase + self.motion * (11.0 / 7.0) * dt) % TAU;
        for (pose, target) in [
            (&mut self.attack, attack),
            (&mut self.windup, windup),
            (&mut self.hit, hit),
            (&mut self.dash, dash),
        ] {
            let response = if target > *pose { 50.0 } else { 18.0 };
            pose.smooth_nudge(&target, response, dt);
        }
    }
}

#[derive(Clone, Copy)]
enum PartKind {
    Body,
    Cape,
    LeftArm,
    RightArm,
    LeftLeg,
    RightLeg,
    Blade,
    Halo,
    Health,
    Shield,
    Slow,
    Eye,
}

#[derive(Component)]
struct ArtPart {
    actor: Entity,
    kind: PartKind,
    base: Transform,
}

fn part(
    commands: &mut Commands,
    parent: Entity,
    art: &SceneArt,
    kind: PartKind,
    mesh: &Handle<Mesh>,
    mat: &Handle<StandardMaterial>,
    pos: Vec3,
    scale: Vec3,
    rotation: Quat,
) {
    let _ = art;
    let base = Transform::from_translation(pos)
        .with_scale(scale)
        .with_rotation(rotation);
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(mat.clone()),
        base,
        ChildOf(parent),
        ArtPart {
            actor: parent,
            kind,
            base,
        },
    ));
}

fn fixed_part(
    commands: &mut Commands,
    parent: Entity,
    mesh: &Handle<Mesh>,
    mat: &Handle<StandardMaterial>,
    pos: Vec3,
    scale: Vec3,
    rotation: Quat,
) {
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(mat.clone()),
        Transform::from_translation(pos)
            .with_scale(scale)
            .with_rotation(rotation),
        ChildOf(parent),
    ));
}

fn spawn_actor(
    commands: &mut Commands,
    art: &SceneArt,
    id: u64,
    kind: Option<EnemyKind>,
    position: Vec3,
    local: bool,
    snapshot: &DreamSnapshot,
) {
    let parent = commands
        .spawn((
            SceneGraphic,
            Transform::from_translation(position),
            Visibility::default(),
            ActorVisual {
                id,
                kind,
                local,
                motion: 0.0,
                gait_phase: 0.0,
                target_motion: 0.0,
                last_position: position,
                last_tick: snapshot.tick,
                sample_age: Timer::from_seconds(ACTOR_SAMPLE_GRACE_SECS, TimerMode::Once),
                seed: snapshot.seed,
                room: snapshot.room,
                windup: 0.0,
                hit: 0.0,
                dash: 0.0,
                attack: 0.0,
                shield: false,
                slowed: false,
                hp: 1.0,
            },
        ))
        .id();
    let size = match kind {
        Some(EnemyKind::Boss) => 2.15,
        Some(EnemyKind::Elite) => 1.35,
        Some(EnemyKind::Ambusher) => 0.85,
        _ => 1.0,
    };
    fixed_part(
        commands,
        parent,
        &art.disk,
        &art.shadow,
        Vec3::Y * 0.035,
        Vec3::splat(0.63 * size),
        Quat::from_rotation_x(-FRAC_PI_2),
    );
    match kind {
        None => {
            // Local teal and stable ally accents distinguish the shared Traveler silhouette.
            let accent = if local {
                &art.glow
            } else {
                match id % 4 {
                    0 => &art.gold_glow,
                    1 => &art.purple_glow,
                    2 => &art.ice_glow,
                    _ => &art.healing_glow,
                }
            };
            let cloth = if local {
                &art.teal
            } else {
                match id % 4 {
                    0 => &art.gold,
                    1 => &art.violet,
                    2 => &art.stone,
                    _ => &art.teal,
                }
            };
            ground_child_ring(commands, parent, art, accent, 0.71, 0.045);
            if local {
                ground_child_ring(commands, parent, art, &art.ivory, 0.82, 0.05);
            }
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.cone,
                cloth,
                Vec3::Y * 0.91,
                Vec3::new(0.42, 0.91, 0.34),
                Quat::IDENTITY,
            );
            part(
                commands,
                parent,
                art,
                PartKind::Cape,
                &art.cone,
                &art.dark,
                Vec3::new(0.0, 0.80, 0.17),
                Vec3::new(0.50, 1.07, 0.31),
                Quat::from_rotation_x(-0.16),
            );
            for side in [-1.0, 1.0] {
                fixed_part(
                    commands,
                    parent,
                    &art.cube,
                    &art.gold,
                    Vec3::new(side * 0.14, 0.95, -0.28),
                    Vec3::new(0.045, 0.59, 0.028),
                    Quat::from_rotation_z(side * 0.09),
                );
            }
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.sphere,
                cloth,
                Vec3::new(0.0, 1.57, 0.01),
                Vec3::new(0.36, 0.40, 0.32),
                Quat::IDENTITY,
            );
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.sphere,
                &art.ivory,
                Vec3::new(0.0, 1.57, -0.22),
                Vec3::new(0.245, 0.30, 0.13),
                Quat::IDENTITY,
            );
            for side in [-1.0, 1.0] {
                part(
                    commands,
                    parent,
                    art,
                    PartKind::Eye,
                    &art.cube,
                    &art.dark,
                    Vec3::new(side * 0.092, 1.60, -0.342),
                    Vec3::new(0.035, 0.10, 0.024),
                    Quat::from_rotation_z(-side * 0.16),
                );
                part(
                    commands,
                    parent,
                    art,
                    if side < 0.0 {
                        PartKind::LeftArm
                    } else {
                        PartKind::RightArm
                    },
                    &art.cube,
                    cloth,
                    Vec3::new(side * 0.40, 1.03, -0.05),
                    Vec3::new(0.19, 0.62, 0.20),
                    Quat::from_rotation_z(side * 0.14),
                );
                part(
                    commands,
                    parent,
                    art,
                    if side < 0.0 {
                        PartKind::LeftLeg
                    } else {
                        PartKind::RightLeg
                    },
                    &art.sphere,
                    &art.dark,
                    Vec3::new(side * 0.18, 0.20, -0.05),
                    Vec3::new(0.14, 0.23, 0.19),
                    Quat::IDENTITY,
                );
            }
            part(
                commands,
                parent,
                art,
                PartKind::Blade,
                &art.cube,
                &art.gold,
                Vec3::new(0.49, 1.0, -0.25),
                Vec3::new(0.065, 0.085, 1.0),
                Quat::IDENTITY,
            );
            part(
                commands,
                parent,
                art,
                PartKind::Blade,
                &art.crescent,
                &art.gold_glow,
                Vec3::new(0.63, 0.99, -0.82),
                Vec3::splat(0.59),
                Quat::from_euler(EulerRot::XYZ, -0.45, -0.65, 0.0),
            );
            part(
                commands,
                parent,
                art,
                PartKind::Shield,
                &art.ring,
                accent,
                Vec3::Y * 1.0,
                Vec3::splat(0.99),
                Quat::from_rotation_x(-0.4),
            );
            part(
                commands,
                parent,
                art,
                PartKind::Shield,
                &art.ring,
                accent,
                Vec3::Y * 0.7,
                Vec3::splat(1.0),
                Quat::from_rotation_x(-FRAC_PI_2),
            );
        }
        Some(EnemyKind::Ranged) => {
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.crystal,
                &art.violet,
                Vec3::Y * 1.05,
                Vec3::new(0.45, 0.80, 0.45),
                Quat::IDENTITY,
            );
            part(
                commands,
                parent,
                art,
                PartKind::Eye,
                &art.crystal,
                &art.coral,
                Vec3::new(0.0, 1.15, -0.4),
                Vec3::splat(0.19),
                Quat::IDENTITY,
            );
            part(
                commands,
                parent,
                art,
                PartKind::Halo,
                &art.ring,
                &art.gold,
                Vec3::Y * 1.0,
                Vec3::splat(0.74),
                Quat::from_rotation_x(-0.5),
            );
            for side in [-1.0, 1.0] {
                part(
                    commands,
                    parent,
                    art,
                    if side < 0.0 {
                        PartKind::LeftArm
                    } else {
                        PartKind::RightArm
                    },
                    &art.crystal,
                    &art.violet,
                    Vec3::new(side * 0.64, 1.0, 0.0),
                    Vec3::new(0.15, 0.44, 0.19),
                    Quat::from_rotation_z(side * 0.35),
                );
            }
        }
        Some(EnemyKind::Support) => {
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.cube,
                &art.violet,
                Vec3::Y * 0.78,
                Vec3::new(0.40, 1.40, 0.4),
                Quat::from_rotation_y(0.8),
            );
            part(
                commands,
                parent,
                art,
                PartKind::Halo,
                &art.ring,
                &art.purple_glow,
                Vec3::Y * 1.60,
                Vec3::splat(0.8),
                Quat::IDENTITY,
            );
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.crystal,
                &art.ivory,
                Vec3::Y * 1.6,
                Vec3::new(0.18, 0.41, 0.18),
                Quat::IDENTITY,
            );
            for i in 0..4 {
                let a = i as f32 * FRAC_PI_2;
                part(
                    commands,
                    parent,
                    art,
                    PartKind::Halo,
                    &art.crystal,
                    &art.purple_glow,
                    Vec3::new(a.cos() * 0.72, 0.63, a.sin() * 0.72),
                    Vec3::splat(0.16),
                    Quat::IDENTITY,
                );
            }
        }
        Some(enemy) => {
            let boss = enemy == EnemyKind::Boss;
            let ambush = enemy == EnemyKind::Ambusher;
            let bodymat = if boss { &art.violet } else { &art.red };
            let h = if ambush { 0.73 } else { 1.0 };
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.cone,
                bodymat,
                Vec3::Y * (0.80 * size * h),
                Vec3::new(0.48 * size, 1.05 * size * h, 0.35 * size),
                Quat::IDENTITY,
            );
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.sphere,
                &art.dark,
                Vec3::Y * (1.47 * size * h),
                Vec3::new(0.37 * size, 0.40 * size, 0.30 * size),
                Quat::IDENTITY,
            );
            part(
                commands,
                parent,
                art,
                PartKind::Body,
                &art.sphere,
                &art.ivory,
                Vec3::new(0.0, 1.48 * size * h, -0.24 * size),
                Vec3::new(0.25 * size, 0.32 * size, 0.10 * size),
                Quat::IDENTITY,
            );
            for side in [-1.0, 1.0] {
                part(
                    commands,
                    parent,
                    art,
                    PartKind::Eye,
                    &art.cube,
                    &art.coral,
                    Vec3::new(side * 0.105 * size, 1.52 * size * h, -0.34 * size),
                    Vec3::new(0.065 * size, 0.08 * size, 0.03),
                    Quat::from_rotation_z(side * 0.2),
                );
                fixed_part(
                    commands,
                    parent,
                    &art.cone,
                    if boss { &art.gold } else { &art.red },
                    Vec3::new(side * 0.28 * size, 1.88 * size * h, 0.04),
                    Vec3::new(0.14 * size, 0.65 * size, 0.14 * size),
                    Quat::from_rotation_z(-side * 0.33),
                );
                part(
                    commands,
                    parent,
                    art,
                    if side < 0.0 {
                        PartKind::LeftArm
                    } else {
                        PartKind::RightArm
                    },
                    &art.cube,
                    bodymat,
                    Vec3::new(side * 0.49 * size, 0.86 * size * h, -0.03),
                    Vec3::new(0.20 * size, 0.75 * size, 0.22 * size),
                    Quat::from_rotation_z(side * 0.14),
                );
                part(
                    commands,
                    parent,
                    art,
                    if side < 0.0 {
                        PartKind::LeftLeg
                    } else {
                        PartKind::RightLeg
                    },
                    &art.cone,
                    &art.dark,
                    Vec3::new(side * 0.20 * size, 0.22 * size, -0.02),
                    Vec3::new(0.16 * size, 0.45 * size, 0.18 * size),
                    Quat::IDENTITY,
                );
                if enemy == EnemyKind::Elite || boss {
                    fixed_part(
                        commands,
                        parent,
                        &art.crystal,
                        &art.gold,
                        Vec3::new(side * 0.58 * size, 1.20 * size, 0.0),
                        Vec3::new(0.38 * size, 0.21 * size, 0.32 * size),
                        Quat::from_rotation_z(side * 0.2),
                    );
                }
            }
            if ambush {
                for side in [-1.0, 1.0] {
                    fixed_part(
                        commands,
                        parent,
                        &art.cone,
                        &art.ivory,
                        Vec3::new(side * 0.52, 0.35, -0.5),
                        Vec3::new(0.10, 0.75, 0.09),
                        Quat::from_rotation_x(-1.3),
                    );
                }
            }
            if boss {
                part(
                    commands,
                    parent,
                    art,
                    PartKind::Halo,
                    &art.ring,
                    &art.gold_glow,
                    Vec3::new(0.0, 3.9, 0.05),
                    Vec3::splat(1.7),
                    Quat::IDENTITY,
                );
                for i in 0..7 {
                    let a = PI * (0.12 + i as f32 * 0.125);
                    fixed_part(
                        commands,
                        parent,
                        &art.crystal,
                        &art.gold_glow,
                        Vec3::new(a.cos() * 1.65, 3.7 + a.sin() * 1.7, 0.05),
                        Vec3::new(0.15, 0.5, 0.12),
                        Quat::from_rotation_z(a - FRAC_PI_2),
                    );
                }
                ground_child_ring(commands, parent, art, &art.coral, 1.45, 0.055);
            }
        }
    }
    if kind.is_some() {
        part(
            commands,
            parent,
            art,
            PartKind::Slow,
            &art.ring,
            &art.ice_glow,
            Vec3::Y * 0.06,
            Vec3::splat(0.8 * size),
            Quat::from_rotation_x(-FRAC_PI_2),
        );
        let height = if kind == Some(EnemyKind::Boss) {
            5.8
        } else {
            2.4 * size
        };
        fixed_part(
            commands,
            parent,
            &art.cube,
            &art.dark,
            Vec3::Y * height,
            Vec3::new(1.07 * size, 0.08, 0.10),
            Quat::IDENTITY,
        );
        part(
            commands,
            parent,
            art,
            PartKind::Health,
            &art.cube,
            &art.coral,
            Vec3::new(0.0, height, 0.02),
            Vec3::new(size, 0.065, 0.10),
            Quat::IDENTITY,
        );
    }
}

fn ground_child_ring(
    commands: &mut Commands,
    parent: Entity,
    art: &SceneArt,
    mat: &Handle<StandardMaterial>,
    radius: f32,
    height: f32,
) {
    fixed_part(
        commands,
        parent,
        &art.ring,
        mat,
        Vec3::Y * height,
        Vec3::splat(radius),
        Quat::from_rotation_x(-FRAC_PI_2),
    );
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum VisualKey {
    Projectile(u64),
    Wisp(u64),
    Warning(u64),
    Presentation(PresentationId),
}
#[derive(Component)]
struct EffectVisual(VisualKey);
#[derive(Component)]
struct DamageLabel(u64);

fn essence_material<'a>(
    art: &'a SceneArt,
    essence: Option<EssenceKind>,
) -> &'a Handle<StandardMaterial> {
    match essence {
        Some(EssenceKind::Echo) => &art.purple_glow,
        Some(EssenceKind::Twin) => &art.gold_glow,
        Some(EssenceKind::Frost) => &art.ice_glow,
        Some(EssenceKind::Leech) => &art.healing_glow,
        _ => &art.glow,
    }
}

fn spawn_effect(
    commands: &mut Commands,
    art: &SceneArt,
    key: VisualKey,
    friendly: bool,
    essence: Option<EssenceKind>,
) -> Entity {
    let mat = if friendly {
        essence_material(art, essence)
    } else {
        &art.coral
    };
    let parent = commands
        .spawn((
            SceneGraphic,
            Transform::default(),
            Visibility::default(),
            EffectVisual(key),
        ))
        .id();
    match key {
        VisualKey::Projectile(_) => {
            fixed_part(
                commands,
                parent,
                &art.sphere,
                mat,
                Vec3::ZERO,
                Vec3::new(1.0, 1.0, 1.35),
                Quat::IDENTITY,
            );
            for i in 1..=3 {
                let f = i as f32;
                fixed_part(
                    commands,
                    parent,
                    &art.crystal,
                    mat,
                    Vec3::Z * (f * 1.2),
                    Vec3::new(0.6 / f, 0.6 / f, 1.0),
                    Quat::IDENTITY,
                );
            }
        }
        VisualKey::Wisp(_) => {
            fixed_part(
                commands,
                parent,
                &art.crystal,
                mat,
                Vec3::ZERO,
                Vec3::new(0.23, 0.37, 0.23),
                Quat::IDENTITY,
            );
            fixed_part(
                commands,
                parent,
                &art.ring,
                &art.gold_glow,
                Vec3::ZERO,
                Vec3::splat(0.42),
                Quat::from_rotation_x(0.4),
            );
        }
        VisualKey::Warning(_) => {
            fixed_part(
                commands,
                parent,
                &art.ring,
                &art.coral,
                Vec3::ZERO,
                Vec3::ONE,
                Quat::from_rotation_x(-FRAC_PI_2),
            );
            fixed_part(
                commands,
                parent,
                &art.disk,
                &art.warning_fill,
                Vec3::NEG_Y * 0.006,
                Vec3::ONE,
                Quat::from_rotation_x(-FRAC_PI_2),
            );
            for i in 0..4 {
                let a = i as f32 * FRAC_PI_2;
                fixed_part(
                    commands,
                    parent,
                    &art.cube,
                    &art.coral,
                    Vec3::new(a.cos() * 0.80, 0.015, a.sin() * 0.80),
                    Vec3::new(0.045, 0.025, 0.20),
                    Quat::from_rotation_y(-a + FRAC_PI_2),
                );
            }
        }
        VisualKey::Presentation(_) => {
            fixed_part(
                commands,
                parent,
                &art.ring,
                mat,
                Vec3::ZERO,
                Vec3::ONE,
                Quat::from_rotation_x(-FRAC_PI_2),
            );
            fixed_part(
                commands,
                parent,
                &art.ring,
                if friendly { &art.gold_glow } else { &art.coral },
                Vec3::Y * 0.08,
                Vec3::splat(0.76),
                Quat::from_rotation_x(-FRAC_PI_2),
            );
            fixed_part(
                commands,
                parent,
                &art.disk,
                if friendly {
                    &art.friendly_fill
                } else {
                    &art.warning_fill
                },
                Vec3::NEG_Y * 0.01,
                Vec3::ONE,
                Quat::from_rotation_x(-FRAC_PI_2),
            );
            for i in 0..8 {
                let a = i as f32 * TAU / 8.0;
                fixed_part(
                    commands,
                    parent,
                    &art.crystal,
                    mat,
                    Vec3::new(a.cos() * 0.90, 0.14, a.sin() * 0.90),
                    Vec3::new(0.04, 0.18, 0.04),
                    Quat::IDENTITY,
                );
            }
        }
    }
    parent
}

fn world(p: [f32; 2], height: f32) -> Vec3 {
    Vec3::new(p[0], height, p[1])
}
fn facing(p: [f32; 2]) -> Quat {
    Quat::from_rotation_y((-p[0]).atan2(-p[1]))
}

/// Reconcile render identities to the simulation snapshot. No receive-event spawning.
fn sync_scene(
    mut commands: Commands,
    view: Res<DreamView>,
    time: Res<Time>,
    mut art: ResMut<SceneArt>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut clear: ResMut<ClearColor>,
    mut actors: Query<(Entity, &mut ActorVisual, &mut Transform)>,
    mut effects: Query<(Entity, &EffectVisual, &mut Transform), Without<ActorVisual>>,
    numbers: Query<(Entity, &DamageLabel)>,
    labels: Query<(Entity, &TravelerLabel)>,
) {
    let snapshot = &view.0;
    if art.realm != snapshot.realm {
        art.realm = snapshot.realm;
        let (floor, stone, cliff, tile, sky) = match snapshot.realm {
            1 => (
                [0.23, 0.13, 0.18],
                [0.43, 0.26, 0.28],
                [0.105, 0.07, 0.13],
                [0.30, 0.18, 0.20],
                [0.055, 0.020, 0.05],
            ),
            2 => (
                [0.14, 0.105, 0.23],
                [0.31, 0.25, 0.43],
                [0.065, 0.045, 0.12],
                [0.18, 0.14, 0.29],
                [0.024, 0.013, 0.062],
            ),
            _ => (
                [0.075, 0.19, 0.20],
                [0.19, 0.31, 0.32],
                [0.055, 0.10, 0.15],
                [0.105, 0.23, 0.24],
                [0.016, 0.025, 0.055],
            ),
        };
        for (handle, color) in [
            (&art.ground, floor),
            (&art.stone, stone),
            (&art.cliff, cliff),
            (&art.tile, tile),
        ] {
            if let Some(mut m) = materials.get_mut(handle) {
                m.base_color = Color::srgb(color[0], color[1], color[2]);
            }
        }
        clear.0 = Color::srgb(sky[0], sky[1], sky[2]);
    }
    let mut existing = HashSet::new();
    let dt = time.delta_secs().min(0.1);
    for (entity, mut actor, mut transform) in &mut actors {
        existing.insert(actor.id);
        let (position, direction, speed, attack, windup, hit, dash) = if actor.kind.is_none() {
            let Some(h) = snapshot.heroes.iter().find(|hero| hero.id == actor.id) else {
                commands.entity(entity).despawn();
                continue;
            };
            actor.shield = h.hp > 0.0 && (h.shield > 0.0 || h.invulnerable);
            actor.hp = h.hp / h.max_hp.max(1.0);
            (
                h.position,
                h.facing,
                Some(Vec2::from_array(h.velocity).length()),
                h.attack_flash,
                0.0,
                h.hit_flash,
                f32::from(h.dashing),
            )
        } else if let Some(e) = snapshot.enemies.iter().find(|e| e.id == actor.id) {
            actor.hp = e.hp / e.max_hp;
            actor.slowed = e.slowed;
            (
                e.position,
                e.facing,
                None,
                0.0,
                e.windup.min(0.6),
                e.hit_flash,
                0.0,
            )
        } else {
            commands.entity(entity).despawn();
            continue;
        };
        let downed = actor.kind.is_none() && actor.hp <= 0.0;
        let target = world(position, 0.0);
        let reset = actor.sample_target(
            target,
            snapshot.seed,
            snapshot.room,
            snapshot.tick,
            time.delta(),
        );
        // Both sampled enemy speed and a hero's retained velocity become stale
        // when prediction stops. Allow normal gaps between fixed ticks first.
        let idle = snapshot.paused
            || snapshot.phase != RunPhase::Combat
            || downed
            || actor.sample_age.is_finished();
        let motion = if idle {
            0.0
        } else {
            speed.unwrap_or(actor.target_motion)
        };
        actor.advance_pose(
            motion,
            if idle { 0.0 } else { attack },
            if idle { 0.0 } else { windup },
            if idle { 0.0 } else { hit },
            if idle { 0.0 } else { dash },
            dt,
        );
        let yaw = facing(direction);
        if reset || transform.translation.distance_squared(target) > 16.0 {
            transform.translation = target;
            transform.rotation = yaw;
        } else {
            // These are renderer-owned transforms. Local prediction remains exact
            // in DreamView while its fixed ticks blend at display frequency.
            transform
                .translation
                .smooth_nudge(&target, if actor.local { 65.0 } else { 24.0 }, dt);
            transform.rotation.smooth_nudge(&yaw, 22.0, dt);
        }
        transform.scale = if downed {
            Vec3::new(0.90, 0.27, 0.90)
        } else {
            Vec3::ONE
        };
    }
    for hero in &snapshot.heroes {
        if !existing.contains(&hero.id) {
            spawn_actor(
                &mut commands,
                &art,
                hero.id,
                None,
                world(hero.position, 0.0),
                hero.id == snapshot.hero.id,
                snapshot,
            );
        }
    }
    for e in &snapshot.enemies {
        if !existing.contains(&e.id) {
            spawn_actor(
                &mut commands,
                &art,
                e.id,
                Some(e.kind),
                world(e.position, 0.0),
                false,
                snapshot,
            );
        }
    }
    let mut label_ids = HashSet::new();
    for (entity, label) in &labels {
        label_ids.insert(label.0);
        if !snapshot.heroes.iter().any(|hero| hero.id == label.0) {
            commands.entity(entity).despawn();
        }
    }
    for hero in &snapshot.heroes {
        if !label_ids.contains(&hero.id) {
            commands.spawn((
                SceneGraphic,
                TravelerLabel(hero.id),
                Text::new("VESPER"),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(companion_color(hero.id)),
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                GlobalZIndex(4),
                bevy::ui::FocusPolicy::Pass,
            ));
        }
    }
    let mut desired = Vec::new();
    for p in &snapshot.projectiles {
        desired.push((
            VisualKey::Projectile(p.id),
            Transform::from_translation(world(p.position, 0.70))
                .with_rotation(facing(p.direction))
                .with_scale(Vec3::splat(p.radius.max(0.14))),
            p.friendly,
            p.essence,
        ));
    }
    for w in &snapshot.wisps {
        desired.push((
            VisualKey::Wisp(w.id),
            Transform::from_translation(world(
                w.position,
                1.15 + (snapshot.elapsed * 3.0 + (w.id % 997) as f32).sin() * 0.1,
            ))
            .with_rotation(Quat::from_rotation_y(snapshot.elapsed)),
            true,
            w.essence,
        ));
    }
    for e in &snapshot.enemies {
        if e.windup > 0.0 {
            desired.push((
                VisualKey::Warning(e.id),
                Transform::from_translation(world(e.target, 0.065))
                    .with_scale(Vec3::splat(e.warn_radius.max(0.45))),
                false,
                None,
            ));
        }
    }
    for p in &snapshot.presentations {
        let age = p.age_ticks as f32 / p.duration_ticks.max(1) as f32;
        desired.push((
            VisualKey::Presentation(p.id),
            Transform::from_translation(world(p.pos, 0.10 + age * 0.08))
                .with_scale(Vec3::splat(p.radius * (0.5 + age * 0.5))),
            p.id.owner < (1_u64 << 63),
            None,
        ));
    }
    let mut effect_ids = HashSet::new();
    for (entity, effect, mut transform) in &mut effects {
        effect_ids.insert(effect.0);
        if let Some((_, next, _, _)) = desired.iter().find(|(key, _, _, _)| *key == effect.0) {
            *transform = *next;
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (key, transform, friendly, essence) in desired {
        if !effect_ids.contains(&key) {
            let id = spawn_effect(&mut commands, &art, key, friendly, essence);
            commands.entity(id).insert(transform);
        }
    }
    let mut number_ids = HashSet::new();
    for (entity, label) in &numbers {
        number_ids.insert(label.0);
        if !snapshot.damage_numbers.iter().any(|n| n.id == label.0) {
            commands.entity(entity).despawn();
        }
    }
    for n in &snapshot.damage_numbers {
        if number_ids.contains(&n.id) {
            continue;
        }
        commands.spawn((
            SceneGraphic,
            DamageLabel(n.id),
            Text::new(format!(
                "{}{:.0}",
                if !n.friendly {
                    "−"
                } else if n.critical {
                    "✦ "
                } else {
                    ""
                },
                n.amount
            )),
            TextFont {
                font_size: FontSize::Px(if n.critical { 24.0 } else { 17.0 }),
                ..default()
            },
            TextColor(if !n.friendly {
                Color::srgb(1.0, 0.36, 0.37)
            } else if n.critical {
                Color::srgb(1.0, 0.83, 0.38)
            } else {
                Color::srgb(0.91, 0.97, 0.88)
            }),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            GlobalZIndex(5),
            bevy::ui::FocusPolicy::Pass,
        ));
    }
}

fn animate_scene(
    time: Res<Time>,
    actors: Query<(&ActorVisual, &Transform), Without<ArtPart>>,
    mut parts: Query<(&ArtPart, &mut Transform, &mut Visibility), Without<ActorVisual>>,
    mut drifters: Query<(&Drift, &mut Transform), (Without<ArtPart>, Without<ActorVisual>)>,
    mut gates: Query<
        &mut Transform,
        (
            With<GateRing>,
            Without<Drift>,
            Without<ArtPart>,
            Without<ActorVisual>,
        ),
    >,
) {
    let t = time.elapsed_secs();
    for (part, mut transform, mut visibility) in &mut parts {
        let Ok((actor, parent)) = actors.get(part.actor) else {
            continue;
        };
        let mut next = part.base;
        let phase = actor.gait_phase;
        let stride = (actor.motion / 7.0).clamp(0.0, 1.0);
        let hover = matches!(actor.kind, Some(EnemyKind::Ranged | EnemyKind::Support));
        let bob = if hover {
            0.10 * (t * 2.4 + (actor.id % 997) as f32).sin()
        } else {
            (1.0 - (phase * 2.0).cos()) * 0.018 * stride
        };
        let breath = (t * 2.1 + (actor.id % 997) as f32).sin() * 0.012 * (1.0 - stride);
        match part.kind {
            PartKind::LeftArm | PartKind::RightArm | PartKind::LeftLeg | PartKind::RightLeg => {
                let side = if matches!(part.kind, PartKind::LeftArm | PartKind::RightLeg) {
                    1.0
                } else {
                    -1.0
                };
                let arm = matches!(part.kind, PartKind::LeftArm | PartKind::RightArm);
                let swing = phase.sin() * stride * if arm { 0.32 } else { 0.40 } * side;
                let commit = if arm {
                    actor.attack * 2.4 + actor.windup * 0.7
                } else {
                    0.0
                };
                next.rotation *= Quat::from_rotation_x(swing - commit);
                next.translation.y += bob;
            }
            PartKind::Cape => {
                next.rotation *= Quat::from_rotation_x(
                    -stride * 0.20 - 0.06 * (t * 5.0).sin() - actor.dash * 0.6,
                );
                next.translation.y += bob + breath;
            }
            PartKind::Blade => {
                // Hilt and blade receive one rigid pose around the same grip.
                let pivot = Vec3::new(0.49, 1.0, -0.25);
                let swing = Quat::from_rotation_y(-actor.attack * 9.0);
                next.translation = pivot + swing * (next.translation - pivot) + Vec3::Y * bob;
                next.rotation = swing * next.rotation;
            }
            PartKind::Halo => {
                next.rotation *= Quat::from_rotation_z(t * 0.5);
                next.translation.y += bob;
            }
            PartKind::Health => {
                next.scale.x *= actor.hp.clamp(0.0, 1.0);
                next.rotation = parent.rotation.inverse();
                *visibility = if actor.hp < 0.999 {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
            }
            PartKind::Shield => {
                next.rotation *= Quat::from_rotation_y(t * 1.7);
                *visibility = if actor.shield {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
            }
            PartKind::Slow => {
                *visibility = if actor.slowed {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
            }
            PartKind::Body | PartKind::Eye => {
                next.translation.y += bob + breath;
            }
        }
        if actor.hit > 0.0 && !matches!(part.kind, PartKind::Health | PartKind::Shield) {
            next.scale *= Vec3::new(
                1.0 + actor.hit * 0.9,
                1.0 - actor.hit * 0.5,
                1.0 + actor.hit * 0.5,
            );
        }
        *transform = next;
    }
    for (drift, mut transform) in &mut drifters {
        transform.translation =
            drift.base + Vec3::Y * ((t * drift.speed + drift.phase).sin() * drift.amplitude);
        if drift.spin != 0.0 {
            transform.rotate_y(drift.spin * time.delta_secs());
        }
    }
    for mut transform in &mut gates {
        transform.rotation = Quat::from_rotation_z(t * 0.035);
    }
}

fn place_damage_numbers(
    view: Res<DreamView>,
    scale: Res<UiScale>,
    cameras: Query<(Entity, &Camera), With<DreamCameraRig>>,
    transforms: TransformHelper,
    mut numbers: Query<(&DamageLabel, &mut Node, &mut TextColor)>,
) {
    let Ok((entity, camera)) = cameras.single() else {
        return;
    };
    let Ok(transform) = transforms.compute_global_transform(entity) else {
        return;
    };
    for (label, mut node, mut color) in &mut numbers {
        if let Some(n) = view.0.damage_numbers.iter().find(|n| n.id == label.0) {
            if let Ok(screen) =
                camera.world_to_viewport(&transform, world(n.position, 1.7 + n.age * 1.3))
            {
                node.left = px(screen.x / scale.0 - 12.0);
                node.top = px(screen.y / scale.0 - 12.0);
            }
            color.0.set_alpha((1.0 - n.age / 0.85).clamp(0.0, 1.0));
        }
    }
}

#[derive(Component)]
struct TravelerLabel(u64);

pub(super) fn companion_color(id: u64) -> Color {
    match id % 4 {
        0 => Color::srgb(0.95, 0.77, 0.43),
        1 => Color::srgb(0.83, 0.59, 1.0),
        2 => Color::srgb(0.48, 0.78, 1.0),
        _ => Color::srgb(0.65, 0.96, 0.48),
    }
}

fn place_traveler_labels(
    view: Res<DreamView>,
    scale: Res<UiScale>,
    cameras: Query<(Entity, &Camera), With<DreamCameraRig>>,
    transforms: TransformHelper,
    actors: Query<(&ActorVisual, &Transform)>,
    mut labels: Query<(&TravelerLabel, &mut Node, &mut Text, &mut TextColor)>,
) {
    let Ok((entity, camera)) = cameras.single() else {
        return;
    };
    let Ok(camera_transform) = transforms.compute_global_transform(entity) else {
        return;
    };
    for (label, mut node, mut text, mut color) in &mut labels {
        let Some(hero) = view.0.heroes.iter().find(|hero| hero.id == label.0) else {
            continue;
        };
        let local = hero.id == view.0.hero.id;
        let next = if local {
            if hero.hp > 0.0 {
                "YOU".into()
            } else {
                "YOU  /  DOWNED".into()
            }
        } else {
            format!(
                "VESPER {:08X}{}",
                hero.id & 0xffff_ffff,
                if hero.hp <= 0.0 { " / DOWNED" } else { "" }
            )
        };
        if text.0 != next {
            text.0 = next;
        }
        color.0 = if hero.hp <= 0.0 {
            Color::srgb(0.80, 0.59, 0.60)
        } else if local {
            Color::srgb(0.40, 0.94, 0.85)
        } else {
            companion_color(hero.id)
        };
        let origin = actors
            .iter()
            .find(|(actor, _)| actor.id == hero.id)
            .map(|(_, transform)| transform.translation)
            .unwrap_or_else(|| world(hero.position, 0.0));
        if let Ok(screen) = camera.world_to_viewport(
            &camera_transform,
            origin + Vec3::Y * if hero.hp > 0.0 { 2.3 } else { 0.6 },
        ) {
            node.display = if view.0.phase == RunPhase::Intro {
                Display::None
            } else {
                Display::Flex
            };
            node.left = px(screen.x / scale.0 - if local { 12.0 } else { 39.0 });
            node.top = px(screen.y / scale.0 - 8.0);
        } else {
            node.display = Display::None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dreamwake_sim::DreamSimulation;

    fn actor_pose() -> ActorVisual {
        ActorVisual {
            id: 1,
            kind: Some(EnemyKind::Ranged),
            local: false,
            motion: 0.0,
            gait_phase: 0.0,
            target_motion: 0.0,
            last_position: Vec3::ZERO,
            last_tick: 0,
            sample_age: Timer::from_seconds(ACTOR_SAMPLE_GRACE_SECS, TimerMode::Once),
            seed: 7,
            room: 0,
            windup: 0.0,
            hit: 0.0,
            dash: 0.0,
            attack: 0.0,
            shield: false,
            slowed: false,
            hp: 1.0,
        }
    }

    #[test]
    fn stalled_snapshot_gait_settles_and_recovers_without_between_tick_flicker() {
        let mut snapshot = DreamSimulation::new(7, false).snapshot();
        snapshot.phase = RunPhase::Combat;
        snapshot.hero.velocity = [7.0, 0.0];
        snapshot.heroes[0] = snapshot.hero.clone();
        let mut app = actor_scene_app(snapshot);
        app.update();
        // Simulate 30 Hz view updates on a 120 Hz display. Intermediate frames
        // must keep walking even though their snapshot tick is unchanged.
        for frame in 0..120 {
            if frame % 4 == 0 {
                let mut view = app.world_mut().resource_mut::<DreamView>();
                view.0.tick += 2;
            }
            app.update();
            let world = app.world_mut();
            let actor = world.query::<&ActorVisual>().single(world).unwrap();
            assert!(!actor.sample_age.is_finished());
            if frame > 30 {
                assert!(actor.motion > 6.8);
            }
        }
        // Frozen hero velocity remains nonzero, just like an interrupted
        // prediction stream. Rendering must still settle to idle.
        for _ in 0..120 {
            app.update();
        }
        {
            let world = app.world_mut();
            let actor = world.query::<&ActorVisual>().single(world).unwrap();
            assert!(actor.sample_age.is_finished());
            assert_eq!(actor.motion, 0.0);
            assert_eq!(actor.target_motion, 0.0);
        }
        app.world_mut().resource_mut::<DreamView>().0.tick += 1;
        app.update();
        let world = app.world_mut();
        let actor = world.query::<&ActorVisual>().single(world).unwrap();
        assert!(!actor.sample_age.is_finished());
        assert!(actor.motion > 0.0);

        // Enemy samples use the same grace period and recover their measured
        // speed from the next tick rather than preserving stale render motion.
        let mut enemy = actor_pose();
        enemy.sample_target(Vec3::X, 7, 0, TICK_HZ, Duration::ZERO);
        assert_eq!(enemy.target_motion, 1.0);
        enemy.sample_target(Vec3::X, 7, 0, TICK_HZ, Duration::from_secs(1));
        assert_eq!(enemy.target_motion, 0.0);
        enemy.sample_target(Vec3::X * 2.0, 7, 0, TICK_HZ * 2, Duration::ZERO);
        assert!(!enemy.sample_age.is_finished());
        assert_eq!(enemy.target_motion, 1.0);
    }

    fn actor_scene_app(snapshot: DreamSnapshot) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f32(1.0 / 120.0),
        ))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(UiScale(1.0))
        .insert_resource(DreamView(snapshot))
        .add_plugins(DreamScenePlugin);
        app
    }

    #[test]
    fn enemy_gait_uses_snapshot_motion_not_render_correction() {
        let mut actor = actor_pose();
        actor.sample_target(Vec3::X, 7, 0, TICK_HZ, Duration::ZERO);
        assert_eq!(actor.target_motion, 1.0);
        // Display frames between simulation ticks retain the sampled speed.
        actor.sample_target(Vec3::X, 7, 0, TICK_HZ, Duration::ZERO);
        assert_eq!(actor.target_motion, 1.0);
        // The target stopped. A renderer still catching up must not imply walking.
        actor.sample_target(Vec3::X, 7, 0, TICK_HZ + 1, Duration::ZERO);
        assert_eq!(actor.target_motion, 0.0);
        actor.sample_target(Vec3::X, 7, 0, TICK_HZ + 1, Duration::ZERO);
        assert_eq!(actor.target_motion, 0.0);
    }

    #[test]
    fn idle_pose_settles_and_stops_its_footfall_phase() {
        let mut actor = actor_pose();
        actor.motion = 7.0;
        actor.attack = 0.2;
        actor.hit = 0.2;
        actor.windup = 0.6;
        actor.dash = 1.0;
        for _ in 0..120 {
            actor.advance_pose(0.0, 0.0, 0.0, 0.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(actor.motion, 0.0);
        assert!(actor.attack < 0.001 && actor.hit < 0.001 && actor.dash < 0.001);
        let phase = actor.gait_phase;
        actor.advance_pose(0.0, 0.0, 0.0, 0.0, 0.0, 1.0 / 30.0);
        assert_eq!(actor.gait_phase, phase);
    }

    #[test]
    fn pose_blending_is_consistent_at_30_and_120_hz() {
        let run = |hz: usize| {
            let mut actor = actor_pose();
            for _ in 0..hz {
                actor.advance_pose(7.0, 0.2, 0.5, 0.1, 1.0, 1.0 / hz as f32);
            }
            actor
        };
        let slow = run(30);
        let fast = run(120);
        for (a, b) in [
            (slow.motion, fast.motion),
            (slow.attack, fast.attack),
            (slow.windup, fast.windup),
            (slow.hit, fast.hit),
            (slow.dash, fast.dash),
        ] {
            assert!((a - b).abs() < 0.0001);
        }
        assert!((slow.gait_phase - fast.gait_phase).abs() < 0.15);
    }

    #[test]
    fn actor_pose_resets_on_teleport_run_room_and_tick_restart() {
        for (position, seed, room, tick) in [
            (Vec3::X * 5.0, 7, 0, 61),
            (Vec3::ZERO, 8, 0, 61),
            (Vec3::ZERO, 7, 1, 61),
            (Vec3::ZERO, 7, 0, 0),
        ] {
            let mut actor = actor_pose();
            actor.last_tick = 60;
            actor.motion = 7.0;
            actor.gait_phase = 2.0;
            actor.attack = 0.2;
            actor.dash = 1.0;
            assert!(actor.sample_target(position, seed, room, tick, Duration::ZERO));
            assert_eq!(actor.motion, 0.0);
            assert_eq!(actor.target_motion, 0.0);
            assert_eq!(actor.gait_phase, 0.0);
            assert_eq!(actor.attack, 0.0);
            assert_eq!(actor.dash, 0.0);
        }
    }

    #[test]
    fn small_reconciliation_rebases_motion_without_snapping_the_render_root() {
        let mut snapshot = DreamSimulation::new(7, false).snapshot();
        snapshot.phase = RunPhase::Combat;
        snapshot.tick = 100;
        snapshot.hero.position = [0.0, 0.0];
        snapshot.heroes[0] = snapshot.hero.clone();
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1.0 / 120.0),
        ))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(UiScale(1.0))
        .insert_resource(DreamView(snapshot))
        .add_plugins(DreamScenePlugin);
        app.update();
        {
            let mut view = app.world_mut().resource_mut::<DreamView>();
            view.0.tick = 101;
            view.0.heroes[0].position = [1.0, 0.0];
        }
        app.update();
        let before = {
            let world = app.world_mut();
            world
                .query_filtered::<&Transform, With<ActorVisual>>()
                .single(world)
                .unwrap()
                .translation
                .x
        };
        assert!(before > 0.0 && before < 1.0);
        {
            let mut view = app.world_mut().resource_mut::<DreamView>();
            view.0.tick = 100;
            view.0.heroes[0].position = [0.9, 0.0];
        }
        app.update();
        let world = app.world_mut();
        let mut query = world.query::<(&ActorVisual, &Transform)>();
        let (actor, transform) = query.single(world).unwrap();
        assert_eq!(actor.target_motion, 0.0);
        assert!(transform.translation.x > before && transform.translation.x < 0.9);
    }

    #[test]
    fn labels_use_current_camera_hierarchy_and_projection_before_ui_content() {
        use bevy::camera::{CameraUpdateSystems, RenderTargetInfo};
        use bevy::ui::UiSystems;
        #[derive(Resource, Default)]
        struct ContentPosition(Option<(Val, Val)>);
        let mut snapshot = DreamSimulation::new(7, false).snapshot();
        snapshot.phase = RunPhase::Combat;
        snapshot.hero.position = [0.0, 0.0];
        snapshot.heroes[0] = snapshot.hero.clone();
        let hero_id = snapshot.hero.id;
        let mut app = App::new();
        app.insert_resource(DreamView(snapshot))
            .insert_resource(UiScale(1.0))
            .init_resource::<ContentPosition>()
            .configure_sets(
                PostUpdate,
                (
                    CameraUpdateSystems,
                    UiSystems::Content,
                    UiSystems::Layout,
                    bevy::transform::TransformSystems::Propagate,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                (
                    (|mut cameras: Query<&mut Camera>| {
                        for mut camera in &mut cameras {
                            camera.computed.clip_from_view =
                                Mat4::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 0.1, 100.0);
                        }
                    })
                    .in_set(CameraUpdateSystems),
                    (|nodes: Query<&Node, With<TravelerLabel>>,
                      mut observed: ResMut<ContentPosition>| {
                        let node = nodes.single().unwrap();
                        observed.0 = Some((node.left, node.top));
                    })
                    .in_set(UiSystems::Content),
                ),
            );
        add_label_placement_systems(&mut app);
        let parent = app
            .world_mut()
            .spawn(Transform::from_xyz(3.0, 0.0, 0.0))
            .id();
        let mut camera = Camera::default();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(800, 600),
            scale_factor: 1.0,
        });
        let camera_entity = app
            .world_mut()
            .spawn((
                camera,
                DreamCameraRig::default(),
                Transform::from_xyz(2.0, 0.0, 10.0),
                GlobalTransform::default(),
                ChildOf(parent),
            ))
            .id();
        app.world_mut().spawn((
            TravelerLabel(hero_id),
            Node::default(),
            Text::default(),
            TextColor::default(),
        ));
        app.world_mut().run_schedule(PostUpdate);
        let camera = app.world().get::<Camera>(camera_entity).unwrap();
        let expected = camera
            .world_to_viewport(
                &GlobalTransform::from(Transform::from_xyz(5.0, 0.0, 10.0)),
                Vec3::Y * 2.3,
            )
            .unwrap();
        assert_eq!(
            app.world().resource::<ContentPosition>().0,
            Some((px(expected.x - 12.0), px(expected.y - 8.0)))
        );
        // No propagation ran: passing this test requires the fresh hierarchy.
        assert_eq!(
            app.world()
                .get::<GlobalTransform>(camera_entity)
                .unwrap()
                .translation(),
            Vec3::ZERO
        );
    }

    #[test]
    fn scene_tracks_every_authoritative_hero_and_cleans_up_departures() {
        let mut snapshot = DreamSimulation::new(7, false).snapshot();
        snapshot.phase = RunPhase::Combat;
        let mut companion = snapshot.hero.clone();
        companion.id = snapshot.hero.id + 1;
        companion.position = [5.0, 3.0];
        snapshot.heroes.push(companion.clone());
        let local_id = snapshot.hero.id;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(UiScale(1.0))
        .insert_resource(DreamView(snapshot))
        .add_plugins(DreamScenePlugin);
        app.update();
        {
            let world = app.world_mut();
            let actors: Vec<_> = world
                .query::<&ActorVisual>()
                .iter(world)
                .map(|actor| (actor.id, actor.local))
                .collect();
            assert_eq!(actors.len(), 2);
            assert!(actors.contains(&(local_id, true)));
            assert!(actors.contains(&(companion.id, false)));
        }
        {
            let mut view = app.world_mut().resource_mut::<DreamView>();
            view.0.heroes.retain(|hero| hero.id == local_id);
            view.0.heroes[0].hp = 0.0;
            view.0.hero.hp = 0.0;
        }
        app.update();
        let world = app.world_mut();
        let mut query = world.query::<(&ActorVisual, &Transform)>();
        let actors: Vec<_> = query.iter(world).collect();
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].0.id, local_id);
        assert_eq!(actors[0].1.scale, Vec3::new(0.90, 0.27, 0.90));
        assert_eq!(world.query::<&TravelerLabel>().iter(world).count(), 1);
    }
}

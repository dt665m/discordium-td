//! Game-owned, versioned collision geometry supplied to authority and replay.
use crate::{ARENA_FLOOR_HALF_EXTENT, ARENA_RADIUS, DT, replication::ReplicationError};
use bevy::prelude::*;
use engine_core::{ColliderKey, CollisionScene, KinematicConfig, StaticCollider};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const SCHEMA: u32 = 1;
const MAX_GAME_COLLIDERS: usize = 64;

/// Positive collision data only. No actor state, world seed, or render assets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollisionManifest {
    schema: u32,
    scene_revision: u64,
    config: KinematicConfig,
    colliders: Vec<StaticCollider>,
}

pub fn movement_profile() -> KinematicConfig {
    KinematicConfig {
        fixed_dt: DT,
        radius: 0.8,
        standing_height: 1.8,
        crouched_height: 1.6,
        speed: 7.8,
        crouched_speed: 3.9,
        // Dreamwake intent has always selected velocity immediately.
        acceleration: 10_000.0,
        braking: 10_000.0,
        air_acceleration: 10_000.0,
        dash_speed: 24.0,
        dash_duration_ticks: 12,
        dash_cooldown_ticks: 69,
        ..Default::default()
    }
}

impl CollisionManifest {
    pub(crate) fn from_parts(
        scene_revision: u64,
        config: KinematicConfig,
        colliders: Vec<StaticCollider>,
    ) -> Result<Self, ReplicationError> {
        let value = Self {
            schema: SCHEMA,
            scene_revision,
            config,
            colliders,
        };
        value.prepare_scene()?;
        Ok(value)
    }
    pub(crate) fn schema_version(&self) -> u32 {
        self.schema
    }
    pub fn current(scene_revision: u64) -> Self {
        let mut colliders = vec![StaticCollider::cuboid(
            ColliderKey {
                index: 1,
                generation: 1,
            },
            [0.0, -0.5, 0.0],
            [ARENA_FLOOR_HALF_EXTENT, 0.5, ARENA_FLOOR_HALF_EXTENT],
        )];
        // Explicit f32 literals make collision identity independent of target
        // trigonometry. The same descriptors are available to the renderer.
        for (index, (direction, rotation)) in PERIMETER.into_iter().enumerate() {
            let wall_radius = ARENA_RADIUS + 0.2;
            let position = [direction[0] * wall_radius, 1.5, direction[1] * wall_radius];
            let mut wall = StaticCollider::cuboid(
                ColliderKey {
                    index: index as u64 + 2,
                    generation: 1,
                },
                position,
                [wall_radius * 0.098491403 + 0.02, 1.5, 0.2],
            );
            wall.rotation = rotation;
            colliders.push(wall);
        }
        Self {
            schema: SCHEMA,
            scene_revision,
            config: movement_profile(),
            colliders,
        }
    }
    pub fn scene_revision(&self) -> u64 {
        self.scene_revision
    }
    pub fn config(&self) -> KinematicConfig {
        self.config
    }
    pub fn colliders(&self) -> &[StaticCollider] {
        &self.colliders
    }
    pub fn identity(&self) -> [u8; 32] {
        *blake3::hash(&crate::checkpoint::canonical(self).expect("validated collision floats"))
            .as_bytes()
    }
    fn prepare_scene(&self) -> Result<CollisionScene, ReplicationError> {
        if self.schema != SCHEMA {
            return Err(ReplicationError::SchemaMismatch);
        }
        if self.scene_revision == 0
            || self.config != movement_profile()
            || self.colliders.is_empty()
            || self.colliders.len() > MAX_GAME_COLLIDERS
            || self
                .colliders
                .windows(2)
                .any(|pair| pair[0].key >= pair[1].key)
        {
            return Err(ReplicationError::InvalidState);
        }
        CollisionScene::new(self.scene_revision, self.colliders.clone())
            .map_err(|_| ReplicationError::InvalidState)
    }
    pub fn encode(&self) -> Result<Vec<u8>, ReplicationError> {
        crate::replication::schema::encode_collision(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ReplicationError> {
        crate::replication::schema::decode_collision(bytes)
    }
    pub fn build(&self) -> Result<CollisionWorld, ReplicationError> {
        let scene = self.prepare_scene()?;
        Ok(CollisionWorld {
            manifest: Arc::new(self.clone()),
            scene: Arc::new(scene),
            identity: self.identity(),
        })
    }
}

/// Immutable prepared queries. History retains this exact version through Arc.
#[derive(Resource, Clone)]
pub struct CollisionWorld {
    manifest: Arc<CollisionManifest>,
    scene: Arc<CollisionScene>,
    identity: [u8; 32],
}
impl std::fmt::Debug for CollisionWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollisionWorld")
            .field("manifest", &self.manifest)
            .finish()
    }
}
impl PartialEq for CollisionWorld {
    fn eq(&self, other: &Self) -> bool {
        self.manifest == other.manifest
    }
}
impl CollisionWorld {
    /// Runtime supports are independently fenced by immutable base dependencies;
    /// the static content manifest and its identity are unchanged.
    pub(crate) fn with_dynamic_colliders(
        &self,
        dynamic: &[StaticCollider],
    ) -> Result<Self, ReplicationError> {
        if self.manifest.colliders.len() + dynamic.len() > MAX_GAME_COLLIDERS {
            return Err(ReplicationError::InvalidState);
        }
        let mut colliders = self.manifest.colliders.clone();
        colliders.extend_from_slice(dynamic);
        let scene = CollisionScene::new(self.manifest.scene_revision, colliders)
            .map_err(|_| ReplicationError::InvalidState)?;
        Ok(Self {
            manifest: Arc::clone(&self.manifest),
            scene: Arc::new(scene),
            identity: self.identity,
        })
    }
    pub fn manifest(&self) -> &CollisionManifest {
        &self.manifest
    }
    pub fn manifest_arc(&self) -> Arc<CollisionManifest> {
        Arc::clone(&self.manifest)
    }
    pub fn identity(&self) -> [u8; 32] {
        self.identity
    }
    pub fn scene(&self) -> &CollisionScene {
        &self.scene
    }
    /// Conservative decoded geometry/config envelope for prediction accounting.
    pub fn retained_bytes(&self) -> usize {
        4096 + self.manifest.colliders.len() * 1024
    }
    pub fn validate_motion(
        &self,
        motion: &engine_core::KinematicState,
        speed: f32,
    ) -> Result<(), ReplicationError> {
        engine_core::validate_kinematic_state(motion, &self.config_for_speed(speed), &self.scene)
            .map_err(|_| ReplicationError::InvalidState)
    }
    pub(crate) fn config_for_speed(&self, speed: f32) -> KinematicConfig {
        let mut config = self.manifest.config;
        config.speed = speed;
        config.crouched_speed = speed * 0.5;
        config
    }
}

// Unit X/Z directions and quaternions for a regular 32-sided perimeter.
// Wall half-length uses tan(pi / 32), plus overlap to close the joins.
const PERIMETER: [([f32; 2], [f32; 4]); 32] = [
    (
        [0.000000000, 1.000000000],
        [0.0, 0.000000000, 0.0, 1.000000000],
    ),
    (
        [0.195090322, 0.980785280],
        [0.0, 0.098017140, 0.0, 0.995184727],
    ),
    (
        [0.382683432, 0.923879533],
        [0.0, 0.195090322, 0.0, 0.980785280],
    ),
    (
        [0.555570233, 0.831469612],
        [0.0, 0.290284677, 0.0, 0.956940336],
    ),
    (
        [0.707106781, 0.707106781],
        [0.0, 0.382683432, 0.0, 0.923879533],
    ),
    (
        [0.831469612, 0.555570233],
        [0.0, 0.471396737, 0.0, 0.881921264],
    ),
    (
        [0.923879533, 0.382683432],
        [0.0, 0.555570233, 0.0, 0.831469612],
    ),
    (
        [0.980785280, 0.195090322],
        [0.0, 0.634393284, 0.0, 0.773010453],
    ),
    (
        [1.000000000, 0.000000000],
        [0.0, 0.707106781, 0.0, 0.707106781],
    ),
    (
        [0.980785280, -0.195090322],
        [0.0, 0.773010453, 0.0, 0.634393284],
    ),
    (
        [0.923879533, -0.382683432],
        [0.0, 0.831469612, 0.0, 0.555570233],
    ),
    (
        [0.831469612, -0.555570233],
        [0.0, 0.881921264, 0.0, 0.471396737],
    ),
    (
        [0.707106781, -0.707106781],
        [0.0, 0.923879533, 0.0, 0.382683432],
    ),
    (
        [0.555570233, -0.831469612],
        [0.0, 0.956940336, 0.0, 0.290284677],
    ),
    (
        [0.382683432, -0.923879533],
        [0.0, 0.980785280, 0.0, 0.195090322],
    ),
    (
        [0.195090322, -0.980785280],
        [0.0, 0.995184727, 0.0, 0.098017140],
    ),
    (
        [0.000000000, -1.000000000],
        [0.0, 1.000000000, 0.0, 0.000000000],
    ),
    (
        [-0.195090322, -0.980785280],
        [0.0, 0.995184727, 0.0, -0.098017140],
    ),
    (
        [-0.382683432, -0.923879533],
        [0.0, 0.980785280, 0.0, -0.195090322],
    ),
    (
        [-0.555570233, -0.831469612],
        [0.0, 0.956940336, 0.0, -0.290284677],
    ),
    (
        [-0.707106781, -0.707106781],
        [0.0, 0.923879533, 0.0, -0.382683432],
    ),
    (
        [-0.831469612, -0.555570233],
        [0.0, 0.881921264, 0.0, -0.471396737],
    ),
    (
        [-0.923879533, -0.382683432],
        [0.0, 0.831469612, 0.0, -0.555570233],
    ),
    (
        [-0.980785280, -0.195090322],
        [0.0, 0.773010453, 0.0, -0.634393284],
    ),
    (
        [-1.000000000, 0.000000000],
        [0.0, 0.707106781, 0.0, -0.707106781],
    ),
    (
        [-0.980785280, 0.195090322],
        [0.0, 0.634393284, 0.0, -0.773010453],
    ),
    (
        [-0.923879533, 0.382683432],
        [0.0, 0.555570233, 0.0, -0.831469612],
    ),
    (
        [-0.831469612, 0.555570233],
        [0.0, 0.471396737, 0.0, -0.881921264],
    ),
    (
        [-0.707106781, 0.707106781],
        [0.0, 0.382683432, 0.0, -0.923879533],
    ),
    (
        [-0.555570233, 0.831469612],
        [0.0, 0.290284677, 0.0, -0.956940336],
    ),
    (
        [-0.382683432, 0.923879533],
        [0.0, 0.195090322, 0.0, -0.980785280],
    ),
    (
        [-0.195090322, 0.980785280],
        [0.0, 0.098017140, 0.0, -0.995184727],
    ),
];

/// Existing combat remains explicitly planar; capsule motion stays canonical 3D.
pub fn planar_position(state: &engine_core::KinematicState) -> [f32; 2] {
    [state.position[0], state.position[2]]
}
pub fn planar_velocity(state: &engine_core::KinematicState) -> [f32; 2] {
    [state.velocity[0], state.velocity[2]]
}

impl CollisionWorld {
    pub(crate) fn blink(
        &self,
        motion: &mut engine_core::KinematicState,
        direction: [f32; 2],
        distance: f32,
    ) -> Result<(), engine_core::KinematicError> {
        let direction = Vec2::from_array(direction).normalize_or_zero();
        if direction == Vec2::ZERO {
            return Ok(());
        }
        let mut swept = *motion;
        swept.base.attachment = None;
        swept.ground = None;
        swept.grounded = false;
        swept.velocity = [0.0; 3];
        swept.movement_lock_ticks = 0;
        swept.dash_ticks = 1;
        swept.dash_direction = direction.to_array();
        let mut config = self.manifest.config;
        config.dash_speed = distance / config.fixed_dt;
        config.gravity = 0.0;
        config.max_step_height = 0.0;
        engine_core::advance_kinematic(&mut swept, &config, Default::default(), &self.scene)?;
        let revision = motion
            .base
            .revision
            .checked_add(1)
            .ok_or(engine_core::KinematicError::BaseRevisionExhausted)?;
        motion.position = swept.position;
        motion.grounded = false;
        motion.ground = None;
        motion.coyote_ticks = 0;
        motion.suppress_snap_ticks = 1;
        motion.base = engine_core::BaseState {
            revision,
            attachment: None,
        };
        Ok(())
    }
}

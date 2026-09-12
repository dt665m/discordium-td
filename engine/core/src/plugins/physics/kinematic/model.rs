use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Stable map identity. A scene contains at most one generation of an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ColliderKey {
    pub index: u64,
    pub generation: u32,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stance {
    #[default]
    Standing,
    Crouched,
}
/// Retained with the character checkpoint. Ground queries consume a frozen
/// scene; the optional base adapter supplies exact historical support motion.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GroundContact {
    pub collider: ColliderKey,
    pub scene_revision: u64,
    pub normal: [f32; 3],
    pub local_position: [f32; 3],
}
/// Authoritative capsule state. Position is the bottom of the upright capsule,
/// with +Y up, so stance changes preserve the foot position.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(KinematicInput, KinematicStatus)]
pub struct KinematicState {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub facing: [f32; 2],
    pub stance: Stance,
    pub grounded: bool,
    pub ground: Option<GroundContact>,
    pub scene_revision: u64,
    pub jump_buffer_ticks: u16,
    pub coyote_ticks: u16,
    pub dash_ticks: u16,
    pub dash_cooldown_ticks: u16,
    pub dash_direction: [f32; 2],
    pub movement_lock_ticks: u16,
    /// Set on a deliberate teleport; jump also suppresses snap during its tick.
    pub suppress_snap_ticks: u16,
    /// Ticked support attachment. Velocity is relative to this base while
    /// attached and world-space otherwise; base transitions convert it once.
    pub base: super::bases::BaseState,
}
impl KinematicState {
    pub fn new(position: [f32; 3], scene_revision: u64) -> Self {
        Self {
            position,
            velocity: [0.0; 3],
            facing: [0.0, -1.0],
            stance: Stance::Standing,
            grounded: false,
            ground: None,
            scene_revision,
            jump_buffer_ticks: 0,
            coyote_ticks: 0,
            dash_ticks: 0,
            dash_cooldown_ticks: 0,
            dash_direction: [0.0, -1.0],
            movement_lock_ticks: 0,
            suppress_snap_ticks: 0,
            base: Default::default(),
        }
    }
}
/// World X/Z intent. Hardware edges must be assigned to one stored command by
/// the caller; this component is not sampled from a device during replay.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct KinematicInput {
    pub movement: [f32; 2],
    pub jump: bool,
    pub crouch: bool,
    pub dash: bool,
}
/// Game-supplied rules. Fixed duration never comes from wall-clock/render time.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KinematicConfig {
    pub fixed_dt: f32,
    pub radius: f32,
    pub standing_height: f32,
    pub crouched_height: f32,
    pub speed: f32,
    pub crouched_speed: f32,
    pub acceleration: f32,
    pub braking: f32,
    pub air_acceleration: f32,
    pub gravity: f32,
    pub jump_speed: f32,
    pub terminal_speed: f32,
    pub dash_speed: f32,
    pub dash_duration_ticks: u16,
    pub dash_cooldown_ticks: u16,
    pub jump_buffer_ticks: u16,
    pub coyote_ticks: u16,
    pub skin: f32,
    pub ground_snap: f32,
    pub max_step_height: f32,
    /// Minimum normal.Y of walkable contacts (cosine of the maximum slope).
    pub walkable_normal_y: f32,
    pub max_slide_iterations: u8,
    pub max_depenetration_iterations: u8,
    /// Counts narrow-phase collider queries across all probes for one step.
    pub max_queries: u32,
    pub max_depenetration_distance: f32,
}
impl Default for KinematicConfig {
    fn default() -> Self {
        Self {
            fixed_dt: 1.0 / 60.0,
            radius: 0.3,
            standing_height: 1.8,
            crouched_height: 1.0,
            speed: 6.0,
            crouched_speed: 3.0,
            acceleration: 40.0,
            braking: 50.0,
            air_acceleration: 12.0,
            gravity: 24.0,
            jump_speed: 8.0,
            terminal_speed: 60.0,
            dash_speed: 18.0,
            dash_duration_ticks: 10,
            dash_cooldown_ticks: 60,
            jump_buffer_ticks: 4,
            coyote_ticks: 4,
            skin: 0.005,
            ground_snap: 0.12,
            max_step_height: 0.35,
            walkable_normal_y: 0.70710677,
            max_slide_iterations: 8,
            max_depenetration_iterations: 8,
            max_queries: 8192,
            max_depenetration_distance: 2.0,
        }
    }
}
impl KinematicConfig {
    pub fn height(&self, stance: Stance) -> f32 {
        match stance {
            Stance::Standing => self.standing_height,
            Stance::Crouched => self.crouched_height,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KinematicError {
    InvalidConfig,
    InvalidState,
    InvalidInput,
    InvalidScene,
    SceneRevisionMismatch,
    QueryBudgetExceeded,
    UnsupportedQuery,
    InvalidQueryResult,
    DepenetrationLimit,
    InvalidBaseFrame,
    MissingBaseHistory,
    BaseMotionLimit,
    BaseRevisionExhausted,
}
impl std::fmt::Display for KinematicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for KinematicError {}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KinematicReport {
    pub queries: u32,
    pub slide_contacts: u8,
    pub depenetrations: u8,
    pub stepped: bool,
    pub stand_blocked: bool,
    pub jumped: bool,
    /// Intent remaining at the iteration cap was discarded; no unchecked move.
    pub slide_limit_reached: bool,
    /// A swept base carry was shortened by another collider.
    pub base_carry_blocked: bool,
    pub base_changed: bool,
}
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KinematicStatus {
    pub last_error: Option<KinematicError>,
    pub report: KinematicReport,
}

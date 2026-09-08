use super::*;

pub(crate) const ENEMY_HERO_AGGRO_RANGE: f32 = 8.5;
pub(crate) const ENEMY_HERO_DISENGAGE_RANGE: f32 = 11.0;
pub(crate) const ENEMY_BASE_ENGAGE_RING_PADDING: f32 = 0.32;
pub(crate) const ENEMY_STEERING_TARGET_WEIGHT: f32 = 1.0;
pub(crate) const ENEMY_STEERING_SEPARATION_WEIGHT: f32 = 1.7;
pub(crate) const ENEMY_SEPARATION_BUFFER: f32 = 0.55;
pub(crate) const ENEMY_ACCEL_FACTOR: f32 = 18.0;
pub(crate) const ENEMY_DAMPING_FACTOR: f32 = 4.3;
pub(crate) const ENEMY_SPATIAL_CELL_MIN_SIZE: f32 = 0.5;
pub(crate) const ENEMY_REPATH_TICKS: u32 = 8;
pub(crate) const ENEMY_WAYPOINT_REACH_RADIUS: f32 = 0.65;
pub(crate) const ENEMY_COLLISION_DAMPING: f32 = 0.82;
pub(crate) const HERO_COLLISION_MAX_DISPLACEMENT_PER_TICK: f32 =
    HERO_SPEED * FIXED_DT_SECONDS * 0.8;
pub(crate) const NAVMESH_MARGIN: f32 = 0.15;
pub(crate) const TOWER_NAV_BLOCK_RADIUS: f32 = 1.05;
pub(crate) const TOWER_NAV_BLOCK_VERTICES: usize = 8;
pub(crate) const NAVMESH_SEARCH_DELTA: f32 = 0.015;
pub(crate) const NAVMESH_SEARCH_STEPS: u32 = 220;

#[derive(Debug, Clone, Copy)]
pub(crate) enum EnemyTarget {
    Hero { client_id: u64, pos: [f32; 2] },
    Base,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EnemyMotionSnapshot {
    pub(crate) id: u64,
    pub(crate) pos: [f32; 2],
    pub(crate) radius: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EnemyCollisionSnapshot {
    pub(crate) id: u64,
    pub(crate) pos: [f32; 2],
    pub(crate) radius: f32,
}

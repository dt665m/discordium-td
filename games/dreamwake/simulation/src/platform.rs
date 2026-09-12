//! One game-owned rotating support and immutable collision intervals for replay.
use crate::{
    collision::CollisionWorld,
    replication::{PublicPlatformView, ReplicationError},
};
use bevy::prelude::*;
use engine_core::{
    BaseConfig, BaseFrame, BasePose, BaseSample, ColliderKey, CollisionScene, StaticCollider,
};
mod rotation;
#[cfg(test)]
mod tests;

pub const MAX_PLATFORMS: usize = 1;
pub const PLATFORM_PERIOD_TICKS: u16 = 1024;
pub const PLATFORM_POSITION: [f32; 3] = [0.0, 0.125, -10.0];
pub const PLATFORM_HALF_EXTENTS: [f32; 3] = [3.0, 0.125, 1.5];
/// Minimum spatial admission envelope; upgraded motion expands it below.
pub const BASE_DEPENDENCY_RADIUS: f32 = 12.0;
const MAX_SAMPLE_DISTANCE: u32 = 256;
const ANGULAR_SPEED: f32 = 0.3681554;

pub(crate) fn pose(phase: u16) -> BasePose {
    let [sin, cos] = rotation::YAW[usize::from(phase)];
    BasePose {
        position: PLATFORM_POSITION,
        rotation: [0.0, f32::from_bits(sin), 0.0, f32::from_bits(cos)],
        linear_velocity: [0.0; 3],
        angular_velocity: [0.0, ANGULAR_SPEED, 0.0],
    }
}
impl PublicPlatformView {
    pub fn valid(&self) -> bool {
        self.id != 0
            && self.collider.index == self.id
            && self.collider.generation != 0
            && self.scene_revision != 0
            && self.motion_revision != 0
            && self.origin_gameplay_tick <= self.gameplay_tick
            && self.phase < PLATFORM_PERIOD_TICKS
            && u32::from(self.phase)
                == (self.gameplay_tick - self.origin_gameplay_tick)
                    % u32::from(PLATFORM_PERIOD_TICKS)
            && self.pose == pose(self.phase)
            && self.half_extents == PLATFORM_HALF_EXTENTS
    }
    /// This declared trajectory is the only source of unreceived predicted poses.
    /// History keeps the original sample/episode; no live rendered pose is queried.
    pub fn pose_at(&self, tick: u32) -> Result<BasePose, ReplicationError> {
        if !self.valid()
            || tick < self.origin_gameplay_tick
            || tick.abs_diff(self.gameplay_tick) > MAX_SAMPLE_DISTANCE
        {
            return Err(ReplicationError::InvalidState);
        }
        let phase = (tick - self.origin_gameplay_tick) % u32::from(PLATFORM_PERIOD_TICKS);
        Ok(pose(phase as u16))
    }
    pub fn base_interval(&self, from: u32, to: u32) -> Result<BaseFrame, ReplicationError> {
        if from.checked_add(1) != Some(to) {
            return Err(ReplicationError::InvalidState);
        }
        BaseFrame::new(
            self.scene_revision,
            u64::from(from),
            u64::from(to),
            vec![BaseSample {
                collider: self.collider,
                previous: self.pose_at(from)?,
                current: self.pose_at(to)?,
            }],
        )
        .map_err(|_| ReplicationError::InvalidState)
    }
    pub fn collider_at(&self, tick: u32) -> Result<StaticCollider, ReplicationError> {
        let pose = self.pose_at(tick)?;
        let mut collider = StaticCollider::cuboid(self.collider, pose.position, self.half_extents);
        collider.rotation = pose.rotation;
        Ok(collider)
    }
}

/// Prepared once per authoritative step, or from exact immutable replay dependencies.
/// The scene contains current poses and the base frame retains both interval endpoints.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct MotionEnvironment {
    collision: CollisionWorld,
    bases: BaseFrame,
}
impl MotionEnvironment {
    fn validate_views(
        collision: &CollisionWorld,
        views: &[PublicPlatformView],
    ) -> Result<(), ReplicationError> {
        if views.len() > MAX_PLATFORMS
            || views
                .iter()
                .any(|v| !v.valid() || v.scene_revision != collision.manifest().scene_revision())
        {
            return Err(ReplicationError::InvalidState);
        }
        Ok(())
    }
    pub fn from_dependencies(
        collision: &CollisionWorld,
        views: &[PublicPlatformView],
        from: u32,
    ) -> Result<Self, ReplicationError> {
        Self::validate_views(collision, views)?;
        let to = from.checked_add(1).ok_or(ReplicationError::InvalidState)?;
        let samples = views
            .iter()
            .map(|view| {
                Ok(BaseSample {
                    collider: view.collider,
                    previous: view.pose_at(from)?,
                    current: view.pose_at(to)?,
                })
            })
            .collect::<Result<Vec<_>, ReplicationError>>()?;
        let dynamic = views
            .iter()
            .map(|v| v.collider_at(to))
            .collect::<Result<Vec<_>, _>>()?;
        let collision = collision.with_dynamic_colliders(&dynamic)?;
        let bases = BaseFrame::new(
            collision.manifest().scene_revision(),
            u64::from(from),
            u64::from(to),
            samples,
        )
        .map_err(|_| ReplicationError::InvalidState)?;
        Ok(Self { collision, bases })
    }
    /// Restore/display validation uses the committed endpoint, without advancing motion.
    pub(crate) fn committed(
        collision: &CollisionWorld,
        views: &[PublicPlatformView],
        tick: u32,
    ) -> Result<Self, ReplicationError> {
        Self::validate_views(collision, views)?;
        let next = tick.checked_add(1).ok_or(ReplicationError::InvalidState)?;
        let samples = views
            .iter()
            .map(|view| {
                let current = view.pose_at(tick)?;
                Ok(BaseSample {
                    collider: view.collider,
                    previous: current,
                    current,
                })
            })
            .collect::<Result<Vec<_>, ReplicationError>>()?;
        let dynamic = views
            .iter()
            .map(|v| v.collider_at(tick))
            .collect::<Result<Vec<_>, _>>()?;
        let collision = collision.with_dynamic_colliders(&dynamic)?;
        let bases = BaseFrame::new(
            collision.manifest().scene_revision(),
            u64::from(tick),
            u64::from(next),
            samples,
        )
        .map_err(|_| ReplicationError::InvalidState)?;
        Ok(Self { collision, bases })
    }
    pub fn validate_motion(
        &self,
        motion: &engine_core::KinematicState,
        speed: f32,
    ) -> Result<(), ReplicationError> {
        self.collision.validate_motion(motion, speed)?;
        self.bases
            .validate_attachment(motion)
            .map_err(|_| ReplicationError::InvalidState)
    }
    pub fn scene(&self) -> &CollisionScene {
        self.collision.scene()
    }
    pub fn collision(&self) -> &CollisionWorld {
        &self.collision
    }
    pub fn bases(&self) -> &BaseFrame {
        &self.bases
    }
    pub fn base_config(&self) -> BaseConfig {
        BaseConfig::default()
    }
}

/// Scheduling/replay bursts are bounded to 32 commands. A separate per-step
/// guard below remains fail-closed if a retained checkpoint is used longer.
pub const DEPENDENCY_LOOKAHEAD_TICKS: u32 = 32;
fn support_radius(config: &engine_core::KinematicConfig) -> f32 {
    Vec2::new(PLATFORM_HALF_EXTENTS[0], PLATFORM_HALF_EXTENTS[2]).length()
        + config.radius
        + config.skin
        + 0.05
}
fn horizontal_speed(motion: &engine_core::KinematicState, speed: f32) -> f32 {
    // Ground projection may redirect vertical momentum into the support tangent.
    speed.max(Vec3::from_array(motion.velocity).length())
}
pub fn dependency_radius(
    motion: &engine_core::KinematicState,
    speed: f32,
    config: &engine_core::KinematicConfig,
) -> f32 {
    let ordinary = horizontal_speed(motion, speed);
    let ticks = DEPENDENCY_LOOKAHEAD_TICKS;
    // Count every possible dash start conservatively, including one immediately.
    let episodes =
        1 + ticks / u32::from(config.dash_cooldown_ticks.max(1)) + u32::from(motion.dash_ticks > 0);
    let dash_ticks =
        ticks.min(episodes * u32::from(config.dash_duration_ticks.max(motion.dash_ticks)));
    let reach = config.fixed_dt
        * (ordinary * ticks as f32 + (config.dash_speed - ordinary).max(0.0) * dash_ticks as f32);
    BASE_DEPENDENCY_RADIUS.max(
        support_radius(config)
            + reach
            + crate::CHARGE_MAX_DISTANCE * (1 + ticks / u32::from(crate::CHARGE_COOLDOWN)) as f32,
    )
}
/// The platform's bounded region is game content, so no unreceived pose is read.
/// Stop before a missing dependency could participate in even one swept step.
pub(crate) fn missing_base_may_interact(
    motion: &engine_core::KinematicState,
    speed: f32,
    input: crate::DreamInput,
    authored_distance: f32,
    config: &engine_core::KinematicConfig,
) -> bool {
    let mut maximum_speed = horizontal_speed(motion, speed);
    if input.dash || motion.dash_ticks > 0 {
        maximum_speed = maximum_speed.max(config.dash_speed);
    }
    let radius = support_radius(config) + maximum_speed * config.fixed_dt + authored_distance;
    Vec2::new(
        motion.position[0] - PLATFORM_POSITION[0],
        motion.position[2] - PLATFORM_POSITION[2],
    )
    .length()
        <= radius
}
pub(crate) fn required(
    motion: &engine_core::KinematicState,
    speed: f32,
    config: &engine_core::KinematicConfig,
    active: bool,
    views: impl IntoIterator<Item = PublicPlatformView>,
) -> Vec<ColliderKey> {
    let radius = dependency_radius(motion, speed, config);
    views
        .into_iter()
        .filter(|view| {
            motion
                .base
                .attachment
                .is_some_and(|a| a.collider == view.collider)
                || (active
                    && Vec2::new(
                        motion.position[0] - view.pose.position[0],
                        motion.position[2] - view.pose.position[2],
                    )
                    .length()
                        <= radius)
        })
        .map(|view| view.collider)
        .collect()
}

pub(crate) fn advance_platforms(
    run: Res<crate::state::Run>,
    collision: Res<CollisionWorld>,
    mut platforms: Query<&mut crate::state::Platform>,
    mut environment: ResMut<MotionEnvironment>,
) {
    let mut views = Vec::with_capacity(MAX_PLATFORMS);
    for mut platform in &mut platforms {
        platform
            .advance(run.tick)
            .expect("bounded platform gameplay clock");
        views.push(platform.presentation(collision.manifest().scene_revision()));
    }
    let from = run
        .tick
        .checked_sub(1)
        .expect("platforms follow begin_tick");
    *environment = MotionEnvironment::from_dependencies(&collision, &views, from)
        .expect("valid authoritative platform interval");
}

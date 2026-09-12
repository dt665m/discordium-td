//! Immutable, tick-addressed support poses for the capsule controller. These
//! values belong in authoritative snapshots and retained replay dependencies.
//! They never reference a rendered transform or a mutable current-world lookup.
use super::{
    model::*,
    scene::{CollisionScene, MAX_COORDINATE, Queries, bounded},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub const MAX_MOVING_BASES: usize = 64;
const ANCHOR_TOLERANCE: f32 = 0.002;
const MAX_ROTATION_SEGMENTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BaseAttachment {
    pub collider: ColliderKey,
    pub scene_revision: u64,
    pub pose_tick: u64,
    pub local_anchor: [f32; 3],
}
/// The revision survives detachment, so returning to the same collider cannot
/// silently reuse an earlier attachment incarnation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct BaseState {
    pub revision: u64,
    pub attachment: Option<BaseAttachment>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BasePose {
    pub position: [f32; 3],
    /// Unit quaternion XYZW.
    pub rotation: [f32; 4],
    pub linear_velocity: [f32; 3],
    /// World-space angular velocity in radians per second.
    pub angular_velocity: [f32; 3],
}
impl BasePose {
    pub fn stationary(position: [f32; 3], rotation: [f32; 4]) -> Self {
        Self {
            position,
            rotation,
            linear_velocity: [0.0; 3],
            angular_velocity: [0.0; 3],
        }
    }
    fn rotation(self) -> Quat {
        Quat::from_array(self.rotation).normalize()
    }
    fn point(self, local: Vec3) -> Vec3 {
        Vec3::from_array(self.position) + self.rotation() * local
    }
    fn point_velocity(self, local: Vec3, config: BaseConfig) -> Vec3 {
        Vec3::from_array(self.linear_velocity) * config.inherit_linear
            + Vec3::from_array(self.angular_velocity).cross(self.rotation() * local)
                * config.inherit_angular
    }
    fn valid(self) -> bool {
        bounded(self.position, MAX_COORDINATE)
            && bounded(self.rotation, 1.001)
            && (Quat::from_array(self.rotation).length_squared() - 1.0).abs() <= 0.001
            && bounded(self.linear_velocity, 10_000.0)
            && bounded(self.angular_velocity, 1_000.0)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BaseSample {
    pub collider: ColliderKey,
    pub previous: BasePose,
    pub current: BasePose,
}
/// One exact consecutive tick interval, bounded and sorted by stable identity.
/// A missing sample means unavailable history, never a guessed stationary pose.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseFrame {
    scene_revision: u64,
    from_tick: u64,
    to_tick: u64,
    samples: Box<[BaseSample]>,
}
impl BaseFrame {
    pub fn new(
        scene_revision: u64,
        from_tick: u64,
        to_tick: u64,
        mut samples: Vec<BaseSample>,
    ) -> Result<Self, KinematicError> {
        if scene_revision == 0
            || from_tick.checked_add(1) != Some(to_tick)
            || samples.len() > MAX_MOVING_BASES
        {
            return Err(KinematicError::InvalidBaseFrame);
        }
        samples.sort_by_key(|sample| sample.collider);
        if samples.iter().any(|sample| {
            sample.collider.index == 0
                || sample.collider.generation == 0
                || !sample.previous.valid()
                || !sample.current.valid()
        }) || samples
            .windows(2)
            .any(|pair| pair[0].collider.index == pair[1].collider.index)
        {
            return Err(KinematicError::InvalidBaseFrame);
        }
        Ok(Self {
            scene_revision,
            from_tick,
            to_tick,
            samples: samples.into_boxed_slice(),
        })
    }
    pub fn scene_revision(&self) -> u64 {
        self.scene_revision
    }
    pub fn from_tick(&self) -> u64 {
        self.from_tick
    }
    pub fn to_tick(&self) -> u64 {
        self.to_tick
    }
    pub fn samples(&self) -> &[BaseSample] {
        &self.samples
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + std::mem::size_of_val(self.samples.as_ref())
    }
    pub fn sample(&self, key: ColliderKey) -> Option<&BaseSample> {
        self.samples
            .binary_search_by_key(&key, |sample| sample.collider)
            .ok()
            .map(|index| &self.samples[index])
    }
    /// Validate an attachment against the retained start pose without stepping it.
    /// Restore paths use a stationary frame at the committed endpoint.
    pub fn validate_attachment(&self, state: &KinematicState) -> Result<(), KinematicError> {
        let Some(attachment) = state.base.attachment else {
            return Ok(());
        };
        let sample = self
            .sample(attachment.collider)
            .ok_or(KinematicError::MissingBaseHistory)?;
        if state.base.revision == 0
            || attachment.scene_revision != self.scene_revision
            || attachment.pose_tick != self.from_tick
            || !bounded(attachment.local_anchor, MAX_COORDINATE * 2.0)
            || state
                .ground
                .is_none_or(|ground| ground.collider != attachment.collider)
            || sample
                .previous
                .point(Vec3::from_array(attachment.local_anchor))
                .distance(Vec3::from_array(state.position))
                > ANCHOR_TOLERANCE
        {
            return Err(KinematicError::InvalidBaseFrame);
        }
        Ok(())
    }
    fn validate_scene(&self, scene: &CollisionScene) -> Result<(), KinematicError> {
        if self.scene_revision != scene.revision() {
            return Err(KinematicError::SceneRevisionMismatch);
        }
        for sample in &self.samples {
            let collider = scene
                .collider(sample.collider)
                .ok_or(KinematicError::MissingBaseHistory)?;
            if collider.position != sample.current.position
                || Quat::from_array(collider.rotation)
                    .normalize()
                    .dot(sample.current.rotation())
                    .abs()
                    < 1.0 - 0.000001
            {
                return Err(KinematicError::InvalidBaseFrame);
            }
        }
        Ok(())
    }
}
/// Game-selected dismount contributions and maximum motion admitted in one
/// fixed step. Exceeding a limit requires an authoritative discontinuity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BaseConfig {
    pub inherit_linear: f32,
    pub inherit_angular: f32,
    pub max_carry_distance: f32,
    pub max_rotation_radians: f32,
}
impl Default for BaseConfig {
    fn default() -> Self {
        Self {
            inherit_linear: 1.0,
            inherit_angular: 1.0,
            max_carry_distance: 2.0,
            max_rotation_radians: 0.5,
        }
    }
}
#[derive(Clone, Copy)]
pub struct BaseStep<'a> {
    pub frame: &'a BaseFrame,
    pub config: BaseConfig,
    /// Ticked detachment retains world position, inherits velocity once, and
    /// suppresses ground reacquisition for this step.
    pub detach: bool,
}
pub(crate) struct Transition {
    previous: Option<BaseAttachment>,
    departure_velocity: Vec3,
    base_velocity: Vec3,
    inherited: bool,
}
impl Transition {
    pub fn inherit_departure(&mut self, velocity: &mut Vec3) {
        if self.previous.is_some() && !self.inherited {
            *velocity += self.departure_velocity;
            self.inherited = true;
        }
    }
}

pub(crate) fn prepare(
    state: &mut KinematicState,
    config: &KinematicConfig,
    scene: &CollisionScene,
    step: BaseStep<'_>,
    queries: &mut Queries<'_>,
    report: &mut KinematicReport,
) -> Result<Transition, KinematicError> {
    let c = step.config;
    if [c.inherit_linear, c.inherit_angular]
        .into_iter()
        .any(|v| !v.is_finite() || !(0.0..=4.0).contains(&v))
        || !c.max_carry_distance.is_finite()
        || !(0.0..=100.0).contains(&c.max_carry_distance)
        || !c.max_rotation_radians.is_finite()
        || !(0.0..=std::f32::consts::PI).contains(&c.max_rotation_radians)
    {
        return Err(KinematicError::InvalidConfig);
    }
    step.frame.validate_scene(scene)?;
    let old = state.base.attachment;
    let mut transition = Transition {
        previous: old,
        departure_velocity: Vec3::ZERO,
        base_velocity: Vec3::ZERO,
        inherited: false,
    };
    if let Some(attachment) = old {
        let sample = step
            .frame
            .sample(attachment.collider)
            .ok_or(KinematicError::MissingBaseHistory)?;
        step.frame.validate_attachment(state)?;
        let local = Vec3::from_array(attachment.local_anchor);
        transition.departure_velocity = sample.current.point_velocity(local, c);
        transition.base_velocity = sample.current.point_velocity(local, BaseConfig::default());
        if !step.detach {
            let q0 = sample.previous.rotation();
            let q1 = sample.current.rotation();
            let angle = q0.angle_between(q1);
            let end = sample.current.point(local);
            // Bound translation as well as arc travel: equal start/end points
            // do not excuse a platform spinning around its rider.
            let travel = Vec3::from_array(sample.previous.position)
                .distance(Vec3::from_array(sample.current.position))
                + angle * local.length();
            if angle > c.max_rotation_radians || travel > c.max_carry_distance {
                return Err(KinematicError::BaseMotionLimit);
            }
            // Shortest-arc interpolation, with a conservative capsule expansion
            // enclosing each curved anchor segment. All casts share one budget.
            let radius = local.length();
            let mut segments = 1;
            while radius * (1.0 - (angle / segments as f32 * 0.5).cos()) > config.skin * 0.25
                && segments < MAX_ROTATION_SEGMENTS
            {
                segments *= 2;
            }
            let expansion = radius * (1.0 - (angle / segments as f32 * 0.5).cos());
            if expansion > config.skin * 0.25 {
                return Err(KinematicError::BaseMotionLimit);
            }
            let mut position = Vec3::from_array(state.position);
            for index in 1..=segments {
                let t = index as f32 / segments as f32;
                let target = if index == segments {
                    end
                } else {
                    Vec3::from_array(sample.previous.position)
                        .lerp(Vec3::from_array(sample.current.position), t)
                        + q0.slerp(q1, t) * local
                };
                let movement = target - position;
                if movement.length_squared() < 0.0000000001 {
                    continue;
                }
                if let Some(hit) = queries.cast_excluding(
                    position - Vec3::Y * expansion,
                    config.height(state.stance) + expansion * 2.0,
                    config.radius + expansion,
                    movement,
                    config.skin,
                    Some(attachment.collider),
                )? {
                    position += movement * hit.fraction;
                    report.base_carry_blocked = true;
                    break;
                }
                position = target;
            }
            state.position = position.to_array();
            let rotation = q1 * q0.inverse();
            state.velocity = (rotation * Vec3::from_array(state.velocity)).to_array();
            for direction in [&mut state.facing, &mut state.dash_direction] {
                let rotated = rotation * Vec3::new(direction[0], 0.0, direction[1]);
                let horizontal = Vec2::new(rotated.x, rotated.z).normalize_or_zero();
                if horizontal != Vec2::ZERO {
                    *direction = horizontal.to_array();
                }
            }
        }
    }
    if step.detach {
        state.grounded = false;
        state.ground = None;
        state.suppress_snap_ticks = state.suppress_snap_ticks.max(1);
    }
    Ok(transition)
}

pub(crate) fn finish(
    state: &mut KinematicState,
    position: Vec3,
    velocity: &mut Vec3,
    step: BaseStep<'_>,
    transition: &mut Transition,
    report: &mut KinematicReport,
) -> Result<(), KinematicError> {
    let next = if step.detach {
        None
    } else {
        state.ground.and_then(|ground| {
            step.frame.sample(ground.collider).map(|sample| {
                let local = sample.current.rotation().inverse()
                    * (position - Vec3::from_array(sample.current.position));
                BaseAttachment {
                    collider: ground.collider,
                    scene_revision: step.frame.scene_revision,
                    pose_tick: step.frame.to_tick,
                    local_anchor: local.to_array(),
                }
            })
        })
    };
    let previous_key = transition.previous.map(|v| v.collider);
    let next_key = next.map(|v| v.collider);
    // Losing support before motion and landing back on the same collider is
    // still a new attachment: its velocity was already converted to world space.
    if previous_key != next_key || (transition.inherited && next.is_some()) {
        if let Some(next) = next {
            // Direct parent changes preserve world velocity independently of
            // the game's optional dismount inheritance coefficients.
            if transition.previous.is_some() && !transition.inherited {
                *velocity += transition.base_velocity;
                transition.inherited = true;
            }
            let sample = step
                .frame
                .sample(next.collider)
                .expect("attachment is from frame");
            *velocity -= sample
                .current
                .point_velocity(Vec3::from_array(next.local_anchor), BaseConfig::default());
        } else {
            transition.inherit_departure(velocity);
        }
        state.base.revision = state
            .base
            .revision
            .checked_add(1)
            .ok_or(KinematicError::BaseRevisionExhausted)?;
        report.base_changed = true;
    }
    state.base.attachment = next;
    Ok(())
}

//! Game pose policy over bounded engine renderer-only history. Never simulation input.
use dreamwake_sim::replication::PublicReplica;
use engine_net::{interpolation::*, replication::Payload, types::*};
use std::{collections::VecDeque, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    position: [f32; 2],
    velocity: [f32; 2],
    deterministic_motion: bool,
    motion_active: bool,
    projectile_direction: [f32; 2],
    projectile_speed: f32,
    projectile_max_distance: Option<f32>,
    paused_elapsed: f32,
    projected_seconds: f32,
    elevation: f32,
    facing: Option<[f32; 2]>,
    remaining: Option<f32>,
    effect_age: Option<f32>,
    damage_age: Option<f32>,
    hit_flash: Option<f32>,
    attack_flash: Option<f32>,
    windup: Option<f32>,
    beam_direction: Option<[f32; 3]>,
    beam_elevation: Option<f32>,
    beam_length: Option<f32>,
    cover_open: Option<bool>,
    platform_tick: Option<u32>,
    platform_motion: Option<u32>,
}
impl Payload for Pose {
    fn retained_bytes(&self) -> usize {
        0
    }
}
struct DreamPosePolicy {
    paused_elapsed: f32,
}

fn interpolate_optional(a: Option<f32>, b: Option<f32>, alpha: f32) -> Option<f32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + (b - a) * alpha),
        (a, b) => {
            if alpha < 1.0 {
                a
            } else {
                b
            }
        }
    }
}

fn interpolate_direction(a: [f32; 3], b: [f32; 3], alpha: f32) -> [f32; 3] {
    let blended = std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * alpha);
    let length = blended.into_iter().map(|v| v * v).sum::<f32>().sqrt();
    if length > 0.000_001 {
        blended.map(|v| v / length)
    } else {
        b
    }
}

fn advance_clocks(pose: &mut Pose, seconds: f32) {
    for countdown in [
        &mut pose.hit_flash,
        &mut pose.attack_flash,
        &mut pose.windup,
    ] {
        if let Some(value) = countdown {
            *value = (*value - seconds).max(0.0);
        }
    }
    if !pose.deterministic_motion
        && let Some(remaining) = &mut pose.remaining
    {
        *remaining = (*remaining - seconds).max(0.0);
    }
    for age in [&mut pose.effect_age, &mut pose.damage_age] {
        if let Some(value) = age {
            *value += seconds;
        }
    }
}

/// Declared ballistic travel plus the visual clocks. Shared by the sampled
/// projection and `apply`'s top-up so the two never disagree on how a
/// projectile advances.
fn advance_motion(pose: &mut Pose, seconds: f32) {
    if pose.deterministic_motion
        && let Some(remaining) = &mut pose.remaining
    {
        pose.motion_active = engine_core::advance_projectile_motion(
            &mut pose.position,
            pose.projectile_direction,
            pose.projectile_speed,
            remaining,
            pose.projectile_max_distance,
            seconds,
        );
    }
    advance_clocks(pose, seconds);
}

fn project_pose(mut pose: Pose, ahead: Duration, paused_elapsed: f32) -> Pose {
    let seconds = (ahead.as_secs_f32() - (paused_elapsed - pose.paused_elapsed).max(0.0)).max(0.0);
    if seconds > 0.0 {
        if !pose.deterministic_motion {
            pose.position = engine_core::spatial::integrate(pose.position, pose.velocity, seconds);
        }
        if let Some(tick) = pose.platform_tick {
            pose.platform_tick =
                Some(tick.saturating_add((seconds * dreamwake_sim::TICK_HZ as f32).floor() as u32));
        }
        advance_motion(&mut pose, seconds);
        pose.projected_seconds += seconds;
    }
    pose
}

impl PosePolicy<Pose> for DreamPosePolicy {
    fn valid(&self, pose: &Pose) -> bool {
        pose.position
            .into_iter()
            .chain([pose.elevation])
            .chain(pose.velocity)
            .chain(pose.projectile_direction)
            .chain([pose.projectile_speed])
            .chain(pose.projectile_max_distance)
            .chain([pose.paused_elapsed, pose.projected_seconds])
            .chain(pose.facing.into_iter().flatten())
            .chain(
                [
                    pose.remaining,
                    pose.effect_age,
                    pose.damage_age,
                    pose.hit_flash,
                    pose.attack_flash,
                    pose.windup,
                    pose.beam_elevation,
                    pose.beam_length,
                ]
                .into_iter()
                .flatten(),
            )
            .chain(pose.beam_direction.into_iter().flatten())
            .all(f32::is_finite)
    }
    fn interpolate(&self, a: &Pose, b: &Pose, alpha: f32) -> Option<Pose> {
        Some(Pose {
            elevation: a.elevation + (b.elevation - a.elevation) * alpha,
            velocity: std::array::from_fn(|i| {
                a.velocity[i] + (b.velocity[i] - a.velocity[i]) * alpha
            }),
            deterministic_motion: if alpha < 1.0 {
                a.deterministic_motion
            } else {
                b.deterministic_motion
            },
            motion_active: if alpha < 1.0 {
                a.motion_active
            } else {
                b.motion_active
            },
            projectile_direction: std::array::from_fn(|axis| {
                a.projectile_direction[axis]
                    + (b.projectile_direction[axis] - a.projectile_direction[axis]) * alpha
            }),
            projectile_speed: a.projectile_speed
                + (b.projectile_speed - a.projectile_speed) * alpha,
            projectile_max_distance: if alpha < 1.0 {
                a.projectile_max_distance
            } else {
                b.projectile_max_distance
            },
            paused_elapsed: a.paused_elapsed + (b.paused_elapsed - a.paused_elapsed) * alpha,
            projected_seconds: 0.0,
            remaining: interpolate_optional(a.remaining, b.remaining, alpha),
            effect_age: interpolate_optional(a.effect_age, b.effect_age, alpha),
            damage_age: interpolate_optional(a.damage_age, b.damage_age, alpha),
            hit_flash: interpolate_optional(a.hit_flash, b.hit_flash, alpha),
            attack_flash: interpolate_optional(a.attack_flash, b.attack_flash, alpha),
            windup: interpolate_optional(a.windup, b.windup, alpha),
            beam_direction: a
                .beam_direction
                .zip(b.beam_direction)
                .map(|(a, b)| interpolate_direction(a, b, alpha)),
            beam_elevation: interpolate_optional(a.beam_elevation, b.beam_elevation, alpha),
            beam_length: interpolate_optional(a.beam_length, b.beam_length, alpha),
            platform_tick: a.platform_tick.zip(b.platform_tick).and_then(|(a, b)| {
                b.checked_sub(a)
                    .map(|span| (f64::from(a) + f64::from(span) * f64::from(alpha)).floor() as u32)
            }),
            platform_motion: a.platform_motion,
            cover_open: if alpha < 1.0 {
                a.cover_open
            } else {
                b.cover_open
            },
            position: std::array::from_fn(|i| {
                a.position[i] + (b.position[i] - a.position[i]) * alpha
            }),
            facing: a.facing.zip(b.facing).map(|(a, b)| {
                let start = a[1].atan2(a[0]);
                let end = b[1].atan2(b[0]);
                let angle = start
                    + ((end - start + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                        - std::f32::consts::PI)
                        * alpha;
                [angle.cos(), angle.sin()]
            }),
        })
    }
    fn project_latest(&self, latest: &Pose, ahead: Duration) -> Option<Pose> {
        Some(project_pose(latest.clone(), ahead, self.paused_elapsed))
    }
    fn extrapolate(&self, _: &Pose, latest: &Pose, _: Duration, ahead: Duration) -> Option<Pose> {
        Some(project_pose(latest.clone(), ahead, self.paused_elapsed))
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RemotePoseMetrics {
    pub known_entities: usize,
    pub active_entities: usize,
    pub retained_samples: usize,
    pub retained_bytes: usize,
    pub cursor: Duration,
    pub buffering: usize,
    pub interpolated: usize,
    pub extrapolated: usize,
    pub frozen: usize,
    pub maximum_pose_age: Duration,
}
pub(super) struct RemotePoses {
    buffer: RemoteBuffer<Pose>,
    scene: SceneRevision,
    combat_entities: std::collections::BTreeSet<EntityId>,
    paused: bool,
    pause_started_at: Option<Duration>,
    pause_history: VecDeque<(Duration, Duration)>,
    retired_paused: Duration,
}
impl RemotePoses {
    /// Pause accounting decides whether a pose advances, so every sample site
    /// reads it from here rather than re-spelling the cursor lookup.
    fn policy(&self) -> DreamPosePolicy {
        DreamPosePolicy {
            paused_elapsed: self.paused_elapsed_at(self.buffer.cursor()).as_secs_f32(),
        }
    }
    pub fn metrics(&self) -> RemotePoseMetrics {
        let accounting = self.buffer.accounting();
        let mut metrics = RemotePoseMetrics {
            known_entities: accounting.known_entities,
            active_entities: accounting.active_entities,
            retained_samples: accounting.samples,
            retained_bytes: accounting.retained_bytes,
            cursor: self.buffer.cursor(),
            ..Default::default()
        };
        let policy = self.policy();
        for identity in self.buffer.identities() {
            if let Ok(Some(pose)) = self.buffer.sample(identity.scope, &policy) {
                metrics.maximum_pose_age = metrics.maximum_pose_age.max(pose.total_age);
                match pose.status {
                    Status::Buffering => metrics.buffering += 1,
                    Status::Interpolated => metrics.interpolated += 1,
                    Status::Extrapolated => metrics.extrapolated += 1,
                    Status::Frozen(_) => metrics.frozen += 1,
                    Status::Sampled => {}
                }
            }
        }
        metrics
    }

    pub fn new(connection: ConnectionEpoch, scene: SceneRevision) -> Result<Self, Error> {
        Ok(Self {
            buffer: RemoteBuffer::new(
                connection,
                Config {
                    rate: TickRate::new(dreamwake_sim::TICK_HZ).unwrap(),
                    delay: Duration::from_millis(100),
                    // Public replicas carry authoritative capability motion. Its
                    // positional projection remains bounded during starvation.
                    extrapolation_cap: Duration::from_millis(100),
                    limits: Limits::default(),
                },
            )?,
            scene,
            combat_entities: Default::default(),
            paused: false,
            pause_started_at: None,
            pause_history: VecDeque::new(),
            retired_paused: Duration::ZERO,
        })
    }
    pub fn set_paused(&mut self, paused: bool, authority_tick: ServerTick) {
        let Some(at) =
            TickRate::new(dreamwake_sim::TICK_HZ).and_then(|rate| rate.deadline(authority_tick))
        else {
            return;
        };
        if paused && !self.paused {
            self.pause_started_at = Some(at);
        } else if !paused
            && self.paused
            && let Some(started) = self.pause_started_at.take()
        {
            self.pause_history.push_back((started, at.max(started)));
            if self.pause_history.len() > 64
                && let Some((start, end)) = self.pause_history.pop_front()
            {
                self.retired_paused += end.saturating_sub(start);
            }
        }
        self.paused = paused;
    }

    fn paused_elapsed_at(&self, at: Duration) -> Duration {
        let completed = self
            .pause_history
            .iter()
            .fold(self.retired_paused, |total, &(started, ended)| {
                total + at.min(ended).saturating_sub(started)
            });
        completed
            + self
                .pause_started_at
                .map_or(Duration::ZERO, |started| at.saturating_sub(started))
    }

    pub fn presentation_elapsed(
        &self,
        global: &dreamwake_sim::replication::DreamGlobalView,
    ) -> f32 {
        if global.paused {
            return global.elapsed;
        }
        let Some(authority_time) = TickRate::new(dreamwake_sim::TICK_HZ)
            .and_then(|rate| rate.deadline(ServerTick(global.stamp.server_tick)))
        else {
            return global.elapsed;
        };
        let offset = self.buffer.cursor().as_secs_f64() - authority_time.as_secs_f64();
        (f64::from(global.elapsed) + offset).max(0.0) as f32
    }
    pub fn receive(
        &mut self,
        scope: ScopeIdentity,
        tick: ServerTick,
        actor: &PublicReplica,
    ) -> Result<(), Error> {
        if matches!(actor, PublicReplica::CoverMarker(_)) {
            return Ok(());
        }
        // Everything the segment check below needs, before the pose itself.
        let (position, velocity, facing, platform_motion) = match actor {
            PublicReplica::Hero(v) => (v.position, v.velocity, Some(v.facing), None),
            PublicReplica::Enemy(v) => (v.position, v.velocity, Some(v.facing), None),
            PublicReplica::Projectile(v) => (
                v.position,
                v.direction.map(|component| component * v.speed),
                Some(v.direction),
                None,
            ),
            PublicReplica::Wisp(v) => (v.position, v.velocity, None, None),
            PublicReplica::Effect(v) => (v.position, [0.0; 2], None, None),
            PublicReplica::Damage(v) => (v.position, [0.0; 2], None, None),
            PublicReplica::Cover(v) => ([v.position[0], v.position[2]], [0.0; 2], None, None),
            PublicReplica::Platform(v) => (
                [v.pose.position[0], v.pose.position[2]],
                [0.0; 2],
                None,
                Some(v.motion_revision),
            ),
            PublicReplica::CoverMarker(_) => unreachable!("attachments use their parent pose"),
        };
        let mut segment = self
            .buffer
            .identity(scope.entity)
            .filter(|v| v.scope == scope)
            .map_or(1, |v| v.segment);
        if let Some(previous) = self.buffer.latest(scope) {
            let distance = engine_core::spatial::distance(position, previous.value.position);
            // Delivery gaps do not prove a movement discontinuity. Preserve the
            // bounded history so late samples can still bracket the render time.
            if tick > previous.tick
                && (distance > 8.0 || platform_motion != previous.value.platform_motion)
            {
                segment = segment.checked_add(1).ok_or(Error::Capacity)?;
            }
        }
        let mut pose = Pose {
            position,
            velocity,
            facing,
            platform_motion,
            deterministic_motion: false,
            motion_active: true,
            projectile_direction: [0.0; 2],
            projectile_speed: 0.0,
            projectile_max_distance: None,
            paused_elapsed: TickRate::new(dreamwake_sim::TICK_HZ)
                .and_then(|rate| rate.deadline(tick))
                .map_or(Duration::ZERO, |at| self.paused_elapsed_at(at))
                .as_secs_f32(),
            projected_seconds: 0.0,
            elevation: 0.0,
            remaining: None,
            effect_age: None,
            damage_age: None,
            hit_flash: None,
            attack_flash: None,
            windup: None,
            beam_direction: None,
            beam_elevation: None,
            beam_length: None,
            platform_tick: None,
            cover_open: None,
        };
        // One arm per replica, so adding a variant or a field cannot leave a
        // silent default behind in an arm nobody thought to update.
        match actor {
            PublicReplica::Hero(v) => {
                pose.elevation = v.elevation;
                pose.hit_flash = Some(v.hit_flash);
                pose.attack_flash = Some(v.attack_flash);
            }
            PublicReplica::Enemy(v) => {
                pose.hit_flash = Some(v.hit_flash);
                pose.windup = Some(v.windup);
            }
            PublicReplica::Projectile(v) => {
                pose.deterministic_motion = true;
                pose.projectile_direction = v.direction;
                pose.projectile_speed = v.speed;
                pose.projectile_max_distance = v.max_distance;
                pose.remaining = Some(v.remaining);
            }
            PublicReplica::Wisp(v) => pose.remaining = Some(v.remaining),
            PublicReplica::Effect(v) => {
                pose.effect_age = Some(v.age_ticks as f32 / dreamwake_sim::TICK_HZ as f32);
                if let engine_core::GraphicsKind::Beam {
                    direction,
                    elevation,
                    length,
                    ..
                } = v.kind
                {
                    pose.beam_direction = Some(direction);
                    pose.beam_elevation = Some(elevation);
                    pose.beam_length = Some(length);
                }
            }
            PublicReplica::Damage(v) => pose.damage_age = Some(v.age),
            PublicReplica::Cover(v) => {
                pose.elevation = v.position[1];
                pose.cover_open = Some(v.open);
            }
            PublicReplica::Platform(v) => {
                pose.elevation = v.pose.position[1];
                pose.platform_tick = Some(v.gameplay_tick);
            }
            PublicReplica::CoverMarker(_) => unreachable!("attachments use their parent pose"),
        }
        let policy = self.policy();
        self.buffer.receive(
            RemoteSample {
                identity: RemoteIdentity {
                    scope,
                    scene: self.scene,
                    segment,
                },
                tick,
                value: pose,
            },
            &policy,
        )?;
        if self
            .buffer
            .latest(scope)
            .is_some_and(|latest| latest.tick == tick)
        {
            // The buffer has accepted this identity and replaced its index's
            // track. Old generation removals may never arrive on the control lane.
            let first = EntityId {
                generation: 0,
                ..scope.entity
            };
            while let Some(old) = self
                .combat_entities
                .range(first..scope.entity)
                .next()
                .copied()
            {
                self.combat_entities.remove(&old);
            }
            if matches!(actor, PublicReplica::Enemy(_))
                || matches!(actor, PublicReplica::Cover(cover) if cover.present)
            {
                // Dreamwake rays query enemies and covers. Friendly heroes are
                // neither targets nor occluders, and their poses use their own time.
                self.combat_entities.insert(scope.entity);
            } else {
                // Removed cover remains a replicated region descriptor, but no
                // longer participates in collision or supplies a rendered blocker.
                self.combat_entities.remove(&scope.entity);
            }
        }
        Ok(())
    }
    pub fn remove(&mut self, entity: EntityId) {
        self.combat_entities.remove(&entity);
        if let Some(identity) = self.buffer.identity(entity) {
            let _ = self.buffer.exit(identity.scope);
        }
    }
    /// Only a coherent rendered combat time can authorize a mixed-time query.
    /// The playback cursor alone is insufficient when a target or cover has frozen.
    pub fn combat_view_time(&self) -> Option<Duration> {
        let mut common = None;
        let policy = self.policy();
        for entity in &self.combat_entities {
            let identity = self.buffer.identity(*entity)?;
            let shown = self.buffer.sample(identity.scope, &policy).ok()??;
            if matches!(shown.status, Status::Buffering) {
                return None;
            }
            // Agreement covers how the time was reached, not just its value. A
            // member carried past its last sample reports the same stamp as one
            // an observation actually brackets, so comparing stamps alone lets
            // a starved target resolve combat against a pose the server never
            // published. A cohort starved together still shares one displayed
            // time and stays usable.
            let observed = matches!(shown.status, Status::Sampled | Status::Interpolated);
            if common.is_some_and(|previous| previous != (shown.pose_time, observed)) {
                return None;
            }
            common = Some((shown.pose_time, observed));
        }
        common
            .map(|(time, _)| time)
            .or_else(|| Some(self.buffer.cursor()))
    }
    pub fn combat_view_tick(&self) -> Option<ServerTick> {
        TickRate::new(dreamwake_sim::TICK_HZ)?.elapsed_ticks(self.combat_view_time()?)
    }
    pub fn advance_time(&mut self, elapsed: Duration) {
        self.buffer.advance(elapsed);
    }
    #[cfg(test)]
    pub fn advance(&mut self, estimated: ServerTick) {
        if let Some(now) = TickRate::new(dreamwake_sim::TICK_HZ)
            .unwrap()
            .deadline(estimated)
        {
            self.advance_time(now);
        }
    }
    pub fn apply(&self, scope: ScopeIdentity, actor: &mut PublicReplica) -> bool {
        let Ok(Some(shown)) = self.buffer.sample(scope, &self.policy()) else {
            return true;
        };
        let mut pose = shown.value;
        if let Some(latest) = self.buffer.latest(scope)
            && let Some(latest_time) =
                TickRate::new(dreamwake_sim::TICK_HZ).and_then(|rate| rate.deadline(latest.tick))
        {
            let active_ahead = (shown.cursor.saturating_sub(latest_time).as_secs_f32()
                - (self.paused_elapsed_at(shown.cursor).as_secs_f32()
                    - latest.value.paused_elapsed)
                    .max(0.0))
            .max(0.0);
            let additional = (active_ahead - pose.projected_seconds).max(0.0);
            // Visual clocks and declared ballistic motion are deterministic and
            // can finish from one accepted sample. Actor tangent drift remains
            // at the engine's starvation cap.
            advance_motion(&mut pose, additional);
        }
        match actor {
            PublicReplica::Hero(v) => {
                v.position = pose.position;
                v.elevation = pose.elevation;
                if let Some(facing) = pose.facing {
                    v.facing = facing;
                }
                if let Some(hit_flash) = pose.hit_flash {
                    v.hit_flash = hit_flash;
                }
                if let Some(attack_flash) = pose.attack_flash {
                    v.attack_flash = attack_flash;
                }
            }
            PublicReplica::Enemy(v) => {
                v.position = pose.position;
                if let Some(facing) = pose.facing {
                    v.facing = facing;
                }
                if let Some(windup) = pose.windup {
                    v.windup = windup;
                }
                if let Some(hit_flash) = pose.hit_flash {
                    v.hit_flash = hit_flash;
                }
            }
            PublicReplica::Projectile(v) => {
                v.position = pose.position;
                if let Some(facing) = pose.facing {
                    v.direction = facing;
                }
                if let Some(remaining) = pose.remaining {
                    v.remaining = remaining;
                }
            }
            PublicReplica::Wisp(v) => {
                v.position = pose.position;
                if let Some(remaining) = pose.remaining {
                    v.remaining = remaining;
                }
            }
            PublicReplica::Effect(v) => {
                v.position = pose.position;
                if let Some(age) = pose.effect_age {
                    v.age_ticks = (age * dreamwake_sim::TICK_HZ as f32)
                        .floor()
                        .min(v.duration_ticks as f32) as u32;
                }
                if let engine_core::GraphicsKind::Beam {
                    direction,
                    elevation,
                    length,
                } = &mut v.kind
                {
                    if let Some(value) = pose.beam_direction {
                        *direction = value;
                    }
                    if let Some(value) = pose.beam_elevation {
                        *elevation = value;
                    }
                    if let Some(value) = pose.beam_length {
                        *length = value;
                    }
                }
            }
            PublicReplica::Damage(v) => {
                v.position = pose.position;
                if let Some(age) = pose.damage_age {
                    v.age = age;
                }
            }
            PublicReplica::Platform(v) => {
                if pose.platform_motion == Some(v.motion_revision)
                    && let Some(tick) = pose.platform_tick
                    && let Ok(shown) = v.pose_at(tick)
                {
                    v.pose = shown;
                    v.gameplay_tick = tick;
                    v.phase = ((tick - v.origin_gameplay_tick)
                        % u32::from(dreamwake_sim::platform::PLATFORM_PERIOD_TICKS))
                        as u16;
                }
            }
            PublicReplica::Cover(v) => {
                v.position[0] = pose.position[0];
                v.position[1] = pose.elevation;
                v.position[2] = pose.position[1];
                if let Some(open) = pose.cover_open {
                    v.open = open;
                }
            }
            PublicReplica::CoverMarker(_) => {}
        }
        match actor {
            PublicReplica::Projectile(v) => pose.motion_active && v.remaining > 0.0,
            PublicReplica::Wisp(v) => v.remaining > 0.0,
            PublicReplica::Effect(v) => v.age_ticks < v.duration_ticks,
            PublicReplica::Damage(v) => v.age <= 0.85,
            _ => true,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newer_generation_retires_combat_membership_without_old_destruction() {
        for combat_replacement in [true, false] {
            let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
            for generation in 1..=32 {
                let scope = ScopeIdentity {
                    connection: ConnectionEpoch(1),
                    entity: EntityId {
                        index: 7,
                        generation,
                    },
                    scope: ScopeEpoch(1),
                    representation: RepresentationRevision(1),
                };
                let combat = generation == 1 || combat_replacement;
                let actor = if combat {
                    PublicReplica::Enemy(dreamwake_sim::replication::PublicEnemyView {
                        id: 7,
                        position: [0.0; 2],
                        facing: [1.0, 0.0],
                        velocity: [0.0, 0.0],
                        hp: 10.0,
                        max_hp: 10.0,
                        kind: dreamwake_sim::EnemyKind::Melee,
                        windup: 0.0,
                        target: [0.0; 2],
                        warn_radius: 0.0,
                        phase: 0,
                        slowed: false,
                        hit_flash: 0.0,
                    })
                } else {
                    actor(0.0)
                };
                poses.receive(scope, ServerTick(60), &actor).unwrap();
                poses.receive(scope, ServerTick(66), &actor).unwrap();
                poses.advance(ServerTick(69));
                assert_eq!(poses.combat_entities.len(), usize::from(combat));
                assert_eq!(poses.combat_view_tick(), Some(ServerTick(63)));
                if generation > 1 {
                    poses.remove(EntityId {
                        generation: generation - 1,
                        ..scope.entity
                    });
                    assert_eq!(poses.buffer.identity(scope.entity).unwrap().scope, scope);
                    assert_eq!(poses.combat_view_tick(), Some(ServerTick(63)));
                }
                assert_eq!(poses.metrics().known_entities, 1);
            }
        }
    }

    #[test]
    fn shutter_height_interpolates_each_frame_in_both_directions() {
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 7,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        for (from, to, open) in [(1.0, 1.24, false), (3.4, 3.16, true)] {
            let cover = |height| {
                PublicReplica::Cover(dreamwake_sim::replication::PublicCoverView {
                    id: 7,
                    revision: 1,
                    present: true,
                    position: [0.0, height, -6.0],
                    half_extents: [1.5, 1.0, 0.15],
                    open,
                })
            };
            let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
            poses.receive(scope, ServerTick(60), &cover(from)).unwrap();
            poses.receive(scope, ServerTick(63), &cover(to)).unwrap();
            for frame in 1..6 {
                poses.advance_time(
                    Duration::from_millis(1100) + Duration::from_secs_f64(f64::from(frame) / 120.0),
                );
                let mut shown = cover(to);
                poses.apply(scope, &mut shown);
                let PublicReplica::Cover(shown) = shown else {
                    unreachable!()
                };
                let expected = from + (to - from) * frame as f32 / 6.0;
                assert!((shown.position[1] - expected).abs() < 0.0001);
                assert!(shown.valid());
                assert_eq!(shown.open, open);
            }
        }
    }
    #[test]
    fn platform_pose_follows_common_render_time_and_cuts_new_motion_episodes() {
        let base = dreamwake_sim::DreamSimulation::new(7, false)
            .snapshot()
            .platforms[0];
        let platform = |tick| {
            let mut v = base;
            v.pose = base.pose_at(tick).unwrap();
            v.gameplay_tick = tick;
            v.phase = tick as u16;
            PublicReplica::Platform(v)
        };
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 7,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses.receive(scope, ServerTick(60), &platform(60)).unwrap();
        poses.receive(scope, ServerTick(66), &platform(66)).unwrap();
        poses.advance(ServerTick(70));
        let mut shown = platform(66);
        poses.apply(scope, &mut shown);
        let PublicReplica::Platform(rendered) = shown else {
            unreachable!()
        };
        assert_eq!(rendered.gameplay_tick, 64);
        assert_eq!(rendered.pose, base.pose_at(64).unwrap());
        assert!(rendered.valid());
        assert_eq!(
            base.gameplay_tick, 0,
            "the immutable dependency is never changed by presentation"
        );

        let restarted = dreamwake_sim::replication::PublicPlatformView {
            gameplay_tick: 72,
            origin_gameplay_tick: 72,
            motion_revision: 2,
            ..base
        };
        assert!(restarted.valid());
        poses
            .receive(scope, ServerTick(72), &PublicReplica::Platform(restarted))
            .unwrap();
        poses.advance(ServerTick(76));
        let mut shown = PublicReplica::Platform(restarted);
        poses.apply(scope, &mut shown);
        assert_eq!(shown, PublicReplica::Platform(restarted));
        assert_eq!(poses.buffer.identity(scope.entity).unwrap().segment, 2);
    }
    fn actor(x: f32) -> PublicReplica {
        PublicReplica::Damage(dreamwake_sim::replication::PublicDamageView {
            id: 7,
            position: [x, 0.0],
            amount: 1.0,
            critical: false,
            friendly: false,
            age: 0.0,
        })
    }
    #[test]
    fn remote_poses_use_engine_history_scope_fences_and_freeze_policy() {
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 7,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses.receive(scope, ServerTick(60), &actor(0.0)).unwrap();
        poses.receive(scope, ServerTick(66), &actor(6.0)).unwrap();
        poses.advance(ServerTick(69));
        let mut shown = actor(6.0);
        poses.apply(scope, &mut shown);
        assert_eq!(shown, actor(3.0));
        poses.advance(ServerTick(100));
        poses.apply(scope, &mut shown);
        let PublicReplica::Damage(shown_damage) = shown else {
            unreachable!()
        };
        assert_eq!(shown_damage.position, [6.0, 0.0]);
        assert!((shown_damage.age - 28.0 / 60.0).abs() < 0.001);
        assert_eq!(
            poses
                .buffer
                .sample(scope, &poses.policy())
                .unwrap()
                .unwrap()
                .status,
            Status::Frozen(FreezeReason::ExtrapolationLimit)
        );
        poses.remove(scope.entity);
        assert!(poses.receive(scope, ServerTick(67), &actor(7.0)).is_err());
        let next = ScopeIdentity {
            scope: ScopeEpoch(2),
            ..scope
        };
        poses.receive(next, ServerTick(101), &actor(50.0)).unwrap();
        poses.apply(next, &mut shown);
        assert_eq!(shown, actor(50.0));
        assert_eq!(poses.buffer.accounting().known_entities, 1);
    }
    #[test]
    fn delayed_enemy_samples_interpolate_then_use_authoritative_velocity_with_a_bound() {
        let enemy = |x| {
            PublicReplica::Enemy(dreamwake_sim::replication::PublicEnemyView {
                id: 7,
                position: [x, 0.0],
                facing: [1.0, 0.0],
                velocity: [10.0, 0.0],
                hp: 100.0,
                max_hp: 100.0,
                kind: dreamwake_sim::EnemyKind::Melee,
                windup: 0.0,
                target: [0.0, 0.0],
                warn_radius: 0.0,
                phase: 0,
                slowed: false,
                hit_flash: 0.0,
            })
        };
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 7,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses.receive(scope, ServerTick(60), &enemy(0.0)).unwrap();
        poses.receive(scope, ServerTick(78), &enemy(3.0)).unwrap();
        poses.advance(ServerTick(81)); // Render tick 75 lies inside the delivery gap.
        let mut shown = enemy(3.0);
        poses.apply(scope, &mut shown);
        assert_eq!(shown, enemy(2.5));
        assert_eq!(poses.metrics().interpolated, 1);
        assert_eq!(poses.buffer.accounting().samples, 2);
        assert_eq!(poses.buffer.identity(scope.entity).unwrap().segment, 1);
        assert_eq!(poses.combat_view_tick(), Some(ServerTick(75)));

        // A reordered sample fills the same history without moving its cursor.
        poses.receive(scope, ServerTick(69), &enemy(1.5)).unwrap();
        poses.apply(scope, &mut shown);
        assert_eq!(shown, enemy(2.5));
        assert_eq!(poses.buffer.accounting().samples, 3);
        assert_eq!(poses.combat_view_tick(), Some(ServerTick(75)));

        poses.advance(ServerTick(90));
        poses.apply(scope, &mut shown);
        assert_eq!(shown, enemy(4.0));
        assert_eq!(poses.metrics().extrapolated, 1);
        assert_eq!(poses.combat_view_tick(), Some(ServerTick(84)));

        poses.advance(ServerTick(100));
        poses.apply(scope, &mut shown);
        assert_eq!(shown, enemy(4.0));
        assert_eq!(poses.metrics().frozen, 1);
        assert_eq!(poses.combat_view_tick(), Some(ServerTick(84)));
    }

    fn projectile(position: [f32; 2], remaining: f32) -> PublicReplica {
        PublicReplica::Projectile(dreamwake_sim::replication::PublicProjectileView {
            id: 71,
            owner: Some(1),
            position,
            direction: [1.0, 0.0],
            speed: 10.0,
            remaining,
            max_distance: Some(64.0),
            radius: 0.3,
            friendly: false,
            essence: None,
        })
    }

    #[test]
    fn one_projectile_sample_drives_bounded_motion_and_full_deterministic_lifetime() {
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 71,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses
            .receive(scope, ServerTick(60), &projectile([0.0, 0.0], 1.0))
            .unwrap();

        poses.advance(ServerTick(69));
        let mut shown = projectile([0.0, 0.0], 1.0);
        assert!(poses.apply(scope, &mut shown));
        let PublicReplica::Projectile(shown) = shown else {
            unreachable!()
        };
        assert!((shown.position[0] - 0.5).abs() < 0.001);
        assert!((shown.remaining - 0.95).abs() < 0.001);

        poses.advance(ServerTick(120));
        let mut shown = projectile([0.0, 0.0], 1.0);
        assert!(poses.apply(scope, &mut shown));
        let PublicReplica::Projectile(shown) = shown else {
            unreachable!()
        };
        assert!((shown.position[0] - 9.0).abs() < 0.001);
        assert!((shown.remaining - 0.1).abs() < 0.001);

        poses.advance(ServerTick(126));
        let mut expired = projectile([0.0, 0.0], 1.0);
        assert!(!poses.apply(scope, &mut expired));
    }

    #[test]
    fn projected_projectile_respects_authoritative_world_distance() {
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 75,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let mut authority = projectile([0.7, 0.0], 2.0);
        let PublicReplica::Projectile(projectile) = &mut authority else {
            unreachable!()
        };
        projectile.max_distance = Some(0.75);
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses.receive(scope, ServerTick(60), &authority).unwrap();
        poses.advance(ServerTick(69));
        assert!(!poses.apply(scope, &mut authority));
    }

    #[test]
    fn pause_freezes_capability_motion_and_clocks() {
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 72,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let authority = projectile([2.0, 3.0], 1.0);
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses.receive(scope, ServerTick(60), &authority).unwrap();
        poses.advance(ServerTick(66));
        poses.set_paused(true, ServerTick(66));
        poses.advance(ServerTick(126));
        let mut shown = authority.clone();
        assert!(poses.apply(scope, &mut shown));
        let PublicReplica::Projectile(shown) = shown else {
            unreachable!()
        };
        assert!((shown.position[0] - 3.0).abs() < 0.001);
        assert!((shown.remaining - 0.9).abs() < 0.001);

        poses.set_paused(false, ServerTick(126));
        poses.advance(ServerTick(138));
        let mut resumed = authority.clone();
        assert!(poses.apply(scope, &mut resumed));
        let PublicReplica::Projectile(resumed) = resumed else {
            unreachable!()
        };
        assert!((resumed.position[0] - 4.0).abs() < 0.001);
        assert!((resumed.remaining - 0.8).abs() < 0.001);
    }

    #[test]
    fn authoritative_projectile_correction_replaces_the_projected_path() {
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 73,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses
            .receive(scope, ServerTick(60), &projectile([0.0, 0.0], 2.0))
            .unwrap();
        poses
            .receive(scope, ServerTick(66), &projectile([2.0, 0.0], 1.9))
            .unwrap();
        poses.advance(ServerTick(69));
        let mut shown = projectile([2.0, 0.0], 1.9);
        assert!(poses.apply(scope, &mut shown));
        let PublicReplica::Projectile(shown) = shown else {
            unreachable!()
        };
        assert!((shown.position[0] - 1.0).abs() < 0.001);
        assert!((shown.remaining - 1.95).abs() < 0.001);
    }

    #[test]
    fn one_effect_sample_ages_and_expires_without_another_packet() {
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 74,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let effect = PublicReplica::Effect(dreamwake_sim::replication::PublicEffectView {
            id: engine_core::GraphicsId {
                scope: None,
                match_epoch: 1,
                owner: 1,
                action_seq: 1,
                slot: 1,
            },
            kind: engine_core::GraphicsKind::RadialPulse,
            position: [0.0, 0.0],
            radius: 2.0,
            age_ticks: 0,
            duration_ticks: 30,
        });
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses.receive(scope, ServerTick(60), &effect).unwrap();
        poses.advance(ServerTick(69));
        let mut shown = effect.clone();
        assert!(poses.apply(scope, &mut shown));
        assert!(matches!(shown, PublicReplica::Effect(v) if v.age_ticks == 3));

        poses.advance(ServerTick(96));
        let mut expired = effect;
        assert!(!poses.apply(scope, &mut expired));
    }
    #[test]
    fn teleport_resets_engine_segment_instead_of_blending_journey() {
        let scope = ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 7,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        poses.receive(scope, ServerTick(60), &actor(0.0)).unwrap();
        poses.receive(scope, ServerTick(66), &actor(50.0)).unwrap();
        poses.advance(ServerTick(69));
        let mut shown = actor(0.0);
        poses.apply(scope, &mut shown);
        assert_eq!(shown, actor(50.0));
        assert_eq!(poses.buffer.identity(scope.entity).unwrap().segment, 2);
    }
    #[test]
    fn combat_cohort_ignores_friendly_poses_but_refuses_target_starvation() {
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        let scope = |index| ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let cover = |id, x, open| {
            PublicReplica::Cover(dreamwake_sim::replication::PublicCoverView {
                revision: 1,
                present: true,
                id,
                position: [x, if open { 3.4 } else { 1.0 }, 0.0],
                half_extents: [0.5, 1.0, 2.0],
                open,
            })
        };
        // A friendly public pose can stop updating while the owner's predicted
        // pose advances. It must not invalidate coherent enemy/cover rendering.
        poses
            .receive(
                scope(3),
                ServerTick(40),
                &PublicReplica::Hero(dreamwake_sim::replication::PublicHeroView {
                    id: 3,
                    color: [82, 242, 242],
                    position: [0.0, 0.0],
                    elevation: 0.0,
                    crouched: false,
                    facing: [1.0, 0.0],
                    velocity: [0.0, 0.0],
                    hp: 100.0,
                    max_hp: 100.0,
                    shield: 0.0,
                    level: 1,
                    invulnerable: false,
                    dashing: false,
                    combo: 0,
                    hit_flash: 0.0,
                    attack_flash: 0.0,
                }),
            )
            .unwrap();
        for tick in [60, 63, 66] {
            poses
                .receive(scope(1), ServerTick(tick), &cover(1, 0.0, tick == 66))
                .unwrap();
            poses
                .receive(
                    scope(2),
                    ServerTick(tick),
                    &cover(2, (tick - 60) as f32, false),
                )
                .unwrap();
        }
        poses.advance(ServerTick(70)); // render R64, before the opening at66
        assert_eq!(poses.combat_view_tick(), Some(ServerTick(64)));
        let mut shown = cover(1, 0.0, true);
        poses.apply(scope(1), &mut shown);
        assert!(
            matches!(shown,PublicReplica::Cover(value) if !value.open && (value.position[1] - 1.8).abs() < 0.0001)
        );
        poses
            .receive(scope(2), ServerTick(69), &cover(2, 9.0, false))
            .unwrap();
        poses.advance(ServerTick(74)); // one scope froze66 while another presents68
        assert_eq!(poses.combat_view_tick(), None);
        poses
            .receive(scope(1), ServerTick(69), &cover(1, 0.0, true))
            .unwrap();
        assert_eq!(poses.combat_view_tick(), Some(ServerTick(68)));
        poses.apply(scope(1), &mut shown);
        assert!(
            matches!(shown,PublicReplica::Cover(value) if value.open && value.position[1] == 3.4)
        );
        let PublicReplica::Cover(mut removed) = cover(1, 0.0, true) else {
            unreachable!()
        };
        removed.present = false;
        poses
            .receive(scope(1), ServerTick(70), &PublicReplica::Cover(removed))
            .unwrap();
        poses
            .receive(scope(2), ServerTick(72), &cover(2, 12.0, false))
            .unwrap();
        poses.advance(ServerTick(77));
        assert_eq!(
            poses.combat_view_tick(),
            Some(ServerTick(71)),
            "dormant absence must not block the remaining combat cohort"
        );
    }
    #[test]
    fn combat_time_keeps_fraction_and_rejects_same_tick_starvation_mismatch() {
        let mut poses = RemotePoses::new(ConnectionEpoch(1), SceneRevision(1)).unwrap();
        let scope = |index| ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        };
        let cover = |id| {
            PublicReplica::Cover(dreamwake_sim::replication::PublicCoverView {
                id,
                revision: 1,
                present: true,
                position: [0.0, 1.0, -6.0],
                half_extents: [1.5, 1.0, 0.15],
                open: false,
            })
        };
        for (id, last) in [(1, 64), (2, 66)] {
            poses
                .receive(scope(id), ServerTick(60), &cover(u64::from(id)))
                .unwrap();
            poses
                .receive(scope(id), ServerTick(last), &cover(u64::from(id)))
                .unwrap();
        }
        poses.advance_time(Duration::from_millis(1175)); // cursor64.5, one actor frozen64
        assert_eq!(poses.combat_view_time(), None);
        poses.receive(scope(1), ServerTick(66), &cover(1)).unwrap();
        assert_eq!(poses.combat_view_time(), Some(Duration::from_millis(1075)));
        assert_eq!(
            TickRate::new(60)
                .unwrap()
                .elapsed_tick_phase(poses.combat_view_time().unwrap()),
            Some((ServerTick(64), 32768))
        );
        poses.advance_time(Duration::from_millis(1300));
        assert_eq!(poses.combat_view_time(), Some(Duration::from_millis(1100)));
    }
}

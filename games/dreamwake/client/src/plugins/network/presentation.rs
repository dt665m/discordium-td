//! Game pose policy over bounded engine renderer-only history. Never simulation input.
use dreamwake_sim::replication::PublicReplica;
use engine_net::{interpolation::*, replication::Payload, types::*};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    position: [f32; 2],
    elevation: f32,
    facing: Option<[f32; 2]>,
    cover_open: Option<bool>,
    platform_tick: Option<u32>,
    platform_motion: Option<u32>,
}
impl Payload for Pose {
    fn retained_bytes(&self) -> usize {
        0
    }
}
struct DreamPosePolicy;
impl PosePolicy<Pose> for DreamPosePolicy {
    fn valid(&self, pose: &Pose) -> bool {
        pose.position
            .into_iter()
            .chain([pose.elevation])
            .chain(pose.facing.into_iter().flatten())
            .all(f32::is_finite)
    }
    fn interpolate(&self, a: &Pose, b: &Pose, alpha: f32) -> Option<Pose> {
        Some(Pose {
            elevation: a.elevation + (b.elevation - a.elevation) * alpha,
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
    fn extrapolate(&self, _: &Pose, _: &Pose, _: Duration, _: Duration) -> Option<Pose> {
        None
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
}
impl RemotePoses {
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
        for identity in self.buffer.identities() {
            if let Ok(Some(pose)) = self.buffer.sample(identity.scope, &DreamPosePolicy) {
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
                    // No trustworthy remote velocity is currently projected by Dreamwake.
                    // Starvation explicitly freezes rather than extrapolating guessed motion.
                    extrapolation_cap: Duration::ZERO,
                    limits: Limits::default(),
                },
            )?,
            scene,
            combat_entities: Default::default(),
        })
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
        let (position, facing) = match actor {
            PublicReplica::Hero(v) => (v.position, Some(v.facing)),
            PublicReplica::Enemy(v) => (v.position, Some(v.facing)),
            PublicReplica::Projectile(v) => (v.position, Some(v.direction)),
            PublicReplica::Wisp(v) => (v.position, None),
            PublicReplica::Effect(v) => (v.position, None),
            PublicReplica::Damage(v) => (v.position, None),
            PublicReplica::Cover(v) => ([v.position[0], v.position[2]], None),
            PublicReplica::Platform(v) => ([v.pose.position[0], v.pose.position[2]], None),
            PublicReplica::CoverMarker(_) => unreachable!("attachments use their parent pose"),
        };
        let platform_motion = match actor {
            PublicReplica::Platform(v) => Some(v.motion_revision),
            _ => None,
        };
        let mut segment = self
            .buffer
            .identity(scope.entity)
            .filter(|v| v.scope == scope)
            .map_or(1, |v| v.segment);
        if let Some(previous) = self.buffer.latest(scope) {
            let distance = (position[0] - previous.value.position[0])
                .hypot(position[1] - previous.value.position[1]);
            // Delivery gaps do not prove a movement discontinuity. Preserve the
            // bounded history so late samples can still bracket the render time.
            if tick > previous.tick
                && (distance > 8.0 || platform_motion != previous.value.platform_motion)
            {
                segment = segment.checked_add(1).ok_or(Error::Capacity)?;
            }
        }
        self.buffer.receive(
            RemoteSample {
                identity: RemoteIdentity {
                    scope,
                    scene: self.scene,
                    segment,
                },
                tick,
                value: Pose {
                    position,
                    elevation: match actor {
                        PublicReplica::Hero(v) => v.elevation,
                        PublicReplica::Cover(v) => v.position[1],
                        PublicReplica::Platform(v) => v.pose.position[1],
                        _ => 0.0,
                    },
                    facing,
                    platform_tick: match actor {
                        PublicReplica::Platform(v) => Some(v.gameplay_tick),
                        _ => None,
                    },
                    platform_motion,
                    cover_open: match actor {
                        PublicReplica::Cover(v) => Some(v.open),
                        _ => None,
                    },
                },
            },
            &DreamPosePolicy,
        )?;
        if self
            .buffer
            .latest(scope)
            .is_some_and(|latest| latest.tick == tick)
        {
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
        for entity in &self.combat_entities {
            let identity = self.buffer.identity(*entity)?;
            let shown = self
                .buffer
                .sample(identity.scope, &DreamPosePolicy)
                .ok()??;
            if common.is_some_and(|previous| previous != shown.pose_time) {
                return None;
            }
            common = Some(shown.pose_time);
        }
        common.or_else(|| Some(self.buffer.cursor()))
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
    pub fn apply(&self, scope: ScopeIdentity, actor: &mut PublicReplica) {
        let Ok(Some(shown)) = self.buffer.sample(scope, &DreamPosePolicy) else {
            return;
        };
        let pose = shown.value;
        match actor {
            PublicReplica::Hero(v) => {
                v.position = pose.position;
                v.elevation = pose.elevation;
                if let Some(facing) = pose.facing {
                    v.facing = facing;
                }
            }
            PublicReplica::Enemy(v) => {
                v.position = pose.position;
                if let Some(facing) = pose.facing {
                    v.facing = facing;
                }
            }
            PublicReplica::Projectile(v) => {
                v.position = pose.position;
                if let Some(facing) = pose.facing {
                    v.direction = facing;
                }
            }
            PublicReplica::Wisp(v) => v.position = pose.position,
            PublicReplica::Effect(v) => v.position = pose.position,
            PublicReplica::Damage(v) => v.position = pose.position,
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
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(shown, actor(6.0));
        assert_eq!(
            poses
                .buffer
                .sample(scope, &DreamPosePolicy)
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
    fn delayed_enemy_samples_preserve_interpolation_without_extrapolating() {
        let enemy = |x| {
            PublicReplica::Enemy(dreamwake_sim::replication::PublicEnemyView {
                id: 7,
                position: [x, 0.0],
                facing: [1.0, 0.0],
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
        assert_eq!(shown, enemy(3.0));
        assert_eq!(poses.metrics().frozen, 1);
        assert_eq!(poses.combat_view_tick(), Some(ServerTick(78)));
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

use super::*;
#[derive(Debug, Clone, PartialEq)]
struct Pose {
    x: f32,
    yaw: f32,
    heap: Vec<u8>,
}
impl Payload for Pose {
    fn retained_bytes(&self) -> usize {
        self.heap.capacity()
    }
}
struct Linear;
impl PosePolicy<Pose> for Linear {
    fn valid(&self, p: &Pose) -> bool {
        p.x.is_finite() && p.yaw.is_finite()
    }
    fn interpolate(&self, a: &Pose, b: &Pose, alpha: f32) -> Option<Pose> {
        assert!((0.0..=1.0).contains(&alpha));
        let angle = (b.yaw - a.yaw + 180.0).rem_euclid(360.0) - 180.0;
        Some(Pose {
            x: a.x + (b.x - a.x) * alpha,
            yaw: (a.yaw + angle * alpha).rem_euclid(360.0),
            heap: vec![],
        })
    }
    fn extrapolate(&self, a: &Pose, b: &Pose, interval: Duration, ahead: Duration) -> Option<Pose> {
        assert!(ahead <= Duration::from_millis(100));
        Some(Pose {
            x: b.x + (b.x - a.x) * ahead.as_secs_f32() / interval.as_secs_f32(),
            yaw: b.yaw,
            heap: vec![],
        })
    }
}
fn config() -> Config {
    Config {
        rate: TickRate::new(60).unwrap(),
        delay: Duration::from_millis(100),
        extrapolation_cap: Duration::from_millis(100),
        limits: Limits::default(),
    }
}
fn identity(index: u64) -> RemoteIdentity {
    RemoteIdentity {
        scope: ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        },
        scene: SceneRevision(1),
        segment: 1,
    }
}
fn sample(tick: u64, x: f32) -> RemoteSample<Pose> {
    RemoteSample {
        identity: identity(7),
        tick: ServerTick(tick),
        value: Pose {
            x,
            yaw: 0.0,
            heap: vec![],
        },
    }
}
fn buffer() -> RemoteBuffer<Pose> {
    RemoteBuffer::new(ConnectionEpoch(1), config()).unwrap()
}
fn at(buffer: &mut RemoteBuffer<Pose>, millis: u64) -> Presented<Pose> {
    buffer.advance(Duration::from_millis(millis));
    buffer.sample(identity(7).scope, &Linear).unwrap().unwrap()
}

#[test]
fn reordering_brackets_by_tick_and_cursor_never_reverses_when_delay_changes() {
    let mut buffer = buffer();
    for (tick, x) in [(60, 0.0), (66, 6.0), (63, 3.0)] {
        buffer.receive(sample(tick, x), &Linear).unwrap();
    }
    let shown = at(&mut buffer, 1150);
    assert_eq!(shown.value.x, 3.0);
    assert_eq!(shown.cursor, Duration::from_millis(1050));
    assert_eq!(
        buffer.receive(sample(62, 2.0), &Linear),
        Ok(Received::Inserted)
    );
    assert_eq!(at(&mut buffer, 1120).cursor, shown.cursor);
    buffer.set_delay(Duration::from_millis(300));
    assert_eq!(at(&mut buffer, 1200).cursor, shown.cursor);
    buffer.set_delay(Duration::from_millis(100));
    assert!(at(&mut buffer, 1210).cursor > shown.cursor);
    assert_eq!(
        buffer.receive(sample(66, 6.0), &Linear),
        Ok(Received::Duplicate)
    );
    assert_eq!(
        buffer.receive(sample(66, 100.0), &Linear),
        Err(Error::Conflict)
    );
}

#[test]
fn teleport_scene_representation_and_generation_changes_do_not_blend() {
    let mut buffer = buffer();
    buffer.receive(sample(60, 0.0), &Linear).unwrap();
    buffer.receive(sample(66, 6.0), &Linear).unwrap();
    assert_eq!(at(&mut buffer, 1150).value.x, 3.0);
    let mut teleport = sample(67, 100.0);
    teleport.identity.segment = 2;
    assert_eq!(
        buffer.receive(teleport.clone(), &Linear),
        Ok(Received::Discontinuity)
    );
    assert_eq!(at(&mut buffer, 1150).value.x, 100.0);
    assert_eq!(buffer.receive(sample(66, 6.0), &Linear), Err(Error::Stale));
    let mut next = teleport;
    for barrier in 0..3 {
        match barrier {
            0 => next.identity.scene = SceneRevision(2),
            1 => next.identity.scope.representation = RepresentationRevision(2),
            _ => {
                next.identity.scope.entity.generation = 2;
                next.identity.scope.scope = ScopeEpoch(1);
            }
        }
        next.value.x += 100.0;
        next.tick.0 += 1;
        assert_eq!(
            buffer.receive(next.clone(), &Linear),
            Ok(Received::Discontinuity)
        );
        let shown = buffer
            .sample(next.identity.scope, &Linear)
            .unwrap()
            .unwrap();
        assert_eq!(shown.value, next.value);
        assert_eq!(buffer.accounting().samples, 1);
    }
    let mut stale_scene = next.clone();
    stale_scene.identity.scope.scope = ScopeEpoch(9);
    stale_scene.identity.scene = SceneRevision(1);
    assert_eq!(buffer.receive(stale_scene, &Linear), Err(Error::Stale));
}

#[test]
fn scope_exit_reentry_keeps_fences_and_never_resurrects_late_messages() {
    let mut buffer = buffer();
    let old = sample(60, 1.0);
    buffer.receive(old.clone(), &Linear).unwrap();
    buffer.exit(old.identity.scope).unwrap();
    let retired = buffer.accounting();
    assert_eq!(retired.samples, 0);
    assert_eq!(retired.active_entities, 0);
    assert_eq!(retired.known_entities, 1);
    assert_eq!(buffer.receive(old.clone(), &Linear), Err(Error::Stale));
    let mut late_segment = old.clone();
    late_segment.identity.segment += 1;
    late_segment.identity.scene = SceneRevision(2);
    assert_eq!(buffer.receive(late_segment, &Linear), Err(Error::Stale));

    assert!(
        buffer
            .sample(old.identity.scope, &Linear)
            .unwrap()
            .is_none()
    );
    let mut new = sample(70, 50.0);
    new.identity.scope.scope = ScopeEpoch(2);
    buffer.receive(new.clone(), &Linear).unwrap();
    assert_eq!(buffer.receive(old.clone(), &Linear), Err(Error::Stale));
    assert_eq!(buffer.exit(old.identity.scope), Err(Error::Stale));
    assert_eq!(
        buffer
            .sample(new.identity.scope, &Linear)
            .unwrap()
            .unwrap()
            .value
            .x,
        50.0
    );
    assert_eq!(buffer.accounting().known_entities, 1);
}

#[test]
fn loss_for_250_and_500_ms_caps_drift_then_reports_frozen() {
    let mut buffer = buffer();
    buffer.receive(sample(54, 0.0), &Linear).unwrap();
    buffer.receive(sample(60, 1.0), &Linear).unwrap();
    let moving = at(&mut buffer, 1150);
    assert!((moving.value.x - 1.5).abs() < 0.001);
    assert_eq!(moving.status, Status::Extrapolated);
    let first = at(&mut buffer, 1250);
    let later = at(&mut buffer, 1500);
    assert!((first.value.x - 2.0).abs() < 0.001);
    assert_eq!(first.value, later.value);
    assert_eq!(first.pose_time, Duration::from_millis(1100));
    assert_eq!(later.total_age, Duration::from_millis(400));
    assert_eq!(
        first.status,
        Status::Frozen(FreezeReason::ExtrapolationLimit)
    );
    assert_eq!(later.status, first.status);
    let mut resumed = sample(90, 2.0);
    resumed.value.yaw = 10.0;
    buffer.receive(resumed, &Linear).unwrap();
    assert!(matches!(at(&mut buffer, 1550).status, Status::Interpolated));
}

#[test]
fn zero_extrapolation_and_policy_rejection_freeze_without_fabricating_motion() {
    let mut cfg = config();
    cfg.extrapolation_cap = Duration::ZERO;
    let mut buffer = RemoteBuffer::new(ConnectionEpoch(1), cfg).unwrap();
    buffer.receive(sample(54, 0.0), &Linear).unwrap();
    buffer.receive(sample(60, 1.0), &Linear).unwrap();
    let frozen = at(&mut buffer, 1200);
    assert_eq!(frozen.value.x, 1.0);
    assert_eq!(
        frozen.status,
        Status::Frozen(FreezeReason::ExtrapolationLimit)
    );
    let before = buffer.accounting();
    assert_eq!(
        buffer.receive(sample(61, f32::NAN), &Linear),
        Err(Error::InvalidPayload)
    );
    assert_eq!(buffer.accounting(), before);
}

#[test]
fn history_and_aggregate_budgets_are_bounded_transactionally_including_retired_fences() {
    let mut cfg = config();
    cfg.limits.samples_per_entity = 3;
    cfg.limits.total_samples = 4;
    cfg.limits.known_entities = 2;
    cfg.limits.history_ticks = 4;
    let mut buffer = RemoteBuffer::new(ConnectionEpoch(1), cfg).unwrap();
    for tick in 1..=20 {
        buffer.receive(sample(tick, tick as f32), &Linear).unwrap();
    }
    assert_eq!(buffer.accounting().samples, 3);
    assert_eq!(
        buffer.receive(sample(16, 16.0), &Linear),
        Ok(Received::TooOld)
    );
    let mut other = sample(20, 5.0);
    other.identity = identity(8);
    buffer.receive(other.clone(), &Linear).unwrap();
    let before = buffer.accounting();
    other.tick = ServerTick(21);
    assert_eq!(buffer.receive(other.clone(), &Linear), Err(Error::Capacity));
    assert_eq!(buffer.accounting(), before);
    buffer.exit(identity(7).scope).unwrap();
    buffer.receive(other, &Linear).unwrap();
    let mut third = sample(22, 0.0);
    third.identity = identity(9);
    assert_eq!(buffer.receive(third, &Linear), Err(Error::Capacity));
    assert_eq!(buffer.accounting().known_entities, 2);
    let mut cfg = config();
    cfg.limits.retained_bytes =
        track_charge::<Pose>() + std::mem::size_of::<RemoteSample<Pose>>() + 16;
    let mut bytes = RemoteBuffer::new(ConnectionEpoch(1), cfg).unwrap();
    let mut big = sample(1, 0.0);
    big.value.heap = Vec::with_capacity(32);
    assert_eq!(bytes.receive(big, &Linear), Err(Error::Capacity));
    assert_eq!(bytes.accounting(), Accounting::default());
}

#[test]
fn pose_policy_controls_shortest_orientation_arc_and_clamped_interpolation() {
    let mut buffer = buffer();
    let mut a = sample(60, 0.0);
    a.value.yaw = 359.0;
    let mut b = sample(66, 1.0);
    b.value.yaw = 1.0;
    buffer.receive(a, &Linear).unwrap();
    buffer.receive(b, &Linear).unwrap();
    let shown = at(&mut buffer, 1150);
    assert!(shown.value.yaw.abs() < 0.001);
    assert!((0.0..=1.0).contains(&shown.value.x));
    let mut bad = sample(70, 0.0);
    bad.identity.scope.connection = ConnectionEpoch(2);
    assert_eq!(buffer.receive(bad, &Linear), Err(Error::WrongEpoch));
}

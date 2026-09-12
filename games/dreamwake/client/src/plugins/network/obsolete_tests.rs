use super::tests::{fixture, public_full_state};
use super::*;
use engine_net::replication::*;

fn full_bytes(session: &Session, state: FullState<Vec<u8>>) -> Vec<u8> {
    encode_state(
        &StateFrame::Full(state),
        session.model.welcome.stream.epoch,
        0,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap()
}

fn assert_obsolete(error: &ReceiveRejection, reason: ObsoleteReason) {
    assert!(
        matches!(error, ReceiveRejection::Replication {
        error: ReplicationError::Obsolete(actual), ..
    } if *actual == reason),
        "{error:?}"
    );
    let mut telemetry = diagnostics::Telemetry::default();
    record_receive_rejection(&mut telemetry, error);
    assert_eq!(telemetry.decode_errors, 1);
    assert_eq!(telemetry.obsolete_rejections, 1);
    assert_eq!(telemetry.decode_failures, 0);
}

fn assert_hard(error: &ReceiveRejection) {
    assert!(
        !matches!(
            error,
            ReceiveRejection::Replication {
                error: ReplicationError::Obsolete(_),
                ..
            }
        ),
        "{error:?}"
    );
    let mut telemetry = diagnostics::Telemetry::default();
    record_receive_rejection(&mut telemetry, error);
    assert_eq!(telemetry.decode_errors, 1);
    assert_eq!(telemetry.obsolete_rejections, 0);
    assert_eq!(telemetry.decode_failures, 1);
}

#[test]
fn delayed_public_full_preserves_state_pose_and_receipts() {
    let (mut session, _) = fixture();
    let current = public_full_state(&session, 100, 2);
    let delayed = public_full_state(&session, 100, 1);
    let bytes = full_bytes(&session, current.clone());
    assert!(matches!(
        session
            .receive_state(&bytes, Duration::from_millis(2))
            .unwrap(),
        Some(ClientControl::Decoded { .. })
    ));
    let poses = session.poses.metrics();
    let bytes = full_bytes(&session, delayed);
    let error = session
        .receive_state(&bytes, Duration::from_millis(3))
        .unwrap_err();
    assert_obsolete(&error, ObsoleteReason::Publication);
    assert_eq!(session.scopes.state(current.scope.entity), Some(&current));
    assert_eq!(
        session.poses.metrics().retained_samples,
        poses.retained_samples
    );
    assert_eq!(session.poses.metrics().cursor, poses.cursor);
    assert!(session.take_decoded_receipts().is_none());
}

#[test]
fn closed_and_prior_scope_full_are_typed_obsolete() {
    let (mut session, _) = fixture();
    let current = public_full_state(&session, 100, 2);
    let bytes = full_bytes(&session, current.clone());
    session
        .receive_state(&bytes, Duration::from_millis(2))
        .unwrap();
    session
        .scopes
        .apply_exit(ScopeExit {
            scope: current.scope,
        })
        .unwrap();
    let error = session
        .receive_state(&bytes, Duration::from_millis(3))
        .unwrap_err();
    assert_obsolete(&error, ObsoleteReason::Scope);
    assert!(session.scopes.state(current.scope.entity).is_none());
    let mut reentry = current.clone();
    reentry.scope.scope = ScopeEpoch(2);
    reentry.scope.representation = RepresentationRevision(2);
    let reentry_bytes = full_bytes(&session, reentry.clone());
    session
        .receive_state(&reentry_bytes, Duration::from_millis(4))
        .unwrap();
    let error = session
        .receive_state(&bytes, Duration::from_millis(5))
        .unwrap_err();
    assert_obsolete(&error, ObsoleteReason::Scope);
    assert_eq!(session.scopes.state(current.scope.entity), Some(&reentry));
    assert!(session.take_decoded_receipts().is_none());
}

#[test]
fn malformed_and_conflicting_public_states_remain_hard_errors() {
    let (mut session, _) = fixture();
    let current = public_full_state(&session, 100, 2);
    let bytes = full_bytes(&session, current.clone());
    session
        .receive_state(&bytes, Duration::from_millis(2))
        .unwrap();
    let mut malformed = public_full_state(&session, 100, 1);
    malformed.payload = vec![255];
    let mut equal_snapshot = current.clone();
    equal_snapshot.end_tick = ServerTick(current.end_tick.0 + 1);
    let mut mixed_version = public_full_state(&session, 100, 1);
    mixed_version.version = StateVersion(3);
    let mut mixed_tick = public_full_state(&session, 100, 1);
    mixed_tick.end_tick = ServerTick(current.end_tick.0 + 1);
    for state in [malformed, equal_snapshot, mixed_version, mixed_tick] {
        let bytes = full_bytes(&session, state);
        assert_hard(
            &session
                .receive_state(&bytes, Duration::from_millis(3))
                .unwrap_err(),
        );
        assert_eq!(session.scopes.state(current.scope.entity), Some(&current));
    }
    // Entity 100 and each identity field fit one LEB128 byte. Mutate only the
    // generation in the decoded transport payload and rebuild its valid frame.
    let epoch = session.model.welcome.stream.epoch;
    let limit = session.model.welcome.frame_limit().unwrap();
    let (header, payload) = engine_net::codec::decode_frame(&bytes, epoch, limit).unwrap();
    let mut payload = payload.to_vec();
    assert_eq!(&payload[..3], &[0, 100, 1]);
    payload[2] = 0;
    let malformed_identity = engine_net::codec::encode_frame(header, &payload, limit).unwrap();
    assert_hard(
        &session
            .receive_state(&malformed_identity, Duration::from_millis(3))
            .unwrap_err(),
    );
    assert_eq!(session.scopes.state(current.scope.entity), Some(&current));
}

#[test]
fn fragment_local_time_regression_is_hard_stale() {
    let (mut session, frames) = fixture();
    assert!(
        frames.len() > 1,
        "fixture must exercise incomplete fragment assembly"
    );
    session
        .receive_state(&frames[0], Duration::from_millis(10))
        .unwrap();
    let error = session
        .receive_state(&frames[1], Duration::from_millis(9))
        .unwrap_err();
    assert!(
        matches!(
            &error,
            ReceiveRejection::Replication {
                error: ReplicationError::Stale | ReplicationError::Conflict,
                ..
            }
        ),
        "{error:?}"
    );
    assert_hard(&error);
    assert!(session.take_decoded_receipts().is_none());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn obsolete_receive_message_does_not_refresh_runtime_or_enqueue_receipt() {
    let (session, _) = fixture();
    let current = public_full_state(&session, 100, 2);
    let delayed = full_bytes(&session, public_full_state(&session, 100, 1));
    let bytes = full_bytes(&session, current.clone());
    let mut runtime = super::tests::receipt_runtime(session);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    receive_message(
        &mut runtime,
        STATE_CHANNEL,
        &bytes,
        Duration::from_millis(10),
        &mut world,
    )
    .unwrap();
    let pending = runtime
        .prediction
        .as_mut()
        .unwrap()
        .take_decoded_receipts()
        .unwrap();
    let ClientControl::Decoded { receipts } = pending else {
        panic!("decoded receipt")
    };
    assert_eq!(receipts.as_slice(), &[current.receipt()]);
    let poses = runtime.prediction.as_ref().unwrap().poses.metrics();
    let error = receive_message(
        &mut runtime,
        STATE_CHANNEL,
        &delayed,
        Duration::from_millis(20),
        &mut world,
    )
    .unwrap_err();
    assert_obsolete(&error, ObsoleteReason::Publication);
    record_receive_rejection(&mut world.resource_mut::<diagnostics::Telemetry>(), &error);
    assert_eq!(runtime.last_received, 0.01);
    assert!(
        runtime
            .prediction
            .as_mut()
            .unwrap()
            .take_decoded_receipts()
            .is_none()
    );
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.scopes.state(current.scope.entity), Some(&current));
    assert_eq!(
        session.poses.metrics().retained_samples,
        poses.retained_samples
    );
    let telemetry = world.resource::<diagnostics::Telemetry>();
    assert_eq!(telemetry.snapshots, 1);
    assert_eq!(telemetry.obsolete_rejections, 1);
    assert_eq!(telemetry.decode_failures, 0);
}

#[cfg(not(target_arch = "wasm32"))]
fn lifecycle_runtime() -> (Runtime, World, FullState<Vec<u8>>) {
    use dreamwake_protocol::live::baselines as wire;
    use engine_net::replication::baselines::{BaselineEncoding, BaselinePacket};
    let (mut session, _) = fixture();
    let mut current = public_full_state(&session, 100, 2);
    current.scope.scope = ScopeEpoch(2);
    current.scope.representation = RepresentationRevision(2);
    let bytes = full_bytes(&session, current.clone());
    session
        .receive_state(&bytes, Duration::from_millis(2))
        .unwrap();
    let publication = GroupPublication {
        connection: current.scope.connection,
        group: OWNER_GROUP,
        revision: GroupRevision(1),
        snapshot: current.snapshot,
    };
    let packet = BaselinePacket {
        target: wire::target(&current, publication, BaselineGeneration(1)),
        encoding: BaselineEncoding::Full,
        bytes: current.payload.clone(),
    };
    session.baselines.reconstruct(&mut current, packet).unwrap();
    assert!(session.baselines.retained_bytes() > 0);
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    (super::tests::receipt_runtime(session), world, current)
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn delayed_exit_acknowledges_idempotently_without_revoking_current_scope() {
    let (mut runtime, mut world, current) = lifecycle_runtime();
    let session = runtime.prediction.as_ref().unwrap();
    let limit = session.model.welcome.frame_limit().unwrap();
    let baseline_bytes = session.baselines.retained_bytes();
    let poses = session.poses.metrics();
    let initial_sequence = session.control_sequence;
    let mut prior = current.scope;
    prior.scope = ScopeEpoch(1);
    prior.representation = RepresentationRevision(1);
    let bytes = encode_control(
        &ServerControl::Exit(ScopeExit { scope: prior }),
        current.scope.connection,
        0,
        limit,
    )
    .unwrap();
    for index in 1..=2 {
        receive_message(
            &mut runtime,
            CONTROL_CHANNEL,
            &bytes,
            Duration::from_millis(10 + u64::from(index)),
            &mut world,
        )
        .unwrap();
        let session = runtime.prediction.as_ref().unwrap();
        assert_eq!(session.control_sequence, initial_sequence + index);
        assert_eq!(session.scopes.state(current.scope.entity), Some(&current));
        assert!(session.entities.contains(&current.scope.entity));
        assert_eq!(
            session.poses.metrics().retained_samples,
            poses.retained_samples
        );
        assert_eq!(
            session.poses.metrics().active_entities,
            poses.active_entities
        );
        assert_eq!(session.baselines.retained_bytes(), baseline_bytes);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn embedded_wrong_epoch_destroy_has_no_lifecycle_side_effects() {
    let (mut runtime, mut world, current) = lifecycle_runtime();
    let session = runtime.prediction.as_ref().unwrap();
    let baseline_bytes = session.baselines.retained_bytes();
    let poses = session.poses.metrics();
    let initial_sequence = session.control_sequence;
    let bytes = encode_control(
        &ServerControl::Destroy(Destroy {
            connection: ConnectionEpoch(current.scope.connection.0 + 1),
            entity: current.scope.entity,
        }),
        current.scope.connection,
        0,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap();
    let error = receive_message(
        &mut runtime,
        CONTROL_CHANNEL,
        &bytes,
        Duration::from_millis(10),
        &mut world,
    )
    .unwrap_err();
    assert_hard(&error);
    let session = runtime.prediction.as_ref().unwrap();
    assert_eq!(session.control_sequence, initial_sequence);
    assert_eq!(session.scopes.state(current.scope.entity), Some(&current));
    assert!(session.entities.contains(&current.scope.entity));
    assert_eq!(
        session.poses.metrics().retained_samples,
        poses.retained_samples
    );
    assert_eq!(
        session.poses.metrics().active_entities,
        poses.active_entities
    );
    assert_eq!(session.baselines.retained_bytes(), baseline_bytes);
}

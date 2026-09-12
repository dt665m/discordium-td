use super::tests::{fixture, fixture_publication, public_full_state, receipt_runtime};
use super::*;
use dreamwake_protocol::{
    live::baselines as wire,
    replication::{ReplicaPayload, decode_replica, encode_replica},
};
use engine_net::replication::{
    baselines::{BaselineEncoding, BaselinePacket, BaselineReset},
    *,
};

fn prepared() -> (Session, GroupChunk<Vec<u8>>) {
    let (mut session, first) = fixture();
    session.begin_frame().unwrap();
    for frame in first {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    session.reconcile().unwrap();
    let (mut source, frames) = fixture_publication(11, 2, 1);
    for frame in frames {
        source
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    let group = source
        .groups
        .published_group(OWNER_GROUP, &source.scopes)
        .unwrap();
    let manifest = group.manifest().to_vec();
    let members = manifest
        .iter()
        .map(|scope| source.scopes.state(scope.entity).unwrap().clone())
        .collect();
    (
        session,
        GroupChunk {
            publication: GroupPublication {
                connection: ConnectionEpoch(1),
                group: OWNER_GROUP,
                revision: GroupRevision(1),
                snapshot: SnapshotId(2),
            },
            end_tick: ServerTick(11),
            manifest,
            members,
            index: 0,
            count: 1,
        },
    )
}

fn wrap_next_generation(group: &mut GroupChunk<Vec<u8>>) {
    for state in &mut group.members {
        state.payload = wire::encode_member(&BaselinePacket {
            target: wire::target(state, group.publication, BaselineGeneration(2)),
            encoding: BaselineEncoding::Full,
            bytes: state.payload.clone(),
        })
        .unwrap();
    }
}

fn frames(session: &Session, group: &GroupChunk<Vec<u8>>) -> Vec<Vec<u8>> {
    let bytes = encode_group(group, group_limits(), |payload| Ok(payload.clone())).unwrap();
    let limit = session.model.welcome.frame_limit().unwrap();
    fragment_group(group.publication, &bytes, byte_limits(limit).unwrap())
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(index, fragment)| {
            encode_state(
                &StateFrame::Fragment(fragment),
                session.model.welcome.stream.epoch,
                index as u32,
                limit,
            )
            .unwrap()
        })
        .collect()
}

#[test]
fn pending_full_does_not_hide_invalid_later_member_or_future_generation() {
    for case in [
        "public actor",
        "wrong role",
        "invalid DTO",
        "mixed content context",
        "malformed envelope",
        "generation three",
    ] {
        let (mut session, mut group) = prepared();
        let owner_before = session.predictor.state(OWNER_GROUP).unwrap().clone();
        let states_before = group
            .manifest
            .iter()
            .map(|scope| session.scopes.state(scope.entity).unwrap().clone())
            .collect::<Vec<_>>();
        let baseline_bytes = session.baselines.retained_bytes();
        let baseline_ids = group
            .manifest
            .iter()
            .map(|scope| session.baselines.identity(scope.entity))
            .collect::<Vec<_>>();
        let global = group
            .members
            .iter()
            .find(|state| state.scope.entity == session.model.welcome.global_entity)
            .unwrap()
            .payload
            .clone();
        let last = group.members.last_mut().unwrap();
        // Every malformed member follows at least one otherwise valid pending Full.
        assert_ne!(last.scope, group.manifest[0]);
        match case {
            "public actor" => last.payload = public_full_state(&session, 100, 1).payload,
            "wrong role" => last.payload = global.clone(),
            "invalid DTO" => {
                last.payload.pop();
            }
            "mixed content context" => {
                let ReplicaPayload::Global(mut value) = decode_replica(&global, None).unwrap()
                else {
                    panic!("global fixture")
                };
                value.stamp.server_tick += 1;
                let state = group
                    .members
                    .iter_mut()
                    .find(|state| state.scope.entity == session.model.welcome.global_entity)
                    .unwrap();
                state.payload = encode_replica(&ReplicaPayload::Global(value)).unwrap();
            }
            _ => {}
        }
        wrap_next_generation(&mut group);
        let publication = group.publication;
        let last = group.members.last_mut().unwrap();
        if case == "malformed envelope" {
            last.payload.pop();
        } else if case == "generation three" {
            let mut packet = wire::decode_member(&last.payload, last, publication).unwrap();
            packet.target.generation = BaselineGeneration(3);
            last.payload = wire::encode_member(&packet).unwrap();
        }
        let mut rejection = None;
        for frame in frames(&session, &group) {
            match session.receive_state(&frame, Duration::from_millis(2)) {
                Ok(None) => {}
                Ok(Some(_)) => panic!("invalid group returned a receipt: {case}"),
                Err(error) => {
                    assert!(rejection.replace(error).is_none());
                }
            }
        }
        let error =
            rejection.unwrap_or_else(|| panic!("invalid pending group was swallowed: {case}"));
        if case == "generation three" {
            assert!(
                matches!(
                    error,
                    ReceiveRejection::Replication {
                        error: ReplicationError::Stale,
                        ..
                    }
                ),
                "{case}: {error:?}"
            );
        } else {
            assert!(
                matches!(
                    error,
                    ReceiveRejection::Replication {
                        error: ReplicationError::InvalidPayload
                            | ReplicationError::InvalidIdentity
                            | ReplicationError::Conflict,
                        ..
                    }
                ),
                "{case}: {error:?}"
            );
        }
        let mut telemetry = diagnostics::Telemetry::default();
        record_receive_rejection(&mut telemetry, &error);
        assert_eq!(telemetry.decode_failures, 1, "{case}");
        assert_eq!(telemetry.obsolete_rejections, 0, "{case}");
        assert_eq!(
            session.metrics().delivery.baseline_reset_pending,
            0,
            "{case}"
        );
        assert_eq!(
            session.predictor.state(OWNER_GROUP),
            Some(&owner_before),
            "{case}"
        );
        assert_eq!(session.baselines.retained_bytes(), baseline_bytes, "{case}");
        for (index, state) in states_before.iter().enumerate() {
            assert_eq!(
                session.scopes.state(state.scope.entity),
                Some(state),
                "{case}"
            );
            assert_eq!(
                session.baselines.identity(state.scope.entity),
                baseline_ids[index],
                "{case}"
            );
        }
        assert!(session.take_decoded_receipts().is_none(), "{case}");
    }
}

#[test]
fn pending_full_receive_preserves_freshness_and_receipts_until_reliable_reset() {
    let (mut session, mut group) = prepared();
    let owner = session.model.welcome.owner_entity;
    let owner_before = session.predictor.state(OWNER_GROUP).unwrap().clone();
    let state_before = session.scopes.state(owner).unwrap().clone();
    let baseline_bytes = session.baselines.retained_bytes();
    let baseline_ids = group
        .manifest
        .iter()
        .map(|scope| session.baselines.identity(scope.entity))
        .collect::<Vec<_>>();
    let pending_receipt = public_full_state(&session, 100, 1).receipt();
    assert!(session.queue_decoded_receipt(pending_receipt).is_none());
    let sequence = session.control_sequence;
    let decoded_groups = session.decoded_groups.clone();
    let resets = group
        .members
        .iter()
        .map(|state| BaselineReset {
            context: wire::context(state, group.publication),
            generation: BaselineGeneration(2),
        })
        .collect::<Vec<_>>();
    wrap_next_generation(&mut group);
    let frames = frames(&session, &group);
    let mut runtime = receipt_runtime(session);
    runtime.last_received = 0.001;
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    for frame in &frames {
        receive_message(
            &mut runtime,
            STATE_CHANNEL,
            frame,
            Duration::from_millis(2),
            &mut world,
        )
        .unwrap();
    }
    assert_eq!(runtime.last_received, 0.001);
    assert_eq!(world.resource::<diagnostics::Telemetry>().snapshots, 0);
    let session = runtime.prediction.as_mut().unwrap();
    assert_eq!(session.metrics().delivery.baseline_reset_pending, 1);
    assert_eq!(session.control_sequence, sequence);
    assert_eq!(session.predictor.state(OWNER_GROUP), Some(&owner_before));
    assert_eq!(session.scopes.state(owner), Some(&state_before));
    assert_eq!(session.decoded_groups, decoded_groups);
    assert_eq!(session.baselines.retained_bytes(), baseline_bytes);
    for (index, scope) in group.manifest.iter().enumerate() {
        assert_eq!(
            session.baselines.identity(scope.entity),
            baseline_ids[index]
        );
    }
    let Some(ClientControl::Decoded { receipts }) = session.take_decoded_receipts() else {
        panic!("existing pending receipt")
    };
    assert_eq!(receipts.as_slice(), &[pending_receipt]);
    assert!(session.take_decoded_receipts().is_none());
    let control = encode_control(
        &ServerControl::BaselineReset {
            resets: BoundedVec::new(resets).unwrap(),
        },
        session.model.welcome.stream.epoch,
        0,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap();
    receive_message(
        &mut runtime,
        CONTROL_CHANNEL,
        &control,
        Duration::from_millis(3),
        &mut world,
    )
    .unwrap();
    let session = runtime.prediction.as_mut().unwrap();
    let mut decoded = None;
    for frame in &frames {
        if let Some(receipt) = session
            .receive_state(frame, Duration::from_millis(4))
            .unwrap()
        {
            assert!(decoded.replace(receipt).is_none());
        }
    }
    let Some(ClientControl::OwnerDecoded { proofs }) = decoded else {
        panic!("retry after reset must decode")
    };
    assert_eq!(proofs.len(), group.members.len());
    assert!(
        proofs
            .as_slice()
            .iter()
            .all(|proof| proof.baseline.generation == BaselineGeneration(2)
                && proof.state.snapshot == SnapshotId(2))
    );
    assert_eq!(session.scopes.state(owner).unwrap().snapshot, SnapshotId(2));
    assert_eq!(session.metrics().delivery.baseline_reset_pending, 1);
}

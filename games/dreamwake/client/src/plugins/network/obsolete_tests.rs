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
fn delayed_owner_topology_preserves_new_reset_and_publication() {
    use dreamwake_protocol::live::baselines as wire;
    use engine_net::replication::baselines::{
        BaselineCodec, BaselineEncoding, BaselinePacket, BaselineReset, BytePatch,
    };
    for published in [false, true] {
        for delta in [false, true] {
            let (mut session, initial) = fixture();
            session
                .receive_state(&initial[0], Duration::from_millis(1))
                .unwrap();
            let base = super::tests::published_group_chunk(&session);
            let (mut older, frames) = super::tests::fixture_publication(11, 2, 1);
            older
                .receive_state(&frames[0], Duration::from_millis(1))
                .unwrap();
            let mut delayed = super::tests::published_group_chunk(&older);
            for (state, base) in delayed.members.iter_mut().zip(&base.members) {
                let target = wire::target(state, delayed.publication, BaselineGeneration(1));
                let (encoding, bytes) = if delta {
                    (
                        BaselineEncoding::Delta {
                            base: wire::target(base, delayed.publication, BaselineGeneration(1)),
                        },
                        BytePatch
                            .encode_delta(&base.payload, &state.payload, wire::MEMBER_BYTES)
                            .unwrap(),
                    )
                } else {
                    (BaselineEncoding::Full, state.payload.clone())
                };
                state.payload = wire::encode_member(&BaselinePacket {
                    target,
                    encoding,
                    bytes,
                })
                .unwrap();
            }
            let (mut newer, frames) = super::tests::fixture_publication(12, 3, 2);
            newer
                .receive_state(&frames[0], Duration::from_millis(1))
                .unwrap();
            let group = super::tests::published_group_chunk(&newer);
            let resets = group
                .members
                .iter()
                .map(|state| BaselineReset {
                    context: wire::context(state, group.publication),
                    generation: BaselineGeneration(1),
                })
                .collect::<Vec<_>>();
            session.baselines.apply_resets(&resets).unwrap();
            if published {
                session
                    .receive_state(&frames[0], Duration::from_millis(2))
                    .unwrap();
            }
            let identities = group
                .manifest
                .iter()
                .map(|scope| session.baselines.identity(scope.entity))
                .collect::<Vec<_>>();
            let publication = session.groups.publication(OWNER_GROUP);
            let baseline_bytes = session.baselines.retained_bytes();
            let scopes = group
                .manifest
                .iter()
                .map(|scope| session.scopes.state(scope.entity).cloned())
                .collect::<Vec<_>>();
            let bytes = super::tests::encoded_group_frame(&session.model.welcome, delayed.clone());
            let error = session
                .receive_state(&bytes, Duration::from_millis(3))
                .unwrap_err();
            assert_obsolete(&error, ObsoleteReason::Publication);
            assert_eq!(session.groups.publication(OWNER_GROUP), publication);
            assert_eq!(session.baselines.retained_bytes(), baseline_bytes);
            assert_eq!(session.metrics().baseline_repairs, 0);
            assert_eq!(session.metrics().incomplete_groups, 0);
            assert!(session.take_decoded_receipts().is_none());
            for ((scope, identity), state) in group.manifest.iter().zip(identities).zip(scopes) {
                assert_eq!(session.baselines.identity(scope.entity), identity);
                assert_eq!(session.scopes.state(scope.entity), state.as_ref());
            }
            if !delta {
                // Age cannot hide a malformed full DTO in a later member.
                let state = delayed.members.last_mut().unwrap();
                let mut packet =
                    wire::decode_member(&state.payload, state, delayed.publication).unwrap();
                packet.bytes.pop();
                state.payload = wire::encode_member(&packet).unwrap();
                let bytes = super::tests::encoded_group_frame(&session.model.welcome, delayed);
                assert_hard(
                    &session
                        .receive_state(&bytes, Duration::from_millis(4))
                        .unwrap_err(),
                );
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod topology_members {
    use super::*;
    use crate::plugins::network::tests::{
        encoded_group_frame, fixture_from_sim, group_frames, published_group_chunk,
        receipt_runtime, wrap_full_baseline,
    };
    use dreamwake_protocol::live::baselines as wire;
    use dreamwake_protocol::replication::{ReplicaPayload, decode_replica, encode_replica};
    use engine_net::replication::baselines::{
        BaselineCodec, BaselineEncoding, BaselinePacket, BaselineReset, BytePatch,
    };

    fn receive_group(session: &mut Session, frames: Vec<Vec<u8>>, millis: u64) {
        for frame in frames {
            session
                .receive_state(&frame, Duration::from_millis(millis))
                .unwrap();
        }
    }

    fn capture_group(
        sim: &DreamSimulation,
        tick: u64,
        snapshot: u64,
        topology: u64,
    ) -> GroupChunk<Vec<u8>> {
        let (mut session, frames) = fixture_from_sim(sim, tick, snapshot, topology);
        receive_group(&mut session, frames, 1);
        published_group_chunk(&session)
    }

    fn reset_group(session: &mut Session, group: &GroupChunk<Vec<u8>>) {
        let resets = group
            .members
            .iter()
            .map(|state| BaselineReset {
                context: wire::context(state, group.publication),
                generation: BaselineGeneration(1),
            })
            .collect::<Vec<_>>();
        session.baselines.apply_resets(&resets).unwrap();
    }

    fn delayed_platform(published: bool, reentered: bool) -> (Runtime, GroupChunk<Vec<u8>>) {
        let mut sim = DreamSimulation::new(71, false);
        sim.continue_run_for(1);
        let (mut session, frames) = fixture_from_sim(&sim, 10, 1, 4);
        receive_group(&mut session, frames, 1);
        let base = published_group_chunk(&session);
        assert_eq!(base.members.len(), 4);
        let platform = base.members[3].scope.entity;
        let mut delayed = capture_group(&sim, 11, 2, 4);
        for (state, base_state) in delayed.members.iter_mut().zip(&base.members) {
            let (encoding, bytes) = if state.scope.entity == platform {
                (
                    BaselineEncoding::Delta {
                        base: wire::target(base_state, base.publication, BaselineGeneration(1)),
                    },
                    BytePatch
                        .encode_delta(&base_state.payload, &state.payload, wire::MEMBER_BYTES)
                        .unwrap(),
                )
            } else {
                (BaselineEncoding::Full, state.payload.clone())
            };
            state.payload = wire::encode_member(&BaselinePacket {
                target: wire::target(state, delayed.publication, BaselineGeneration(1)),
                encoding,
                bytes,
            })
            .unwrap();
        }
        {
            let world = sim.world_mut();
            *world
                .query::<&mut engine_core::KinematicState>()
                .single_mut(world)
                .unwrap() = engine_core::KinematicState::new([0.0, 0.0, 15.0], 1);
        }
        sim.step(DreamInput::default());
        let removed = capture_group(&sim, 12, 3, 5);
        assert_eq!(removed.members.len(), 3);
        reset_group(&mut session, &removed);
        assert!(session.baselines.identity(platform).is_none());
        if published {
            let frames = group_frames(&session.model.welcome, removed);
            receive_group(&mut session, frames, 2);
        }
        if reentered {
            assert!(published);
            {
                let world = sim.world_mut();
                *world
                    .query::<&mut engine_core::KinematicState>()
                    .single_mut(world)
                    .unwrap() = engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
            }
            sim.step(DreamInput::default());
            let mut reentry = capture_group(&sim, 13, 4, 6);
            assert_eq!(reentry.members.len(), 4);
            let state = reentry
                .members
                .iter_mut()
                .find(|s| s.scope.entity == platform)
                .unwrap();
            state.scope.scope = ScopeEpoch(2);
            state.scope.representation = RepresentationRevision(2);
            reentry.manifest = reentry.members.iter().map(|s| s.scope).collect();
            reset_group(&mut session, &reentry);
            let frames = group_frames(&session.model.welcome, reentry);
            receive_group(&mut session, frames, 3);
            assert_eq!(
                session.baselines.identity(platform).unwrap().0.scope.scope,
                ScopeEpoch(2)
            );
        }
        let mut runtime = receipt_runtime(session);
        runtime.last_received = 0.25;
        (runtime, delayed)
    }

    fn reject_unchanged(runtime: &mut Runtime, group: GroupChunk<Vec<u8>>) -> ReceiveRejection {
        let session = runtime.prediction.as_ref().unwrap();
        let before = session.metrics();
        let current = published_group_chunk(session);
        let decoded = session.decoded_groups.clone();
        let entities = session.entities.clone();
        let scopes = group
            .manifest
            .iter()
            .map(|scope| {
                (
                    scope.entity,
                    session.baselines.identity(scope.entity),
                    session.scopes.state(scope.entity).cloned(),
                    session.scopes.fence(scope.entity.index),
                )
            })
            .collect::<Vec<_>>();
        let bytes = encoded_group_frame(&session.model.welcome, group);
        let mut world = World::new();
        world.init_resource::<diagnostics::Telemetry>();
        let error = receive_message(
            runtime,
            OWNER_GROUP_CHANNEL,
            &bytes,
            Duration::from_secs(1),
            &mut world,
        )
        .unwrap_err();
        assert_eq!(runtime.last_received, 0.25);
        assert!(runtime.last_snapshot.is_none());
        assert_eq!(world.resource::<diagnostics::Telemetry>().snapshots, 0);
        let session = runtime.prediction.as_mut().unwrap();
        let after = session.metrics();
        assert_eq!(after.baseline_bytes, before.baseline_bytes);
        assert_eq!(after.scope_bytes, before.scope_bytes);
        assert_eq!(after.group_bytes, before.group_bytes);
        assert_eq!(after.baseline_repairs, before.baseline_repairs);
        assert_eq!(after.incomplete_groups, before.incomplete_groups);
        assert_eq!(after.committed_server_tick, before.committed_server_tick);
        assert_eq!(
            after.interpolation.retained_samples,
            before.interpolation.retained_samples
        );
        assert_eq!(after.interpolation.cursor, before.interpolation.cursor);
        assert_eq!(published_group_chunk(session), current);
        assert_eq!(session.decoded_groups, decoded);
        assert_eq!(session.entities, entities);
        assert!(session.take_decoded_receipts().is_none());
        assert!(
            session
                .receive_pending_reset_group(Duration::from_secs(1))
                .unwrap()
                .is_none()
        );
        for (entity, identity, state, fence) in scopes {
            assert_eq!(session.baselines.identity(entity), identity);
            assert_eq!(session.scopes.state(entity), state.as_ref());
            assert_eq!(session.scopes.fence(entity.index), fence);
        }
        error
    }

    #[test]
    fn mixed_old_topology_with_removed_platform_delta_is_obsolete() {
        for published in [false, true] {
            let (mut runtime, delayed) = delayed_platform(published, false);
            assert_obsolete(
                &reject_unchanged(&mut runtime, delayed),
                ObsoleteReason::Publication,
            );
        }
    }

    #[test]
    fn mixed_old_topology_preserves_reentered_platform_scope() {
        let (mut runtime, delayed) = delayed_platform(true, true);
        assert_obsolete(
            &reject_unchanged(&mut runtime, delayed),
            ObsoleteReason::Publication,
        );
    }

    #[test]
    fn changed_scope_in_same_topology_remains_a_hard_conflict() {
        let (mut runtime, delayed) = delayed_platform(true, true);
        let mut conflicting = published_group_chunk(runtime.prediction.as_ref().unwrap());
        conflicting.members.last_mut().unwrap().scope = delayed.members.last().unwrap().scope;
        conflicting.manifest = conflicting
            .members
            .iter()
            .map(|state| state.scope)
            .collect();
        for state in &mut conflicting.members {
            wrap_full_baseline(state, conflicting.publication);
        }
        assert_hard(&reject_unchanged(&mut runtime, conflicting));
    }

    #[test]
    fn old_topology_with_lower_representation_in_current_scope_is_hard() {
        let (mut runtime, mut delayed) = delayed_platform(true, true);
        let state = delayed.members.last_mut().unwrap();
        let mut packet = wire::decode_member(&state.payload, state, delayed.publication).unwrap();
        state.scope.scope = ScopeEpoch(2);
        state.scope.representation = RepresentationRevision(1);
        packet.target.context.scope = state.scope;
        let BaselineEncoding::Delta { base } = &mut packet.encoding else {
            panic!("platform delta");
        };
        base.context.scope = state.scope;
        state.payload = wire::encode_member(&packet).unwrap();
        delayed.manifest = delayed.members.iter().map(|state| state.scope).collect();
        assert_hard(&reject_unchanged(&mut runtime, delayed));
    }

    #[test]
    fn old_platform_delta_cannot_hide_mismatched_global_epoch_or_scene() {
        for published in [false, true] {
            for changed_scene in [false, true] {
                let (mut runtime, mut delayed) = delayed_platform(published, false);
                let state = &mut delayed.members[0];
                let mut packet =
                    wire::decode_member(&state.payload, state, delayed.publication).unwrap();
                assert!(matches!(packet.encoding, BaselineEncoding::Full));
                let ReplicaPayload::Global(mut global) =
                    decode_replica(&packet.bytes, None).unwrap()
                else {
                    panic!("global full member");
                };
                if changed_scene {
                    global.stamp.scene_revision += 1;
                } else {
                    global.stamp.match_epoch += 1;
                }
                packet.bytes = encode_replica(&ReplicaPayload::Global(global)).unwrap();
                state.payload = wire::encode_member(&packet).unwrap();
                assert_hard(&reject_unchanged(&mut runtime, delayed));
            }
        }
    }

    #[test]
    fn old_delta_cannot_hide_malformed_later_full_member() {
        let mut sim = DreamSimulation::new(71, false);
        sim.continue_run_for(1);
        let base = capture_group(&sim, 10, 1, 4);
        for published in [false, true] {
            for malformed_wrapper in [false, true] {
                let (mut runtime, mut delayed) = delayed_platform(published, false);
                // The wire follows manifest order. Make its first mandatory member
                // a real Delta so an early stale return would hide the later bad Full.
                let state = &mut delayed.members[0];
                let mut packet =
                    wire::decode_member(&state.payload, state, delayed.publication).unwrap();
                packet.encoding = BaselineEncoding::Delta {
                    base: wire::target(&base.members[0], base.publication, BaselineGeneration(1)),
                };
                packet.bytes = BytePatch
                    .encode_delta(&base.members[0].payload, &packet.bytes, wire::MEMBER_BYTES)
                    .unwrap();
                state.payload = wire::encode_member(&packet).unwrap();
                let state = &mut delayed.members[2];
                if malformed_wrapper {
                    state.payload.pop();
                } else {
                    let mut packet =
                        wire::decode_member(&state.payload, state, delayed.publication).unwrap();
                    assert!(matches!(packet.encoding, BaselineEncoding::Full));
                    packet.bytes.pop();
                    state.payload = wire::encode_member(&packet).unwrap();
                }
                assert_hard(&reject_unchanged(&mut runtime, delayed));
            }
        }
    }
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
fn whole_group_local_time_regression_is_hard_stale() {
    let (mut session, frames) = fixture();
    assert_eq!(frames.len(), 1);
    session
        .receive_state(&frames[0], Duration::from_millis(10))
        .unwrap();
    let error = session
        .receive_state(&frames[0], Duration::from_millis(9))
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

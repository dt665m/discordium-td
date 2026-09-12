//! Reliable retirement acknowledgements can arrive after a newer owner topology.
use super::tests::{fixture_from_sim, receipt_runtime};
use super::*;
use dreamwake_protocol::live::baselines as wire;
use engine_net::replication::{baselines::*, *};
use std::collections::BTreeMap;

struct Fixture {
    runtime: Runtime,
    delayed: Vec<BaselineRetirement>,
    current: Vec<BaselineRetirement>,
    optional: FullState<Vec<u8>>,
}

fn receive_group(session: &mut Session, frames: Vec<Vec<u8>>, millis: u64) {
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(millis))
            .unwrap();
    }
}

fn server_decoded(session: &Session, servers: &mut BTreeMap<EntityId, ServerBaselines<Vec<u8>>>) {
    let group = session
        .groups
        .published_group(OWNER_GROUP, &session.scopes)
        .unwrap();
    for scope in group.manifest() {
        let state = session.scopes.state(scope.entity).unwrap();
        let receipt = wire::target(state, group.publication(), BaselineGeneration(1));
        let server = servers.entry(scope.entity).or_insert_with(|| {
            ServerBaselines::new(receipt.context, receipt.generation, wire::limits()).unwrap()
        });
        assert!(
            server
                .transmit(
                    BaselineState {
                        receipt,
                        payload: state.payload.clone(),
                    },
                    None,
                    &BytePatch,
                    |_| true,
                )
                .unwrap()
        );
        server.acknowledge(receipt).unwrap();
    }
}

fn fixture() -> Fixture {
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    let (mut session, frames) = fixture_from_sim(&sim, 10, 1, 4);
    receive_group(&mut session, frames, 1);
    let mut servers = BTreeMap::new();
    server_decoded(&session, &mut servers);
    let (_, frames) = fixture_from_sim(&sim, 11, 2, 4);
    receive_group(&mut session, frames, 2);
    server_decoded(&session, &mut servers);
    let delayed = session.baselines.retirements().unwrap();
    assert_eq!(delayed.len(), 4);
    // Use the real server cache acceptance before delaying its reliable reply.
    for &fence in &delayed {
        servers
            .get_mut(&fence.context.scope.entity)
            .unwrap()
            .retire(fence)
            .unwrap();
    }
    let optional = session
        .scopes
        .state(EntityId {
            index: 4,
            generation: 1,
        })
        .unwrap()
        .clone();
    let world = sim.world_mut();
    *world
        .query::<&mut engine_core::KinematicState>()
        .single_mut(world)
        .unwrap() = engine_core::KinematicState::new([0.0, 0.0, 15.0], 1);
    sim.step(DreamInput::default());
    for (tick, snapshot) in [(12, 3), (13, 4)] {
        let (_, frames) = fixture_from_sim(&sim, tick, snapshot, 5);
        receive_group(&mut session, frames, snapshot);
    }
    let current = session.baselines.retirements().unwrap();
    assert_eq!(current.len(), 3);
    assert!(session.baselines.identity(optional.scope.entity).is_none());
    assert_eq!(
        session
            .scopes
            .fence(optional.scope.entity.index)
            .unwrap()
            .scope,
        optional.scope
    );
    session.begin_frame().unwrap();
    session.reconcile().unwrap();
    session.acknowledge_active(SnapshotId(4), ServerTick(13));
    session.reconcile().unwrap();
    assert!(session.active);
    let mut runtime = receipt_runtime(session);
    runtime.last_received = 0.25;
    Fixture {
        runtime,
        delayed,
        current,
        optional,
    }
}

fn receive(fixture: &mut Fixture, fences: Vec<BaselineRetirement>) -> ReceiveRejection {
    let session = fixture.runtime.prediction.as_ref().unwrap();
    // Hostile cases must reach the receiver even if a conforming sender refuses them.
    let bytes = encode_control(
        &UnvalidatedControl(ServerControl::BaselineRetired {
            fences: BoundedVec::new(fences).unwrap(),
        }),
        session.model.welcome.stream.epoch,
        0,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap();
    reject_encoded(fixture, &bytes)
}

fn reject_encoded(fixture: &mut Fixture, bytes: &[u8]) -> ReceiveRejection {
    let session = fixture.runtime.prediction.as_ref().unwrap();
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    let before = session.baselines.retained_bytes();
    let control_sequence = session.control_sequence;
    let owner_identity = session.predictor.identity(OWNER_GROUP).unwrap().clone();
    let owner_state = session.predictor.state(OWNER_GROUP).unwrap().clone();
    let states = fixture
        .current
        .iter()
        .map(|f| {
            session
                .scopes
                .state(f.context.scope.entity)
                .unwrap()
                .clone()
        })
        .collect::<Vec<_>>();
    let error = receive_message(
        &mut fixture.runtime,
        CONTROL_CHANNEL,
        bytes,
        Duration::from_secs(1),
        &mut world,
    )
    .unwrap_err();
    assert_eq!(fixture.runtime.last_received, 0.25);
    assert_eq!(world.resource::<diagnostics::Telemetry>().snapshots, 0);
    let session = fixture.runtime.prediction.as_mut().unwrap();
    assert!(session.active);
    assert!(!session.recovering);
    assert_eq!(session.control_sequence, control_sequence);
    assert_eq!(
        session.predictor.identity(OWNER_GROUP),
        Some(&owner_identity)
    );
    assert_eq!(session.predictor.state(OWNER_GROUP), Some(&owner_state));
    assert_eq!(session.baselines.retained_bytes(), before);
    assert!(session.take_decoded_receipts().is_none());
    assert!(session.baselines.retirements().unwrap().is_empty());
    for state in states {
        assert_eq!(session.scopes.state(state.scope.entity), Some(&state));
    }
    // Exact current proposals remain acceptable on a clone after the rejection.
    session
        .baselines
        .stage()
        .unwrap()
        .acknowledge_retirements(&fixture.current)
        .unwrap();
    error
}

fn assert_partition(error: &ReceiveRejection, obsolete: bool) {
    let mut telemetry = diagnostics::Telemetry::default();
    record_receive_rejection(&mut telemetry, error);
    assert_eq!(telemetry.decode_errors, 1);
    assert_eq!(telemetry.obsolete_rejections, u64::from(obsolete));
    assert_eq!(telemetry.decode_failures, u64::from(!obsolete));
    assert_eq!(telemetry.stale_epoch_rejections, 0);
    assert_eq!(telemetry.clock_resync_rejections, 0);
    assert_eq!(telemetry.transport_errors, 0);
}

#[test]
fn delayed_retirement_after_optional_member_removal_is_obsolete_in_live_receive() {
    let mut fixture = fixture();
    let fences = fixture.delayed.clone();
    let error = receive(&mut fixture, fences);
    assert!(
        matches!(
            error,
            ReceiveRejection::Replication {
                error: ReplicationError::Obsolete(ObsoleteReason::Publication),
                ..
            }
        ),
        "{error:?}"
    );
    assert_partition(&error, true);
}

#[test]
fn delayed_optional_only_retirement_uses_current_group_and_closed_scope_proof() {
    for newer_closed_scope in [false, true] {
        let mut fixture = fixture();
        let optional = fixture.optional.clone();
        let session = fixture.runtime.prediction.as_mut().unwrap();
        session
            .scopes
            .apply_exit(ScopeExit {
                scope: optional.scope,
            })
            .unwrap();
        if newer_closed_scope {
            let mut newer = optional.clone();
            newer.scope.scope.0 += 1;
            newer.scope.representation.0 += 1;
            let scope = newer.scope;
            session.scopes.apply_full(newer, |_| true).unwrap();
            session.scopes.apply_exit(ScopeExit { scope }).unwrap();
        }
        let fences = fixture
            .delayed
            .iter()
            .copied()
            .filter(|f| f.context.scope.entity == optional.scope.entity)
            .collect::<Vec<_>>();
        assert_eq!(fences.len(), 1);
        let error = receive(&mut fixture, fences);
        assert!(
            matches!(
                error,
                ReceiveRejection::Replication {
                    error: ReplicationError::Obsolete(ObsoleteReason::Publication),
                    ..
                }
            ),
            "{error:?}"
        );
        assert_partition(&error, true);
    }
}

#[test]
fn missing_or_conflicting_retirement_scope_never_hides_behind_an_old_group() {
    for optional_only in [false, true] {
        for reversed in [false, true] {
            for case in [
                "missing fence",
                "unknown entity",
                "zero entity index",
                "entity generation",
                "connection",
                "schema",
                "future scope",
                "representation",
            ] {
                let mut fixture = fixture();
                let entity = fixture.optional.scope.entity;
                let mut fences = fixture.delayed.clone();
                let optional = fences
                    .iter_mut()
                    .find(|f| f.context.scope.entity == entity)
                    .unwrap();
                match case {
                    "missing fence" => {
                        let session = fixture.runtime.prediction.as_mut().unwrap();
                        let mut scopes =
                            ClientScopes::new(session.scopes.connection(), ScopeLimits::default())
                                .unwrap();
                        for fence in &fixture.current {
                            let state = session
                                .scopes
                                .state(fence.context.scope.entity)
                                .unwrap()
                                .clone();
                            scopes.apply_full(state, |_| true).unwrap();
                        }
                        session.scopes = scopes;
                    }
                    "unknown entity" => optional.context.scope.entity.index = u64::MAX,
                    "zero entity index" => optional.context.scope.entity.index = 0,
                    "entity generation" => optional.context.scope.entity.generation += 1,
                    "connection" => optional.context.scope.connection.0 += 1,
                    "schema" => optional.context.schema = SchemaId(99),
                    "future scope" => optional.context.scope.scope.0 += 1,
                    "representation" => optional.context.scope.representation.0 += 1,
                    _ => unreachable!(),
                }
                if optional_only {
                    fences = vec![*optional];
                }
                if reversed {
                    fences.reverse();
                }
                let error = receive(&mut fixture, fences);
                assert!(
                    !matches!(
                        error,
                        ReceiveRejection::Replication {
                            error: ReplicationError::Obsolete(_),
                            ..
                        }
                    ),
                    "{case}, optional_only={optional_only}, reversed={reversed}: {error:?}"
                );
                assert_partition(&error, false);
                if case == "unknown entity" {
                    let ReceiveRejection::Replication { context, .. } = error else {
                        panic!("typed unknown identity");
                    };
                    assert!(context.contains(&format!("entity={}/1", u64::MAX)));
                    assert!(context.contains("through=1"));
                    assert!(context.chars().count() <= 768);
                }
            }
        }
    }
}

#[test]
fn mixed_or_duplicate_retirement_batches_and_unrequested_current_fences_stay_hard() {
    for reversed in [false, true] {
        for case in [
            "mixed revision",
            "wrong group",
            "duplicate",
            "unrequested current",
        ] {
            let mut fixture = fixture();
            let mut fences = fixture.delayed.clone();
            match case {
                "mixed revision" => fences[0] = fixture.current[0],
                "wrong group" => fences[3].context.group = Some((GroupId(99), GroupRevision(4))),
                "duplicate" => fences[3] = fences[0],
                "unrequested current" => {
                    fences = fixture.current.clone();
                    fences[2].through.0 -= 1;
                }
                _ => unreachable!(),
            }
            if reversed {
                fences.reverse();
            }
            let error = receive(&mut fixture, fences);
            assert!(
                !matches!(
                    error,
                    ReceiveRejection::Replication {
                        error: ReplicationError::Obsolete(_),
                        ..
                    }
                ),
                "{case}, reversed={reversed}: {error:?}"
            );
            if case == "unrequested current" {
                assert!(matches!(
                    error,
                    ReceiveRejection::Replication {
                        error: ReplicationError::Unknown,
                        ..
                    }
                ));
            }
            assert_partition(&error, false);
        }
    }
}

#[derive(serde::Serialize)]
struct UnvalidatedControl(ServerControl);
impl engine_net::codec::Validate for UnvalidatedControl {
    fn validate(&self) -> Result<(), engine_net::codec::CodecError> {
        Ok(())
    }
}

#[test]
fn malformed_retirement_controls_fail_the_live_decoder_before_age_classification() {
    for reversed in [false, true] {
        for case in [
            "generation",
            "through",
            "entity generation",
            "connection",
            "scope",
            "representation",
            "schema",
            "group",
            "group revision",
        ] {
            let mut fixture = fixture();
            let mut fences = fixture.delayed.clone();
            let bad = &mut fences[3];
            match case {
                "generation" => bad.generation.0 = 0,
                "through" => bad.through.0 = 0,
                "entity generation" => bad.context.scope.entity.generation = 0,
                "connection" => bad.context.scope.connection.0 = 0,
                "scope" => bad.context.scope.scope.0 = 0,
                "representation" => bad.context.scope.representation.0 = 0,
                "schema" => bad.context.schema.0 = 0,
                "group" => bad.context.group.as_mut().unwrap().0.0 = 0,
                "group revision" => bad.context.group.as_mut().unwrap().1.0 = 0,
                _ => unreachable!(),
            }
            if reversed {
                fences.reverse();
            }
            let control = ServerControl::BaselineRetired {
                fences: BoundedVec::new(fences).unwrap(),
            };
            let session = fixture.runtime.prediction.as_ref().unwrap();
            let epoch = session.model.welcome.stream.epoch;
            let limit = session.model.welcome.frame_limit().unwrap();
            assert!(encode_control(&control, epoch, 0, limit).is_err());
            // Bypass only sender validation; the real framed receiver must reject it.
            let bytes = encode_control(&UnvalidatedControl(control), epoch, 0, limit).unwrap();
            let error = reject_encoded(&mut fixture, &bytes);
            assert_partition(&error, false);
        }
    }
}

#[test]
fn current_matching_retirement_acknowledgements_still_release_only_the_old_prefix() {
    let mut fixture = fixture();
    let session = fixture.runtime.prediction.as_ref().unwrap();
    let before = session.baselines.retained_bytes();
    let bytes = encode_control(
        &ServerControl::BaselineRetired {
            fences: BoundedVec::new(fixture.current.clone()).unwrap(),
        },
        session.model.welcome.stream.epoch,
        0,
        session.model.welcome.frame_limit().unwrap(),
    )
    .unwrap();
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    for _ in 0..2 {
        receive_message(
            &mut fixture.runtime,
            CONTROL_CHANNEL,
            &bytes,
            Duration::from_secs(1),
            &mut world,
        )
        .unwrap();
        assert_eq!(fixture.runtime.last_received, 0.25);
        let session = fixture.runtime.prediction.as_mut().unwrap();
        assert!(session.baselines.retained_bytes() < before);
        assert!(session.baselines.retirements().unwrap().is_empty());
        assert!(session.take_decoded_receipts().is_none());
        assert_eq!(session.control_sequence, 0);
    }
    assert_eq!(world.resource::<diagnostics::Telemetry>().decode_errors, 0);
}

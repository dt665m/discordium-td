//! Published index reuse retires old generations without relying on removal order.
use super::*;
use dreamwake_protocol::replication::{ReplicaPayload, encode_replica};
use dreamwake_sim::replication::{PublicCoverMarkerView, PublicDamageView, PublicReplica};
use engine_net::replication::*;

fn receive_full(
    session: &mut Session,
    state: FullState<Vec<u8>>,
) -> Result<Option<ClientControl>, ReceiveRejection> {
    let welcome = &session.model.welcome;
    let bytes = encode_state(
        &StateFrame::Full(state),
        welcome.stream.epoch,
        0,
        welcome.frame_limit().unwrap(),
    )
    .unwrap();
    session.receive_state(&bytes, Duration::ZERO)
}

fn assert_current(runtime: &mut Runtime, scope: ScopeIdentity, count: usize, tick: ServerTick) {
    let session = runtime.prediction.as_mut().unwrap();
    assert!(session.entities.contains(&scope.entity));
    assert_eq!(session.entities.len(), count);
    assert_eq!(
        session
            .entities
            .iter()
            .filter(|entity| entity.index == scope.entity.index)
            .count(),
        1
    );
    assert_eq!(session.scopes.state(scope.entity).unwrap().scope, scope);
    session.poses.advance(ServerTick(tick.0 + 6));
    assert_eq!(session.poses.combat_view_tick(), Some(tick));
    assert!(
        runtime
            .capture_combat_view(Duration::from_secs(10))
            .is_some()
    );
}

fn delayed_removals(runtime: &mut Runtime, old: ScopeIdentity) {
    let mut world = World::new();
    world.init_resource::<diagnostics::Telemetry>();
    for message in [
        ServerControl::Exit(ScopeExit { scope: old }),
        ServerControl::Destroy(Destroy {
            connection: old.connection,
            entity: old.entity,
        }),
    ] {
        let welcome = &runtime.prediction.as_ref().unwrap().model.welcome;
        let bytes = encode_control(
            &message,
            welcome.stream.epoch,
            0,
            welcome.frame_limit().unwrap(),
        )
        .unwrap();
        receive_message(runtime, CONTROL_CHANNEL, &bytes, Duration::ZERO, &mut world).unwrap();
        assert!(
            !runtime
                .prediction
                .as_ref()
                .unwrap()
                .entities
                .contains(&old.entity)
        );
    }
}

fn initialize(session: &mut Session, frames: Vec<Vec<u8>>, snapshot: u64, tick: u64) {
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.acknowledge_active(SnapshotId(snapshot), ServerTick(tick));
    session.reconcile().unwrap();
    assert!(session.active);
    session
        .observe_clock(TimeExchange {
            client_send: Duration::ZERO,
            server_receive: Duration::ZERO,
            server_send: Duration::ZERO,
            client_receive: Duration::ZERO,
        })
        .unwrap();
}

#[test]
fn standalone_generation_reuse_is_bounded_and_old_controls_preserve_current_combat_view() {
    for replacement in ["enemy", "damage", "attachment"] {
        let (mut session, frames) = tests::fixture();
        initialize(&mut session, frames, 1, 10);
        let count = session.entities.len() + 1;
        let mut runtime = tests::receipt_runtime(session);
        let mut previous = None;
        for generation in 1..=32 {
            let session = runtime.prediction.as_mut().unwrap();
            let mut state = tests::public_full_state(session, 100, u64::from(generation) * 2);
            state.scope.entity.generation = generation;
            state.end_tick = ServerTick(12);
            if generation > 1 {
                state.payload = match replacement {
                    "damage" => encode_replica(&ReplicaPayload::Actor(PublicReplica::Damage(
                        PublicDamageView {
                            id: 100,
                            position: [0.0; 2],
                            amount: 1.0,
                            critical: false,
                            friendly: false,
                            age: 0.0,
                        },
                    )))
                    .unwrap(),
                    "attachment" => encode_replica(&ReplicaPayload::Actor(
                        PublicReplica::CoverMarker(PublicCoverMarkerView {
                            id: 100,
                            parent: ScopeIdentity {
                                entity: EntityId {
                                    index: 200,
                                    generation: 1,
                                },
                                ..state.scope
                            },
                            revision: 1,
                            local_offset: [0.0, 1.35, 0.0],
                            open: false,
                        }),
                    ))
                    .unwrap(),
                    _ => state.payload,
                };
            }
            let scope = state.scope;
            receive_full(session, state.clone()).unwrap();
            state.snapshot.0 += 1;
            state.version.0 += 1;
            state.end_tick = ServerTick(14);
            receive_full(session, state).unwrap();
            assert_current(&mut runtime, scope, count, ServerTick(12));
            if let Some(old) = previous {
                delayed_removals(&mut runtime, old);
                assert_current(&mut runtime, scope, count, ServerTick(12));
            }
            previous = Some(scope);
        }
    }
}

#[test]
fn invalid_standalone_replacement_preserves_old_membership_and_pose() {
    let (mut session, frames) = tests::fixture();
    initialize(&mut session, frames, 1, 10);
    let state = tests::public_full_state(&session, 100, 2);
    let scope = state.scope;
    receive_full(&mut session, state.clone()).unwrap();
    let before = session.entities.clone();
    let mut invalid = state.clone();
    invalid.scope.entity.generation = 2;
    invalid.payload.pop();
    assert!(receive_full(&mut session, invalid).is_err());
    assert_eq!(session.entities, before);
    assert_eq!(session.scopes.state(scope.entity), Some(&state));
    let mut runtime = tests::receipt_runtime(session);
    assert_current(&mut runtime, scope, before.len(), ServerTick(12));
}

#[test]
fn atomic_platform_generation_replacement_publishes_before_retiring_the_old_enemy() {
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    let world = sim.world_mut();
    *world
        .query::<&mut engine_core::KinematicState>()
        .single_mut(world)
        .unwrap() = engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
    sim.step(DreamInput::default());
    let (session, _) = tests::fixture_from_sim(&sim, 10, 1, 1);
    let mut runtime = tests::receipt_runtime(session);
    for cycle in 1..=16 {
        let generation = cycle * 2;
        let tick = 10 + u64::from(generation);
        let (mut source, frames) =
            tests::fixture_from_sim(&sim, tick, u64::from(generation), u64::from(generation));
        initialize(&mut source, frames, u64::from(generation), tick);
        let mut group = tests::published_group_chunk(&source);
        assert_eq!(
            group.members.len(),
            4,
            "fixture must admit the required platform"
        );
        let platform = group
            .members
            .iter_mut()
            .find(|state| state.scope.entity.index == 4)
            .unwrap();
        platform.scope.entity.generation = generation;
        let scope = platform.scope;
        group.manifest = group.members.iter().map(|state| state.scope).collect();
        let session = runtime.prediction.as_mut().unwrap();
        let mut enemy = tests::public_full_state(session, 4, u64::from(generation));
        enemy.scope.entity.generation = generation - 1;
        enemy.end_tick = ServerTick(tick);
        let old = enemy.scope;
        receive_full(session, enemy).unwrap();
        let before = session.entities.clone();
        let frames = tests::group_frames(&session.model.welcome, group.clone());
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert!(
            session
                .receive_state(&frame[..frame.len() - 1], Duration::ZERO)
                .is_err()
        );
        assert_eq!(
            session.entities, before,
            "incomplete group cannot retire membership"
        );
        assert!(
            session
                .receive_state(frames.last().unwrap(), Duration::ZERO)
                .unwrap()
                .is_some()
        );
        if cycle == 1 {
            session
                .observe_clock(TimeExchange {
                    client_send: Duration::ZERO,
                    server_receive: Duration::ZERO,
                    server_send: Duration::ZERO,
                    client_receive: Duration::ZERO,
                })
                .unwrap();
        }
        session.active = true;
        assert_current(&mut runtime, scope, 4, ServerTick(tick));
        delayed_removals(&mut runtime, old);
        assert_current(&mut runtime, scope, 4, ServerTick(tick));

        // A complete but invalid owner group must likewise preserve the platform.
        group.publication.revision.0 += 1;
        group.publication.snapshot.0 += 1;
        let invalid = group
            .members
            .iter_mut()
            .find(|state| state.scope.entity.index == 4)
            .unwrap();
        invalid.scope.entity.generation += 1;
        invalid.payload =
            tests::public_full_state(runtime.prediction.as_ref().unwrap(), 4, 1).payload;
        group.manifest = group.members.iter().map(|state| state.scope).collect();
        let session = runtime.prediction.as_mut().unwrap();
        let mut rejected = false;
        for frame in tests::group_frames(&session.model.welcome, group) {
            match session.receive_state(&frame, Duration::ZERO) {
                Ok(None) => {}
                Ok(Some(_)) => panic!("invalid atomic replacement published"),
                Err(_) => rejected = true,
            }
        }
        assert!(rejected);
        assert_current(&mut runtime, scope, 4, ServerTick(tick));
    }
}

//! Public targeting inputs are bounded, time-valid and retained for replay.
use super::model::{CollisionDependency, MAX_PRESENTATION_TARGETS};
use super::*;
use dreamwake_protocol::replication::{ReplicaPayload, decode_replica, encode_replica};
use engine_net::prediction::DependencyValue;
use engine_net::replication::ScopeExit;

fn initialized() -> Session {
    let (mut session, frames) = tests::fixture();
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.reconcile().unwrap();
    session
}

fn publish_target(session: &mut Session, index: u64, revision: u64, x: f32) -> EntityId {
    let mut state = tests::public_full_state(session, index, revision);
    let ReplicaPayload::Actor(dreamwake_sim::replication::PublicReplica::Enemy(mut target)) =
        decode_replica(&state.payload, None).unwrap()
    else {
        panic!("enemy fixture");
    };
    target.position = [x, 0.0];
    state.payload = encode_replica(&ReplicaPayload::Actor(
        dreamwake_sim::replication::PublicReplica::Enemy(target),
    ))
    .unwrap();
    let entity = state.scope.entity;
    session.scopes.apply_full(state, |_| true).unwrap();
    session.entities.insert(entity);
    entity
}

#[test]
fn targeting_inputs_keep_original_pose_after_update_and_scope_exit() {
    let mut session = initialized();
    let entity = publish_target(&mut session, 100, 1, 2.0);
    assert!(
        !session
            .prediction_dependencies(ServerTick(10))
            .unwrap()
            .records
            .contains_key(&entity)
    );
    let original = session.prediction_dependencies(ServerTick(11)).unwrap();
    publish_target(&mut session, 100, 2, 8.0);
    let updated = session.prediction_dependencies(ServerTick(12)).unwrap();
    let position = |frame: &engine_net::prediction::DependencyFrame<CollisionDependency>| {
        let DependencyValue::Known(CollisionDependency::PresentationTarget(target)) =
            &frame.records[&entity].value
        else {
            panic!("target dependency");
        };
        target.position
    };
    assert_eq!(position(&original), [2.0, 0.0]);
    assert_eq!(position(&updated), [8.0, 0.0]);
    let scope = session.scopes.state(entity).unwrap().scope;
    session.scopes.apply_exit(ScopeExit { scope }).unwrap();
    assert!(
        !session
            .prediction_dependencies(ServerTick(13))
            .unwrap()
            .records
            .contains_key(&entity)
    );
    assert_eq!(position(&original), [2.0, 0.0]);
}

#[test]
fn targeting_inputs_choose_a_bounded_nearest_set() {
    let mut session = initialized();
    for offset in 0..MAX_PRESENTATION_TARGETS + 8 {
        publish_target(&mut session, 100 + offset as u64, 1, offset as f32);
    }
    let frame = session.prediction_dependencies(ServerTick(11)).unwrap();
    let targets = frame
        .records
        .values()
        .filter_map(|record| match &record.value {
            DependencyValue::Known(CollisionDependency::PresentationTarget(target)) => {
                Some(target.id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(targets.len(), MAX_PRESENTATION_TARGETS);
    assert!(targets.contains(&100));
    assert!(!targets.contains(&(100 + MAX_PRESENTATION_TARGETS as u64 + 7)));
}

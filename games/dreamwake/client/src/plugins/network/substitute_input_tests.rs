//! Substitute replay consumes committed held intent without authoring commands.
use super::*;
use dreamwake_protocol::{
    live::baselines as wire,
    replication::{ReplicaPayload, decode_replica, encode_replica},
};
use dreamwake_sim::replication::OwnerPredictionState;
use engine_net::{
    commands::InputContinuity,
    prediction::{Correction, DependencyFrame, DependencyValue, EventChanges, ReplayInput},
    replication::{
        GroupChunk,
        baselines::{BaselineEncoding, BaselinePacket},
        encode_group, fragment_group,
    },
};

fn publication(snapshot: u64, continuity: InputContinuity<DreamInput>) -> (Session, Vec<Vec<u8>>) {
    let (mut source, frames) = tests::fixture_publication(10, snapshot, 1);
    for frame in frames {
        source.receive_state(&frame, Duration::ZERO).unwrap();
    }
    let proof = source
        .groups
        .published_group(OWNER_GROUP, &source.scopes)
        .unwrap();
    let mut group = GroupChunk {
        publication: proof.publication(),
        end_tick: proof.end_tick(),
        manifest: proof.manifest().to_vec(),
        index: 0,
        count: 1,
        members: proof.states().cloned().collect::<Vec<_>>(),
    };
    let state = group
        .members
        .iter_mut()
        .find(|state| state.scope.entity == source.model.welcome.owner_entity)
        .unwrap();
    let ReplicaPayload::Owner(owner) =
        decode_replica(&state.payload, Some(source.model.expectation())).unwrap()
    else {
        panic!("owner checkpoint fixture");
    };
    state.payload = encode_replica(&ReplicaPayload::Owner(
        owner
            .with_input_continuity(group.end_tick, continuity)
            .unwrap(),
    ))
    .unwrap();
    for state in &mut group.members {
        state.payload = wire::encode_member(&BaselinePacket {
            target: wire::target(state, group.publication, BaselineGeneration(1)),
            encoding: BaselineEncoding::Full,
            bytes: state.payload.clone(),
        })
        .unwrap();
    }
    let limit = source.model.welcome.frame_limit().unwrap();
    let bytes = encode_group(&group, group_limits(), |payload| Ok(payload.clone())).unwrap();
    let frames = fragment_group(group.publication, &bytes, byte_limits(limit).unwrap())
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(index, fragment)| {
            encode_state(
                &StateFrame::Fragment(fragment),
                source.model.welcome.stream.epoch,
                index as u32,
                limit,
            )
            .unwrap()
        })
        .collect();
    (Session::new(source.model.welcome.clone()).unwrap(), frames)
}

fn initialized(snapshot: u64, continuity: InputContinuity<DreamInput>) -> Session {
    let (mut session, frames) = publication(snapshot, continuity.clone());
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.reconcile().unwrap();
    assert_eq!(
        session
            .predictor
            .state(OWNER_GROUP)
            .unwrap()
            .0
            .checkpoint()
            .input_continuity(),
        &continuity
    );
    session
}

fn dependencies(session: &Session) -> DependencyFrame<model::CollisionDependency> {
    session
        .predictor
        .checkpoint_dependencies(OWNER_GROUP)
        .unwrap()
        .clone()
}

fn substitute(session: &mut Session, tick: u64) {
    assert_eq!(
        session.predictor.predict_substitute(
            OWNER_GROUP,
            ServerTick(tick),
            dependencies(session),
            &session.model,
        ),
        Ok(EventChanges::default())
    );
    assert!(
        session
            .predictor
            .command_history(OWNER_GROUP, ServerTick(tick))
            .is_none()
    );
    assert!(matches!(
        session
            .predictor
            .replay_input_history(OWNER_GROUP, ServerTick(tick)),
        Some(ReplayInput::Substitute)
    ));
}

fn step_explicit_input(session: &Session, state: &mut OwnerPredictionState, input: DreamInput) {
    let deps = dependencies(session);
    let DependencyValue::Known(model::CollisionDependency::StaticCollision(collision)) =
        deps.value(session.model.welcome.collision_entity).unwrap()
    else {
        panic!("static collision dependency");
    };
    let bases = deps
        .records
        .values()
        .filter_map(|record| match &record.value {
            DependencyValue::Known(model::CollisionDependency::Platform(base)) => {
                Some(base.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    state
        .step_restricted_combat_with_bases(input, collision, &bases, None, None)
        .unwrap();
}

#[test]
fn authoritative_continuity_holds_three_missing_ticks_then_predicts_neutral() {
    assert_eq!(dreamwake_sim::HELD_INPUT_GRACE_TICKS, 3);
    let held = DreamInput {
        movement: [1.0, 0.0],
        aim: [1.0, 0.0],
        ..Default::default()
    };
    let mut session = initialized(
        1,
        InputContinuity {
            last_input: Some(held),
            missing_streak: 0,
        },
    );
    let mut expected = session.predictor.state(OWNER_GROUP).unwrap().0.clone();
    for missing in 1..=5 {
        // This reference directly supplies the contract's input to the shared
        // simulation; it does not call the substitution implementation.
        step_explicit_input(
            &session,
            &mut expected,
            if missing <= 3 {
                held
            } else {
                DreamInput::default()
            },
        );
        substitute(&mut session, 10 + missing);
        let actual = &session.predictor.state(OWNER_GROUP).unwrap().0;
        assert_eq!(actual.hero_view(), expected.hero_view());
        assert_eq!(actual.gameplay_tick(), expected.gameplay_tick());
        assert!(!actual.hero_view().dashing);
        assert_eq!(
            actual.checkpoint().input_continuity(),
            &InputContinuity {
                last_input: Some(held),
                missing_streak: missing as u32,
            }
        );
    }
    assert_eq!(session.sequence, CommandSeq(0));
}

#[test]
fn substitutes_continue_one_authored_dash_without_recording_another_edge() {
    let mut session = initialized(1, InputContinuity::default());
    let held = DreamInput {
        movement: [1.0, 0.0],
        aim: [1.0, 0.0],
        ..Default::default()
    };
    let first = Command {
        owner: session.model.welcome.stream,
        sequence: CommandSeq(1),
        target: TargetTick(11),
        input: held.into(),
        actions: BoundedVec::new(vec![ActionEdge {
            slot: 0,
            action: DreamAction::Dash {
                direction: [1.0, 0.0],
            },
        }])
        .unwrap(),
    };
    session
        .predictor
        .predict(
            OWNER_GROUP,
            InputCommand(first.clone()),
            dependencies(&session),
            &session.model,
        )
        .unwrap();
    let mut expected = session.predictor.state(OWNER_GROUP).unwrap().0.clone();
    assert!(expected.hero_view().dashing);
    let mut previous_cooldown = expected.hero_view().dash_cooldown;
    assert!(previous_cooldown > 0.0);
    for tick in 12..=15 {
        step_explicit_input(
            &session,
            &mut expected,
            if tick < 15 {
                held
            } else {
                DreamInput::default()
            },
        );
        substitute(&mut session, tick);
        let actual = &session.predictor.state(OWNER_GROUP).unwrap().0;
        assert_eq!(actual.hero_view(), expected.hero_view());
        assert!(actual.hero_view().dash_cooldown < previous_cooldown);
        previous_cooldown = actual.hero_view().dash_cooldown;
        assert_eq!(
            actual.checkpoint().input_continuity().last_input,
            Some(held)
        );
    }
    let recorded = session
        .predictor
        .command_history(OWNER_GROUP, ServerTick(11))
        .unwrap();
    assert_eq!(recorded.0, first);
    // The next genuine command keeps the next sequence; substitute ticks have
    // neither manufactured commands nor copied the old Dash action key.
    let next = Command {
        sequence: CommandSeq(2),
        target: TargetTick(16),
        actions: BoundedVec::default(),
        ..first
    };
    session
        .predictor
        .predict(
            OWNER_GROUP,
            InputCommand(next.clone()),
            dependencies(&session),
            &session.model,
        )
        .unwrap();
    assert_eq!(
        session
            .predictor
            .command_history(OWNER_GROUP, ServerTick(16))
            .unwrap()
            .0,
        next
    );
}

#[test]
fn corrected_authoritative_continuity_replays_substitutes_from_restored_input() {
    let held = |direction| DreamInput {
        movement: [direction, 0.0],
        aim: [direction, 0.0],
        ..Default::default()
    };
    let mut session = initialized(
        1,
        InputContinuity {
            last_input: Some(held(1.0)),
            missing_streak: 0,
        },
    );
    let initial_hero = session.predictor.state(OWNER_GROUP).unwrap().0.hero_view();
    for tick in 11..=14 {
        substitute(&mut session, tick);
    }
    let old_frontier = session.predictor.state(OWNER_GROUP).unwrap().clone();
    let corrected = InputContinuity {
        last_input: Some(held(-1.0)),
        missing_streak: 2,
    };
    let mut fresh = initialized(2, corrected.clone());
    // The authority changes only continuation and publication identity; its
    // initial pose is identical, so cached old held input cannot explain replay.
    assert_eq!(
        fresh.predictor.state(OWNER_GROUP).unwrap().0.hero_view(),
        initial_hero
    );
    for tick in 11..=14 {
        substitute(&mut fresh, tick);
    }
    let (_, frames) = publication(2, corrected);
    session.begin_frame().unwrap();
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_millis(1))
            .unwrap();
    }
    let proof = session
        .groups
        .published_group(OWNER_GROUP, &session.scopes)
        .unwrap();
    assert_eq!(
        session.predictor.reconcile(
            &proof,
            session.model.binding(),
            ServerTick(10),
            &session.model,
        ),
        Ok(Correction::Replayed {
            from: ServerTick(11),
            through: ServerTick(14),
            events: EventChanges::default(),
        })
    );
    assert_eq!(session.predictor.work().replay_ticks, 4);
    assert_eq!(session.predictor.work().predicted_ticks, 0);
    assert!(
        session
            .predictor
            .replay_input_history(OWNER_GROUP, ServerTick(10))
            .is_none()
    );
    for tick in 11..=14 {
        assert_eq!(
            session
                .predictor
                .history_state(OWNER_GROUP, ServerTick(tick)),
            fresh.predictor.history_state(OWNER_GROUP, ServerTick(tick))
        );
        assert!(matches!(
            session
                .predictor
                .replay_input_history(OWNER_GROUP, ServerTick(tick)),
            Some(ReplayInput::Substitute)
        ));
        assert!(
            session
                .predictor
                .command_history(OWNER_GROUP, ServerTick(tick))
                .is_none()
        );
    }
    let replayed = session.predictor.state(OWNER_GROUP).unwrap();
    assert_ne!(
        replayed.0.hero_view().position,
        old_frontier.0.hero_view().position
    );
    assert_eq!(replayed.0.checkpoint().input_continuity().missing_streak, 6);
    assert_eq!(session.sequence, CommandSeq(0));
}

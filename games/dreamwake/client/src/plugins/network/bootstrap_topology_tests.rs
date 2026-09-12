//! Fresh authoritative topology may advance an owner that has not predicted input.
use super::*;
use engine_net::prediction::ReplayInput;

const INITIAL_TICK: u64 = 441;

fn move_authority(sim: &mut DreamSimulation, position: [f32; 3]) {
    let world = sim.world_mut();
    *world
        .query::<&mut engine_core::KinematicState>()
        .single_mut(world)
        .unwrap() = engine_core::KinematicState::new(position, 1);
    sim.step(DreamInput::default());
}

fn active_checkpoint() -> (DreamSimulation, Session) {
    let mut sim = DreamSimulation::new(71, false);
    sim.continue_run_for(1);
    // Outside the platform's expanded dependency envelope: exactly three scopes.
    move_authority(&mut sim, [0.0, 0.0, 15.0]);
    let (mut session, frames) = tests::fixture_from_sim(&sim, INITIAL_TICK, 1, 1);
    session.begin_frame().unwrap();
    for frame in frames {
        session.receive_state(&frame, Duration::ZERO).unwrap();
    }
    session.reconcile().unwrap();
    session.acknowledge_active(SnapshotId(1), ServerTick(INITIAL_TICK));
    session.reconcile().unwrap();
    assert!(session.active);
    assert!(!session.recovering);
    assert_eq!(session.sequence, CommandSeq(0));
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(INITIAL_TICK))
    );
    assert!(
        session
            .predictor
            .replay_input_history(OWNER_GROUP, ServerTick(INITIAL_TICK))
            .is_none()
    );
    assert_eq!(
        session
            .predictor
            .state(OWNER_GROUP)
            .unwrap()
            .0
            .stamp()
            .server_tick,
        INITIAL_TICK
    );
    assert_eq!(
        session
            .predictor
            .identity(OWNER_GROUP)
            .unwrap()
            .scopes
            .len(),
        3
    );
    (sim, session)
}

fn receive_topology(sim: &DreamSimulation, session: &mut Session, tick: u64) {
    let identity = session.predictor.identity(OWNER_GROUP).unwrap().clone();
    let generation = session
        .predictor
        .state(OWNER_GROUP)
        .unwrap()
        .0
        .combat_generation();
    let (_, frames) = tests::fixture_from_sim(sim, tick, 2, 2);
    // A separate receive frame rules out already-spent work as a rejection cause.
    session.begin_frame().unwrap();
    session.finalized = ServerTick(tick);
    for frame in frames {
        session
            .receive_state(&frame, Duration::from_secs(1))
            .unwrap();
    }
    let proof = session
        .groups
        .published_group(OWNER_GROUP, &session.scopes)
        .unwrap();
    assert_eq!(proof.end_tick(), ServerTick(tick));
    assert_eq!(proof.publication().revision, GroupRevision(2));
    assert_eq!(identity.binding, session.model.binding());
    for entity in [
        session.model.welcome.global_entity,
        session.model.welcome.owner_entity,
        session.model.welcome.collision_entity,
    ] {
        assert_eq!(
            identity.scopes.iter().find(|scope| scope.entity == entity),
            proof.manifest().iter().find(|scope| scope.entity == entity)
        );
    }
    let decoded = model::validate_owner_group(
        &session.model,
        OWNER_GROUP,
        proof.end_tick(),
        proof.states(),
    )
    .unwrap();
    assert_eq!(decoded.state.0.combat_generation(), generation);
    assert_eq!(session.predictor.work().predicted_ticks, 0);
    assert_eq!(session.predictor.work().replay_ticks, 0);
}

#[test]
fn active_unstarted_owner_adopts_farther_platform_topology_without_consuming_intent() {
    let (mut sim, mut session) = active_checkpoint();
    let identity = session.predictor.identity(OWNER_GROUP).unwrap().clone();
    let generation = session
        .predictor
        .state(OWNER_GROUP)
        .unwrap()
        .0
        .combat_generation();
    let held: TickInput = DreamInput {
        movement: [1.0, 0.0],
        ..Default::default()
    }
    .into();
    let action = DreamAction::Dash {
        direction: [1.0, 0.0],
    };
    session
        .journal
        .capture(HardwareSample {
            at: Duration::from_millis(500),
            held: Some(held),
            edges: BoundedVec::new(vec![action.clone()]).unwrap(),
            mouse_delta: [0.0; 2],
        })
        .unwrap();
    move_authority(&mut sim, [1.0, 0.25, -10.0]);
    receive_topology(&sim, &mut session, 480);
    assert_eq!(
        session
            .groups
            .published_group(OWNER_GROUP, &session.scopes)
            .unwrap()
            .manifest()
            .len(),
        4
    );

    session.reconcile().unwrap();

    let current = session.predictor.identity(OWNER_GROUP).unwrap();
    assert_eq!(current.group_revision, GroupRevision(2));
    assert_eq!(current.binding, identity.binding);
    assert_eq!(current.scopes.len(), 4);
    assert!(identity.scopes.is_subset(&current.scopes));
    assert_eq!(
        session
            .predictor
            .state(OWNER_GROUP)
            .unwrap()
            .0
            .combat_generation(),
        generation
    );
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(480))
    );
    assert!(
        session
            .predictor
            .replay_input_history(OWNER_GROUP, ServerTick(480))
            .is_none()
    );
    assert_eq!(session.predictor.work().predicted_ticks, 0);
    assert_eq!(session.predictor.work().replay_ticks, 0);
    assert_eq!(session.sequence, CommandSeq(0));
    assert!(session.active && !session.recovering);
    assert_eq!(session.journal.pending_samples(), 1);
    let sample = session
        .command_sample(TargetTick(481), TargetTick(481), Duration::from_secs(1))
        .unwrap();
    assert_eq!(sample.held, held);
    assert_eq!(sample.edges.as_slice(), &[action]);
    assert!(
        session
            .command_sample(TargetTick(482), TargetTick(482), Duration::from_secs(1))
            .unwrap()
            .edges
            .is_empty()
    );
}

#[test]
fn active_unstarted_owner_adopts_farther_revision_with_unchanged_closure() {
    let (sim, mut session) = active_checkpoint();
    let identity = session.predictor.identity(OWNER_GROUP).unwrap().clone();
    receive_topology(&sim, &mut session, 480);
    session.reconcile().unwrap();
    let current = session.predictor.identity(OWNER_GROUP).unwrap();
    assert_eq!(current.group_revision, GroupRevision(2));
    assert_eq!(current.binding, identity.binding);
    assert_eq!(current.scopes, identity.scopes);
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(480))
    );
    assert!(
        session
            .predictor
            .replay_input_history(OWNER_GROUP, ServerTick(480))
            .is_none()
    );
    assert_eq!(session.predictor.work().predicted_ticks, 0);
    assert_eq!(session.predictor.work().replay_ticks, 0);
    assert_eq!(session.sequence, CommandSeq(0));
}

#[derive(Clone, Copy)]
enum StoredInput {
    Command,
    Substitute,
}

fn started_prediction(input: StoredInput) -> (DreamSimulation, Session) {
    let (sim, mut session) = active_checkpoint();
    let dependencies = session
        .predictor
        .checkpoint_dependencies(OWNER_GROUP)
        .unwrap()
        .clone();
    match input {
        StoredInput::Command => {
            session
                .predictor
                .predict(
                    OWNER_GROUP,
                    InputCommand(Command {
                        owner: session.model.welcome.stream,
                        sequence: CommandSeq(1),
                        target: TargetTick(442),
                        input: DreamInput::default().into(),
                        actions: BoundedVec::default(),
                    }),
                    dependencies,
                    &session.model,
                )
                .unwrap();
            session.sequence = CommandSeq(1);
        }
        StoredInput::Substitute => {
            session
                .predictor
                .predict_substitute(OWNER_GROUP, ServerTick(442), dependencies, &session.model)
                .unwrap();
            assert_eq!(session.sequence, CommandSeq(0));
        }
    }
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(442))
    );
    assert_stored_input(&session, input);
    (sim, session)
}

fn assert_stored_input(session: &Session, input: StoredInput) {
    let stored = session
        .predictor
        .replay_input_history(OWNER_GROUP, ServerTick(442));
    match input {
        StoredInput::Command => assert!(
            matches!(stored, Some(ReplayInput::Command(command)) if command.0.sequence == CommandSeq(1) && command.0.target == TargetTick(442))
        ),
        StoredInput::Substitute => assert!(matches!(stored, Some(ReplayInput::Substitute))),
    }
}

fn rejects_thirteen_tick_advance(input: StoredInput) {
    let (mut sim, mut session) = started_prediction(input);
    let identity = session.predictor.identity(OWNER_GROUP).unwrap().clone();
    let state = session.predictor.state(OWNER_GROUP).unwrap().clone();
    let bytes = session.predictor.retained_bytes();
    move_authority(&mut sim, [1.0, 0.25, -10.0]);
    receive_topology(&sim, &mut session, 455);
    assert_eq!(
        session.reconcile().unwrap_err(),
        format!(
            "topology replay exceeds frame work budget: predicted=442 incoming=455 revision=2 replay=0 work_replay=0 work_predicted=0 unstarted=false; active=true sequence={}",
            session.sequence.0,
        )
    );
    assert_eq!(session.predictor.identity(OWNER_GROUP), Some(&identity));
    assert_eq!(session.predictor.state(OWNER_GROUP), Some(&state));
    assert_eq!(session.predictor.retained_bytes(), bytes);
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(442))
    );
    assert_eq!(session.predictor.work().predicted_ticks, 0);
    assert_eq!(session.predictor.work().replay_ticks, 0);
    assert_stored_input(&session, input);
}

fn allows_twelve_tick_advance(input: StoredInput) {
    let (mut sim, mut session) = started_prediction(input);
    let binding = session.model.binding();
    let sequence = session.sequence;
    move_authority(&mut sim, [1.0, 0.25, -10.0]);
    receive_topology(&sim, &mut session, 454);
    session.reconcile().unwrap();
    let identity = session.predictor.identity(OWNER_GROUP).unwrap();
    assert_eq!(identity.binding, binding);
    assert_eq!(identity.group_revision, GroupRevision(2));
    assert_eq!(identity.scopes.len(), 4);
    assert_eq!(
        session.predictor.predicted_tick(OWNER_GROUP),
        Some(ServerTick(454))
    );
    assert_eq!(session.sequence, sequence);
    assert_eq!(session.predictor.work().predicted_ticks, 0);
    assert_eq!(session.predictor.work().replay_ticks, 0);
}

#[test]
fn authored_history_rejects_topology_more_than_twelve_ticks_ahead() {
    rejects_thirteen_tick_advance(StoredInput::Command);
}

#[test]
fn substitute_history_with_zero_sequence_rejects_topology_more_than_twelve_ticks_ahead() {
    rejects_thirteen_tick_advance(StoredInput::Substitute);
}

#[test]
fn authored_history_allows_topology_exactly_twelve_ticks_ahead() {
    allows_twelve_tick_advance(StoredInput::Command);
}

#[test]
fn substitute_history_allows_topology_exactly_twelve_ticks_ahead() {
    allows_twelve_tick_advance(StoredInput::Substitute);
}

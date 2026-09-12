use super::*;

fn substitute(manager: &mut PredictionManager<Model>, tick: u64, model: &Model) -> EventChanges {
    manager
        .predict_substitute(
            GroupId(1),
            ServerTick(tick),
            DependencyFrame::default(),
            model,
        )
        .unwrap()
}

#[test]
fn recorded_substitutes_replay_restored_motion_without_new_command_or_action_identity() {
    let (mut manager, mut replicas, model) = initialized(
        PredictionLimits::default(),
        State::default(),
        DependencyFrame::default(),
    );
    let first = input(1, 1);
    let first_events = manager
        .predict(
            GroupId(1),
            first.clone(),
            DependencyFrame::default(),
            &model,
        )
        .unwrap();
    assert_eq!(first_events.started.len(), 1);
    assert_eq!(substitute(&mut manager, 2, &model), EventChanges::default());
    assert_eq!(substitute(&mut manager, 3, &model), EventChanges::default());
    let mut second = input(1, 4);
    second.sequence = CommandSeq(2);
    second.fire = true;
    let second_events = manager
        .predict(
            GroupId(1),
            second.clone(),
            DependencyFrame::default(),
            &model,
        )
        .unwrap();
    assert_eq!(second_events.started.len(), 1);
    assert_eq!(
        second_events.started.first().unwrap().command,
        CommandSeq(2)
    );
    assert_eq!(manager.work().predicted_ticks, 4);
    assert_eq!(
        manager.command_history(GroupId(1), ServerTick(1)),
        Some(&first)
    );
    assert!(manager.command_history(GroupId(1), ServerTick(2)).is_none());
    assert!(
        manager
            .replay_input_history(GroupId(1), ServerTick(0))
            .is_none()
    );
    for tick in [2, 3] {
        assert_eq!(
            manager.replay_input_history(GroupId(1), ServerTick(tick)),
            Some(&ReplayInput::Substitute)
        );
    }

    // Restoring different authoritative motion changes both subsequent held
    // transitions. Their old predicted results must not be cached as commands.
    let mut authority = manager
        .history_state(GroupId(1), ServerTick(1))
        .unwrap()
        .clone();
    authority.position = 100;
    authority.velocity = 5;
    replicas.publish(1, 1, 2, authority, DependencyFrame::default());
    manager.begin_frame(ReplicationFrame(2)).unwrap();
    model.calls.borrow_mut().clear();
    assert_eq!(
        manager.reconcile(&replicas.proof(1), binding(1), ServerTick(4), &model),
        Ok(Correction::Replayed {
            from: ServerTick(2),
            through: ServerTick(4),
            events: EventChanges::default(),
        })
    );
    assert_eq!(
        *model.calls.borrow(),
        [ServerTick(2), ServerTick(3), ServerTick(4)]
    );
    assert_eq!(manager.state(GroupId(1)).unwrap().position, 116);
    assert_eq!(manager.state(GroupId(1)).unwrap().velocity, 6);
    assert_eq!(manager.work().replay_ticks, 3);
    assert_eq!(manager.work().predicted_ticks, 0);
    assert!(
        manager
            .replay_input_history(GroupId(1), ServerTick(1))
            .is_none()
    );
    assert_eq!(
        manager.command_history(GroupId(1), ServerTick(4)),
        Some(&second)
    );
    for tick in [2, 3] {
        assert_eq!(
            manager.replay_input_history(GroupId(1), ServerTick(tick)),
            Some(&ReplayInput::Substitute)
        );
    }
}

#[test]
fn substitute_identity_is_immutable_idempotent_and_does_not_consume_sequence() {
    let (mut manager, _, model) = initialized(
        PredictionLimits::default(),
        State::default(),
        DependencyFrame::default(),
    );
    let mut first = input(1, 1);
    first.sequence = CommandSeq(9);
    manager
        .predict(
            GroupId(1),
            first.clone(),
            DependencyFrame::default(),
            &model,
        )
        .unwrap();
    substitute(&mut manager, 2, &model);
    let bytes = manager.retained_bytes();
    let work = manager.work();
    let calls = model.calls.borrow().clone();
    assert_eq!(substitute(&mut manager, 2, &model), EventChanges::default());
    assert_eq!(manager.retained_bytes(), bytes);
    assert_eq!(manager.work(), work);
    assert_eq!(*model.calls.borrow(), calls);
    assert_eq!(
        manager.predict_substitute(
            GroupId(1),
            ServerTick(1),
            DependencyFrame::default(),
            &model
        ),
        Err(PredictionError::Equivocation)
    );
    assert_eq!(
        manager.predict(GroupId(1), input(1, 2), DependencyFrame::default(), &model),
        Err(PredictionError::Equivocation)
    );
    assert_eq!(
        manager.predict_substitute(
            GroupId(1),
            ServerTick(0),
            DependencyFrame::default(),
            &model
        ),
        Err(PredictionError::WrongTick)
    );
    assert_eq!(
        manager.predict_substitute(
            GroupId(1),
            ServerTick(4),
            DependencyFrame::default(),
            &model
        ),
        Err(PredictionError::WrongTick)
    );
    let mut next = input(1, 3);
    next.sequence = CommandSeq(9);
    assert_eq!(
        manager.predict(GroupId(1), next.clone(), DependencyFrame::default(), &model),
        Err(PredictionError::WrongTick)
    );
    next.sequence = CommandSeq(10);
    manager
        .predict(GroupId(1), next.clone(), DependencyFrame::default(), &model)
        .unwrap();
    assert_eq!(
        manager.command_history(GroupId(1), ServerTick(3)),
        Some(&next)
    );
    assert_eq!(manager.work().predicted_ticks, 3);
}

#[test]
fn equal_checkpoint_clears_only_its_transition_and_later_substitutes_still_replay() {
    let (mut manager, mut replicas, model) = initialized(
        PredictionLimits::default(),
        State {
            velocity: 1,
            ..Default::default()
        },
        DependencyFrame::default(),
    );
    substitute(&mut manager, 1, &model);
    substitute(&mut manager, 2, &model);
    let mut command = input(1, 3);
    command.sequence = CommandSeq(1);
    manager
        .predict(
            GroupId(1),
            command.clone(),
            DependencyFrame::default(),
            &model,
        )
        .unwrap();
    let checkpoint = manager
        .history_state(GroupId(1), ServerTick(1))
        .unwrap()
        .clone();
    replicas.publish(1, 1, 2, checkpoint.clone(), DependencyFrame::default());
    assert_eq!(
        manager.reconcile(&replicas.proof(1), binding(1), ServerTick(3), &model),
        Ok(Correction::Equal)
    );
    assert!(
        manager
            .replay_input_history(GroupId(1), ServerTick(1))
            .is_none()
    );
    assert_eq!(
        manager.replay_input_history(GroupId(1), ServerTick(2)),
        Some(&ReplayInput::Substitute)
    );
    let mut correction = checkpoint;
    correction.velocity = 9;
    replicas.publish(1, 1, 3, correction, DependencyFrame::default());
    manager.begin_frame(ReplicationFrame(2)).unwrap();
    assert!(matches!(
        manager.reconcile(&replicas.proof(1), binding(1), ServerTick(3), &model),
        Ok(Correction::Replayed {
            from: ServerTick(2),
            through: ServerTick(3),
            ..
        })
    ));
    assert_eq!(manager.state(GroupId(1)).unwrap().position, 20);
    assert_eq!(
        manager.command_history(GroupId(1), ServerTick(3)),
        Some(&command)
    );
}

#[test]
fn substitute_replay_uses_pinned_dependency_updates_and_rejects_missing_closure() {
    let state = State {
        velocity: 1,
        dependency: Some(entity(99)),
        ..Default::default()
    };
    let deps = dependency(1, DependencyValue::Known(Dependency(1)));
    let (mut manager, mut replicas, model) =
        initialized(PredictionLimits::default(), state.clone(), deps.clone());
    manager
        .predict_substitute(GroupId(1), ServerTick(1), deps.clone(), &model)
        .unwrap();
    assert_eq!(manager.state(GroupId(1)).unwrap().position, 2);
    let replacement = dependency(2, DependencyValue::Known(Dependency(4)));
    manager
        .update_dependencies(GroupId(1), ServerTick(1), replacement.clone())
        .unwrap();
    replicas.publish(1, 1, 2, state, deps);
    manager.begin_frame(ReplicationFrame(2)).unwrap();
    assert!(matches!(
        manager.reconcile(&replicas.proof(1), binding(1), ServerTick(0), &model),
        Ok(Correction::Replayed {
            from: ServerTick(1),
            through: ServerTick(1),
            ..
        })
    ));
    assert_eq!(manager.state(GroupId(1)).unwrap().position, 5);
    assert_eq!(
        manager.dependency_history(GroupId(1), ServerTick(1)),
        Some(&replacement)
    );
    assert_eq!(
        manager.replay_input_history(GroupId(1), ServerTick(1)),
        Some(&ReplayInput::Substitute)
    );
    let before = manager.state(GroupId(1)).unwrap().clone();
    let work = manager.work();
    assert_eq!(
        manager.predict_substitute(
            GroupId(1),
            ServerTick(2),
            DependencyFrame::default(),
            &model
        ),
        Err(PredictionError::ResyncRequired(
            RecoveryReason::MissingDependency
        ))
    );
    assert_eq!(manager.state(GroupId(1)), Some(&before));
    assert_eq!(manager.work(), work);
    assert!(
        manager
            .replay_input_history(GroupId(1), ServerTick(2))
            .is_none()
    );
}

#[test]
fn substitutes_obey_prediction_and_replay_budgets_without_partial_commit() {
    let limits = PredictionLimits {
        prediction_ticks_per_frame: 2,
        ..Default::default()
    };
    let (mut manager, _, model) = initialized(limits, State::default(), DependencyFrame::default());
    substitute(&mut manager, 1, &model);
    substitute(&mut manager, 2, &model);
    let before = manager.state(GroupId(1)).unwrap().clone();
    assert_eq!(
        manager.predict_substitute(
            GroupId(1),
            ServerTick(3),
            DependencyFrame::default(),
            &model
        ),
        Err(PredictionError::ResyncRequired(
            RecoveryReason::ReplayBudget
        ))
    );
    assert_eq!(manager.state(GroupId(1)), Some(&before));
    assert_eq!(manager.work().predicted_ticks, 2);

    let limits = PredictionLimits {
        replay_ticks_per_frame: 1,
        ..Default::default()
    };
    let (mut manager, mut replicas, model) =
        initialized(limits, State::default(), DependencyFrame::default());
    substitute(&mut manager, 1, &model);
    substitute(&mut manager, 2, &model);
    let before = manager.state(GroupId(1)).unwrap().clone();
    replicas.publish(
        1,
        1,
        2,
        State {
            velocity: 4,
            ..Default::default()
        },
        DependencyFrame::default(),
    );
    manager.begin_frame(ReplicationFrame(2)).unwrap();
    assert_eq!(
        manager.reconcile(&replicas.proof(1), binding(1), ServerTick(0), &model),
        Err(PredictionError::ResyncRequired(
            RecoveryReason::ReplayBudget
        ))
    );
    assert_eq!(manager.state(GroupId(1)), Some(&before));
    assert_eq!(manager.work().replay_ticks, 0);
}

#[test]
fn substitutes_remain_memory_bounded_and_keep_failed_frame_unpublished() {
    let limits = PredictionLimits {
        history_bytes: 4096,
        checkpoint_bytes: 128,
        dependency_bytes: 128,
        command_bytes: 128,
        events_per_tick: 1,
        ..Default::default()
    };
    let (mut manager, _, model) = initialized(limits, State::default(), DependencyFrame::default());
    let mut failed = false;
    for tick in 1..=20 {
        let before = manager.state(GroupId(1)).unwrap().clone();
        if let Err(error) = manager.predict_substitute(
            GroupId(1),
            ServerTick(tick),
            DependencyFrame::default(),
            &model,
        ) {
            assert_eq!(
                error,
                PredictionError::ResyncRequired(RecoveryReason::MemoryBudget)
            );
            assert_eq!(manager.state(GroupId(1)), Some(&before));
            assert!(
                manager
                    .replay_input_history(GroupId(1), ServerTick(tick))
                    .is_none()
            );
            failed = true;
            break;
        }
        assert!(manager.retained_bytes() <= limits.history_bytes);
        assert!(
            manager
                .command_history(GroupId(1), ServerTick(tick))
                .is_none()
        );
    }
    assert!(failed);
}

#[test]
fn failed_substitute_replay_keeps_state_and_history_but_charges_planned_work() {
    let (mut manager, mut replicas, mut model) = initialized(
        PredictionLimits::default(),
        State::default(),
        DependencyFrame::default(),
    );
    substitute(&mut manager, 1, &model);
    substitute(&mut manager, 2, &model);
    let before = manager.state(GroupId(1)).unwrap().clone();
    replicas.publish(
        1,
        1,
        2,
        State {
            velocity: 4,
            ..Default::default()
        },
        DependencyFrame::default(),
    );
    manager.begin_frame(ReplicationFrame(2)).unwrap();
    model.fail_at = Some(ServerTick(2));
    assert_eq!(
        manager.reconcile(&replicas.proof(1), binding(1), ServerTick(0), &model),
        Err(PredictionError::ResyncRequired(
            RecoveryReason::InvalidState
        ))
    );
    assert_eq!(manager.state(GroupId(1)), Some(&before));
    assert_eq!(manager.work().replay_ticks, 2);
    for tick in [1, 2] {
        assert_eq!(
            manager.replay_input_history(GroupId(1), ServerTick(tick)),
            Some(&ReplayInput::Substitute)
        );
    }
}

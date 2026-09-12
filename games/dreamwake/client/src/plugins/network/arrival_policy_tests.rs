use super::*;
use engine_net::synchronization::LeadController;

fn issue(session: &mut Session, sequence: u64, target: u64, newest: u64) {
    session.sequence = CommandSeq(sequence);
    session.record_command_issuance(CommandSeq(sequence), TargetTick(target), TargetTick(newest));
}

fn late(sequence: u64, target: u64, count: u64) -> ArrivalFeedback {
    ArrivalFeedback {
        sample: Some(ArrivalSample {
            sequence: CommandSeq(sequence),
            target: TargetTick(target),
            arrival_tick: ServerTick(target),
            slack: 0,
        }),
        observed_commands: count,
        late_commands: count,
    }
}

#[test]
fn feedback_waits_for_commands_that_benefited_from_the_current_policy() {
    let (mut session, _) = tests::fixture();
    session.lead = LeadController::new(3).unwrap();
    for sequence in 1..=4 {
        issue(&mut session, sequence, sequence + 10, 11);
    }
    session.observe_arrival_feedback(late(1, 11, 1)).unwrap();
    assert_eq!(session.lead.lead(), 4);
    session.observe_arrival_feedback(late(2, 12, 2)).unwrap();
    assert_eq!(
        session.lead.lead(),
        4,
        "old in-flight input cannot retune the new policy"
    );

    // A lead increase fills targets below its first new head. They remain valid
    // commands and their missed deadlines are counted, but are not new-policy proof.
    for (sequence, target) in [(5, 15), (6, 16), (7, 17)] {
        issue(&mut session, sequence, target, 17);
    }
    session.observe_arrival_feedback(late(6, 16, 3)).unwrap();
    assert_eq!(session.lead.lead(), 4);
    session.observe_arrival_feedback(late(7, 17, 4)).unwrap();
    assert_eq!(session.lead.lead(), 5);
    let metrics = session.metrics();
    assert_eq!(metrics.arrival_feedback.late_commands, 4);
    assert_eq!(metrics.arrival_feedback.observed_commands, 4);
    assert_eq!(metrics.arrival_feedback_applied, 2);
    assert_eq!(metrics.arrival_feedback_ignored, 2);
    assert_eq!(metrics.lead_policy, 2);
}

#[test]
fn impossible_arrival_identity_and_counters_are_rejected_without_tuning() {
    let (mut session, _) = tests::fixture();
    session.lead = LeadController::new(3).unwrap();
    issue(&mut session, 1, 11, 11);
    // Future sequence, a different immutable target, and a missing unretired
    // sequence are errors rather than vaguely classified old feedback.
    for invalid in [late(2, 12, 1), late(1, 12, 1)] {
        assert_eq!(
            session.observe_arrival_feedback(invalid),
            Err(SyncError::InvalidSample)
        );
        assert_eq!(
            session.metrics().arrival_feedback,
            ArrivalFeedback::default()
        );
    }
    session.sequence = CommandSeq(3);
    assert_eq!(
        session.observe_arrival_feedback(late(2, 12, 1)),
        Err(SyncError::InvalidSample)
    );
    session.observe_arrival_feedback(late(1, 11, 1)).unwrap();
    let committed_feedback = session.metrics().arrival_feedback;
    assert_eq!(
        session.observe_arrival_feedback(late(1, 11, 2)),
        Err(SyncError::InvalidSample)
    );
    assert_eq!(
        session.observe_arrival_feedback(ArrivalFeedback::default()),
        Err(SyncError::InvalidSample)
    );
    assert_eq!(session.metrics().arrival_feedback, committed_feedback);
    assert_eq!(session.lead.lead(), 4);
}

#[test]
fn bounded_retired_history_and_recovery_ignore_only_proven_old_issuance() {
    let (mut session, _) = tests::fixture();
    session.lead = LeadController::new(3).unwrap();
    for sequence in 1..=257 {
        issue(&mut session, sequence, sequence + 10, 11);
    }
    session.observe_arrival_feedback(late(1, 11, 1)).unwrap();
    assert_eq!(session.lead.lead(), 3);
    assert_eq!(session.metrics().arrival_feedback_ignored, 1);
    session.recover();
    session.observe_arrival_feedback(late(257, 267, 2)).unwrap();
    assert_eq!(session.metrics().arrival_feedback_ignored, 2);
    assert_eq!(
        session.observe_arrival_feedback(late(258, 268, 3)),
        Err(SyncError::InvalidSample)
    );
    assert_eq!(session.metrics().arrival_feedback.late_commands, 2);
}

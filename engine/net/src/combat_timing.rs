//! Bounds untrusted historical view requests with server-owned timing evidence.
//! All ticks are on one match clock. Games choose fairness limits; this module
//! neither estimates clocks nor subtracts transport RTT from a requested view.
use crate::types::*;

#[derive(Clone, Copy, Debug)]
pub struct TimingPolicy {
    pub maximum_sample_age: u64,
    pub maximum_future_sample: u64,
    pub minimum_execution_lead: u64,
    pub maximum_execution_lead: u64,
    pub minimum_presentation_age: u64,
    pub maximum_presentation_age: u64,
    pub maximum_rewind: u64,
    pub maximum_reference_advance: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct ViewRequest {
    pub sampled_at: ServerTick,
    pub sampled_fraction: u16,
    pub viewed_at: ServerTick,
    pub viewed_fraction: u16,
    pub reference: SnapshotId,
}
#[derive(Clone, Copy, Debug)]
pub struct TimingContext {
    pub connection: ConnectionEpoch,
    pub scene: SceneRevision,
    pub arrival: ServerTick,
    pub execution: ServerTick,
    pub oldest_history: ServerTick,
    pub newest_history: ServerTick,
}
/// Returned only by the authority's record of actually sent snapshots, never
/// constructed from fields asserted by the requesting client.
#[derive(Clone, Copy, Debug)]
pub struct SentReference {
    pub connection: ConnectionEpoch,
    pub scene: SceneRevision,
    pub state_at: ServerTick,
    pub sent_at: ServerTick,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidatedView {
    pub execution: ServerTick,
    pub query: ServerTick,
    pub fraction: u16,
    pub clamped: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingError {
    InvalidPolicy,
    InvalidContext,
    InvalidSample,
    InvalidExecutionLead,
    UnknownReference,
    WrongReference,
    OutsidePolicy,
    MissingHistory,
    Overflow,
}
/// The callback must look up the exact reference in this connection's bounded
/// sent-state ledger. A reference constrains knowledge, not what was displayed.
pub fn validate_view(
    policy: TimingPolicy,
    context: TimingContext,
    request: ViewRequest,
    sent_reference: impl FnOnce(SnapshotId) -> Option<SentReference>,
) -> Result<ValidatedView, TimingError> {
    if policy.minimum_execution_lead > policy.maximum_execution_lead
        || policy.minimum_presentation_age > policy.maximum_presentation_age
    {
        return Err(TimingError::InvalidPolicy);
    }
    if context.connection.0 == 0
        || context.scene.0 == 0
        || context.arrival > context.execution
        || context.oldest_history > context.newest_history
        || context.newest_history > context.execution
    {
        return Err(TimingError::InvalidContext);
    }
    let units =
        |tick: ServerTick, fraction: u16| u128::from(tick.0) * 65_536 + u128::from(fraction);
    let duration = |ticks: u64| u128::from(ticks) * 65_536;
    let sample = units(request.sampled_at, request.sampled_fraction);
    let arrival = units(context.arrival, 0);
    let execution = units(context.execution, 0);
    let latest_sample = arrival + duration(policy.maximum_future_sample);
    if sample < arrival.saturating_sub(duration(policy.maximum_sample_age))
        || sample > latest_sample
    {
        return Err(TimingError::InvalidSample);
    }
    let lead = execution
        .checked_sub(sample)
        .ok_or(TimingError::InvalidExecutionLead)?;
    if !(duration(policy.minimum_execution_lead)..=duration(policy.maximum_execution_lead))
        .contains(&lead)
    {
        return Err(TimingError::InvalidExecutionLead);
    }
    if request.reference.0 == 0 {
        return Err(TimingError::UnknownReference);
    }
    let reference = sent_reference(request.reference).ok_or(TimingError::UnknownReference)?;
    if reference.connection != context.connection
        || reference.scene != context.scene
        || reference.state_at > reference.sent_at
        || reference.sent_at > context.arrival
    {
        return Err(TimingError::WrongReference);
    }
    let lower = sample
        .saturating_sub(duration(policy.maximum_presentation_age))
        .max(execution.saturating_sub(duration(policy.maximum_rewind)))
        .max(units(reference.state_at, 0));
    let upper = sample
        .checked_sub(duration(policy.minimum_presentation_age))
        .ok_or(TimingError::OutsidePolicy)?
        .min(execution)
        .min(units(reference.state_at, 0) + duration(policy.maximum_reference_advance));
    if lower > upper {
        return Err(TimingError::OutsidePolicy);
    }
    let lower = lower.max(units(context.oldest_history, 0));
    let upper = upper.min(units(context.newest_history, 0));
    if lower > upper {
        return Err(TimingError::MissingHistory);
    }
    let requested = units(request.viewed_at, request.viewed_fraction);
    let query = requested.clamp(lower, upper);
    Ok(ValidatedView {
        execution: context.execution,
        query: ServerTick(u64::try_from(query / 65_536).map_err(|_| TimingError::Overflow)?),
        fraction: (query % 65_536) as u16,
        clamped: query != requested,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (TimingPolicy, TimingContext, ViewRequest, SentReference) {
        // 1000 Hz makes normative millisecond values exact integer ticks.
        (
            TimingPolicy {
                maximum_sample_age: 100,
                maximum_future_sample: 2,
                minimum_execution_lead: 0,
                maximum_execution_lead: 100,
                minimum_presentation_age: 50,
                maximum_presentation_age: 100,
                maximum_rewind: 150,
                maximum_reference_advance: 50,
            },
            TimingContext {
                connection: ConnectionEpoch(1),
                scene: SceneRevision(1),
                arrival: ServerTick(1025),
                execution: ServerTick(1050),
                oldest_history: ServerTick(800),
                newest_history: ServerTick(1050),
            },
            ViewRequest {
                sampled_at: ServerTick(1000),
                sampled_fraction: 0,
                viewed_at: ServerTick(925),
                viewed_fraction: 0,
                reference: SnapshotId(7),
            },
            SentReference {
                connection: ConnectionEpoch(1),
                scene: SceneRevision(1),
                state_at: ServerTick(900),
                sent_at: ServerTick(910),
            },
        )
    }
    #[test]
    fn normative_mixed_time_query_does_not_subtract_rtt_again() {
        let (p, c, r, s) = setup();
        assert_eq!(
            validate_view(p, c, r, |id| {
                assert_eq!(id, SnapshotId(7));
                Some(s)
            }),
            Ok(ValidatedView {
                execution: ServerTick(1050),
                query: ServerTick(925),
                fraction: 0,
                clamped: false
            })
        );
    }
    #[test]
    fn cap_delay_and_retained_history_intersect() {
        let (mut p, mut c, mut r, s) = setup();
        r.viewed_at = ServerTick(1);
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)).unwrap().query,
            ServerTick(900)
        );
        p.maximum_rewind = 125;
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)).unwrap().query,
            ServerTick(925)
        );
        c.oldest_history = ServerTick(960);
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)),
            Err(TimingError::MissingHistory)
        );
    }
    #[test]
    fn future_view_is_clamped_but_unsent_and_wrong_scene_references_are_rejected() {
        let (p, c, mut r, mut s) = setup();
        r.viewed_at = ServerTick(9999);
        let view = validate_view(p, c, r, |_| Some(s)).unwrap();
        assert_eq!(view.query, ServerTick(950));
        assert!(view.clamped);
        s.sent_at = ServerTick(1026);
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)),
            Err(TimingError::WrongReference)
        );
        s.sent_at = ServerTick(910);
        s.scene = SceneRevision(2);
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)),
            Err(TimingError::WrongReference)
        );
    }
    #[test]
    fn forged_reference_sample_and_context_fail_closed() {
        let (p, mut c, mut r, mut s) = setup();
        assert_eq!(
            validate_view(p, c, r, |_| None),
            Err(TimingError::UnknownReference)
        );
        s.connection = ConnectionEpoch(2);
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)),
            Err(TimingError::WrongReference)
        );
        r.sampled_at = ServerTick(2000);
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)),
            Err(TimingError::InvalidSample)
        );
        c.arrival = ServerTick(1100);
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)),
            Err(TimingError::InvalidContext)
        );
    }
    #[test]
    fn fractional_view_preserves_phase_and_exact_policy_boundaries() {
        let (p, c, mut r, s) = setup();
        r.sampled_fraction = 32768;
        r.viewed_fraction = 32768;
        let view = validate_view(p, c, r, |_| Some(s)).unwrap();
        assert_eq!(
            (view.query, view.fraction, view.clamped),
            (ServerTick(925), 32768, false)
        );
        r.viewed_at = ServerTick(950);
        r.viewed_fraction = 1;
        let limited = validate_view(p, c, r, |_| Some(s)).unwrap();
        assert_eq!(
            (limited.query, limited.fraction, limited.clamped),
            (ServerTick(950), 0, true)
        );
        r.sampled_at = ServerTick(c.arrival.0 + p.maximum_future_sample);
        r.sampled_fraction = 1;
        assert_eq!(
            validate_view(p, c, r, |_| Some(s)),
            Err(TimingError::InvalidSample)
        );
    }
}

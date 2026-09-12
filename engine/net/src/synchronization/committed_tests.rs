use super::*;
use crate::{
    codec::BoundedVec,
    commands::{
        Admission, Command, CommandInbox, CommandLimits, CommandRules, FinalizedReceipt,
        FinalizedStatus, OwnerStream, pack_redundant,
    },
    prediction::PredictionLimits,
    types::{CommandSeq, CommandStream, ConnectionEpoch, ConnectionId, EntityId, OwnershipEpoch},
};
use std::collections::{BTreeMap, BTreeSet};

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn replay_budget() -> u32 {
    u32::try_from(PredictionLimits::default().replay_ticks_per_frame).unwrap()
}

#[test]
fn delayed_commits_prove_a_floor_without_changing_the_timestamp_anchor() {
    let rate = TickRate::new(60).unwrap();
    let mut clock = CommittedClock::new(rate, ServerTick(100), ms(1000), 32).unwrap();
    clock.commit(ServerTick(108));
    clock.commit(ServerTick(103));
    assert_eq!(clock.committed(), ServerTick(108));
    assert_eq!(clock.anchor(), (ServerTick(100), ms(1000)));
    assert_eq!(clock.estimate(ms(999), 4), Ok(ServerTick(108)));
    assert_eq!(clock.estimate(ms(1100), 4), Ok(ServerTick(108)));
    assert_eq!(clock.estimate(ms(1200), 4), Ok(ServerTick(112)));

    // A clock reply can arrive after a newer owner snapshot or finalized receipt.
    // Its timestamp still gives useful phase, while the publication remains a floor.
    clock.observe(ServerTick(104), ms(1100)).unwrap();
    assert_eq!(clock.committed(), ServerTick(108));
    assert_eq!(clock.estimate(ms(1100), 4), Ok(ServerTick(108)));
    assert_eq!(clock.estimate(ms(1300), 4), Ok(ServerTick(116)));
    clock.observe(ServerTick(120), ms(1400)).unwrap();
    assert_eq!(clock.committed(), ServerTick(120));
    assert_eq!(clock.estimate(ms(1400), 4), Ok(ServerTick(120)));
}

#[test]
fn rejected_observations_preserve_the_last_valid_clock_and_commit_floor() {
    let rate = TickRate::new(60).unwrap();
    let mut clock = CommittedClock::new(rate, ServerTick(100), ms(1000), 32).unwrap();
    clock.commit(ServerTick(105));
    for (tick, at, error) in [
        (ServerTick(101), ms(999), SyncError::NonMonotonicTime),
        (ServerTick(99), ms(1001), SyncError::NonMonotonicTime),
    ] {
        assert_eq!(clock.observe(tick, at), Err(error));
        assert_eq!(clock.anchor(), (ServerTick(100), ms(1000)));
        assert_eq!(clock.committed(), ServerTick(105));
        assert_eq!(clock.estimate(ms(1100), 4), Ok(ServerTick(106)));
    }
    clock.observe(ServerTick(100), ms(1000)).unwrap();
    clock.observe(ServerTick(100), ms(1100)).unwrap();
    assert_eq!(clock.anchor(), (ServerTick(100), ms(1100)));
    assert_eq!(clock.committed(), ServerTick(105));
    assert_eq!(clock.estimate(ms(1100), 4), Ok(ServerTick(105)));
}

#[test]
fn bounded_catchup_polls_can_commit_more_ticks_at_one_server_timestamp() {
    let rate = TickRate::new(60).unwrap();
    let mut clock = CommittedClock::new(rate, ServerTick(100), ms(1000), 32).unwrap();
    clock.commit(ServerTick(105));
    clock.observe(ServerTick(101), ms(1000)).unwrap();
    assert_eq!(clock.anchor(), (ServerTick(101), ms(1000)));
    assert_eq!(clock.committed(), ServerTick(105));
    assert_eq!(clock.estimate(ms(1000), 4), Ok(ServerTick(105)));

    clock.observe(ServerTick(108), ms(1000)).unwrap();
    assert_eq!(clock.anchor(), (ServerTick(108), ms(1000)));
    assert_eq!(clock.committed(), ServerTick(108));
    assert_eq!(clock.estimate(ms(1000), 4), Ok(ServerTick(108)));
    assert_eq!(clock.estimate_elapsed(ms(1008), 4), Ok(ms(1808)));
}

#[test]
fn fractional_simulation_time_preserves_phase_under_debt_and_stops_at_the_cap() {
    let rate = TickRate::new(60).unwrap();
    let mut clock = CommittedClock::new(rate, ServerTick(120), ms(10_000), 32).unwrap();
    // Authority is eight seconds behind the wall clock. Half-tick phase belongs
    // to simulation time so input timing and presentation use the same domain.
    let elapsed = clock.estimate_elapsed(ms(10_025), 4).unwrap();
    assert_eq!(elapsed, ms(2025));
    assert_eq!(
        rate.elapsed_tick_phase(elapsed),
        Some((ServerTick(121), 32768))
    );
    assert_eq!(clock.estimate(ms(10_025), 4), Ok(ServerTick(121)));

    clock.commit(ServerTick(122));
    assert_eq!(
        clock.estimate_elapsed(ms(10_025), 4),
        Ok(rate.deadline(ServerTick(122)).unwrap())
    );
    // A fresh same-tick reply records further retained debt, preserving the
    // publication floor and the fractional phase once extrapolation catches up.
    clock.observe(ServerTick(120), ms(20_000)).unwrap();
    let elapsed = clock.estimate_elapsed(ms(20_075), 4).unwrap();
    assert_eq!(elapsed, ms(2075));
    assert_eq!(
        rate.elapsed_tick_phase(elapsed),
        Some((ServerTick(124), 32768))
    );
    assert_eq!(clock.estimate_elapsed(ms(20_499), 4), Ok(ms(2499)));
    for now in [20_500, 20_501, 21_025, 100_000] {
        let elapsed = clock.estimate_elapsed(ms(now), 4).unwrap();
        assert_eq!(elapsed, ms(2500));
        assert_eq!(rate.elapsed_tick_phase(elapsed), Some((ServerTick(150), 0)));
        assert_eq!(clock.estimate(ms(now), 4), Ok(ServerTick(150)));
    }
    assert_eq!(
        clock.estimate_elapsed(ms(100_000), 32),
        Ok(rate.deadline(ServerTick(122)).unwrap())
    );
}

#[test]
fn healthy_reply_phase_preserves_fractional_presentation_rate() {
    let rate = TickRate::new(60).unwrap();
    // A steady server commits ticks 60/75 at 1000/1250 ms. The later reply
    // arrives between render frames, with either a later or earlier tick phase.
    for (first_reply, next_reply) in [(1001, 1259), (1009, 1251)] {
        let mut clock = CommittedClock::new(rate, ServerTick(60), ms(first_reply), 32).unwrap();
        let mut cursor = super::PresentationCursor::default();
        let before = cursor.advance(clock.estimate_elapsed(ms(1258), 4).unwrap(), ms(100));
        clock.observe(ServerTick(75), ms(next_reply)).unwrap();
        let after = cursor.advance(clock.estimate_elapsed(ms(1266), 4).unwrap(), ms(100));
        assert_eq!(after - before, ms(8));
    }
}

#[test]
fn an_exact_rational_tick_difference_reanchors_instead_of_preserving_phase() {
    let rate = TickRate::new(60).unwrap();
    for (reported, observed) in [
        (ServerTick(11), rate.deadline(ServerTick(12)).unwrap()),
        (ServerTick(12), rate.deadline(ServerTick(11)).unwrap()),
    ] {
        let mut clock = CommittedClock::new(
            rate,
            ServerTick(10),
            rate.deadline(ServerTick(10)).unwrap(),
            32,
        )
        .unwrap();
        clock.observe(reported, observed).unwrap();
        assert_eq!(
            clock.estimate_elapsed(observed + ms(50), 3),
            Ok(rate.deadline(reported).unwrap() + ms(50))
        );
    }
}

#[test]
fn preserved_phase_does_not_hide_same_tick_or_advancing_authority_debt() {
    let rate = TickRate::new(60).unwrap();
    let mut clock = CommittedClock::new(rate, ServerTick(60), ms(1001), 32).unwrap();
    clock.observe(ServerTick(75), ms(1259)).unwrap();
    assert_eq!(clock.estimate_elapsed(ms(1266), 4), Ok(ms(1265)));
    clock.observe(ServerTick(75), ms(1259)).unwrap();
    assert_eq!(clock.estimate_elapsed(ms(1266), 4), Ok(ms(1265)));

    clock.observe(ServerTick(75), ms(1300)).unwrap();
    assert_eq!(clock.estimate_elapsed(ms(1300), 4), Ok(ms(1250)));
    clock.observe(ServerTick(78), ms(1500)).unwrap();
    assert_eq!(clock.estimate_elapsed(ms(1500), 4), Ok(ms(1300)));
    assert_eq!(clock.estimate_elapsed(ms(1600), 4), Ok(ms(1400)));
    assert_eq!(clock.estimate(ms(10_000), 4), Ok(ServerTick(78 + 32 - 4)));
}

#[test]
fn headroom_and_tick_exhaustion_cannot_escape_the_speculation_budget() {
    let rate = TickRate::new(60).unwrap();
    assert!(matches!(
        CommittedClock::new(rate, ServerTick(100), ms(1000), 0),
        Err(SyncError::InvalidConfiguration)
    ));
    let clock = CommittedClock::new(rate, ServerTick(100), ms(1000), 32).unwrap();
    assert_eq!(clock.estimate(ms(2000), 0), Ok(ServerTick(132)));
    assert_eq!(clock.estimate(ms(2000), 12), Ok(ServerTick(120)));
    assert_eq!(clock.estimate(ms(2000), 32), Ok(ServerTick(100)));
    assert_eq!(
        clock.estimate(ms(2000), 33),
        Err(SyncError::InvalidConfiguration)
    );
    assert_eq!(clock.estimate(Duration::MAX, 4), Err(SyncError::TimeRange));
    let exhausted = CommittedClock::new(rate, ServerTick(u64::MAX - 8), ms(1000), 32).unwrap();
    assert_eq!(exhausted.estimate(ms(1000), 4), Err(SyncError::TimeRange));
}

#[test]
fn fresh_same_tick_replies_do_not_refill_a_stalled_authoritys_budget() {
    let rate = TickRate::new(60).unwrap();
    let budget = replay_budget();
    assert_eq!(budget, 32);
    let mut clock = CommittedClock::new(rate, ServerTick(100), ms(1000), budget).unwrap();
    let mut lead = LeadController::new(4).unwrap();
    let mut targets = Vec::new();
    for now in 1000..=2000 {
        let estimated = clock.estimate(ms(now), lead.lead()).unwrap();
        if let Some(target) = lead.assign_target(estimated).unwrap() {
            targets.push(target);
        }
    }
    assert_eq!(targets.last(), Some(&TargetTick(132)));
    for reply in 1..=80 {
        let sent_at = ms(2000 + reply * 250);
        clock.observe(ServerTick(100), sent_at).unwrap();
        clock.commit(ServerTick(100));
        // Exercise both arrival and a long wait after every fresh same-tick reply.
        for estimated_now in [sent_at + ms(50), sent_at + ms(10_000)] {
            let estimated = clock.estimate(estimated_now, lead.lead()).unwrap();
            assert!(estimated.0 + u64::from(lead.lead()) <= 132);
            assert_eq!(lead.assign_target(estimated).unwrap(), None);
        }
        assert_eq!(clock.committed(), ServerTick(100));
    }
    assert_eq!(lead.needs_resynchronization(), None);

    // Only actual progress grants another slot, even without a new clock reply.
    clock.commit(ServerTick(101));
    let estimated = clock.estimate(ms(32_000), lead.lead()).unwrap();
    assert_eq!(
        lead.assign_target(estimated).unwrap(),
        Some(TargetTick(133))
    );
    assert_eq!(lead.assign_target(estimated).unwrap(), None);
    clock.observe(ServerTick(130), ms(32_100)).unwrap();
    assert_eq!(
        lead.assign_target(clock.estimate(ms(32_100), lead.lead()).unwrap())
            .unwrap(),
        Some(TargetTick(134))
    );
}

struct Rules;

impl CommandRules<(), ()> for Rules {
    fn owns_at(&self, _: OwnerStream, _: TargetTick) -> bool {
        true
    }
    fn valid_input(&self, _: &()) -> bool {
        true
    }
    fn valid_action(&self, _: &()) -> bool {
        true
    }
    fn held_input(&self, _: &()) {}
    fn neutral_input(&self) {}
}

fn owner_stream() -> OwnerStream {
    OwnerStream {
        connection: ConnectionId(1),
        epoch: ConnectionEpoch(1),
        stream: CommandStream(1),
        owner: EntityId {
            index: 1,
            generation: 1,
        },
        ownership: OwnershipEpoch(1),
    }
}

#[test]
fn stalled_speculation_can_be_future_until_the_same_command_becomes_admissible() {
    let rate = TickRate::new(60).unwrap();
    let owner = owner_stream();
    let limits = CommandLimits::default();
    let mut inbox = CommandInbox::new(owner, ServerTick(100), limits).unwrap();
    let mut clock = CommittedClock::new(rate, ServerTick(100), ms(1000), replay_budget()).unwrap();
    let mut lead = LeadController::new(4).unwrap();
    let mut sequence = 0;
    let mut accepted = 0;
    let mut retry = None;

    for now in 1000..=2000 {
        let estimated = clock.estimate(ms(now), lead.lead()).unwrap();
        if let Some(target) = lead.assign_target(estimated).unwrap() {
            sequence += 1;
            let command = Command {
                owner,
                sequence: CommandSeq(sequence),
                target,
                input: (),
                actions: Default::default(),
            };
            match inbox
                .admit(owner.connection, command.clone(), &Rules)
                .unwrap()
            {
                Admission::Accepted { arrival_slack } => {
                    accepted += 1;
                    assert!(arrival_slack <= u64::from(limits.future_ticks));
                    assert!(target.0 <= 112);
                }
                Admission::Future => retry = Some(command),
                other => panic!("stalled command admission: {other:?}"),
            }
        }
    }
    assert!(accepted > 0);
    let command = retry.expect("speculation reaches beyond the admission window");
    assert_eq!(command.target, TargetTick(132));
    assert_eq!(
        inbox.admit(owner.connection, command.clone(), &Rules),
        Ok(Admission::Future)
    );
    for reply in 1..=80 {
        let sent_at = ms(2000 + reply * 250);
        clock.observe(ServerTick(100), sent_at).unwrap();
        let estimated = clock.estimate(sent_at + ms(10_000), lead.lead()).unwrap();
        assert_eq!(estimated.0 + u64::from(lead.lead()), 132);
        assert_eq!(lead.assign_target(estimated).unwrap(), None);
        assert_eq!(clock.committed(), ServerTick(100));
        assert_eq!(inbox.finalized_through(), ServerTick(100));
    }

    while inbox.finalized_through().0 < command.target.0 - u64::from(limits.future_ticks) {
        let prepared = inbox.prepare_next(&Rules).unwrap();
        let receipt = inbox.commit(prepared.tick()).unwrap();
        clock.commit(receipt.tick);
    }
    assert_eq!(clock.committed(), ServerTick(120));
    // The exact retained command is retried: its original sequence, target,
    // input, and actions remain immutable even while admission was Future.
    assert_eq!(
        inbox.admit(owner.connection, command.clone(), &Rules),
        Ok(Admission::Accepted { arrival_slack: 12 })
    );
    assert_eq!(
        inbox.admit(owner.connection, command, &Rules),
        Ok(Admission::Duplicate)
    );
    assert_eq!(lead.needs_resynchronization(), None);
}

/// Each complete block of 100 packets loses three, with independent bounded
/// one-way jitter. Reliable delivery retransmits after 100 ms and preserves order.
#[derive(Default)]
struct Link {
    packets: u64,
    lost: u64,
    reliable_through: u64,
}

impl Link {
    fn transit(&mut self, now: u64) -> (u64, bool) {
        self.packets += 1;
        let lost = matches!(self.packets % 100, 17 | 49 | 83);
        self.lost += u64::from(lost);
        (now + 35 + (self.packets * 17) % 31, lost)
    }
    fn unreliable(&mut self, now: u64) -> Option<u64> {
        let (arrival, lost) = self.transit(now);
        (!lost).then_some(arrival)
    }
    fn reliable(&mut self, now: u64) -> u64 {
        let (arrival, lost) = self.transit(now);
        let arrival = arrival + if lost { 100 } else { 0 };
        self.reliable_through = self.reliable_through.max(arrival);
        self.reliable_through
    }
}

fn schedule<T>(queue: &mut BTreeMap<u64, Vec<T>>, at: u64, event: T) {
    queue.entry(at).or_default().push(event);
}

fn take_due<T>(queue: &mut BTreeMap<u64, Vec<T>>, now: u64) -> Vec<T> {
    let mut due = Vec::new();
    while queue.first_key_value().is_some_and(|(&at, _)| at <= now) {
        due.extend(queue.pop_first().unwrap().1);
    }
    due
}

enum Uplink {
    Commands(BoundedVec<Command<(), ()>, 8>),
    Probe(Duration),
}

struct CommandTiming {
    filled_slot: bool,
    lead: u8,
    headroom: u64,
}

enum Downlink {
    Owner(ServerTick),
    Finalized(Vec<FinalizedReceipt>, Option<i32>),
    Clock(TimeExchange, ServerTick),
}

#[test]
fn delayed_owner_and_finalized_feedback_keeps_lead_stable_at_one_hundred_ms_rtt() {
    let rate = TickRate::new(60).unwrap();
    let owner = owner_stream();
    // Simulation tick zero intentionally has no relationship to server wall time.
    // The first probe travels 50 ms each way while authority advances at 60 Hz.
    let server_offset = ms(10_000);
    let base_tick = 1200;
    let mut wall = ClockEstimator::new(rate, Duration::ZERO, ClockConfig::default()).unwrap();
    let sample = wall
        .observe(TimeExchange {
            client_send: ms(0),
            server_receive: server_offset + ms(50),
            server_send: server_offset + ms(50),
            client_receive: ms(100),
        })
        .unwrap();
    let mut clock = CommittedClock::new(
        rate,
        ServerTick(base_tick + 3),
        server_offset + ms(50),
        replay_budget(),
    )
    .unwrap();
    let mut lead = LeadController::bootstrap(rate, sample.network_round_trip, ms(20)).unwrap();
    let mut inbox =
        CommandInbox::new(owner, ServerTick(base_tick + 6), CommandLimits::default()).unwrap();
    let mut uplink = Link::default();
    let mut control_uplink = Link::default();
    let mut downlink = Link::default();
    let mut reliable = Link::default();
    let mut to_server = BTreeMap::new();
    let mut to_client = BTreeMap::new();
    let mut sequence = 0;
    let mut arrival_slack: Option<i32> = None;
    let mut unpublished_receipts = Vec::new();
    let mut accepted = 0;
    let mut late = 0;
    let mut owner_received = 0;
    let mut finalized_received = 0;
    let mut finalized_envelopes = 0;
    let mut clock_received = 0;
    let mut steady_min = u8::MAX;
    let mut steady_max = 0;
    let mut last_target = TargetTick(0);
    let mut first_target = None;
    let mut delivered_targets = BTreeSet::new();
    let mut command_timing = BTreeMap::new();
    let mut pending_commands = BTreeMap::new();
    let mut admissions_by_lead = BTreeMap::<u8, [u64; 2]>::new();
    let mut admissions_by_headroom = BTreeMap::<u64, [u64; 2]>::new();
    let mut filled_admissions = [0_u64; 2];
    let mut executed = 0;
    let mut substituted = 0;
    let mut late_feedback = 0;
    let mut max_feedback_batch = 0;
    let mut duplicate = 0;
    let mut raw_late = 0;

    for now in 100..=20_000 {
        for event in take_due(&mut to_server, now) {
            match event {
                Uplink::Commands(records) => {
                    // Newest-first bundles can first deliver an older command.
                    // The retained audit distinguishes that useful observation
                    // from a repeated Accepted or Late record in later packets.
                    for command in records.as_slice() {
                        let first_delivery = delivered_targets.insert(command.target);
                        let previous_admission = inbox.retained_admission(command.sequence);
                        let admission = inbox
                            .admit(owner.connection, command.clone(), &Rules)
                            .unwrap();
                        let slack = match admission {
                            Admission::Accepted { arrival_slack } => {
                                assert!(first_delivery);
                                accepted += 1;
                                Some(arrival_slack as i32)
                            }
                            Admission::Late => {
                                raw_late += 1;
                                late += usize::from(first_delivery);
                                Some(0)
                            }
                            Admission::Duplicate => {
                                assert!(!first_delivery);
                                duplicate += 1;
                                None
                            }
                            other => panic!("healthy command admission at {now} ms: {other:?}"),
                        };
                        if first_delivery {
                            let timing: &CommandTiming = &command_timing[&command.target];
                            let missed = usize::from(slack == Some(0));
                            admissions_by_lead.entry(timing.lead).or_default()[missed] += 1;
                            admissions_by_headroom.entry(timing.headroom).or_default()[missed] += 1;
                            if timing.filled_slot {
                                filled_admissions[missed] += 1;
                            }
                        }
                        if !matches!(
                            previous_admission,
                            Some(Admission::Accepted { .. } | Admission::Late)
                        ) {
                            if let Some(slack) = slack {
                                arrival_slack =
                                    Some(arrival_slack.map_or(slack, |old| old.min(slack)));
                            }
                        }
                    }
                }
                Uplink::Probe(client_send) => {
                    // Clock replies share ordered CONTROL delivery with Finalized;
                    // a lost receipt also delays a clock reply queued behind it.
                    let at = reliable.reliable(now);
                    schedule(
                        &mut to_client,
                        at,
                        Downlink::Clock(
                            TimeExchange {
                                client_send,
                                server_receive: server_offset + ms(now),
                                server_send: server_offset + ms(now),
                                client_receive: ms(at),
                            },
                            inbox.finalized_through(),
                        ),
                    );
                }
            }
        }

        let due = ServerTick(base_tick + rate.elapsed_ticks(ms(now)).unwrap().0);
        while inbox.finalized_through() < due {
            let prepared = inbox.prepare_next(&Rules).unwrap();
            let receipt = inbox.commit(prepared.tick()).unwrap();
            if first_target.is_some_and(|first: TargetTick| receipt.tick.0 >= first.0) {
                match receipt.status {
                    FinalizedStatus::Executed => executed += 1,
                    FinalizedStatus::Substituted => substituted += 1,
                }
            }
            unpublished_receipts.push(receipt);
            if receipt.tick.0 % 3 == 0 {
                // Authority simulates at 60 Hz but publishes CONTROL at 20 Hz,
                // retaining the minimum first-effective slack across the batch.
                schedule(
                    &mut to_client,
                    reliable.reliable(now),
                    Downlink::Finalized(
                        std::mem::take(&mut unpublished_receipts),
                        arrival_slack.take(),
                    ),
                );
                if let Some(at) = downlink.unreliable(now) {
                    schedule(&mut to_client, at, Downlink::Owner(receipt.tick));
                }
            }
        }

        let mut feedback_batch = 0;
        for event in take_due(&mut to_client, now) {
            match event {
                Downlink::Owner(tick) => {
                    owner_received += 1;
                    clock.commit(tick);
                }
                Downlink::Finalized(receipts, slack) => {
                    assert_eq!(receipts.len(), 3);
                    finalized_received += receipts.len();
                    finalized_envelopes += 1;
                    let through = receipts.last().unwrap().tick;
                    clock.commit(through);
                    pending_commands.retain(|target: &TargetTick, _| target.0 > through.0);
                    if let Some(slack) = slack {
                        feedback_batch += 1;
                        late_feedback += usize::from(slack == 0);
                        lead.observe_arrival_slack(slack).unwrap();
                    }
                }
                Downlink::Clock(exchange, tick) => {
                    clock_received += 1;
                    wall.observe(exchange).unwrap();
                    clock.observe(tick, exchange.server_send).unwrap();
                }
            }
        }
        max_feedback_batch = max_feedback_batch.max(feedback_batch);
        if now % 250 == 0 {
            schedule(
                &mut to_server,
                control_uplink.reliable(now),
                Uplink::Probe(ms(now)),
            );
        }
        let server_now = wall.estimate_server_time(ms(now)).unwrap();
        let estimated_tick = clock.estimate(server_now, lead.lead()).unwrap();
        if let Some(target) = lead.assign_target(estimated_tick).unwrap() {
            first_target.get_or_insert(target);
            assert!(target > last_target);
            assert!(target.0 <= clock.committed().0 + u64::from(replay_budget()));
            // Bootstrap at the first assignment; afterwards match generation's
            // contiguous frontier fill when clock progress or lead adds ticks.
            let first = if last_target.0 == 0 {
                target.0
            } else {
                last_target.0 + 1
            };
            for tick in first..=target.0 {
                sequence += 1;
                let command = Command {
                    owner,
                    sequence: CommandSeq(sequence),
                    target: TargetTick(tick),
                    input: (),
                    actions: Default::default(),
                };
                command_timing.insert(
                    command.target,
                    CommandTiming {
                        filled_slot: tick < target.0,
                        lead: lead.lead(),
                        headroom: tick - inbox.finalized_through().0,
                    },
                );
                pending_commands.insert(command.target, command.clone());
                let older = pending_commands
                    .values()
                    .rev()
                    .skip(1)
                    .cloned()
                    .collect::<Vec<_>>();
                let records = pack_redundant(&command, &older, 8, 2048).unwrap();
                // Loss and jitter apply once to this complete INPUT packet,
                // allowing a later packet's redundant record to arrive first.
                if let Some(at) = uplink.unreliable(now) {
                    schedule(&mut to_server, at, Uplink::Commands(records));
                }
            }
            last_target = target;
        }
        if now >= 10_000 {
            steady_min = steady_min.min(lead.lead());
            steady_max = steady_max.max(lead.lead());
        }
        assert_eq!(wall.needs_resynchronization(), None);
        assert_eq!(lead.needs_resynchronization(), None);
    }

    eprintln!(
        "clock feedback: accepted={accepted}, unique_late={late}, late_percent={:.2}, \
         executed={executed}, substituted={substituted}, steady_lead={steady_min}..={steady_max}, \
         raw_late={raw_late}, duplicate={duplicate}, late_feedback={late_feedback}, \
         max_feedback_batch={max_feedback_batch}, finalized_receipts={finalized_received}, \
         finalized_envelopes={finalized_envelopes}; \
         [accepted, late] by_issued_lead={admissions_by_lead:?}, \
         by_headroom={admissions_by_headroom:?}, filled_slots={filled_admissions:?}",
        100.0 * late as f64 / (accepted + late) as f64
    );
    assert!(accepted > 1000, "only {accepted} commands were accepted");
    assert!(duplicate > 0);
    assert!(late * 20 < accepted, "late={late}, accepted={accepted}");
    assert!(owner_received > 300);
    assert!(finalized_received > 1000);
    assert!(clock_received > 60);
    assert!(uplink.lost > 0 && control_uplink.lost > 0 && downlink.lost > 0 && reliable.lost > 0);
    assert!(steady_min >= 3, "steady lead fell to {steady_min}");
    assert!(steady_max <= 7, "steady lead rose to {steady_max}");
    assert!(steady_max - steady_min <= 2);
    assert!(last_target.0 > base_tick + 1190);
}

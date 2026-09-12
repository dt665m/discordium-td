//! Authority-owned timing evidence; none of these records originate from a client.
use super::*;
use engine_net::combat_timing::{self, SentReference, TimingContext, TimingError, TimingPolicy};
use engine_net::replication::StateReceipt;

const REFERENCE_LIMIT: usize = 64;
const CLOCK_LIMIT: usize = 64;
#[derive(Clone, Copy)]
struct Reference {
    receipt: StateReceipt,
    state_at: ServerTick,
    sent_at: ServerTick,
    decoded: bool,
}
#[derive(Default)]
pub(super) struct PeerCombat {
    references: VecDeque<Reference>,
    arrivals: BTreeMap<CommandSeq, ServerTick>,
}
impl PeerCombat {
    pub fn sent(&mut self, receipt: StateReceipt, state_at: ServerTick, sent_at: ServerTick) {
        if self.references.iter().any(|entry| entry.receipt == receipt) {
            return;
        }
        if self.references.len() == REFERENCE_LIMIT {
            self.references.pop_front();
        }
        self.references.push_back(Reference {
            receipt,
            state_at,
            sent_at,
            decoded: false,
        });
    }
    pub fn decoded(&mut self, receipt: StateReceipt) {
        if let Some(entry) = self
            .references
            .iter_mut()
            .find(|entry| entry.receipt == receipt)
        {
            entry.decoded = true;
        }
    }
    pub fn admit(&mut self, sequence: CommandSeq, arrival: ServerTick) -> Result<(), String> {
        // Only successfully admitted future commands enter this map. Redundant
        // delivery cannot refresh their first observed arrival or consume memory.
        if self.arrivals.contains_key(&sequence) {
            return Ok(());
        }
        if self.arrivals.len()
            >= usize::from(engine_net::commands::CommandLimits::default().future_ticks)
        {
            return Err("Combat arrival capacity".into());
        }
        self.arrivals.insert(sequence, arrival);
        Ok(())
    }
    pub fn take_arrival(&mut self, sequence: CommandSeq) -> Option<ServerTick> {
        self.arrivals.remove(&sequence)
    }
    pub fn validate(
        &self,
        view: live::CombatViewStamp,
        arrival: ServerTick,
        execution: ServerTick,
        clock: &GameplayClock,
        epoch: u32,
    ) -> Result<(ServerTick, u32, u16, bool), TimingError> {
        let reference = self
            .references
            .iter()
            .find(|entry| entry.receipt == view.reference && entry.decoded)
            .ok_or(TimingError::UnknownReference)?;
        let (oldest, newest) = clock.bounds(epoch).ok_or(TimingError::MissingHistory)?;
        let result = combat_timing::validate_view(
            policy(),
            TimingContext {
                connection: reference.receipt.scope.connection,
                scene: SCENE,
                arrival,
                execution,
                oldest_history: oldest,
                newest_history: newest,
            },
            combat_timing::ViewRequest {
                sampled_at: view.sampled_at,
                sampled_fraction: view.sampled_fraction,
                viewed_at: view.viewed_at,
                viewed_fraction: view.viewed_fraction,
                reference: view.reference.snapshot,
            },
            |_| {
                Some(SentReference {
                    connection: reference.receipt.scope.connection,
                    scene: SCENE,
                    state_at: reference.state_at,
                    sent_at: reference.sent_at,
                })
            },
        )?;
        // A stamped edge describes one actual displayed instant. Never replace
        // that instant with a clamped history endpoint after validation.
        if result.clamped {
            return Err(TimingError::OutsidePolicy);
        }
        let gameplay = clock
            .at(epoch, result.query)
            .ok_or(TimingError::MissingHistory)?;
        let fraction = if result.fraction == 0 {
            0
        } else {
            let next = clock
                .at(
                    epoch,
                    ServerTick(result.query.0.checked_add(1).ok_or(TimingError::Overflow)?),
                )
                .ok_or(TimingError::MissingHistory)?;
            if next == gameplay {
                // Server time advanced while gameplay was paused: one exact pose.
                0
            } else if gameplay.checked_add(1) == Some(next) {
                result.fraction
            } else {
                return Err(TimingError::MissingHistory);
            }
        };
        Ok((result.query, gameplay, fraction, false))
    }
}
#[derive(Default)]
pub(super) struct GameplayClock {
    entries: VecDeque<(u32, ServerTick, u32)>,
}
impl GameplayClock {
    pub fn record(&mut self, epoch: u32, server: ServerTick, gameplay: u32) {
        if self.entries.back().is_some_and(|(old, _, _)| *old != epoch) {
            self.entries.clear();
        }
        if self.entries.len() == CLOCK_LIMIT {
            self.entries.pop_front();
        }
        self.entries.push_back((epoch, server, gameplay));
    }
    fn bounds(&self, epoch: u32) -> Option<(ServerTick, ServerTick)> {
        let first = self.entries.front()?;
        let last = self.entries.back()?;
        (first.0 == epoch && last.0 == epoch).then_some((first.1, last.1))
    }
    fn at(&self, epoch: u32, server: ServerTick) -> Option<u32> {
        self.entries
            .iter()
            .find(|entry| entry.0 == epoch && entry.1 == server)
            .map(|entry| entry.2)
    }
}
fn ticks(millis: u64) -> u64 {
    (millis * u64::from(dreamwake_sim::TICK_HZ)).div_ceil(1000)
}
fn policy() -> TimingPolicy {
    TimingPolicy {
        maximum_sample_age: ticks(250),
        maximum_future_sample: 2,
        minimum_execution_lead: 0,
        maximum_execution_lead: 12,
        minimum_presentation_age: 0,
        maximum_presentation_age: ticks(200),
        maximum_rewind: ticks(150),
        maximum_reference_advance: ticks(150),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receipt() -> StateReceipt {
        StateReceipt {
            scope: ScopeIdentity {
                connection: ConnectionEpoch(1),
                entity: EntityId {
                    index: 1,
                    generation: 1,
                },
                scope: ScopeEpoch(1),
                representation: RepresentationRevision(1),
            },
            baseline_generation: BaselineGeneration(1),
            snapshot: SnapshotId(1),
            version: StateVersion(1),
        }
    }
    #[test]
    fn duplicate_arrival_stays_first_and_reference_requires_exact_decoded_proof() {
        let mut peer = PeerCombat::default();
        peer.admit(CommandSeq(1), ServerTick(10)).unwrap();
        peer.admit(CommandSeq(1), ServerTick(12)).unwrap();
        assert_eq!(peer.take_arrival(CommandSeq(1)), Some(ServerTick(10)));
        let mut clock = GameplayClock::default();
        for tick in 1..=20 {
            clock.record(
                1,
                ServerTick(tick),
                if tick < 12 { tick as u32 } else { 12 },
            );
        }
        let view = live::CombatViewStamp {
            sampled_at: ServerTick(18),
            sampled_fraction: 0,
            viewed_at: ServerTick(14),
            viewed_fraction: 0,
            reference: receipt(),
        };
        peer.sent(receipt(), ServerTick(10), ServerTick(11));
        assert_eq!(
            peer.validate(view, ServerTick(19), ServerTick(20), &clock, 1),
            Err(TimingError::UnknownReference)
        );
        peer.decoded(receipt());
        assert_eq!(
            peer.validate(view, ServerTick(19), ServerTick(20), &clock, 1),
            Ok((ServerTick(14), 12, 0, false))
        );
        let mut forged = view;
        forged.reference.version = StateVersion(2);
        assert_eq!(
            peer.validate(forged, ServerTick(19), ServerTick(20), &clock, 1),
            Err(TimingError::UnknownReference)
        );
        assert_eq!(
            peer.validate(view, ServerTick(19), ServerTick(20), &clock, 2),
            Err(TimingError::MissingHistory)
        );
        clock.record(2, ServerTick(21), 0);
        assert!(clock.bounds(1).is_none());
        assert_eq!(clock.at(2, ServerTick(21)), Some(0));
    }
    #[test]
    fn fractional_clock_mapping_requires_exact_neighbors_and_never_clamps() {
        let mut peer = PeerCombat::default();
        peer.sent(receipt(), ServerTick(10), ServerTick(11));
        peer.decoded(receipt());
        let mut clock = GameplayClock::default();
        for tick in 1..=20 {
            clock.record(1, ServerTick(tick), (tick as u32).min(12));
        }
        let mut view = live::CombatViewStamp {
            sampled_at: ServerTick(18),
            sampled_fraction: 32768,
            viewed_at: ServerTick(11),
            viewed_fraction: 32768,
            reference: receipt(),
        };
        assert_eq!(
            peer.validate(view, ServerTick(19), ServerTick(20), &clock, 1),
            Ok((ServerTick(11), 11, 32768, false))
        );
        view.viewed_at = ServerTick(14);
        assert_eq!(
            peer.validate(view, ServerTick(19), ServerTick(20), &clock, 1),
            Ok((ServerTick(14), 12, 0, false))
        );
        view.viewed_at = ServerTick(1);
        assert_eq!(
            peer.validate(view, ServerTick(19), ServerTick(20), &clock, 1),
            Err(TimingError::OutsidePolicy)
        );
        view.viewed_at = ServerTick(11);
        clock
            .entries
            .retain(|(_, server, _)| *server != ServerTick(12));
        assert_eq!(
            peer.validate(view, ServerTick(19), ServerTick(20), &clock, 1),
            Err(TimingError::MissingHistory)
        );
    }
}

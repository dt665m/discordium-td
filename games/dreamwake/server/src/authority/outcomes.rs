//! Game outcome mapping over the engine's bounded terminal-result ledger.
use super::*;
use dreamwake_sim::combat::{ActionReason, ActionVerdict};
use engine_net::outcomes::{OutcomeLedger, OutcomeLimits, OutcomeSequencer, TerminalOutcome};
use live::{ActionOutcome, ActionOutcomeData, OutcomeReason, TerminalStatus};

pub(super) struct Outcomes {
    pub normal: OutcomeLedger<ActionOutcomeData>,
    /// Separate refusal capacity; never stolen by successful gameplay traffic.
    pub overload: OutcomeLedger<ActionOutcomeData>,
}
impl Default for Outcomes {
    fn default() -> Self {
        let retention_ticks = u64::from(dreamwake_sim::TICK_HZ) * 30;
        let sequence = OutcomeSequencer::default();
        Self {
            normal: OutcomeLedger::with_sequencer(
                OutcomeLimits {
                    records: 131_072,
                    records_per_owner: 16_384,
                    retained_bytes: 64 * 1024 * 1024,
                    maximum_payload_bytes: 256,
                    retention_ticks,
                },
                sequence.clone(),
            )
            .unwrap(),
            overload: OutcomeLedger::with_sequencer(
                OutcomeLimits {
                    records: 4096,
                    records_per_owner: 128,
                    retained_bytes: 4 * 1024 * 1024,
                    maximum_payload_bytes: 256,
                    retention_ticks,
                },
                sequence,
            )
            .unwrap(),
        }
    }
}
impl Outcomes {
    pub fn reported(
        &mut self,
        owner: u64,
        keys: impl Iterator<Item = ActionKey>,
    ) -> Result<(), String> {
        for key in keys {
            let ledger = if self.normal.lookup(owner, key).is_some() {
                &mut self.normal
            } else {
                &mut self.overload
            };
            ledger.reported(owner, &[key]).map_err(failure)?;
        }
        Ok(())
    }
    pub fn lookup(&self, owner: u64, key: ActionKey) -> Option<ActionOutcome> {
        self.normal
            .lookup(owner, key)
            .or_else(|| self.overload.lookup(owner, key))
            .map(wire)
    }
    pub fn can_consume_bundle(&self, owner: u64) -> bool {
        // A legal bundle may introduce 8 commands with 8 edges each. Existing
        // duplicates need less, but this gate reserves the conservative new work.
        self.normal.available_for(owner) >= 64 || self.overload.available_for(owner) >= 64
    }
    pub fn expire(&mut self, tick: ServerTick) {
        self.normal.expire(tick);
        self.overload.expire(tick);
    }
    pub fn revoke(&mut self, owner: u64, connection: ConnectionEpoch, tick: ServerTick) {
        for ledger in [&mut self.normal, &mut self.overload] {
            // Only unfinished actions acquire a rejection. Existing final
            // accepted/rejected verdicts remain immutable across handoff.
            for key in ledger.pending(owner, connection) {
                let _ = ledger.finish(
                    owner,
                    key,
                    tick,
                    rejection(TerminalStatus::Rejected, OutcomeReason::OwnershipDenied),
                );
            }
            ledger.close_delivery(owner, connection);
        }
    }
    pub fn close(&mut self, owner: u64, connection: ConnectionEpoch, tick: ServerTick) {
        for ledger in [&mut self.normal, &mut self.overload] {
            for key in ledger.pending(owner, connection) {
                let _ = ledger.finish(
                    owner,
                    key,
                    tick,
                    rejection(TerminalStatus::Expired, OutcomeReason::Expired),
                );
            }
            ledger.close_delivery(owner, connection);
        }
    }
    #[cfg(test)]
    pub fn verdict(&mut self, owner: u64, verdict: ActionVerdict) -> Result<(), String> {
        self.verdict_with_bindings(owner, verdict, vec![])
    }
    pub fn verdict_with_bindings(
        &mut self,
        owner: u64,
        verdict: ActionVerdict,
        bindings: Vec<(u8, EntityId)>,
    ) -> Result<(), String> {
        let key = ActionKey {
            connection: ConnectionEpoch(verdict.key.connection_epoch),
            stream: CommandStream(verdict.key.command_stream),
            command: CommandSeq(verdict.key.command_sequence),
            slot: verdict.key.action_slot,
        };
        let accepted = verdict.reason == ActionReason::Accepted;
        let mut data = rejection(
            if accepted {
                TerminalStatus::Accepted
            } else {
                TerminalStatus::Rejected
            },
            OutcomeReason::Game(verdict.reason),
        );
        if accepted {
            data.spawn_bindings = engine_net::codec::BoundedVec::new(bindings).map_err(failure)?;
        }
        if let Some(combat) = verdict.combat {
            data.query_tick = Some(ServerTick(combat.query_server_tick));
            data.reason = OutcomeReason::Combat(combat.reason);
            if accepted {
                // Hit feedback is approved for the shooter, but a hit is not
                // independent disclosure of the target's identity/incarnation.
                // Exact transaction/target/region stay in authority diagnostics.
                // Public actor slots do not prove combat-generation entitlement.
                data.damage = combat.damage;
            }
        }
        self.normal
            .finish(owner, key, ServerTick(verdict.execution_server_tick), data)
            .map_err(failure)?;
        Ok(())
    }
}
pub(super) fn rejection(status: TerminalStatus, reason: OutcomeReason) -> ActionOutcomeData {
    ActionOutcomeData {
        status,
        reason,
        query_tick: None,
        target: None,
        hit_region: 0,
        damage: 0.0,
        transaction: None,
        binding: None,
        spawn_bindings: Default::default(),
    }
}
pub(super) fn wire(value: &TerminalOutcome<ActionOutcomeData>) -> ActionOutcome {
    ActionOutcome {
        sequence: value.sequence,
        key: value.key,
        execution_tick: value.at,
        data: value.payload.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_hit_receipt_never_discloses_unscoped_target_or_internal_transaction() {
        use dreamwake_sim::combat::*;
        let mut ledger = Outcomes::default();
        let key = ActionKey {
            connection: ConnectionEpoch(1),
            stream: CommandStream(1),
            command: CommandSeq(1),
            slot: 0,
        };
        ledger.normal.reserve(7, &[key]).unwrap();
        let action = RayActionKey {
            match_epoch: 1,
            connection_epoch: 1,
            command_stream: 1,
            ownership_epoch: 1,
            actor: 7,
            actor_generation: 1,
            command_sequence: 1,
            action_slot: 0,
        };
        let target = CombatTarget {
            id: 9007199254740991,
            generation: 777,
        };
        let transaction = DamageTransaction {
            action,
            target,
            hit_slot: 0,
        };
        ledger
            .verdict(
                7,
                ActionVerdict {
                    key: action,
                    execution_server_tick: 10,
                    execution_gameplay_tick: 10,
                    reason: ActionReason::Accepted,
                    combat: Some(CombatVerdict {
                        key: action,
                        execution_server_tick: 10,
                        execution_gameplay_tick: 10,
                        query_server_tick: 7,
                        query_gameplay_tick: 7,
                        reason: CombatReason::Hit,
                        target: Some(target),
                        hit_region: 3,
                        damage: 9.0,
                        transaction: Some(transaction),
                    }),
                },
            )
            .unwrap();
        let outcome = ledger.lookup(7, key).unwrap();
        assert_eq!(outcome.data.damage, 9.0);
        assert_eq!(
            outcome.data.reason,
            OutcomeReason::Combat(CombatReason::Hit)
        );
        assert_eq!(outcome.data.target, None);
        assert_eq!(outcome.data.transaction, None);
        assert_eq!(outcome.data.binding, None);
        assert_eq!(outcome.data.hit_region, 0);
        let encoded = live::encode_control(
            &live::ServerControl::ActionOutcomes {
                batch: 1,
                records: BoundedVec::new(vec![outcome]).unwrap(),
            },
            ConnectionEpoch(1),
            1,
            engine_net::codec::FrameLimit::new(1100).unwrap(),
        )
        .unwrap();
        assert!(
            !encoded
                .windows(8)
                .any(|bytes| bytes == target.id.to_le_bytes())
        );
    }
}

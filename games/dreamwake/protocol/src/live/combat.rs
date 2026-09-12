//! Per-hardware-edge timing evidence. This never supplies a muzzle or damage rule.
use engine_net::{
    codec::{CodecError, Validate},
    replication::StateReceipt,
    types::ServerTick,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatViewStamp {
    /// Estimated server time when this exact hardware edge was captured (E).
    pub sampled_at: ServerTick,
    /// Phase within the captured tick, in exact units of 1/65536.
    pub sampled_fraction: u16,
    /// Effective rendered remote pose time, including a starvation freeze (R).
    pub viewed_at: ServerTick,
    pub viewed_fraction: u16,
    /// Exact complete owner checkpoint decoded before the edge was captured.
    pub reference: StateReceipt,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BeamAimSample {
    pub aim: [f32; 2],
    pub view: CombatViewStamp,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TickInput {
    pub held: dreamwake_sim::DreamInput,
    pub beam: Option<BeamAimSample>,
}
impl From<dreamwake_sim::DreamInput> for TickInput {
    fn from(held: dreamwake_sim::DreamInput) -> Self {
        Self { held, beam: None }
    }
}
impl Validate for CombatViewStamp {
    fn validate(&self) -> Result<(), CodecError> {
        if self.sampled_at.0 == 0
            || (self.viewed_at, self.viewed_fraction) > (self.sampled_at, self.sampled_fraction)
            || !super::valid_receipt(&self.reference)
        {
            return Err(CodecError::InvalidPayload(
                "invalid combat view stamp".into(),
            ));
        }
        Ok(())
    }
}

use dreamwake_sim::combat::{ActionReason, CombatReason, CombatTarget, DamageTransaction};
use engine_net::{
    replication::Payload,
    types::{ActionKey, EntityId},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalStatus {
    Accepted,
    Rejected,
    Expired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutcomeReason {
    Game(ActionReason),
    Combat(CombatReason),
    Timing,
    Expired,
    Capacity,
    OwnershipDenied,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionOutcomeData {
    pub status: TerminalStatus,
    pub reason: OutcomeReason,
    pub query_tick: Option<ServerTick>,
    pub target: Option<CombatTarget>,
    pub hit_region: u8,
    pub damage: f32,
    pub transaction: Option<DamageTransaction>,
    pub binding: Option<EntityId>,
    pub spawn_bindings: engine_net::codec::BoundedVec<(u8, EntityId), 3>,
}
impl Payload for ActionOutcomeData {
    fn retained_bytes(&self) -> usize {
        3 * std::mem::size_of::<(u8, EntityId)>()
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionOutcome {
    pub sequence: u64,
    pub key: ActionKey,
    pub execution_tick: ServerTick,
    pub data: ActionOutcomeData,
}
pub(super) fn valid_action_key(key: &ActionKey) -> bool {
    key.connection.0 != 0 && key.stream.0 != 0 && key.command.0 != 0 && key.slot < 8
}
impl Validate for ActionOutcome {
    fn validate(&self) -> Result<(), CodecError> {
        let valid = self.sequence != 0
            && valid_action_key(&self.key)
            && self.data.damage.is_finite()
            && self.data.damage >= 0.0
            && self
                .data
                .query_tick
                .is_none_or(|tick| tick <= self.execution_tick)
            && self
                .data
                .target
                .is_none_or(|target| target.id != 0 && target.generation != 0)
            && self
                .data
                .binding
                .is_none_or(|entity| entity.generation != 0)
            && self.data.spawn_bindings.as_slice().iter().enumerate().all(
                |(i, (ordinal, entity))| {
                    *ordinal <= 3
                        && entity.index > 0
                        && entity.generation > 0
                        && self.data.spawn_bindings.as_slice()[..i]
                            .iter()
                            .all(|(old, _)| old != ordinal)
                },
            )
            && (self.data.status == TerminalStatus::Accepted
                || self.data.spawn_bindings.is_empty())
            && (self.data.status == TerminalStatus::Accepted
                || (self.data.target.is_none()
                    && self.data.transaction.is_none()
                    && self.data.damage == 0.0))
            && self.data.transaction.is_none_or(|transaction| {
                transaction.action.connection_epoch == self.key.connection.0
                    && transaction.action.command_stream == self.key.stream.0
                    && transaction.action.command_sequence == self.key.command.0
                    && transaction.action.action_slot == self.key.slot
                    && Some(transaction.target) == self.data.target
            });
        if valid {
            Ok(())
        } else {
            Err(CodecError::InvalidPayload(
                "invalid terminal action outcome".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_net::types::*;
    #[test]
    fn combat_stamp_checks_fractional_order_including_maximum_phase() {
        let mut view = CombatViewStamp {
            sampled_at: ServerTick(10),
            sampled_fraction: 2,
            viewed_at: ServerTick(10),
            viewed_fraction: 2,
            reference: StateReceipt {
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
            },
        };
        assert!(view.validate().is_ok());
        view.viewed_fraction = 3;
        assert!(view.validate().is_err());
        view.viewed_at = ServerTick(9);
        view.viewed_fraction = u16::MAX;
        assert!(view.validate().is_ok());
    }
}

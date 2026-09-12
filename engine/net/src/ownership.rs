//! Effective-tick control fences supplied by an authenticated game authority.
//! This value neither authenticates callers nor allocates ownership identities.
use crate::{
    commands::OwnerStream,
    types::{ServerTick, TargetTick},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffError {
    InvalidIdentity,
    ChangedEntity,
    StaleEpoch,
    EffectiveTickCommitted,
}

/// One immutable transition; games bound pending transitions and retain their
/// existing identity allocator. Old commands stop before `effective_tick`, and
/// the new stream still requires the game's normal full-checkpoint admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnershipHandoff {
    previous: OwnerStream,
    next: OwnerStream,
    effective_tick: ServerTick,
}
impl OwnershipHandoff {
    pub fn new(
        previous: OwnerStream,
        next: OwnerStream,
        effective_tick: ServerTick,
        committed: ServerTick,
    ) -> Result<Self, HandoffError> {
        for stream in [previous, next] {
            if stream.connection.0 == 0
                || stream.epoch.0 == 0
                || stream.stream.0 == 0
                || stream.owner.generation == 0
                || stream.ownership.0 == 0
            {
                return Err(HandoffError::InvalidIdentity);
            }
        }
        if previous.owner != next.owner {
            return Err(HandoffError::ChangedEntity);
        }
        if next.epoch <= previous.epoch || next.ownership <= previous.ownership {
            return Err(HandoffError::StaleEpoch);
        }
        if effective_tick <= committed {
            return Err(HandoffError::EffectiveTickCommitted);
        }
        Ok(Self {
            previous,
            next,
            effective_tick,
        })
    }
    pub fn previous(self) -> OwnerStream {
        self.previous
    }
    pub fn next(self) -> OwnerStream {
        self.next
    }
    pub fn effective_tick(self) -> ServerTick {
        self.effective_tick
    }
    pub fn owns_at(self, stream: OwnerStream, target: TargetTick) -> bool {
        stream
            == if target.0 < self.effective_tick.0 {
                self.previous
            } else {
                self.next
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    fn stream(connection: u64, epoch: u64) -> OwnerStream {
        OwnerStream {
            connection: ConnectionId(connection),
            epoch: ConnectionEpoch(epoch),
            stream: CommandStream(1),
            owner: EntityId {
                index: 1,
                generation: 1,
            },
            ownership: OwnershipEpoch(epoch as u32),
        }
    }
    #[test]
    fn boundary_fences_both_streams_and_rejects_stale_transitions() {
        let old = stream(1, 1);
        let new = stream(2, 2);
        let fence = OwnershipHandoff::new(old, new, ServerTick(10), ServerTick(4)).unwrap();
        assert!(fence.owns_at(old, TargetTick(9)));
        assert!(!fence.owns_at(new, TargetTick(9)));
        assert!(!fence.owns_at(old, TargetTick(10)));
        assert!(fence.owns_at(new, TargetTick(10)));
        assert_eq!(
            OwnershipHandoff::new(old, new, ServerTick(4), ServerTick(4)),
            Err(HandoffError::EffectiveTickCommitted)
        );
        assert_eq!(
            OwnershipHandoff::new(new, old, ServerTick(10), ServerTick(4)),
            Err(HandoffError::StaleEpoch)
        );
        let mut changed = new;
        changed.owner.generation += 1;
        assert_eq!(
            OwnershipHandoff::new(old, changed, ServerTick(10), ServerTick(4)),
            Err(HandoffError::ChangedEntity)
        );
    }
}

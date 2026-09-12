use crate::{replication::Payload, types::*};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct JournalScope {
    pub connection: ConnectionEpoch,
    pub stream: CommandStream,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventKind(pub u32);
/// Kind and ordinal are game-supplied stable semantics, never global counters or
/// an index into the branch-dependent order in which replay emitted effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventKey {
    pub action: ActionKey,
    pub kind: EventKind,
    pub spawn_ordinal: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventIdentity {
    pub key: EventKey,
    pub origin_tick: ServerTick,
    pub schema: SchemaId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Reversible,
    /// A reversible simulation object bounded by record capacity. Only explicit
    /// simulation removal, authoritative generation replacement, replay absence,
    /// rejection or reset retires it; wall ticks cannot expire a paused object.
    /// The expiry metadata remains fixed.
    SimulationOwned,
    /// A cosmetic cue such as a sound. Deliver once; absence cannot unplay it.
    /// This does not authorize irreversible external gameplay transactions.
    OneShot,
}
#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord<P> {
    pub identity: EventIdentity,
    /// Fixed lifetime bound for this identity. Changed parameters belong in the
    /// payload; extending a retired identity's lifetime never resurrects it.
    pub expires_at: ServerTick,
    pub delivery: Delivery,
    pub payload: P,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayRange {
    pub first: ServerTick,
    pub last: ServerTick,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accepted { binding: Option<EntityId> },
    Rejected,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Unresolved,
    Accepted,
    Rejected,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Unseen,
    Live,
    ReplayAbsent,
    Expired,
    Rejected,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    ReplayAbsent,
    Rejected,
    Expired,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Change<P> {
    Create {
        event: EventRecord<P>,
        committed: bool,
        binding: Option<EntityId>,
    },
    Update(EventRecord<P>),
    Cancel {
        key: EventKey,
        reason: CancelReason,
    },
    Confirm {
        key: EventKey,
    },
    Bind {
        key: EventKey,
        entity: EntityId,
    },
    /// Records an authoritative identity only. The old predicted visual remains
    /// retired: consumers must not create, replay or resurrect an effect here.
    BindRetired {
        key: EventKey,
        entity: EntityId,
    },
    /// Drop every old-scope handle and binding before reusing any pooled visual.
    Reset {
        previous: JournalScope,
        current: JournalScope,
    },
}
#[derive(Debug)]
pub struct Changes<P> {
    pub(super) values: Vec<Change<P>>,
    pub(super) payload_bytes: usize,
}
impl<P: Payload> Changes<P> {
    pub fn as_slice(&self) -> &[Change<P>] {
        &self.values
    }
    pub fn into_vec(self) -> Vec<Change<P>> {
        self.values
    }
    pub fn retained_bytes(&self) -> usize {
        self.values.capacity() * std::mem::size_of::<Change<P>>() + self.payload_bytes
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Includes live records and tombstones until explicit command retirement.
    pub records: usize,
    /// Highest-generation index fences are retained until a scope reset.
    pub bindings: usize,
    pub action_fences: usize,
    pub payload_bytes: usize,
    pub resident_bytes: usize,
    pub transitions: usize,
    pub transition_bytes: usize,
    /// Hard working allowance: two resident journals, returned transitions and
    /// one maximum-size replacement payload while the old payload is dropped.
    pub peak_bytes: usize,
    pub history_ticks: u64,
    pub lifetime_ticks: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            records: 2048,
            bindings: 2048,
            action_fences: 2048,
            payload_bytes: 16 * 1024,
            resident_bytes: 4 * 1024 * 1024,
            transitions: 1024,
            transition_bytes: 2 * 1024 * 1024,
            peak_bytes: 11 * 1024 * 1024,
            history_ticks: 256,
            lifetime_ticks: 4096,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    InvalidConfiguration,
    InvalidIdentity,
    InvalidPayload,
    WrongScope,
    Stale,
    OutsideHistory,
    DuplicateKey,
    Conflict,
    Unknown,
    Capacity,
    ResetRequired,
    LiveHistory,
}
impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "event journal: {self:?}")
    }
}
impl std::error::Error for EventError {}
#[derive(Debug, Clone, Copy)]
pub struct EventView<'a, P> {
    pub identity: EventIdentity,
    pub phase: Phase,
    pub decision: Decision,
    pub binding: Option<EntityId>,
    pub payload: Option<&'a P>,
}

//! Replication primitives without sockets or gameplay: audience and region
//! policy, and the acknowledged per-connection entity state stream.
pub mod audience;
pub mod regions;
pub mod stream;

/// Canonical, already validated representation storage. Implementations must
/// account for owned heap capacity, not only encoded length. The adapter must
/// bound decoding allocations before constructing this value.
pub trait Payload: Clone + PartialEq {
    fn retained_bytes(&self) -> usize;

    /// Whether losing this value only degrades presentation.
    ///
    /// A dependency the simulation must read to reproduce a frame is required:
    /// if its disclosure is revoked, the frame can no longer be replayed and
    /// prediction has to recover. A value that merely guides reversible
    /// presentation is not, and revoking it must not invalidate frames that
    /// already read it, because a scope leaving an observer's range is ordinary
    /// and frequent rather than a fault. Default to required, so a new
    /// dependency is only exempt when its author says so.
    fn optional(&self) -> bool {
        false
    }
}
impl Payload for Vec<u8> {
    fn retained_bytes(&self) -> usize {
        self.capacity()
    }
}
impl Payload for Box<[u8]> {
    fn retained_bytes(&self) -> usize {
        self.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationError {
    InvalidConfiguration,
    InvalidIdentity,
    Stale,
    Conflict,
    Unauthorized,
    Unknown,
    ResetRequired,
    Capacity,
    Expired,
    Incomplete,
    InvalidPayload,
}

impl std::fmt::Display for ReplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidConfiguration => "invalid replication limits",
            Self::InvalidIdentity => "invalid replication identity",
            Self::Stale => "stale replication incarnation or publication",
            Self::Conflict => "conflicting replication identity or payload",
            Self::Unauthorized => "representation not authorized at the current barrier",
            Self::Unknown => "unknown replication proof or entity",
            Self::ResetRequired => "bounded replication fence exhausted; reset required",
            Self::Capacity => "replication capacity exceeded",
            Self::Expired => "replication assembly expired",
            Self::Incomplete => "replication group incomplete",
            Self::InvalidPayload => "invalid canonical replication payload",
        };
        f.write_str(message)
    }
}
impl std::error::Error for ReplicationError {}

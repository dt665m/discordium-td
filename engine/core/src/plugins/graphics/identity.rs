use serde::{Deserialize, Serialize};

/// Optional input-source incarnation. Games map their authority identities into
/// these fields; the graphics capability does not depend on a transport.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GraphicsScope {
    pub source_epoch: u64,
    pub stream: u32,
    pub control_epoch: u32,
    pub generation: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GraphicsId {
    pub scope: Option<GraphicsScope>,
    pub match_epoch: u32,
    pub owner: u64,
    pub action_seq: u32,
    pub slot: u16,
}

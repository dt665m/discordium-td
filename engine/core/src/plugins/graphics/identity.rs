use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct GraphicsId {
    pub match_epoch: u32,
    pub owner: u64,
    pub action_seq: u32,
    pub slot: u16,
}

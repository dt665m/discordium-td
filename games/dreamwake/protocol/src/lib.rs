//! Dreamwake wire contract over the reusable engine networking layer.
use dreamwake_sim::{DreamInput, DreamSnapshot};
pub use engine_net::{
    ACTION_CHANNEL, CONTROL_CHANNEL, INPUT_CHANNEL, STATE_CHANNEL, connection_config, decode,
    decode_with_limit, encode, newer,
};
use serde::{Deserialize, Serialize};
pub const PROTOCOL_ID: u64 = 0x4452_4541_4d00_0003;
pub const DEFAULT_MAX_PLAYERS: usize = 8;
pub const MAX_PLAYERS: usize = 1024;
pub const MAX_INPUT_BYTES: usize = 2048;
pub const SNAPSHOT_HZ: u32 = 20;
pub const TICKS_PER_SNAPSHOT: u32 = dreamwake_sim::TICK_HZ / SNAPSHOT_HZ;
const _: () = assert!(dreamwake_sim::TICK_HZ.is_multiple_of(SNAPSHOT_HZ));

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DreamAction {
    Start { lucid: bool },
    Cast { slot: u8, aim: [f32; 2] },
    Dash { direction: [f32; 2] },
    Choose { choice: u8, slot: u8 },
    Continue,
    Swap { a: u8, b: u8 },
    BuyMemoryUpgrade { slot: u8 },
    Restart,
    Pause { paused: bool },
}

pub type DreamClientMessage = engine_net::ClientMessage<DreamInput, DreamAction>;
pub type DreamServerMessage = engine_net::ServerMessage<DreamSnapshot>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_are_bounded_and_sequences_wrap() {
        let message = DreamClientMessage::Action {
            epoch: 1,
            seq: 1,
            action: DreamAction::Cast {
                slot: 2,
                aim: [1.0, 0.0],
            },
        };
        assert_eq!(
            decode_with_limit::<DreamClientMessage>(&encode(&message), MAX_INPUT_BYTES).unwrap(),
            message
        );
        assert!(
            decode_with_limit::<DreamClientMessage>(&vec![0; MAX_INPUT_BYTES + 2], MAX_INPUT_BYTES)
                .is_err()
        );
        assert!(newer(0, Some(u32::MAX)));
        assert!(!newer(3, Some(3)));
        assert!(!newer(2, Some(3)));
    }
}

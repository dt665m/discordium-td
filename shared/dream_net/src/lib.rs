//! Dreamwake's bounded wire contract, shared by the dedicated server and clients.
//! Transport and compression reuse the existing Renet/renet-cross stack.
use game_sim::dream::{DreamInput, DreamSnapshot};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub use game_shared::{decode, decode_with_limit, encode};
/// Distinct from the tower-defense protocol: incompatible clients cannot join.
pub const PROTOCOL_ID: u64 = 0x4452_4541_4d00_0001;
pub const INPUT_CHANNEL: u8 = 0;
pub const ACTION_CHANNEL: u8 = 2;
pub const STATE_CHANNEL: u8 = 0;
pub const CONTROL_CHANNEL: u8 = 2;
pub const DEFAULT_MAX_PLAYERS: usize = 8;
pub const MAX_PLAYERS: usize = 1024;
pub const MAX_INPUT_BYTES: usize = 2048;
pub const SNAPSHOT_HZ: u32 = 20;
pub const TICKS_PER_SNAPSHOT: u32 = game_sim::dream::TICK_HZ / SNAPSHOT_HZ;
const _: () = assert!(game_sim::dream::TICK_HZ.is_multiple_of(SNAPSHOT_HZ));

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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DreamClientMessage {
    /// Movement and held attack only. Edge actions use the reliable action channel.
    Input {
        epoch: u32,
        seq: u32,
        input: DreamInput,
    },
    Action {
        epoch: u32,
        seq: u32,
        action: DreamAction,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DreamServerMessage {
    State {
        epoch: u32,
        revision: u64,
        client_id: u64,
        /// Reserved for wire compatibility. Always zero; no player has authority over others.
        host_id: u64,
        ack_input: Option<u32>,
        ack_action: Option<u32>,
        snapshot: Box<DreamSnapshot>,
    },
    Notice {
        text: String,
    },
}

pub fn newer(seq: u32, previous: Option<u32>) -> bool {
    previous.is_none_or(|old| seq != old && seq.wrapping_sub(old) < (1 << 31))
}

pub fn connection_config() -> renet::ConnectionConfig {
    let mut config = renet::ConnectionConfig::default();
    // Bounded compressed full snapshots carry complete rollback state at20Hz.
    // This accommodates four-player encounters without allowing unbounded queues.
    config.available_bytes_per_tick = 256 * 1024;
    for channel in config
        .server_channels_config
        .iter_mut()
        .chain(config.client_channels_config.iter_mut())
    {
        channel.max_memory_usage_bytes = 4 * 1024 * 1024;
        if let renet::SendType::ReliableOrdered { resend_time } = &mut channel.send_type {
            *resend_time = Duration::from_millis(80);
        }
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_are_bounded_and_sequences_wrap() {
        let msg = DreamClientMessage::Action {
            epoch: 1,
            seq: 1,
            action: DreamAction::Cast {
                slot: 2,
                aim: [1.0, 0.0],
            },
        };
        let bytes = encode(&msg);
        assert_eq!(
            decode_with_limit::<DreamClientMessage>(&bytes, MAX_INPUT_BYTES).unwrap(),
            msg
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

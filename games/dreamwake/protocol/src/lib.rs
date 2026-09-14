//! Dreamwake wire contract over the reusable engine networking layer.
use dreamwake_sim::{DreamInput, DreamSnapshot};
pub use engine_net::{
    ACTION_CHANNEL, CONTROL_CHANNEL, INPUT_CHANNEL, STATE_CHANNEL, decode, decode_with_limit,
    encode, newer,
};
use serde::{Deserialize, Serialize};
pub mod live;
pub mod player;
pub mod replication;
pub const PROTOCOL_ID: u64 = 0x4452_4541_4d00_0017;
pub const SESSION_SERVICE: &str = "dreamwake";
pub const SESSION_MATCH: &str = "default";
pub const DEFAULT_MAX_PLAYERS: usize = 8;
pub const MAX_PLAYERS: usize = 1024;
pub const MAX_INPUT_BYTES: usize = 2048;
pub const SNAPSHOT_HZ: u32 = 20;
pub const TICKS_PER_SNAPSHOT: u32 = dreamwake_sim::TICK_HZ / SNAPSHOT_HZ;
const _: () = assert!(dreamwake_sim::TICK_HZ.is_multiple_of(SNAPSHOT_HZ));

pub const OWNER_GROUP_CHANNEL: u8 = 1;
pub const OWNER_GROUP_SEND_BYTES: usize = live::OWNER_GROUP_MESSAGE_BYTES;
pub const OWNER_GROUP_RECEIVE_BYTES: usize = 128 * 1024;
pub const CONTROL_MEMORY_BYTES: usize = 128 * 1024;
pub const REMOTE_STATE_MEMORY_BYTES: usize = 64 * 1024;

/// Channel order gives controls first use of each stock Renet packetization pass.
/// Sender payload retention and receiver slice allocation have separate bounds.
pub fn connection_config() -> renet::ConnectionConfig {
    let mut config = engine_net::connection_config();
    for channel in config.server_channels_config.iter_mut() {
        channel.max_memory_usage_bytes = match channel.channel_id {
            OWNER_GROUP_CHANNEL => OWNER_GROUP_RECEIVE_BYTES,
            CONTROL_CHANNEL => CONTROL_MEMORY_BYTES,
            _ => REMOTE_STATE_MEMORY_BYTES,
        };
    }
    for channel in config.client_channels_config.iter_mut() {
        channel.max_memory_usage_bytes = CONTROL_MEMORY_BYTES;
    }
    config
        .server_channels_config
        .sort_by_key(|channel| match channel.channel_id {
            CONTROL_CHANNEL => 0,
            STATE_CHANNEL => 1,
            _ => 2,
        });
    config
}

pub fn server_connection_config() -> renet::ConnectionConfig {
    let mut config = connection_config();
    // A 20 Hz publication can queue several remote actors together. Stock Renet
    // discards unreliable messages beyond this per-flush allowance, so it must
    // admit the publication burst, not merely one sixtieth of the byte rate.
    // The separate final-egress pacer charges physical framing and retries.
    config.available_bytes_per_tick = 7000;
    config
        .server_channels_config
        .iter_mut()
        .find(|channel| channel.channel_id == OWNER_GROUP_CHANNEL)
        .expect("default Renet reliable-unordered channel")
        .max_memory_usage_bytes = OWNER_GROUP_SEND_BYTES;
    config
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DreamAction {
    Start {
        lucid: bool,
    },
    Cast {
        slot: u8,
        aim: [f32; 2],
    },
    Dash {
        direction: [f32; 2],
    },
    Dreamlance {
        aim: [f32; 2],
        view: live::CombatViewStamp,
    },
    BeamBegin {
        aim: [f32; 2],
        view: live::CombatViewStamp,
    },
    BeamStop,
    ChargeBegin,
    ChargeRelease {
        episode: u64,
    },
    ChargeCancel {
        episode: u64,
    },
    Choose {
        choice: u8,
        slot: u8,
    },
    Continue,
    Swap {
        a: u8,
        b: u8,
    },
    BuyMemoryUpgrade {
        slot: u8,
    },
    Restart,
    Pause {
        paused: bool,
    },
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

//! Game-neutral bounded messages, sequence ordering and channel configuration.
use serde::{Deserialize, Serialize};
use std::time::Duration;
pub mod clock;
pub mod wire;
pub use wire::{decode, decode_with_limit, encode};
pub const INPUT_CHANNEL: u8 = 0;
pub const ACTION_CHANNEL: u8 = 2;
pub const STATE_CHANNEL: u8 = 0;
pub const CONTROL_CHANNEL: u8 = 2;
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage<I, A> {
    /// Continuously sampled input. Discrete actions use the reliable action channel.
    Input {
        epoch: u32,
        seq: u32,
        input: I,
    },
    Action {
        epoch: u32,
        seq: u32,
        action: A,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage<S> {
    State {
        epoch: u32,
        revision: u64,
        client_id: u64,
        /// Optional application-defined host identity; zero when unused.
        host_id: u64,
        ack_input: Option<u32>,
        ack_action: Option<u32>,
        snapshot: Box<S>,
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
    // Bound per-tick throughput and reliable queues independently of payload types.
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
    fn sequence_order_wraps_and_generic_commands_roundtrip() {
        assert!(newer(0, Some(u32::MAX)));
        assert!(!newer(3, Some(3)));
        assert!(!newer(2, Some(3)));
        assert!(!newer(1 << 31, Some(0)));
        let message: ClientMessage<[f32; 2], u8> = ClientMessage::Action {
            epoch: 2,
            seq: 9,
            action: 4,
        };
        assert_eq!(
            decode::<ClientMessage<[f32; 2], u8>>(&encode(&message)).unwrap(),
            message
        );
    }
}

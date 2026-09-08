//! Headless and UI configuration for simulated server distance.
#[cfg(feature = "ui")]
use bevy::prelude::Resource;
use clap::Args;
use renet_cross::server_conditioner::ServerConditionerHandle;

#[cfg(feature = "ui")]
#[derive(Resource, Clone)]
pub(crate) struct ServerConditioner(pub ServerConditionerHandle);
use renet_cross::conditioner::ConditionerConfig;
use std::time::Duration;

#[derive(Debug, Clone, Args, Default)]
pub struct NetworkConditionerArgs {
    /// Added delay in EACH direction for every client (150 adds about 300 ms RTT).
    #[arg(long, env = "TD_NET_DELAY_MS", default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=5000))]
    pub net_delay_ms: u64,
    /// Symmetric random jitter around each direction's delay, clamped at zero.
    #[arg(long, env = "TD_NET_JITTER_MS", default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=5000))]
    pub net_jitter_ms: u64,
    /// Independent packet loss percentage in each direction (0 through 100).
    #[arg(long, env = "TD_NET_LOSS_PERCENT", default_value_t = 0.0, value_parser = parse_loss)]
    pub net_loss_percent: f32,
}

fn parse_loss(raw: &str) -> Result<f32, String> {
    let value: f32 = raw
        .parse()
        .map_err(|_| "expected a loss percentage".to_owned())?;
    if value.is_finite() && (0.0..=100.0).contains(&value) {
        Ok(value)
    } else {
        Err("loss percentage must be finite and between 0 and 100".into())
    }
}

impl NetworkConditionerArgs {
    pub fn config(&self) -> ConditionerConfig {
        ConditionerConfig {
            enabled: self.net_delay_ms > 0 || self.net_jitter_ms > 0 || self.net_loss_percent > 0.0,
            latency: Duration::from_millis(self.net_delay_ms),
            jitter: Duration::from_millis(self.net_jitter_ms),
            packet_loss: self.net_loss_percent / 100.0,
            max_queue_packets: 512,
            max_queue_bytes: 512 * 1024,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        network: NetworkConditionerArgs,
    }

    #[test]
    fn startup_values_are_added_per_direction_and_loss_is_a_percentage() {
        let cli = Cli::try_parse_from([
            "server",
            "--net-delay-ms",
            "150",
            "--net-jitter-ms",
            "20",
            "--net-loss-percent",
            "1.5",
        ])
        .unwrap();
        let config = cli.network.config();
        assert!(config.enabled);
        assert_eq!(config.latency, Duration::from_millis(150));
        assert_eq!(config.jitter, Duration::from_millis(20));
        assert_eq!(config.packet_loss, 0.015);
        assert!(config.validate().is_ok());
        assert!(!NetworkConditionerArgs::default().config().enabled);
    }

    #[test]
    fn invalid_impairments_fail_before_startup() {
        for loss in ["NaN", "inf", "-1", "100.1"] {
            assert!(Cli::try_parse_from(["server", "--net-loss-percent", loss]).is_err());
        }
        assert!(Cli::try_parse_from(["server", "--net-delay-ms", "5001"]).is_err());
        assert!(Cli::try_parse_from(["server", "--net-jitter-ms", "-1"]).is_err());
    }
}

pub(crate) fn snapshot(handle: &ServerConditionerHandle) -> game_shared::DebugServerConditioner {
    let config = handle.config().packets;
    let stats = handle.stats();
    let direction =
        |s: renet_cross::conditioner::DirectionStats| game_shared::DebugConditionerDirection {
            queued_packets: s.queued_packets,
            queued_bytes: s.queued_bytes,
            simulated_loss_drops: s.simulated_loss_drops,
            outage_drops: s.outage_drops,
            overflow_drops: s.overflow_drops,
            transition_drops: s.transition_drops,
        };
    game_shared::DebugServerConditioner {
        packets: game_shared::DebugConditioner {
            enabled: config.enabled,
            delay_each_way_ms: config.latency.as_secs_f64() * 1000.0,
            jitter_ms: config.jitter.as_secs_f64() * 1000.0,
            packet_loss: config.packet_loss,
            baseline_rtt_ms: None,
            outage_active: stats.packets.outage_active,
            incoming: direction(stats.packets.incoming),
            outgoing: direction(stats.packets.outgoing),
        },
        peers: stats.peers,
        peer_limit_drops: stats.peer_limit_drops,
        expired_peers: stats.expired_peers,
    }
}

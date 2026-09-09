use clap::Args;
use renet_cross::conditioner::ConditionerConfig;
use std::time::Duration;

#[derive(Debug, Clone, Args, Default)]
pub struct NetworkConditionerArgs {
    /// Added delay in EACH direction for every client (150 adds about 300 ms RTT).
    #[arg(long, env = "ENGINE_NET_DELAY_MS", default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=5000))]
    pub net_delay_ms: u64,
    /// Symmetric random jitter around each direction's delay, clamped at zero.
    #[arg(long, env = "ENGINE_NET_JITTER_MS", default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=5000))]
    pub net_jitter_ms: u64,
    /// Independent packet loss percentage in each direction (0 through 100).
    #[arg(long, env = "ENGINE_NET_LOSS_PERCENT", default_value_t = 0.0, value_parser = parse_loss)]
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

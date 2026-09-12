//! Server configuration and transport/bootstrap construction.
use crate::SharedNet;
use crate::http_api::{self, HttpTlsConfig};
use axum::http::HeaderValue;
use renet_cross::{
    BootstrapConfig, BootstrapLimits, BootstrapService, EgressBasis, EgressConfig,
    EgressConfigError, MixedServerTransport, MixedTransportBuilder, MonotonicClientIdAllocator,
    SecureSessionAuthPolicy, ServerAuthentication, SessionAdmission, SessionAuthPolicy,
    UnsecureDevAuthPolicy,
};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
/// Per-connection transport-byte pacing. Native counts encrypted UDP payloads
/// plus IP/UDP headers; WebRTC counts encrypted logical messages, excluding its
/// browser/SCTP/DTLS/ICE framing and retransmissions.
#[derive(Debug, Clone, clap::Args)]
pub struct ServerEgressArgs {
    /// Egress bytes/sec per connection (native UDP/IP; WebRTC encrypted logical).
    /// Generic secure default: 60000; games may override it. Explicit development default: unpaced.
    #[arg(long = "egress-bytes-per-second", env = "ENGINE_EGRESS_BYTES_PER_SECOND", value_parser = clap::value_parser!(u64).range(1..=1_000_000_000))]
    pub bytes_per_second: Option<u64>,
    /// Maximum accumulated transport-byte credit when pacing is enabled.
    #[arg(long = "egress-burst-bytes", env = "ENGINE_EGRESS_BURST_BYTES", value_parser = clap::value_parser!(u64).range(2048..=67_108_864))]
    pub burst_bytes: Option<u64>,
    /// Credit reserved for netcode control within the same total ceiling.
    #[arg(long = "egress-control-reserve-bytes", env = "ENGINE_EGRESS_CONTROL_RESERVE_BYTES", default_value_t = 2400, value_parser = clap::value_parser!(u64).range(256..=67_108_864))]
    pub control_reserve_bytes: u64,
}
impl Default for ServerEgressArgs {
    fn default() -> Self {
        Self {
            bytes_per_second: None,
            burst_bytes: None,
            control_reserve_bytes: 2400,
        }
    }
}
#[derive(Debug, Clone, Copy)]
struct TransportEgress {
    native_udp_ip: Option<EgressConfig>,
    webrtc_logical: Option<EgressConfig>,
}
impl ServerEgressArgs {
    fn resolve(&self, secure: bool) -> Result<TransportEgress, EgressConfigError> {
        // Validate even disabled development settings, before opening sockets.
        let rate = self.bytes_per_second.unwrap_or(60_000);
        let native = EgressConfig::new(
            EgressBasis::NativeUdpIp,
            rate,
            self.burst_bytes.unwrap_or(6000),
            self.control_reserve_bytes,
        )?;
        let logical = EgressConfig::new(
            EgressBasis::EncryptedLogical,
            rate,
            self.burst_bytes.unwrap_or(6000),
            self.control_reserve_bytes,
        )?;
        let enabled = secure || self.bytes_per_second.is_some();
        Ok(TransportEgress {
            native_udp_ip: enabled.then_some(native),
            webrtc_logical: enabled.then_some(logical),
        })
    }
}
impl TransportEgress {
    fn apply(self, transport: &mut MixedServerTransport) -> Result<(), EgressConfigError> {
        transport.udp_mut().set_egress_limit(self.native_udp_ip)?;
        transport
            .webrtc_mut()
            .set_egress_limit(self.webrtc_logical)?;
        Ok(())
    }
}

#[derive(Debug, Clone, clap::Args)]
pub struct ServerConfig {
    #[command(flatten)]
    pub egress: ServerEgressArgs,
    #[command(flatten)]
    pub network_conditioner: crate::conditioner::NetworkConditionerArgs,
    /// Admission limit; benchmark the chosen player/entity workload before raising it.
    #[arg(long, env = "ENGINE_MAX_CLIENTS", default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=1024))]
    pub max_clients: u16,
    #[arg(long, env = "ENGINE_HTTP_BIND")]
    pub http_bind: Option<SocketAddr>,
    #[arg(long, env = "ENGINE_HTTP_TLS_CERT")]
    pub http_tls_cert: Option<PathBuf>,
    #[arg(long, env = "ENGINE_HTTP_TLS_KEY")]
    pub http_tls_key: Option<PathBuf>,
    #[arg(long, env = "ENGINE_UDP_BIND", default_value = "0.0.0.0:5000")]
    pub udp_bind: SocketAddr,
    #[arg(long, env = "ENGINE_WEBRTC_BIND", default_value = "0.0.0.0:5001")]
    pub webrtc_bind: SocketAddr,
    #[arg(long, env = "ENGINE_PUBLIC_UDP_ADDR", default_value = "127.0.0.1:5000")]
    pub public_udp_addr: SocketAddr,
    #[arg(
        long,
        env = "ENGINE_PUBLIC_WEBRTC_ADDR",
        default_value = "127.0.0.1:5001"
    )]
    pub public_webrtc_addr: SocketAddr,
    #[arg(long, env = "ENGINE_PUBLIC_HTTP_BASE")]
    pub public_http_base: Option<String>,
    #[arg(long, env = "ENGINE_CORS_ALLOWED_ORIGINS")]
    pub cors_allowed_origins: Option<String>,
    /// Permit plaintext token delivery for an explicitly trusted development LAN.
    /// HTTPS and loopback bootstrap do not need this option.
    #[arg(long, env = "ENGINE_ALLOW_INSECURE_BOOTSTRAP_HTTP")]
    pub allow_insecure_bootstrap_http: bool,
}
fn default_http_bind(tls_enabled: bool) -> SocketAddr {
    if tls_enabled {
        SocketAddr::from(([0, 0, 0, 0], 443))
    } else {
        SocketAddr::from(([0, 0, 0, 0], 8080))
    }
}

fn default_public_http_base(http_bind: SocketAddr, tls_enabled: bool) -> String {
    let scheme = if tls_enabled { "https" } else { "http" };
    let default_host = "127.0.0.1";
    let default_port = if tls_enabled { 443 } else { 80 };

    if http_bind.port() == default_port {
        format!("{scheme}://{default_host}")
    } else {
        format!("{scheme}://{default_host}:{}", http_bind.port())
    }
}

fn parse_cors_allowed_origins(
    raw: Option<String>,
) -> Result<Option<Vec<HeaderValue>>, std::io::Error> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let origins: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .collect();

    if origins.is_empty() || (origins.len() == 1 && origins[0] == "*") {
        return Ok(None);
    }

    if origins.contains(&"*") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ENGINE_CORS_ALLOWED_ORIGINS cannot mix '*' with explicit origins",
        ));
    }

    let mut values = Vec::with_capacity(origins.len());
    for origin in origins {
        let value = HeaderValue::from_str(origin).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid CORS origin '{origin}': {err}"),
            )
        })?;
        values.push(value);
    }

    Ok(Some(values))
}

/// Explicit development transport. Production games select their credential or
/// guest admission verifier through `start_with_admission`.
pub fn start_development(
    config: ServerConfig,
    protocol_id: u64,
) -> Result<SharedNet, Box<dyn std::error::Error>> {
    start_configured(
        config,
        protocol_id,
        ServerAuthentication::Unsecure,
        UnsecureDevAuthPolicy,
        false,
        renet::PacketConfig::default(),
    )
}

/// Authenticated native/WebRTC transport using one ephemeral server key. Account
/// verification remains game-supplied and runs during bootstrap, outside ticks.
pub fn start_with_admission<V: SessionAdmission + 'static>(
    config: ServerConfig,
    protocol_id: u64,
    verifier: V,
) -> Result<SharedNet, Box<dyn std::error::Error>> {
    let private_key = renet_cross::generate_random_bytes();
    let packet_profile = renet_cross::PacketProfile::ipv6_1200();
    start_configured(
        config,
        protocol_id,
        ServerAuthentication::Secure { private_key },
        SecureSessionAuthPolicy::new(verifier, private_key).with_packet_profile(packet_profile)?,
        true,
        packet_profile.packet_config()?,
    )
}

fn start_configured<P: SessionAuthPolicy + Send + Sync + 'static>(
    config: ServerConfig,
    protocol_id: u64,
    authentication: ServerAuthentication,
    auth_policy: P,
    secure: bool,
    packet_config: renet::PacketConfig,
) -> Result<SharedNet, Box<dyn std::error::Error>> {
    let egress = config.egress.resolve(secure)?;
    let ServerConfig {
        egress: _,
        network_conditioner,
        max_clients,
        http_bind,
        http_tls_cert,
        http_tls_key,
        udp_bind,
        webrtc_bind,
        public_udp_addr,
        public_webrtc_addr,
        public_http_base,
        cors_allowed_origins,
        allow_insecure_bootstrap_http,
    } = config;
    let http_tls = match (http_tls_cert, http_tls_key) {
        (Some(cert_path), Some(key_path)) => Some(HttpTlsConfig {
            cert_path,
            key_path,
        }),
        (None, None) => None,
        (Some(_), None) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "ENGINE_HTTP_TLS_CERT was set but ENGINE_HTTP_TLS_KEY is missing",
            )
            .into());
        }
        (None, Some(_)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "ENGINE_HTTP_TLS_KEY was set but ENGINE_HTTP_TLS_CERT is missing",
            )
            .into());
        }
    };
    let tls_enabled = http_tls.is_some();
    let http_bind = http_bind.unwrap_or_else(|| default_http_bind(tls_enabled));
    let mut public_http_base =
        public_http_base.unwrap_or_else(|| default_public_http_base(http_bind, tls_enabled));
    if tls_enabled && public_http_base.starts_with("http://") {
        public_http_base = format!("https://{}", public_http_base.trim_start_matches("http://"));
    }
    validate_bootstrap_url(&public_http_base, secure && !allow_insecure_bootstrap_http)?;
    let cors_allowed_origins = parse_cors_allowed_origins(cors_allowed_origins)?;

    let conditioner_config = network_conditioner.config();
    log::info!("server network conditioner startup: {conditioner_config:?}");
    let conditioner = renet_cross::server_conditioner::ServerConditionerHandle::new(
        renet_cross::server_conditioner::ServerConditionerConfig {
            packets: conditioner_config,
            max_peers: (usize::from(max_clients) * 2).max(32),
            ..Default::default()
        },
    )?;
    let mut transport = MixedTransportBuilder::new(protocol_id)
        .transport_config(renet_cross::ServerTransportConfig {
            conditioner: Some(conditioner.clone()),
        })
        .udp_bind(udp_bind)
        .webrtc_bind(webrtc_bind)
        .public_udp_addr(public_udp_addr)
        .public_webrtc_addr(public_webrtc_addr)
        .max_clients(usize::from(max_clients))
        .authentication(authentication)
        .build()?;
    egress.apply(&mut transport)?;
    let shared_transport = Arc::new(Mutex::new(transport));

    let bootstrap = Arc::new(BootstrapService::with_limits(
        BootstrapConfig {
            session_ttl: Duration::from_secs(120),
            public_udp_addr,
            public_webrtc_addr,
            public_http_base,
        },
        MonotonicClientIdAllocator::new(1),
        auth_policy,
        BootstrapLimits {
            pending_sessions: (usize::from(max_clients) * 4).clamp(32, 1024),
            active_sessions: usize::from(max_clients),
            replay_grants: 8192,
        },
    )?);

    let health = crate::InstanceHealth::default();
    let _http_thread = http_api::spawn_http_server_thread(
        Arc::clone(&bootstrap),
        Arc::clone(&shared_transport),
        health.clone(),
        http_bind,
        public_webrtc_addr,
        http_tls,
        cors_allowed_origins,
    )?;

    Ok(SharedNet {
        health,
        max_clients: usize::from(max_clients),
        packet_config,
        transport: shared_transport,
        bootstrap,
    })
}

fn validate_bootstrap_url(base: &str, require_confidential: bool) -> Result<(), std::io::Error> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bootstrap requires an absolute HTTP(S) URL; secure tokens require HTTPS, loopback, or explicit --allow-insecure-bootstrap-http",
        )
    };
    let uri: axum::http::Uri = base.parse().map_err(|_| invalid())?;
    let host = uri.host().ok_or_else(invalid)?;
    if uri.authority().is_none_or(|a| a.as_str().contains('@')) || uri.query().is_some() {
        return Err(invalid());
    }
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    match uri.scheme_str() {
        Some("https") => Ok(()),
        Some("http") if !require_confidential || loopback => Ok(()),
        _ => Err(invalid()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_config(extra: &[&str]) -> ServerConfig {
        use clap::Parser;
        #[derive(Parser)]
        struct Args {
            #[command(flatten)]
            net: ServerConfig,
        }
        let mut args = vec![
            "server",
            "--udp-bind",
            "127.0.0.1:0",
            "--webrtc-bind",
            "127.0.0.1:0",
        ];
        args.extend_from_slice(extra);
        Args::try_parse_from(args).unwrap().net
    }

    #[test]
    fn secure_egress_defaults_are_installed_before_updates_with_distinct_units() {
        let config = isolated_config(&[]);
        let resolved = config.egress.resolve(true).unwrap();
        let mut transport = MixedTransportBuilder::new(90)
            .udp_bind("127.0.0.1:0".parse().unwrap())
            .webrtc_bind("127.0.0.1:0".parse().unwrap())
            .authentication(ServerAuthentication::Secure {
                private_key: [7; 32],
            })
            .build()
            .unwrap();
        resolved.apply(&mut transport).unwrap();
        let bootstrap = BootstrapService::new(
            BootstrapConfig {
                session_ttl: Duration::from_secs(120),
                public_udp_addr: transport.udp().addresses()[0],
                public_webrtc_addr: transport.webrtc().addresses()[0],
                public_http_base: "http://127.0.0.1".into(),
            },
            MonotonicClientIdAllocator::new(1),
            UnsecureDevAuthPolicy,
        );
        let shared = SharedNet {
            health: Default::default(),
            max_clients: 8,
            packet_config: renet::PacketConfig::default(),
            transport: Arc::new(Mutex::new(transport)),
            bootstrap: Arc::new(bootstrap),
        };
        let stats = shared.egress_stats().unwrap();
        assert_eq!(stats.native_udp_ip.basis, EgressBasis::NativeUdpIp);
        assert_eq!(stats.webrtc_logical.basis, EgressBasis::EncryptedLogical);
        for stats in [stats.native_udp_ip, stats.webrtc_logical] {
            let limit = stats.config.unwrap();
            assert_eq!(limit.basis(), stats.basis);
            assert_eq!(limit.bytes_per_second(), 60_000);
            assert_eq!(limit.burst_bytes(), 6000);
            assert_eq!(limit.control_reserve_bytes(), 2400);
            assert_eq!(
                stats.sent_packets, 0,
                "configured before the first handshake"
            );
        }
        assert_eq!(
            shared.peer_egress_stats(1).unwrap(),
            crate::PeerEgressStats {
                native_udp_ip: None,
                webrtc_logical: None
            }
        );
    }

    #[test]
    fn explicit_development_can_remain_unpaced_and_overrides_apply_to_both_bases() {
        let defaults = isolated_config(&[]).egress.resolve(false).unwrap();
        assert!(defaults.native_udp_ip.is_none() && defaults.webrtc_logical.is_none());
        let args = isolated_config(&[
            "--egress-bytes-per-second",
            "32000",
            "--egress-burst-bytes",
            "8000",
            "--egress-control-reserve-bytes",
            "1000",
        ]);
        for secure in [false, true] {
            let configured = args.egress.resolve(secure).unwrap();
            for limit in [
                configured.native_udp_ip.unwrap(),
                configured.webrtc_logical.unwrap(),
            ] {
                assert_eq!(limit.bytes_per_second(), 32_000);
                assert_eq!(limit.burst_bytes(), 8000);
                assert_eq!(limit.control_reserve_bytes(), 1000);
            }
        }
    }

    #[test]
    fn invalid_egress_is_rejected_before_occupied_sockets_or_http_are_touched() {
        struct Refuse;
        impl SessionAdmission for Refuse {
            fn admit(
                &self,
                _: &renet_cross::SessionCreateRequest,
                _: Duration,
            ) -> Result<renet_cross::SessionGrant, renet_cross::BootstrapAuthError> {
                Err(renet_cross::BootstrapAuthError::InvalidRequest)
            }
        }
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        for (rate, burst, reserve) in [(0, 6000, 2400), (60_000, 1000, 256), (60_000, 6000, 5999)] {
            let mut config = isolated_config(&[]);
            config.udp_bind = udp.local_addr().unwrap();
            config.http_bind = Some(http.local_addr().unwrap());
            config.egress = ServerEgressArgs {
                bytes_per_second: Some(rate),
                burst_bytes: Some(burst),
                control_reserve_bytes: reserve,
            };
            let error = start_with_admission(config, 93, Refuse)
                .err()
                .expect("bad pacing must fail before binding");
            assert_eq!(
                error.downcast_ref::<EgressConfigError>(),
                Some(&EgressConfigError::InvalidBudget)
            );
        }
    }

    #[test]
    fn occupied_bootstrap_endpoint_fails_server_startup() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let config = isolated_config(&["--http-bind", &address]);
        let error = start_development(config, 91)
            .err()
            .expect("occupied bootstrap must fail startup");
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse
        );
    }

    #[test]
    fn missing_tls_identity_fails_before_server_is_returned() {
        let missing =
            std::env::temp_dir().join(format!("engine-missing-tls-{}", std::process::id()));
        let config = isolated_config(&[
            "--http-bind",
            "127.0.0.1:0",
            "--http-tls-cert",
            missing.to_str().unwrap(),
            "--http-tls-key",
            missing.to_str().unwrap(),
        ]);
        let error = start_development(config, 92)
            .err()
            .expect("invalid TLS must fail startup");
        assert!(error.to_string().contains("TLS configuration failed"));
    }

    #[test]
    fn secure_bootstrap_confidentiality_is_explicit() {
        for base in [
            "https://games.example.test",
            "http://127.0.0.1:18082",
            "http://[::1]:8080",
            "http://localhost:8080",
        ] {
            assert!(validate_bootstrap_url(base, true).is_ok(), "{base}");
        }
        for base in [
            "http://192.168.0.2:8080",
            "http://127.0.0.1.evil.test",
            "https://user:password@games.example.test",
            "ftp://games.example.test",
            "games.example.test",
            "https://games.example.test/?token=secret",
        ] {
            assert!(validate_bootstrap_url(base, true).is_err(), "{base}");
        }
        assert!(validate_bootstrap_url("http://192.168.0.2:8080", false).is_ok());
    }
}

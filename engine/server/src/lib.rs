//! Game-neutral HTTP bootstrap, transport construction, and server driver.
use axum::http::HeaderValue;
use http_api::HttpTlsConfig;
use renet_cross::{
    BootstrapConfig, BootstrapService, MixedTransportBuilder, MonotonicClientIdAllocator,
    ServerAuthentication, UnsecureDevAuthPolicy,
};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
pub mod conditioner;
pub mod http_api;
mod net;
pub mod runtime;
pub use net::SharedNet;
#[derive(Debug, Clone, clap::Args)]
pub struct ServerConfig {
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

pub fn start(
    config: ServerConfig,
    protocol_id: u64,
) -> Result<SharedNet, Box<dyn std::error::Error>> {
    let ServerConfig {
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
    let shared_transport = Arc::new(Mutex::new(
        MixedTransportBuilder::new(protocol_id)
            .transport_config(renet_cross::ServerTransportConfig {
                conditioner: Some(conditioner.clone()),
            })
            .udp_bind(udp_bind)
            .webrtc_bind(webrtc_bind)
            .public_udp_addr(public_udp_addr)
            .public_webrtc_addr(public_webrtc_addr)
            .max_clients(usize::from(max_clients))
            .authentication(ServerAuthentication::Unsecure)
            .build()?,
    ));

    let bootstrap = Arc::new(BootstrapService::new(
        BootstrapConfig {
            session_ttl: Duration::from_secs(120),
            public_udp_addr,
            public_webrtc_addr,
            public_http_base,
        },
        MonotonicClientIdAllocator::new(1),
        UnsecureDevAuthPolicy,
    ));

    let _http_thread = http_api::spawn_http_server_thread(
        Arc::clone(&bootstrap),
        Arc::clone(&shared_transport),
        http_bind,
        public_webrtc_addr,
        http_tls,
        cors_allowed_origins,
    );

    Ok(SharedNet {
        max_clients: usize::from(max_clients),
        transport: shared_transport,
        bootstrap,
    })
}

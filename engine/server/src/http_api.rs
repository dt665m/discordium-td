use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::http::HeaderValue;
use axum_server::tls_rustls::RustlsConfig;

use renet_cross::{
    BootstrapAxumState, DefaultBootstrapService, MixedServerTransport, SdpHttpHookConfig,
    bootstrap_router,
};
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
pub struct HttpTlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

pub fn spawn_http_server_thread(
    bootstrap: Arc<DefaultBootstrapService>,
    transport: Arc<Mutex<MixedServerTransport>>,
    bind_addr: SocketAddr,
    webrtc_candidate_addr: SocketAddr,
    http_tls: Option<HttpTlsConfig>,
    cors_allowed_origins: Option<Vec<HeaderValue>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("engine-http".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    log::error!("failed to build tokio runtime for HTTP server: {err}");
                    return;
                }
            };

            runtime.block_on(async move {
                let app_state = BootstrapAxumState {
                    bootstrap,
                    transport,
                    hook_config: SdpHttpHookConfig::new(webrtc_candidate_addr),
                };

                let cors = if let Some(origins) = cors_allowed_origins {
                    let origin_list = origins
                        .iter()
                        .map(|origin| origin.to_str().unwrap_or("<non-utf8-origin>"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    log::info!("HTTP CORS allowlist enabled: {origin_list}");
                    CorsLayer::new()
                        .allow_origin(origins)
                        .allow_methods(Any)
                        .allow_headers(Any)
                } else {
                    log::info!("HTTP CORS allowlist not set; allowing any origin");
                    CorsLayer::new()
                        .allow_origin(Any)
                        .allow_methods(Any)
                        .allow_headers(Any)
                };
                let app = bootstrap_router(app_state).layer(cors);

                if let Some(tls) = http_tls {
                    let tls_config = match RustlsConfig::from_pem_file(
                        tls.cert_path.clone(),
                        tls.key_path.clone(),
                    )
                    .await
                    {
                        Ok(config) => config,
                        Err(err) => {
                            log::error!(
                                "failed to load TLS cert/key cert_path={} key_path={}: {err}",
                                tls.cert_path.display(),
                                tls.key_path.display()
                            );
                            return;
                        }
                    };

                    log::info!("bootstrap/signaling server listening on https://{bind_addr}");
                    if let Err(err) = axum_server::bind_rustls(bind_addr, tls_config)
                        .serve(app.into_make_service())
                        .await
                    {
                        log::error!("HTTPS server exited with error: {err}");
                    }
                } else {
                    let listener = match tokio::net::TcpListener::bind(bind_addr).await {
                        Ok(listener) => listener,
                        Err(err) => {
                            log::error!(
                                "failed to bind HTTP bootstrap listener on {bind_addr}: {err}"
                            );
                            return;
                        }
                    };

                    log::info!("bootstrap/signaling server listening on http://{bind_addr}");
                    if let Err(err) = axum::serve(listener, app).await {
                        log::error!("HTTP server exited with error: {err}");
                    }
                }
            });
        })
        .expect("failed to spawn HTTP server thread")
}

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use renet_cross::{
    BootstrapAxumState, DefaultBootstrapService, MixedServerTransport, SdpHttpHookConfig,
    bootstrap_router,
};
use tower_http::cors::{Any, CorsLayer};

pub fn spawn_http_server_thread(
    bootstrap: Arc<DefaultBootstrapService>,
    transport: Arc<Mutex<MixedServerTransport>>,
    bind_addr: SocketAddr,
    webrtc_candidate_addr: SocketAddr,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("discordium-http".to_owned())
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

                let app = bootstrap_router(app_state).layer(
                    CorsLayer::new()
                        .allow_origin(Any)
                        .allow_methods(Any)
                        .allow_headers(Any),
                );
                let listener = match tokio::net::TcpListener::bind(bind_addr).await {
                    Ok(listener) => listener,
                    Err(err) => {
                        log::error!("failed to bind HTTP bootstrap listener on {bind_addr}: {err}");
                        return;
                    }
                };

                log::info!("bootstrap/signaling server listening on http://{bind_addr}");
                if let Err(err) = axum::serve(listener, app).await {
                    log::error!("HTTP server exited with error: {err}");
                }
            });
        })
        .expect("failed to spawn HTTP server thread")
}

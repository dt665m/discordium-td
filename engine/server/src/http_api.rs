use std::{
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use axum_server::tls_rustls::RustlsConfig;

use renet_cross::{
    BootstrapAxumState, BootstrapService, MixedServerTransport, SdpHttpHookConfig,
    SessionAuthPolicy, SessionIdAllocator, bootstrap_router,
};
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
pub struct HttpTlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

pub fn spawn_http_server_thread<A, P>(
    bootstrap: Arc<BootstrapService<A, P>>,
    transport: Arc<Mutex<MixedServerTransport>>,
    health: crate::InstanceHealth,
    bind_addr: SocketAddr,
    webrtc_candidate_addr: SocketAddr,
    http_tls: Option<HttpTlsConfig>,
    cors_allowed_origins: Option<Vec<HeaderValue>>,
) -> std::io::Result<std::thread::JoinHandle<()>>
where
    A: SessionIdAllocator + Send + Sync + 'static,
    P: SessionAuthPolicy + Send + Sync + 'static,
{
    // Bind synchronously: a headless server must not report successful startup
    // when no client can reach its bootstrap endpoint.
    let listener = TcpListener::bind(bind_addr)?;
    listener.set_nonblocking(true)?;
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("engine-http".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = ready_tx.send(Err(format!("HTTP runtime startup failed: {err}")));
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
                let readiness = Router::new()
                    .route("/readyz", get(readiness))
                    .with_state(health.clone());
                let app = bootstrap_router(app_state)
                    .layer(middleware::from_fn_with_state(health, admission_readiness))
                    .merge(readiness)
                    .layer(cors);

                if let Some(tls) = http_tls {
                    let tls_config = match RustlsConfig::from_pem_file(
                        tls.cert_path.clone(),
                        tls.key_path.clone(),
                    )
                    .await
                    {
                        Ok(config) => config,
                        Err(err) => {
                            let _ =
                                ready_tx.send(Err(format!("HTTP TLS configuration failed: {err}")));
                            return;
                        }
                    };

                    if ready_tx.send(Ok(())).is_err() {
                        return;
                    }
                    log::info!("bootstrap/signaling server listening on https://{bind_addr}");
                    if let Err(err) = axum_server::from_tcp_rustls(listener, tls_config)
                        .serve(app.into_make_service())
                        .await
                    {
                        log::error!("HTTPS server exited with error: {err}");
                    }
                } else {
                    let listener = match tokio::net::TcpListener::from_std(listener) {
                        Ok(listener) => listener,
                        Err(err) => {
                            let _ =
                                ready_tx.send(Err(format!("HTTP listener startup failed: {err}")));
                            return;
                        }
                    };

                    if ready_tx.send(Ok(())).is_err() {
                        return;
                    }
                    log::info!("bootstrap/signaling server listening on http://{bind_addr}");
                    if let Err(err) = axum::serve(listener, app).await {
                        log::error!("HTTP server exited with error: {err}");
                    }
                }
            });
        })?;
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(reason)) => Err(std::io::Error::other(reason)),
        Err(error) => Err(std::io::Error::other(format!(
            "HTTP startup did not complete: {error}"
        ))),
    }
}

async fn readiness(State(health): State<crate::InstanceHealth>) -> Response {
    let report = health.report();
    let status = if report.ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(report)).into_response()
}

async fn admission_readiness(
    State(health): State<crate::InstanceHealth>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == "/api/session/new" {
        let report = health.report();
        if !report.ready() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(axum::http::header::RETRY_AFTER, "1")],
                Json(report),
            )
                .into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::post};
    use engine_net::{
        clock::TickHealth,
        types::{ServerTick, TickRate},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    #[tokio::test]
    async fn readiness_sheds_new_bootstrap_work_and_keeps_liveness_and_signaling_available() {
        let health = crate::InstanceHealth::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let admissions = calls.clone();
        let routes = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route("/api/webrtc/offer/1", post(|| async { "offer" }))
            .route(
                "/api/session/new",
                post(move || {
                    let calls = admissions.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        "admitted"
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(
                health.clone(),
                admission_readiness,
            ))
            .merge(
                Router::new()
                    .route("/readyz", get(readiness))
                    .with_state(health.clone()),
            );
        let request = |method: &str, path: &str| {
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap()
        };
        let starting = routes
            .clone()
            .oneshot(request("POST", "/api/session/new"))
            .await
            .unwrap();
        assert_eq!(starting.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(starting.headers().get("retry-after").unwrap(), "1");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        for (method, path) in [("GET", "/healthz"), ("POST", "/api/webrtc/offer/1")] {
            assert_eq!(
                routes
                    .clone()
                    .oneshot(request(method, path))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
        health.bind(TickRate::new(1).unwrap(), 4);
        health.observe(
            Duration::ZERO,
            std::time::Instant::now(),
            TickHealth::default(),
        );
        assert_eq!(
            routes
                .clone()
                .oneshot(request("POST", "/api/session/new"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        health.observe(
            Duration::from_secs(60),
            std::time::Instant::now(),
            TickHealth::default(),
        );
        let overloaded = routes
            .clone()
            .oneshot(request("GET", "/readyz"))
            .await
            .unwrap();
        assert_eq!(overloaded.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(overloaded.into_body(), 1024)
            .await
            .unwrap();
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("\"status\":\"overloaded\"")
        );
        assert!(health.report().debt_ticks >= 60);
        assert_eq!(
            routes
                .clone()
                .oneshot(request("POST", "/api/session/new"))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        health.observe(
            Duration::from_secs(60),
            std::time::Instant::now(),
            TickHealth {
                committed: ServerTick(60),
                ..Default::default()
            },
        );
        assert_eq!(
            routes
                .clone()
                .oneshot(request("GET", "/readyz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        health.stop();
        assert_eq!(
            routes
                .oneshot(request("GET", "/readyz"))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

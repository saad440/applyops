//! ApplyOps — a local-first, single-user opportunity and application tracker.
//!
//! Starts an HTTP server bound to the loopback interface by default; the
//! `APPLYOPS_BIND_ADDR` environment variable overrides the listen address.
//! The routes defined here are a minimal development scaffold; application
//! features build on the same request path: browser → router → handler → response.

use std::time::Instant;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::net::TcpListener;

/// Default listen address: loopback only, for local single-user use.
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";

/// Environment variable that overrides the listen address.
const BIND_ADDR_ENV: &str = "APPLYOPS_BIND_ADDR";

/// Resolves the listen address from an optional environment value.
fn resolve_bind_addr(env_value: Option<String>) -> String {
    env_value.unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string())
}

/// Returns `true` when the address targets the loopback interface.
fn is_loopback_addr(addr: &str) -> bool {
    let host = addr.rsplit_once(':').map_or(addr, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

#[tokio::main]
async fn main() {
    // Log level defaults to `info`; `RUST_LOG` overrides it.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let bind_addr = resolve_bind_addr(std::env::var(BIND_ADDR_ENV).ok());

    if !is_loopback_addr(&bind_addr) {
        tracing::warn!(%bind_addr, "binding to a non-loopback address; ApplyOps has no authentication");
    }

    // Middleware applies to the routes registered before it.
    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(health))
        .layer(middleware::from_fn(log_requests));

    let listener = TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|err| panic!("could not bind {bind_addr}: {err}"));

    tracing::info!("ApplyOps listening on http://{bind_addr}");

    axum::serve(listener, app).await.expect("server error");
}

/// `GET /` — index route.
async fn root() -> impl IntoResponse {
    "ApplyOps is running"
}

/// `GET /healthz` — health check for scripted verification.
async fn health() -> impl IntoResponse {
    (StatusCode::OK, "OK: ApplyOps dev server")
}

/// Logs method, path, status code and duration for every request.
async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let started = Instant::now();

    let response = next.run(req).await;

    tracing::info!(
        %method,
        %path,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "request handled"
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn root_responds_with_ok() {
        let response = root().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_responds_with_ok() {
        let response = health().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn bind_addr_defaults_to_loopback() {
        assert_eq!(resolve_bind_addr(None), "127.0.0.1:8080");
    }

    #[test]
    fn bind_addr_env_overrides_default() {
        assert_eq!(
            resolve_bind_addr(Some("0.0.0.0:9000".to_string())),
            "0.0.0.0:9000"
        );
    }

    #[test]
    fn loopback_addresses_are_detected() {
        assert!(is_loopback_addr("127.0.0.1:8080"));
        assert!(is_loopback_addr("localhost:8080"));
        assert!(is_loopback_addr("[::1]:9000"));
    }

    #[test]
    fn non_loopback_addresses_are_detected() {
        assert!(!is_loopback_addr("0.0.0.0:8080"));
        assert!(!is_loopback_addr("192.168.1.10:8080"));
    }
}

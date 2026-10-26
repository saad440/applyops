//! ApplyOps — a local-first, single-user opportunity and application tracker.
//!
//! Starts an HTTP server bound to the loopback interface. The routes defined
//! here are a minimal development scaffold; application features build on the
//! same request path: browser → router → handler → response.

use std::time::Instant;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::net::TcpListener;

/// Loopback only: the server is meant for local, single-user use.
const BIND_ADDR: &str = "127.0.0.1:8080";

#[tokio::main]
async fn main() {
    // Log level defaults to `info`; `RUST_LOG` overrides it.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    // Middleware applies to the routes registered before it.
    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(health))
        .layer(middleware::from_fn(log_requests));

    let listener = TcpListener::bind(BIND_ADDR)
        .await
        .unwrap_or_else(|err| panic!("could not bind {BIND_ADDR}: {err}"));

    tracing::info!("ApplyOps listening on http://{BIND_ADDR}");

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
}

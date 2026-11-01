//! ApplyOps — a local-first, single-user opportunity and application tracker.
//!
//! Starts an HTTP server bound to the loopback interface by default; the
//! `APPLYOPS_BIND_ADDR` environment variable overrides the listen address and
//! `APPLYOPS_DB_PATH` the SQLite database location. Routes serve
//! Askama-rendered HTML and static assets, backed by SQLite via SQLx.

mod db;

mod web;

use std::time::Instant;

use askama::Template;
use axum::Router;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

/// Default listen address: loopback only, for local single-user use.
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";

/// Environment variable that overrides the listen address.
const BIND_ADDR_ENV: &str = "APPLYOPS_BIND_ADDR";

/// Default database file path, relative to the working directory.
const DEFAULT_DB_PATH: &str = "data/applyops.db";

/// Environment variable that overrides the database file path.
const DB_PATH_ENV: &str = "APPLYOPS_DB_PATH";

/// Environment variable that overrides the display time zone, as an IANA
/// name such as `Europe/Berlin`.
const TIME_ZONE_ENV: &str = "APPLYOPS_TIME_ZONE";

/// Default display time zone for countdowns and urgency labels.
const DEFAULT_TIME_ZONE: &str = "Europe/Berlin";

/// Directory of local static assets, resolved relative to the working directory.
const STATIC_DIR: &str = "static";

/// Stand-in user id for the local single-user phase; real user management
/// is not implemented yet. The `user_id` column exists from the
/// first migration so introducing accounts later does not require rewriting
/// stored rows.
const DEFAULT_USER_ID: i64 = 1;

/// Resolves the listen address from an optional environment value.
fn resolve_bind_addr(env_value: Option<String>) -> String {
    env_value.unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string())
}

/// Resolves the database file path from an optional environment value.
fn resolve_db_path(env_value: Option<String>) -> String {
    env_value.unwrap_or_else(|| DEFAULT_DB_PATH.to_string())
}

/// Returns `true` when the address targets the loopback interface.
fn is_loopback_addr(addr: &str) -> bool {
    let host = addr.rsplit_once(':').map_or(addr, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// Resolves the display time zone from an optional environment value. An
/// unloadable name — unknown identifier or missing system time zone
/// database — falls back to UTC with a warning, keeping the app usable.
fn resolve_time_zone(env_value: Option<String>) -> jiff::tz::TimeZone {
    let name = env_value.unwrap_or_else(|| DEFAULT_TIME_ZONE.to_string());
    jiff::tz::TimeZone::get(&name).unwrap_or_else(|err| {
        tracing::warn!(time_zone = %name, %err, "could not load time zone; falling back to UTC");
        jiff::tz::TimeZone::UTC
    })
}

/// Shared application state handed to handlers.
#[derive(Clone)]
struct AppState {
    db: SqlitePool,
    /// The time zone countdowns and urgency labels are measured against.
    time_zone: jiff::tz::TimeZone,
}

/// Wraps an Askama template so it can be returned directly from handlers.
struct HtmlTemplate<T>(T);

impl<T> IntoResponse for HtmlTemplate<T>
where
    T: Template,
{
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(body) => {
                ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
            }
            Err(err) => {
                tracing::error!(%err, "failed to render template");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "template rendering failed",
                )
                    .into_response()
            }
        }
    }
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

    let db_path = resolve_db_path(std::env::var(DB_PATH_ENV).ok());
    if let Some(parent) = std::path::Path::new(&db_path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("could not create data directory for {db_path}: {err}"));
    }

    let pool = db::connect(std::path::Path::new(&db_path))
        .await
        .unwrap_or_else(|err| panic!("could not open database {db_path}: {err}"));
    db::run_migrations(&pool)
        .await
        .unwrap_or_else(|err| panic!("could not apply migrations: {err}"));

    let seeded_streams =
        db::streams::seed_streams_if_empty(&pool, DEFAULT_USER_ID, &db::streams::DEFAULT_STREAMS)
            .await
            .unwrap_or_else(|err| panic!("could not seed default streams: {err}"));
    if seeded_streams > 0 {
        tracing::info!(seeded_streams, "seeded default streams");
    }

    let seeded_authorities = db::authorities::seed_authorities_if_empty(
        &pool,
        DEFAULT_USER_ID,
        &db::authorities::DEFAULT_AUTHORITIES,
    )
    .await
    .unwrap_or_else(|err| panic!("could not seed default authority labels: {err}"));
    if seeded_authorities > 0 {
        tracing::info!(seeded_authorities, "seeded default authority labels");
    }

    tracing::info!(%db_path, "database ready");

    let time_zone = resolve_time_zone(std::env::var(TIME_ZONE_ENV).ok());
    tracing::info!("display time zone set");

    // Middleware applies to the routes registered before it.
    let app = Router::new()
        .route("/", get(web::root))
        .route("/dashboard", get(web::dashboard))
        .route("/healthz", get(health))
        .route("/diagnostics", get(web::diagnostics))
        .route("/diagnostics/notes", post(web::create_note))
        .route("/opportunities/new", get(web::new_opportunity_form))
        .route("/opportunities", post(web::create_opportunity))
        .route("/opportunities/{id}", get(web::opportunity_details))
        .route(
            "/opportunities/{id}/edit",
            get(web::edit_opportunity_form).post(web::update_opportunity),
        )
        .route(
            "/opportunities/{id}/applications",
            post(web::create_application),
        )
        .route("/applications/{id}/prepare", post(web::prepare_application))
        .route(
            "/applications/{id}/unprepare",
            post(web::unprepare_application),
        )
        .route(
            "/applications/{id}/submit",
            get(web::submit_application_form).post(web::record_application_submission),
        )
        .route(
            "/applications/{id}/correct",
            get(web::correct_application_form).post(web::correct_application),
        )
        .route(
            "/applications/{id}/withdraw",
            get(web::withdraw_application_form).post(web::withdraw_application),
        )
        .route(
            "/applications/{id}/outcome",
            get(web::outcome_form).post(web::record_outcome),
        )
        .route(
            "/applications/{id}/report",
            get(web::report_form).post(web::record_report),
        )
        .route("/reports", get(web::report_backlog))
        .route(
            "/applications/{id}/actions",
            post(web::add_application_action),
        )
        .route(
            "/actions/{id}/complete",
            post(web::complete_application_action),
        )
        .route("/actions/{id}/reopen", post(web::reopen_application_action))
        .route("/actions/{id}/delete", post(web::delete_application_action))
        .nest_service("/static", ServeDir::new(STATIC_DIR))
        .with_state(AppState {
            db: pool,
            time_zone,
        })
        .layer(middleware::from_fn(log_requests));

    let listener = TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|err| panic!("could not bind {bind_addr}: {err}"));

    tracing::info!("ApplyOps listening on http://{bind_addr}");

    axum::serve(listener, app).await.expect("server error");
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
mod tests;

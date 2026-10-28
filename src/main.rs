//! ApplyOps — a local-first, single-user opportunity and application tracker.
//!
//! Starts an HTTP server bound to the loopback interface by default; the
//! `APPLYOPS_BIND_ADDR` environment variable overrides the listen address and
//! `APPLYOPS_DB_PATH` the SQLite database location. Routes serve
//! Askama-rendered HTML and static assets, backed by SQLite via SQLx.

mod db;

use std::path::Path;
use std::time::Instant;

use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
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

/// Directory of local static assets, resolved relative to the working directory.
const STATIC_DIR: &str = "static";

/// Stand-in user id for the local single-user phase; a later milestone
/// introduces real user management. The `user_id` column exists from the
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

/// Shared application state handed to handlers.
#[derive(Clone)]
struct AppState {
    db: SqlitePool,
}

/// Index page, rendered from `templates/index.html` at compile time.
#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    note_count: i64,
    notes: Vec<db::DiagnosticNote>,
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
    if let Some(parent) = Path::new(&db_path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("could not create data directory for {db_path}: {err}"));
    }

    let pool = db::connect(Path::new(&db_path))
        .await
        .unwrap_or_else(|err| panic!("could not open database {db_path}: {err}"));
    db::run_migrations(&pool)
        .await
        .unwrap_or_else(|err| panic!("could not apply migrations: {err}"));
    tracing::info!(%db_path, "database ready");

    // Middleware applies to the routes registered before it.
    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(health))
        .route("/diagnostics/notes", post(create_note))
        .nest_service("/static", ServeDir::new(STATIC_DIR))
        .with_state(AppState { db: pool })
        .layer(middleware::from_fn(log_requests));

    let listener = TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|err| panic!("could not bind {bind_addr}: {err}"));

    tracing::info!("ApplyOps listening on http://{bind_addr}");

    axum::serve(listener, app).await.expect("server error");
}

/// `GET /` — index page with the current diagnostic records.
async fn root(State(state): State<AppState>) -> impl IntoResponse {
    let count = match db::count_notes(&state.db).await {
        Ok(count) => count,
        Err(err) => {
            tracing::error!(%err, "failed to count diagnostic notes");
            0
        }
    };

    let notes = match db::recent_notes(&state.db, 10).await {
        Ok(notes) => notes,
        Err(err) => {
            tracing::error!(%err, "failed to load diagnostic notes");
            Vec::new()
        }
    };

    HtmlTemplate(IndexTemplate {
        note_count: count,
        notes,
    })
}

/// `POST /diagnostics/notes` — stores a diagnostic note, then redirects back
/// to the index page.
async fn create_note(
    State(state): State<AppState>,
    Form(form): Form<Vec<(String, String)>>,
) -> impl IntoResponse {
    let note = form
        .iter()
        .find(|(field, _)| field == "note")
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default();

    if !note.is_empty()
        && let Err(err) = db::insert_note(&state.db, DEFAULT_USER_ID, &note).await
    {
        tracing::error!(%err, "failed to store diagnostic note");
    }

    Redirect::to("/")
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

    /// Application state over a fresh in-memory database with migrations run.
    async fn test_state() -> AppState {
        let pool = db::connect_in_memory().await.expect("in-memory pool");
        db::run_migrations(&pool).await.expect("migrations run");
        AppState { db: pool }
    }

    #[tokio::test]
    async fn root_responds_with_html() {
        let state = test_state().await;
        let response = root(State(state)).await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&"text/html; charset=utf-8".parse().unwrap())
        );
    }

    #[tokio::test]
    async fn stored_note_appears_on_index_page() {
        let state = test_state().await;
        db::insert_note(&state.db, DEFAULT_USER_ID, "round trip")
            .await
            .unwrap();

        let response = root(State(state)).await.into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("round trip"));
        assert!(body.contains("Persistent records"));
    }

    #[tokio::test]
    async fn diagnostic_notes_round_trip() {
        let pool = db::connect_in_memory().await.expect("in-memory pool");
        db::run_migrations(&pool).await.expect("migrations run");
        assert_eq!(db::count_notes(&pool).await.unwrap(), 0);

        db::insert_note(&pool, DEFAULT_USER_ID, "smoke note")
            .await
            .unwrap();

        let notes = db::recent_notes(&pool, 10).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].note, "smoke note");
        assert_eq!(notes[0].user_id, DEFAULT_USER_ID);
        assert_eq!(db::count_notes(&pool).await.unwrap(), 1);
    }

    #[test]
    fn index_template_links_stylesheet() {
        let template = IndexTemplate {
            note_count: 0,
            notes: Vec::new(),
        };
        let html = template.render().expect("index template renders");
        assert!(html.contains(r#"href="/static/css/app.css""#));
        assert!(html.contains("ApplyOps"));
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
    fn db_path_defaults_to_data_dir() {
        assert_eq!(resolve_db_path(None), "data/applyops.db");
    }

    #[test]
    fn db_path_env_overrides_default() {
        assert_eq!(
            resolve_db_path(Some("/tmp/other.db".to_string())),
            "/tmp/other.db"
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

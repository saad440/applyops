//! ApplyOps — a local-first, single-user opportunity and application tracker.
//!
//! Starts an HTTP server bound to the loopback interface by default; the
//! `APPLYOPS_BIND_ADDR` environment variable overrides the listen address and
//! `APPLYOPS_DB_PATH` the SQLite database location. Routes serve
//! Askama-rendered HTML and static assets, backed by SQLite via SQLx.

mod db;

use std::time::Instant;

use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Path, Query, Request, State};
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

/// Index page: the opportunity list, optionally filtered by a search term,
/// rendered from `templates/index.html` at compile time.
#[derive(Template)]
#[template(path = "index.html")]
struct OpportunityListTemplate {
    search_query: String,
    opportunities: Vec<db::opportunities::Opportunity>,
    list_count: i64,
}

/// Diagnostics page, rendered from `templates/diagnostics.html` at compile time.
#[derive(Template)]
#[template(path = "diagnostics.html")]
struct DiagnosticsTemplate {
    note_count: i64,
    notes: Vec<db::DiagnosticNote>,
}

/// View model of the opportunity form, used for both creating and editing:
/// the entered values, the known stream labels for suggestions, and
/// per-field validation errors.
#[derive(Template)]
#[template(path = "opportunity_form.html")]
struct OpportunityFormTemplate {
    form_heading: String,
    form_action: String,
    submit_label: String,
    streams: Vec<String>,
    values: OpportunityFormValues,
    organization_error: Option<String>,
    title_error: Option<String>,
    stream_error: Option<String>,
    priority_error: Option<String>,
    stage_error: Option<String>,
    deadline_date_error: Option<String>,
    source_url_error: Option<String>,
    form_error: Option<String>,
}

/// The text values currently entered in the opportunity form, kept for
/// redisplay when validation fails.
#[derive(Default)]
struct OpportunityFormValues {
    organization: String,
    title: String,
    stream: String,
    city: String,
    country: String,
    priority: String,
    deadline_date: String,
    source_url: String,
    notes: String,
    stage: String,
}

impl OpportunityFormTemplate {
    /// An empty form with stream suggestions but no values or errors.
    fn fresh(
        form_heading: String,
        form_action: String,
        submit_label: String,
        streams: Vec<String>,
    ) -> Self {
        Self {
            form_heading,
            form_action,
            submit_label,
            streams,
            values: OpportunityFormValues::default(),
            organization_error: None,
            title_error: None,
            stream_error: None,
            priority_error: None,
            stage_error: None,
            deadline_date_error: None,
            source_url_error: None,
            form_error: None,
        }
    }
}

/// Rendered details of one stored opportunity. Display labels are prepared
/// here so the template stays free of formatting logic.
#[derive(Template)]
#[template(path = "opportunity_details.html")]
struct OpportunityDetailsTemplate {
    opportunity_id: i64,
    title_heading: String,
    external_url: Option<String>,
    stream: String,
    priority_label: String,
    location_display: Option<String>,
    deadline_display: String,
    notes: Option<String>,
    stage: String,
    created_at: String,
}

impl OpportunityDetailsTemplate {
    fn from_opportunity(opportunity: &db::opportunities::Opportunity) -> Self {
        let location_display = match (&opportunity.city, &opportunity.country) {
            (Some(city), Some(country)) => Some(format!("{city}, {country}")),
            (Some(place), None) | (None, Some(place)) => Some(place.clone()),
            (None, None) => None,
        };

        Self {
            opportunity_id: opportunity.id,
            title_heading: format!("{} — {}", opportunity.organization, opportunity.title),
            external_url: opportunity.source_url.clone(),
            stream: opportunity.stream.clone(),
            priority_label: opportunity.priority.to_uppercase(),
            location_display,
            deadline_display: opportunity
                .deadline_date
                .as_deref()
                .map(format_date_human)
                .unwrap_or_else(|| "No deadline recorded".to_string()),
            notes: opportunity.notes.clone(),
            stage: opportunity.stage.clone(),
            created_at: opportunity.created_at.clone(),
        }
    }
}

/// Formats an ISO `YYYY-MM-DD` date as e.g. `15 Jan 2027` for display.
/// Values that are not ten characters long are passed through unchanged.
fn format_date_human(iso_date: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    if iso_date.len() != 10 {
        return iso_date.to_string();
    }

    let year = &iso_date[0..4];
    let day = iso_date[8..10].trim_start_matches('0');
    let month: usize = iso_date[5..7].parse().unwrap_or(0);
    match month {
        1..=12 => format!("{day} {} {year}", MONTHS[month - 1]),
        _ => iso_date.to_string(),
    }
}

/// Raw text fields of a submitted opportunity form. Every value arrives as
/// text and is trimmed; optional fields become `None` when left empty.
struct OpportunityFormInput {
    organization: String,
    title: String,
    stream: String,
    city: String,
    country: String,
    priority: String,
    deadline_date: String,
    source_url: String,
    notes: String,
    stage: String,
}

impl OpportunityFormInput {
    /// Collects the named fields from raw form pairs.
    fn from_pairs(pairs: &[(String, String)]) -> Self {
        fn field(pairs: &[(String, String)], name: &str) -> String {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.trim().to_string())
                .unwrap_or_default()
        }

        Self {
            organization: field(pairs, "organization"),
            title: field(pairs, "title"),
            stream: field(pairs, "stream"),
            city: field(pairs, "city"),
            country: field(pairs, "country"),
            priority: field(pairs, "priority"),
            deadline_date: field(pairs, "deadline_date"),
            source_url: field(pairs, "source_url"),
            notes: field(pairs, "notes"),
            stage: field(pairs, "stage"),
        }
    }

    /// Converts the submitted text into the validated data-layer input.
    fn to_new_opportunity(&self, user_id: i64) -> db::opportunities::NewOpportunity {
        fn optional(value: &str) -> Option<String> {
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }

        db::opportunities::NewOpportunity {
            user_id,
            organization: self.organization.clone(),
            title: self.title.clone(),
            stream: self.stream.clone(),
            city: optional(&self.city),
            country: optional(&self.country),
            priority: self.priority.clone(),
            deadline_date: optional(&self.deadline_date),
            source_url: optional(&self.source_url),
            notes: optional(&self.notes),
            stage: self.stage.clone(),
        }
    }
}

/// Fills the redisplay values of a form from submitted input.
fn fill_form_values(template: &mut OpportunityFormTemplate, input: &OpportunityFormInput) {
    template.values = OpportunityFormValues {
        organization: input.organization.clone(),
        title: input.title.clone(),
        stream: input.stream.clone(),
        city: input.city.clone(),
        country: input.country.clone(),
        priority: input.priority.clone(),
        deadline_date: input.deadline_date.clone(),
        source_url: input.source_url.clone(),
        notes: input.notes.clone(),
        stage: input.stage.clone(),
    };
}

/// Converts a stored opportunity into redisplay values for the edit form.
fn form_values_from(opportunity: &db::opportunities::Opportunity) -> OpportunityFormValues {
    fn text(value: &Option<String>) -> String {
        value.as_deref().unwrap_or_default().to_string()
    }

    OpportunityFormValues {
        organization: opportunity.organization.clone(),
        title: opportunity.title.clone(),
        stream: opportunity.stream.clone(),
        city: text(&opportunity.city),
        country: text(&opportunity.country),
        priority: opportunity.priority.clone(),
        deadline_date: text(&opportunity.deadline_date),
        source_url: text(&opportunity.source_url),
        notes: text(&opportunity.notes),
        stage: opportunity.stage.clone(),
    }
}

/// Places a validation error on the form field it belongs to.
fn apply_validation_error(
    template: &mut OpportunityFormTemplate,
    err: &db::opportunities::OpportunityValidationError,
) {
    let message = Some(err.reason.to_string());
    match err.field {
        "organization" => template.organization_error = message,
        "title" => template.title_error = message,
        "stream" => template.stream_error = message,
        "priority" => template.priority_error = message,
        "stage" => template.stage_error = message,
        "deadline_date" => template.deadline_date_error = message,
        "source_url" => template.source_url_error = message,
        _ => template.form_error = Some(format!("{} {}", err.field, err.reason)),
    }
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

    tracing::info!(%db_path, "database ready");

    // Middleware applies to the routes registered before it.
    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(health))
        .route("/diagnostics", get(diagnostics))
        .route("/diagnostics/notes", post(create_note))
        .route("/opportunities/new", get(new_opportunity_form))
        .route("/opportunities", post(create_opportunity))
        .route("/opportunities/{id}", get(opportunity_details))
        .route(
            "/opportunities/{id}/edit",
            get(edit_opportunity_form).post(update_opportunity),
        )
        .nest_service("/static", ServeDir::new(STATIC_DIR))
        .with_state(AppState { db: pool })
        .layer(middleware::from_fn(log_requests));

    let listener = TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|err| panic!("could not bind {bind_addr}: {err}"));

    tracing::info!("ApplyOps listening on http://{bind_addr}");

    axum::serve(listener, app).await.expect("server error");
}

/// `GET /` — the opportunity list, optionally filtered by `?q=`.
async fn root(
    State(state): State<AppState>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let search_query = pairs
        .iter()
        .find(|(key, _)| key == "q")
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default();

    let opportunities = if search_query.is_empty() {
        db::opportunities::list_opportunities(&state.db, 100).await
    } else {
        db::opportunities::search_opportunities(&state.db, &search_query, 100).await
    };

    let opportunities = match opportunities {
        Ok(opportunities) => opportunities,
        Err(err) => {
            tracing::error!(%err, "failed to load opportunities");
            Vec::new()
        }
    };

    HtmlTemplate(OpportunityListTemplate {
        list_count: opportunities.len() as i64,
        search_query,
        opportunities,
    })
    .into_response()
}

/// `GET /diagnostics` — the persistence smoke-test notes page.
async fn diagnostics(State(state): State<AppState>) -> Response {
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

    HtmlTemplate(DiagnosticsTemplate {
        note_count: count,
        notes,
    })
    .into_response()
}

/// `POST /diagnostics/notes` — stores a diagnostic note, then redirects back
/// to the diagnostics page.
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

    Redirect::to("/diagnostics")
}

/// Loads the known stream labels for form suggestions, or an empty list when
/// they cannot be read; the stream field stays usable as free text either way.
async fn stream_suggestions(state: &AppState) -> Vec<String> {
    match db::streams::list_streams(&state.db, DEFAULT_USER_ID).await {
        Ok(streams) => streams.into_iter().map(|stream| stream.name).collect(),
        Err(err) => {
            tracing::error!(%err, "failed to load stream labels");
            Vec::new()
        }
    }
}

/// `GET /opportunities/new` — the empty opportunity create form.
async fn new_opportunity_form(State(state): State<AppState>) -> Response {
    let template = OpportunityFormTemplate::fresh(
        "New opportunity".to_string(),
        "/opportunities".to_string(),
        "Save opportunity".to_string(),
        stream_suggestions(&state).await,
    );
    HtmlTemplate(template).into_response()
}

/// `POST /opportunities` — validates and stores a new opportunity, then
/// redirects to its details page. Invalid input re-renders the form with the
/// entered values and a field error.
async fn create_opportunity(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let input = OpportunityFormInput::from_pairs(&pairs);

    let mut template = OpportunityFormTemplate::fresh(
        "New opportunity".to_string(),
        "/opportunities".to_string(),
        "Save opportunity".to_string(),
        stream_suggestions(&state).await,
    );
    fill_form_values(&mut template, &input);

    match db::opportunities::insert_opportunity(
        &state.db,
        &input.to_new_opportunity(DEFAULT_USER_ID),
    )
    .await
    {
        Ok(id) => {
            // Remember a newly used stream label so future forms offer it.
            if let Err(err) =
                db::streams::insert_stream(&state.db, DEFAULT_USER_ID, &input.stream).await
            {
                tracing::warn!(%err, "could not remember stream label");
            }
            Redirect::to(&format!("/opportunities/{id}")).into_response()
        }
        Err(db::opportunities::OpportunityInsertError::Validation(err)) => {
            apply_validation_error(&mut template, &err);
            HtmlTemplate(template).into_response()
        }
        Err(db::opportunities::OpportunityInsertError::Database(err)) => {
            tracing::error!(%err, "failed to store opportunity");
            template.form_error =
                Some("The opportunity could not be stored. Please try again.".to_string());
            HtmlTemplate(template).into_response()
        }
    }
}

/// `GET /opportunities/{id}` — details of one stored opportunity.
async fn opportunity_details(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::opportunities::get_opportunity(&state.db, id).await {
        Ok(Some(opportunity)) => {
            HtmlTemplate(OpportunityDetailsTemplate::from_opportunity(&opportunity)).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No such opportunity").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load opportunity");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// `GET /opportunities/{id}/edit` — the edit form pre-filled with the stored
/// values.
async fn edit_opportunity_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::opportunities::get_opportunity(&state.db, id).await {
        Ok(Some(opportunity)) => {
            let mut template = OpportunityFormTemplate::fresh(
                "Edit opportunity".to_string(),
                format!("/opportunities/{id}/edit"),
                "Save changes".to_string(),
                stream_suggestions(&state).await,
            );
            template.values = form_values_from(&opportunity);
            HtmlTemplate(template).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No such opportunity").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load opportunity");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// `POST /opportunities/{id}/edit` — validates and stores changes, then
/// redirects to the details page. Invalid input re-renders the edit form
/// with the entered values and a field error.
async fn update_opportunity(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let input = OpportunityFormInput::from_pairs(&pairs);

    let mut template = OpportunityFormTemplate::fresh(
        "Edit opportunity".to_string(),
        format!("/opportunities/{id}/edit"),
        "Save changes".to_string(),
        stream_suggestions(&state).await,
    );
    fill_form_values(&mut template, &input);

    match db::opportunities::update_opportunity(
        &state.db,
        id,
        &input.to_new_opportunity(DEFAULT_USER_ID),
    )
    .await
    {
        Ok(Some(_)) => Redirect::to(&format!("/opportunities/{id}")).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "No such opportunity").into_response(),
        Err(db::opportunities::OpportunityInsertError::Validation(err)) => {
            apply_validation_error(&mut template, &err);
            HtmlTemplate(template).into_response()
        }
        Err(db::opportunities::OpportunityInsertError::Database(err)) => {
            tracing::error!(%err, "failed to update opportunity");
            template.form_error =
                Some("The opportunity could not be stored. Please try again.".to_string());
            HtmlTemplate(template).into_response()
        }
    }
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

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

/// Rendered details of one stored opportunity, including its application
/// attempts and the recorded history. Display labels are prepared here so
/// the template stays free of formatting logic.
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
    applications: Vec<ApplicationAttemptVm>,
    /// Whether a draft or preparing attempt exists; controls the
    /// start-application affordance on the page.
    has_active_attempt: bool,
    /// Label for the start-application button: first attempt or another one.
    start_attempt_label: String,
    events: Vec<ApplicationEventVm>,
    /// Feedback for a rejected application transition, e.g. from a stale
    /// page; shown above the application section.
    application_form_error: Option<String>,
}

impl OpportunityDetailsTemplate {
    fn from_opportunity(
        opportunity: &db::opportunities::Opportunity,
        applications: Vec<ApplicationAttemptVm>,
        events: Vec<ApplicationEventVm>,
        has_active_attempt: bool,
    ) -> Self {
        let location_display = match (&opportunity.city, &opportunity.country) {
            (Some(city), Some(country)) => Some(format!("{city}, {country}")),
            (Some(place), None) | (None, Some(place)) => Some(place.clone()),
            (None, None) => None,
        };

        let start_attempt_label = if applications.is_empty() {
            "Start an application"
        } else {
            "Start another application"
        }
        .to_string();

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
            applications,
            has_active_attempt,
            start_attempt_label,
            events,
            application_form_error: None,
        }
    }
}

/// One application attempt as displayed on the opportunity details page,
/// with display labels prepared here and flags telling the template which
/// transitions are available in the current stage.
struct ApplicationAttemptVm {
    id: i64,
    stage: String,
    submitted_display: String,
    channel_display: String,
    outcome_display: String,
    can_prepare: bool,
    can_unprepare: bool,
    can_submit: bool,
    can_correct: bool,
    is_withdrawn: bool,
    /// Whether the recorded channel is meaningful: only after a confirmed
    /// submission (or on a closed attempt) does the channel say anything.
    show_channel: bool,
    /// Employer outcomes exist only on submitted attempts.
    can_record_outcome: bool,
    /// Whether a concrete outcome is recorded (anything but `unknown`).
    has_outcome: bool,
    /// Reporting obligations exist only on submitted attempts.
    can_record_report: bool,
    /// Whether a reporting record exists (anything saved, including `unknown`).
    has_report: bool,
    report_display: String,
    /// Whether the submitted-to-draft correction is blocked because a
    /// reporting obligation of this attempt is pending or reported.
    correction_blocked: bool,
}

impl ApplicationAttemptVm {
    fn from_application(
        application: &db::applications::Application,
        report: Option<&db::obligations::Obligation>,
        correction_blocked: bool,
    ) -> Self {
        let stage = application.stage.as_str();
        let outcome_display = match application.outcome_at.as_deref() {
            Some(date) if application.employer_outcome != "unknown" => format!(
                "{} · {}",
                display_label(&application.employer_outcome),
                format_date_human(date)
            ),
            _ => display_label(&application.employer_outcome),
        };

        let report_display = match report {
            None => "No record yet".to_string(),
            Some(report) => match report.status.as_str() {
                "reported" => match report.reported_at.as_deref() {
                    Some(date) => format!("Reported · {}", format_date_human(date)),
                    None => "Reported".to_string(),
                },
                other => display_label(other),
            },
        };

        Self {
            id: application.id,
            stage: stage.to_string(),
            submitted_display: application
                .submitted_at
                .as_deref()
                .map(format_date_human)
                .unwrap_or_else(|| "Not submitted yet".to_string()),
            channel_display: display_label(&application.submission_channel),
            outcome_display,
            can_prepare: stage == "draft",
            can_unprepare: stage == "preparing",
            can_submit: stage == "preparing",
            can_correct: stage == "submitted",
            is_withdrawn: stage == "withdrawn",
            show_channel: stage == "submitted" || stage == "withdrawn",
            can_record_outcome: stage == "submitted",
            has_outcome: application.employer_outcome != "unknown",
            can_record_report: stage == "submitted",
            has_report: report.is_some(),
            report_display,
            correction_blocked,
        }
    }
}

/// One recorded history entry as displayed on the details page.
struct ApplicationEventVm {
    event_label: String,
    created_at: String,
    detail: Option<String>,
}

impl ApplicationEventVm {
    fn from_event(event: &db::application_events::ApplicationEvent) -> Self {
        let event_label = match event.event_type.as_str() {
            db::application_events::EVENT_SUBMITTED => "Submission recorded",
            db::application_events::EVENT_SUBMISSION_CORRECTED => "Submission corrected",
            db::application_events::EVENT_WITHDRAWN => "Application withdrawn",
            db::application_events::EVENT_OUTCOME_CHANGED => "Employer outcome changed",
            db::application_events::EVENT_REPORT_STATUS_CHANGED => "Reporting status changed",
            other => other,
        }
        .to_string();
        let detail = match (&event.old_value, &event.new_value, &event.note) {
            (Some(old), Some(new), Some(note)) => Some(format!("{old} to {new}: {note}")),
            (Some(old), Some(new), None) => Some(format!("{old} to {new}")),
            _ => event.note.clone(),
        };
        Self {
            event_label,
            created_at: event.created_at.clone(),
            detail,
        }
    }
}

/// Turns a stored code such as `employment_agency` into a readable label such
/// as `Employment agency`; unknown codes pass through unchanged.
fn display_label(code: &str) -> String {
    const LABELS: [(&str, &str); 16] = [
        ("email", "Email"),
        ("employer_portal", "Employer portal"),
        ("employment_agency", "Employment agency"),
        ("other", "Other"),
        ("unknown", "Unknown"),
        ("pending", "Pending"),
        ("interview", "Interview"),
        ("offer", "Offer"),
        ("rejected", "Rejected"),
        ("withdrawn", "Withdrawn"),
        ("reported", "Reported"),
        ("not_required", "Not required"),
        ("jobcenter", "Jobcenter"),
        ("foreign_authority", "Foreign authority"),
        ("portal", "Authority portal"),
        ("manual", "Manual"),
    ];

    LABELS
        .iter()
        .find(|(stored, _)| *stored == code)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| code.to_string())
}

/// The confirmation form for recording a submission with its date and
/// channel; an application is submitted only through this explicit step.
#[derive(Template)]
#[template(path = "application_submit_form.html")]
struct SubmissionFormTemplate {
    application_id: i64,
    opportunity_id: i64,
    context_heading: String,
    submitted_at: String,
    submission_channel: String,
    submitted_at_error: Option<String>,
    channel_error: Option<String>,
    form_error: Option<String>,
}

/// The confirmation page for a significant stage change (submission
/// correction or withdrawal); the warning states the consequences.
#[derive(Template)]
#[template(path = "application_transition_confirm.html")]
struct TransitionConfirmTemplate {
    context_heading: String,
    heading: String,
    warning: String,
    form_action: String,
    submit_label: String,
    back_href: String,
}

/// The form for recording or updating the employer outcome of a submitted
/// application.
#[derive(Template)]
#[template(path = "application_outcome_form.html")]
struct OutcomeFormTemplate {
    application_id: i64,
    opportunity_id: i64,
    context_heading: String,
    employer_outcome: String,
    outcome_at: String,
    outcome_error: Option<String>,
    outcome_at_error: Option<String>,
    form_error: Option<String>,
}

/// The form for recording or updating the reporting status of a submitted
/// application; the first save creates the obligation row.
#[derive(Template)]
#[template(path = "application_report_form.html")]
struct ReportFormTemplate {
    application_id: i64,
    opportunity_id: i64,
    context_heading: String,
    status: String,
    reported_at: String,
    method: String,
    reference: String,
    recipient_name: String,
    notes: String,
    /// Known authority labels offered as suggestions.
    authorities: Vec<String>,
    status_error: Option<String>,
    reported_at_error: Option<String>,
    recipient_error: Option<String>,
    form_error: Option<String>,
}

/// The report backlog page: submitted attempts whose report is pending or
/// not yet recorded, with separate counts.
#[derive(Template)]
#[template(path = "report_backlog.html")]
struct ReportBacklogTemplate {
    counts: db::obligations::BacklogCounts,
    entries: Vec<BacklogEntryVm>,
}

/// One backlog row prepared for display.
struct BacklogEntryVm {
    opportunity_id: i64,
    heading: String,
    submitted_display: String,
    status_display: String,
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
        .route("/opportunities/{id}/applications", post(create_application))
        .route("/applications/{id}/prepare", post(prepare_application))
        .route("/applications/{id}/unprepare", post(unprepare_application))
        .route(
            "/applications/{id}/submit",
            get(submit_application_form).post(record_application_submission),
        )
        .route(
            "/applications/{id}/correct",
            get(correct_application_form).post(correct_application),
        )
        .route(
            "/applications/{id}/withdraw",
            get(withdraw_application_form).post(withdraw_application),
        )
        .route(
            "/applications/{id}/outcome",
            get(outcome_form).post(record_outcome),
        )
        .route(
            "/applications/{id}/report",
            get(report_form).post(record_report),
        )
        .route("/reports", get(report_backlog))
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

/// `GET /opportunities/{id}` — details of one stored opportunity, its
/// application attempts, and the recorded history.
async fn opportunity_details(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    render_opportunity_details(&state, id, None).await
}

/// Loads one opportunity with its attempts and history and renders the
/// details page, optionally showing a message about a rejected application
/// transition.
async fn render_opportunity_details(
    state: &AppState,
    opportunity_id: i64,
    application_form_error: Option<String>,
) -> Response {
    let opportunity = match db::opportunities::get_opportunity(&state.db, opportunity_id).await {
        Ok(Some(opportunity)) => opportunity,
        Ok(None) => return (StatusCode::NOT_FOUND, "No such opportunity").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load opportunity");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let applications = match db::applications::list_applications_for_opportunity(
        &state.db,
        opportunity_id,
    )
    .await
    {
        Ok(applications) => applications,
        Err(err) => {
            tracing::error!(%err, "failed to load applications");
            Vec::new()
        }
    };
    let obligations =
        match db::obligations::list_obligations_for_opportunity(&state.db, opportunity_id).await {
            Ok(obligations) => obligations,
            Err(err) => {
                tracing::error!(%err, "failed to load reporting obligations");
                Vec::new()
            }
        };
    let attempts: Vec<ApplicationAttemptVm> = applications
        .iter()
        .map(|application| {
            let report = obligations.iter().find(|obligation| {
                obligation.application_id == application.id
                    && obligation.kind == db::obligations::DEFAULT_KIND
            });
            let correction_blocked = obligations.iter().any(|obligation| {
                obligation.application_id == application.id
                    && (obligation.status == "pending" || obligation.status == "reported")
            });
            ApplicationAttemptVm::from_application(application, report, correction_blocked)
        })
        .collect();

    let has_active_attempt =
        match db::applications::active_application_for_opportunity(&state.db, opportunity_id).await
        {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(err) => {
                tracing::error!(%err, "failed to load the active application");
                false
            }
        };

    let events =
        match db::application_events::list_events_for_opportunity(&state.db, opportunity_id, 20)
            .await
        {
            Ok(events) => events,
            Err(err) => {
                tracing::error!(%err, "failed to load event history");
                Vec::new()
            }
        };
    let event_views: Vec<ApplicationEventVm> =
        events.iter().map(ApplicationEventVm::from_event).collect();

    let mut template = OpportunityDetailsTemplate::from_opportunity(
        &opportunity,
        attempts,
        event_views,
        has_active_attempt,
    );
    template.application_form_error = application_form_error;
    HtmlTemplate(template).into_response()
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

/// `POST /opportunities/{id}/applications` — starts a draft application for
/// the opportunity. Submitting the form again while an attempt is still
/// active simply returns to the details page without creating a duplicate.
async fn create_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::applications::insert_application(&state.db, DEFAULT_USER_ID, id).await {
        Ok(_) | Err(db::applications::ApplicationInsertError::ActiveAttemptExists) => {
            Redirect::to(&format!("/opportunities/{id}")).into_response()
        }
        Err(db::applications::ApplicationInsertError::OpportunityMissing) => {
            (StatusCode::NOT_FOUND, "No such opportunity").into_response()
        }
        Err(db::applications::ApplicationInsertError::Database(err)) => {
            tracing::error!(%err, "failed to store application");
            render_opportunity_details(
                &state,
                id,
                Some("The application could not be stored. Please try again.".to_string()),
            )
            .await
        }
    }
}

/// The stage changes a details-page button can request.
enum RequestedTransition {
    Prepare,
    Unprepare,
    CorrectToDraft,
    Withdraw,
}

/// Applies a requested stage change and renders the outcome: a redirect to
/// the opportunity details page on success, a 404 for a missing attempt,
/// or the details page with an explanatory message when the transition no
/// longer applies.
async fn apply_stage_transition(
    state: &AppState,
    application_id: i64,
    requested: RequestedTransition,
) -> Response {
    let application = match db::applications::get_application(&state.db, application_id).await {
        Ok(Some(application)) => application,
        Ok(None) => return (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let result = match requested {
        RequestedTransition::Prepare => {
            db::applications::set_in_progress_stage(&state.db, application_id, "preparing").await
        }
        RequestedTransition::Unprepare => {
            db::applications::set_in_progress_stage(&state.db, application_id, "draft").await
        }
        RequestedTransition::CorrectToDraft => {
            db::applications::correct_submission_to_draft(&state.db, application_id).await
        }
        RequestedTransition::Withdraw => {
            db::applications::withdraw(&state.db, application_id).await
        }
    };

    match result {
        Ok(()) => Redirect::to(&format!("/opportunities/{}", application.opportunity_id))
            .into_response(),
        Err(db::applications::StageTransitionError::ApplicationMissing) => {
            (StatusCode::NOT_FOUND, "No such application").into_response()
        }
        Err(db::applications::StageTransitionError::InvalidTransition { from, to }) => {
            render_opportunity_details(
                state,
                application.opportunity_id,
                Some(format!(
                    "This application is currently {from}; the requested change to {to} does not apply. Reload the page and retry."
                )),
            )
            .await
        }
        Err(db::applications::StageTransitionError::ReportingBlocked) => {
            render_opportunity_details(
                state,
                application.opportunity_id,
                Some(
                    "The recorded submission cannot be corrected while a report for it is pending or already reported. Update the reporting status first."
                        .to_string(),
                ),
            )
            .await
        }
        Err(db::applications::StageTransitionError::Database(err)) => {
            tracing::error!(%err, "failed to apply application transition");
            render_opportunity_details(
                state,
                application.opportunity_id,
                Some("The change could not be stored. Please try again.".to_string()),
            )
            .await
        }
    }
}

/// `POST /applications/{id}/prepare` — moves a draft attempt to preparing.
async fn prepare_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    apply_stage_transition(&state, id, RequestedTransition::Prepare).await
}

/// `POST /applications/{id}/unprepare` — moves a preparing attempt back to
/// draft.
async fn unprepare_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    apply_stage_transition(&state, id, RequestedTransition::Unprepare).await
}

/// Reads one named, trimmed field from raw form pairs.
fn form_field(pairs: &[(String, String)], name: &str) -> String {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default()
}

/// Builds the submission form view model for one application, with the
/// organization and title of the parent opportunity for context.
async fn submission_form_template(
    state: &AppState,
    application: &db::applications::Application,
) -> SubmissionFormTemplate {
    let context_heading =
        match db::opportunities::get_opportunity(&state.db, application.opportunity_id).await {
            Ok(Some(opportunity)) => {
                format!("{} — {}", opportunity.organization, opportunity.title)
            }
            Ok(None) | Err(_) => format!("Application attempt {}", application.id),
        };

    SubmissionFormTemplate {
        application_id: application.id,
        opportunity_id: application.opportunity_id,
        context_heading,
        submitted_at: String::new(),
        submission_channel: "email".to_string(),
        submitted_at_error: None,
        channel_error: None,
        form_error: None,
    }
}

/// `GET /applications/{id}/submit` — the confirmation form for recording a
/// submission with its date and channel.
async fn submit_application_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::applications::get_application(&state.db, id).await {
        Ok(Some(application)) => {
            HtmlTemplate(submission_form_template(&state, &application).await).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// `POST /applications/{id}/submit` — validates the confirmed submission
/// facts and records them. Invalid input re-renders the form with the
/// entered values and field errors.
async fn record_application_submission(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let submitted_at = form_field(&pairs, "submitted_at");
    let submission_channel = form_field(&pairs, "submission_channel");

    let application = match db::applications::get_application(&state.db, id).await {
        Ok(Some(application)) => application,
        Ok(None) => return (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut template = submission_form_template(&state, &application).await;
    template.submitted_at = submitted_at.clone();
    template.submission_channel = submission_channel.clone();

    match db::applications::record_submission(&state.db, id, &submitted_at, &submission_channel)
        .await
    {
        Ok(()) => Redirect::to(&format!("/opportunities/{}", application.opportunity_id))
            .into_response(),
        Err(db::applications::SubmissionError::Validation(err)) => {
            let message = Some(err.reason.to_string());
            match err.field {
                "submitted_at" => template.submitted_at_error = message,
                "submission_channel" => template.channel_error = message,
                _ => template.form_error = Some(format!("{} {}", err.field, err.reason)),
            }
            HtmlTemplate(template).into_response()
        }
        Err(db::applications::SubmissionError::InvalidTransition { from, .. }) => {
            render_opportunity_details(
                &state,
                application.opportunity_id,
                Some(format!(
                    "This application is currently {from}; only an application that is preparing can be recorded as submitted."
                )),
            )
            .await
        }
        Err(db::applications::SubmissionError::ApplicationMissing) => {
            (StatusCode::NOT_FOUND, "No such application").into_response()
        }
        Err(db::applications::SubmissionError::Database(err)) => {
            tracing::error!(%err, "failed to record submission");
            template.form_error =
                Some("The submission could not be stored. Please try again.".to_string());
            HtmlTemplate(template).into_response()
        }
    }
}

/// The fixed text of one of the confirmation pages.
struct TransitionConfirmText {
    heading: &'static str,
    warning: &'static str,
    action_suffix: &'static str,
    submit_label: &'static str,
}

/// Renders the confirmation page for a significant stage change.
async fn transition_confirm_form(
    state: &AppState,
    application_id: i64,
    text: TransitionConfirmText,
) -> Response {
    match db::applications::get_application(&state.db, application_id).await {
        Ok(Some(application)) => {
            let context_heading =
                match db::opportunities::get_opportunity(&state.db, application.opportunity_id)
                    .await
                {
                    Ok(Some(opportunity)) => {
                        format!("{} — {}", opportunity.organization, opportunity.title)
                    }
                    Ok(None) | Err(_) => format!("Application attempt {application_id}"),
                };
            HtmlTemplate(TransitionConfirmTemplate {
                context_heading,
                heading: text.heading.to_string(),
                warning: text.warning.to_string(),
                form_action: format!("/applications/{application_id}/{}", text.action_suffix),
                submit_label: text.submit_label.to_string(),
                back_href: format!("/opportunities/{}", application.opportunity_id),
            })
            .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// `GET /applications/{id}/correct` — the confirmation page for correcting
/// a mistakenly recorded submission back to draft.
async fn correct_application_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    transition_confirm_form(
        &state,
        id,
        TransitionConfirmText {
            heading: "Correct the recorded submission",
            warning: "This clears the recorded submission date and channel and resets the employer outcome to unknown. The submission itself stays in the history: the correction is recorded, not erased.",
            action_suffix: "correct",
            submit_label: "Correct to draft",
        },
    )
    .await
}

/// `POST /applications/{id}/correct` — applies the submitted-to-draft
/// correction.
async fn correct_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    apply_stage_transition(&state, id, RequestedTransition::CorrectToDraft).await
}

/// `GET /applications/{id}/withdraw` — the confirmation page for
/// withdrawing a submitted application.
async fn withdraw_application_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    transition_confirm_form(
        &state,
        id,
        TransitionConfirmText {
            heading: "Withdraw the application",
            warning: "Marks this attempt as withdrawn. A withdrawn attempt is closed for good; applying again starts a new attempt.",
            action_suffix: "withdraw",
            submit_label: "Withdraw application",
        },
    )
    .await
}

/// `POST /applications/{id}/withdraw` — records the retraction of a
/// submitted application.
async fn withdraw_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    apply_stage_transition(&state, id, RequestedTransition::Withdraw).await
}

/// Builds the outcome form view model for one application, prefilled with
/// the stored outcome.
async fn outcome_form_template(
    state: &AppState,
    application: &db::applications::Application,
) -> OutcomeFormTemplate {
    let context_heading =
        match db::opportunities::get_opportunity(&state.db, application.opportunity_id).await {
            Ok(Some(opportunity)) => {
                format!("{} — {}", opportunity.organization, opportunity.title)
            }
            Ok(None) | Err(_) => format!("Application attempt {}", application.id),
        };

    OutcomeFormTemplate {
        application_id: application.id,
        opportunity_id: application.opportunity_id,
        context_heading,
        employer_outcome: application.employer_outcome.clone(),
        outcome_at: application.outcome_at.clone().unwrap_or_default(),
        outcome_error: None,
        outcome_at_error: None,
        form_error: None,
    }
}

/// `GET /applications/{id}/outcome` — the form for recording or updating
/// the employer outcome.
async fn outcome_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::applications::get_application(&state.db, id).await {
        Ok(Some(application)) => {
            HtmlTemplate(outcome_form_template(&state, &application).await).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// `POST /applications/{id}/outcome` — validates and records the employer
/// outcome. Invalid input re-renders the form with the entered values and
/// field errors.
async fn record_outcome(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let employer_outcome = form_field(&pairs, "employer_outcome");
    let outcome_at = form_field(&pairs, "outcome_at");

    let application = match db::applications::get_application(&state.db, id).await {
        Ok(Some(application)) => application,
        Ok(None) => return (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut template = outcome_form_template(&state, &application).await;
    template.employer_outcome = employer_outcome.clone();
    template.outcome_at = outcome_at.clone();

    match db::applications::record_employer_outcome(
        &state.db,
        id,
        &employer_outcome,
        Some(&outcome_at),
    )
    .await
    {
        Ok(()) => Redirect::to(&format!("/opportunities/{}", application.opportunity_id))
            .into_response(),
        Err(db::applications::OutcomeError::Validation(err)) => {
            let message = Some(err.reason.to_string());
            match err.field {
                "employer_outcome" => template.outcome_error = message,
                "outcome_at" => template.outcome_at_error = message,
                _ => template.form_error = Some(format!("{} {}", err.field, err.reason)),
            }
            HtmlTemplate(template).into_response()
        }
        Err(db::applications::OutcomeError::NotSubmitted { stage }) => {
            render_opportunity_details(
                &state,
                application.opportunity_id,
                Some(format!(
                    "This application is currently {stage}; only a submitted application can receive an employer outcome."
                )),
            )
            .await
        }
        Err(db::applications::OutcomeError::ApplicationMissing) => {
            (StatusCode::NOT_FOUND, "No such application").into_response()
        }
        Err(db::applications::OutcomeError::Database(err)) => {
            tracing::error!(%err, "failed to record employer outcome");
            template.form_error =
                Some("The outcome could not be stored. Please try again.".to_string());
            HtmlTemplate(template).into_response()
        }
    }
}

/// Loads the known authority labels for form suggestions, or an empty list
/// when they cannot be read; the authority field stays usable as free text
/// either way.
async fn authority_suggestions(state: &AppState) -> Vec<String> {
    match db::authorities::list_authorities(&state.db, DEFAULT_USER_ID).await {
        Ok(authorities) => authorities.into_iter().map(|a| a.name).collect(),
        Err(err) => {
            tracing::error!(%err, "failed to load authority labels");
            Vec::new()
        }
    }
}

/// Builds the report form view model for one application, prefilled with
/// the stored reporting status when an obligation row exists.
async fn report_form_template(
    state: &AppState,
    application: &db::applications::Application,
) -> ReportFormTemplate {
    let context_heading =
        match db::opportunities::get_opportunity(&state.db, application.opportunity_id).await {
            Ok(Some(opportunity)) => {
                format!("{} — {}", opportunity.organization, opportunity.title)
            }
            Ok(None) | Err(_) => format!("Application attempt {}", application.id),
        };

    let stored =
        db::obligations::get_obligation(&state.db, application.id, db::obligations::DEFAULT_KIND)
            .await
            .ok()
            .flatten();

    ReportFormTemplate {
        application_id: application.id,
        opportunity_id: application.opportunity_id,
        context_heading,
        status: stored
            .as_ref()
            .map(|o| o.status.clone())
            .unwrap_or_else(|| "pending".to_string()),
        reported_at: stored
            .as_ref()
            .and_then(|o| o.reported_at.clone())
            .unwrap_or_default(),
        method: stored
            .as_ref()
            .and_then(|o| o.method.clone())
            .unwrap_or_default(),
        reference: stored
            .as_ref()
            .and_then(|o| o.reference.clone())
            .unwrap_or_default(),
        recipient_name: stored
            .as_ref()
            .and_then(|o| o.recipient_name.clone())
            .unwrap_or_default(),
        notes: stored
            .as_ref()
            .and_then(|o| o.notes.clone())
            .unwrap_or_default(),
        authorities: authority_suggestions(state).await,
        status_error: None,
        reported_at_error: None,
        recipient_error: None,
        form_error: None,
    }
}

/// `GET /applications/{id}/report` — the form for recording or updating the
/// reporting status of a submitted application.
async fn report_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::applications::get_application(&state.db, id).await {
        Ok(Some(application)) => {
            HtmlTemplate(report_form_template(&state, &application).await).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// `POST /applications/{id}/report` — validates and records the reporting
/// status, creating the obligation row on first save. Invalid input
/// re-renders the form with the entered values and field errors.
async fn record_report(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let status = form_field(&pairs, "status");
    let reported_at = form_field(&pairs, "reported_at");
    let method = form_field(&pairs, "method");
    let reference = form_field(&pairs, "reference");
    let recipient_name = form_field(&pairs, "recipient_name");
    let notes = form_field(&pairs, "notes");

    let application = match db::applications::get_application(&state.db, id).await {
        Ok(Some(application)) => application,
        Ok(None) => return (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut template = report_form_template(&state, &application).await;
    template.status = status.clone();
    template.reported_at = reported_at.clone();
    template.method = method.clone();
    template.reference = reference.clone();
    template.recipient_name = recipient_name.clone();
    template.notes = notes.clone();

    let new = db::obligations::NewReport {
        status: &status,
        reported_at: Some(&reported_at),
        method: Some(&method),
        reference: Some(&reference),
        recipient_name: Some(&recipient_name),
        notes: Some(&notes),
    };

    match db::obligations::record_report(&state.db, id, db::obligations::DEFAULT_KIND, &new).await
    {
        Ok(()) => {
            // Remember a newly used authority label so future forms offer it.
            if !recipient_name.is_empty()
                && let Err(err) =
                    db::authorities::insert_authority(&state.db, DEFAULT_USER_ID, &recipient_name)
                        .await
            {
                tracing::warn!(%err, "could not remember authority label");
            }
            Redirect::to(&format!("/opportunities/{}", application.opportunity_id))
                .into_response()
        }
        Err(db::obligations::ReportError::Validation(err)) => {
            let message = Some(err.reason.to_string());
            match err.field {
                "status" => template.status_error = message,
                "reported_at" => template.reported_at_error = message,
                "recipient_name" => template.recipient_error = message,
                _ => template.form_error = Some(format!("{} {}", err.field, err.reason)),
            }
            HtmlTemplate(template).into_response()
        }
        Err(db::obligations::ReportError::NotSubmitted { stage }) => {
            render_opportunity_details(
                &state,
                application.opportunity_id,
                Some(format!(
                    "This application is currently {stage}; a reporting status exists only for submitted applications."
                )),
            )
            .await
        }
        Err(db::obligations::ReportError::ApplicationMissing) => {
            (StatusCode::NOT_FOUND, "No such application").into_response()
        }
        Err(db::obligations::ReportError::Database(err)) => {
            tracing::error!(%err, "failed to record reporting status");
            template.form_error =
                Some("The reporting status could not be stored. Please try again.".to_string());
            HtmlTemplate(template).into_response()
        }
    }
}

/// `GET /reports` — the report backlog: submitted attempts whose report is
/// pending or not yet recorded, with separate counts from SQL data.
async fn report_backlog(State(state): State<AppState>) -> Response {
    let counts = match db::obligations::reporting_counts(&state.db).await {
        Ok(counts) => counts,
        Err(err) => {
            tracing::error!(%err, "failed to count reporting states");
            db::obligations::BacklogCounts::default()
        }
    };

    let entries = match db::obligations::reporting_backlog(&state.db, 100).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::error!(%err, "failed to load the report backlog");
            Vec::new()
        }
    };

    let entry_views: Vec<BacklogEntryVm> = entries
        .iter()
        .map(|entry| BacklogEntryVm {
            opportunity_id: entry.opportunity_id,
            heading: format!("{} — {}", entry.organization, entry.title),
            submitted_display: entry
                .submitted_at
                .as_deref()
                .map(format_date_human)
                .unwrap_or_else(|| "Unknown date".to_string()),
            status_display: entry
                .status
                .as_deref()
                .map(display_label)
                .unwrap_or_else(|| "Unknown".to_string()),
        })
        .collect();

    HtmlTemplate(ReportBacklogTemplate {
        counts,
        entries: entry_views,
    })
    .into_response()
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

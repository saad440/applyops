//! Opportunity pages: the list, the create/edit form, and the details page
//! with its application attempts and history.

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use crate::{AppState, DEFAULT_USER_ID, HtmlTemplate, db};

use super::applications::{ActionVm, ApplicationAttemptVm, ApplicationEventVm};
use super::display::{
    display_label, format_date_human, format_deadline_detail, format_fit, format_location,
};

/// Index page: the opportunity list, optionally filtered by a search term,
/// rendered from `templates/index.html` at compile time.
#[derive(Template)]
#[template(path = "index.html")]
pub struct OpportunityListTemplate {
    pub search_query: String,
    pub opportunities: Vec<db::opportunities::Opportunity>,
    pub list_count: i64,
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
    fit_min_error: Option<String>,
    fit_max_error: Option<String>,
    effort_error: Option<String>,
    rationale_error: Option<String>,
    deadline_kind_error: Option<String>,
    deadline_confidence_error: Option<String>,
    recruitment_page_url_error: Option<String>,
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
    fit_min: String,
    fit_max: String,
    effort: String,
    rationale: String,
    deadline_date: String,
    deadline_kind: String,
    deadline_confidence: String,
    source_url: String,
    recruitment_page_url: String,
    source_notes: String,
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
            fit_min_error: None,
            fit_max_error: None,
            effort_error: None,
            rationale_error: None,
            deadline_kind_error: None,
            deadline_confidence_error: None,
            recruitment_page_url_error: None,
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
    /// The deadline's kind and confidence, e.g. `Hard close · Verified`.
    deadline_kind_display: Option<String>,
    /// Self-assessed fit such as `4/5` or `3.5–4/5`, or `None` when unrated.
    fit_display: Option<String>,
    effort_display: Option<String>,
    rationale: Option<String>,
    /// Fallback recruitment page for speculative leads.
    recruitment_url: Option<String>,
    /// Notes on how the source and its dates were verified.
    source_notes: Option<String>,
}

impl OpportunityDetailsTemplate {
    fn from_opportunity(
        opportunity: &db::opportunities::Opportunity,
        applications: Vec<ApplicationAttemptVm>,
        events: Vec<ApplicationEventVm>,
        has_active_attempt: bool,
    ) -> Self {
        let location_display =
            format_location(opportunity.city.as_deref(), opportunity.country.as_deref());

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
            deadline_kind_display: format_deadline_detail(
                opportunity.deadline_kind.as_deref(),
                opportunity.deadline_confidence.as_deref(),
            ),
            notes: opportunity.notes.clone(),
            stage: opportunity.stage.clone(),
            created_at: opportunity.created_at.clone(),
            applications,
            has_active_attempt,
            start_attempt_label,
            events,
            application_form_error: None,
            fit_display: format_fit(opportunity.fit_min, opportunity.fit_max),
            effort_display: opportunity.effort.as_deref().map(display_label),
            rationale: opportunity.rationale.clone(),
            recruitment_url: opportunity.recruitment_page_url.clone(),
            source_notes: opportunity.source_notes.clone(),
        }
    }
}

/// Parses an optional fit value from form text. Empty text means unrated;
/// a comma is accepted as the decimal separator. Anything that is not a
/// number within 1 to 5 is rejected with the field named, so the form can
/// point at the input that caused the rejection.
fn parse_fit(
    field: &'static str,
    raw: &str,
) -> Result<Option<f64>, db::opportunities::OpportunityValidationError> {
    let text = raw.trim().replace(',', ".");
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<f64>()
        .ok()
        .filter(|value| (1.0..=5.0).contains(value))
        .map(Some)
        .ok_or(db::opportunities::OpportunityValidationError {
            field,
            reason: "must be a number between 1 and 5",
        })
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
    fit_min: String,
    fit_max: String,
    effort: String,
    rationale: String,
    deadline_date: String,
    deadline_kind: String,
    deadline_confidence: String,
    source_url: String,
    recruitment_page_url: String,
    source_notes: String,
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
            fit_min: field(pairs, "fit_min"),
            fit_max: field(pairs, "fit_max"),
            effort: field(pairs, "effort"),
            rationale: field(pairs, "rationale"),
            deadline_date: field(pairs, "deadline_date"),
            deadline_kind: field(pairs, "deadline_kind"),
            deadline_confidence: field(pairs, "deadline_confidence"),
            source_url: field(pairs, "source_url"),
            recruitment_page_url: field(pairs, "recruitment_page_url"),
            source_notes: field(pairs, "source_notes"),
            notes: field(pairs, "notes"),
            stage: field(pairs, "stage"),
        }
    }

    /// Converts the submitted text into the validated data-layer input.
    /// The fit values are parsed here so a malformed number is reported on
    /// its own field before the data-layer validation runs.
    fn to_new_opportunity(
        &self,
        user_id: i64,
    ) -> Result<db::opportunities::NewOpportunity, db::opportunities::OpportunityValidationError>
    {
        fn optional(value: &str) -> Option<String> {
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }

        Ok(db::opportunities::NewOpportunity {
            user_id,
            organization: self.organization.clone(),
            title: self.title.clone(),
            stream: self.stream.clone(),
            city: optional(&self.city),
            country: optional(&self.country),
            priority: self.priority.clone(),
            fit_min: parse_fit("fit_min", &self.fit_min)?,
            fit_max: parse_fit("fit_max", &self.fit_max)?,
            effort: optional(&self.effort),
            rationale: optional(&self.rationale),
            deadline_date: optional(&self.deadline_date),
            deadline_kind: optional(&self.deadline_kind),
            deadline_confidence: optional(&self.deadline_confidence),
            source_url: optional(&self.source_url),
            recruitment_page_url: optional(&self.recruitment_page_url),
            source_notes: optional(&self.source_notes),
            notes: optional(&self.notes),
            stage: self.stage.clone(),
        })
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
        fit_min: input.fit_min.clone(),
        fit_max: input.fit_max.clone(),
        effort: input.effort.clone(),
        rationale: input.rationale.clone(),
        deadline_date: input.deadline_date.clone(),
        deadline_kind: input.deadline_kind.clone(),
        deadline_confidence: input.deadline_confidence.clone(),
        source_url: input.source_url.clone(),
        recruitment_page_url: input.recruitment_page_url.clone(),
        source_notes: input.source_notes.clone(),
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
        fit_min: opportunity
            .fit_min
            .map(|value| value.to_string())
            .unwrap_or_default(),
        fit_max: opportunity
            .fit_max
            .map(|value| value.to_string())
            .unwrap_or_default(),
        effort: text(&opportunity.effort),
        rationale: text(&opportunity.rationale),
        deadline_date: text(&opportunity.deadline_date),
        deadline_kind: text(&opportunity.deadline_kind),
        deadline_confidence: text(&opportunity.deadline_confidence),
        source_url: text(&opportunity.source_url),
        recruitment_page_url: text(&opportunity.recruitment_page_url),
        source_notes: text(&opportunity.source_notes),
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
        "fit_min" => template.fit_min_error = message,
        "fit_max" => template.fit_max_error = message,
        "effort" => template.effort_error = message,
        "rationale" => template.rationale_error = message,
        "deadline_kind" => template.deadline_kind_error = message,
        "deadline_confidence" => template.deadline_confidence_error = message,
        "recruitment_page_url" => template.recruitment_page_url_error = message,
        _ => template.form_error = Some(format!("{} {}", err.field, err.reason)),
    }
}

/// `GET /` — the opportunity list, optionally filtered by `?q=`.
pub async fn root(
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
pub async fn new_opportunity_form(State(state): State<AppState>) -> Response {
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
pub async fn create_opportunity(
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

    let new = match input.to_new_opportunity(DEFAULT_USER_ID) {
        Ok(new) => new,
        Err(err) => {
            apply_validation_error(&mut template, &err);
            return HtmlTemplate(template).into_response();
        }
    };

    match db::opportunities::insert_opportunity(&state.db, &new).await {
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
pub async fn opportunity_details(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    render_opportunity_details(&state, id, None).await
}

/// Loads one opportunity with its attempts and history and renders the
/// details page, optionally showing a message about a rejected application
/// transition.
pub async fn render_opportunity_details(
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
    let mut attempts = Vec::with_capacity(applications.len());
    for application in &applications {
        let report = obligations.iter().find(|obligation| {
            obligation.application_id == application.id
                && obligation.kind == db::obligations::DEFAULT_KIND
        });
        let correction_blocked = obligations.iter().any(|obligation| {
            obligation.application_id == application.id
                && (obligation.status == "pending" || obligation.status == "reported")
        });
        let actions =
            match db::actions::list_actions_for_application(&state.db, application.id).await {
                Ok(actions) => actions.iter().map(ActionVm::from_action).collect(),
                Err(err) => {
                    tracing::error!(%err, "failed to load follow-up actions");
                    Vec::new()
                }
            };
        attempts.push(ApplicationAttemptVm::from_application(
            application,
            report,
            correction_blocked,
            actions,
        ));
    }

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
pub async fn edit_opportunity_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
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
pub async fn update_opportunity(
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

    let new = match input.to_new_opportunity(DEFAULT_USER_ID) {
        Ok(new) => new,
        Err(err) => {
            apply_validation_error(&mut template, &err);
            return HtmlTemplate(template).into_response();
        }
    };

    match db::opportunities::update_opportunity(&state.db, id, &new).await {
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

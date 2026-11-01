//! Application workflow pages: attempt creation, stage transitions,
//! submissions, employer outcomes, reporting status, and follow-up actions.

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use crate::{AppState, DEFAULT_USER_ID, HtmlTemplate, db};

use super::display::{display_label, format_date_human};
use super::opportunities::render_opportunity_details;

/// One application attempt as displayed on the opportunity details page,
/// with display labels prepared here and flags telling the template which
/// transitions are available in the current stage.
pub struct ApplicationAttemptVm {
    pub id: i64,
    pub stage: String,
    pub submitted_display: String,
    pub channel_display: String,
    pub outcome_display: String,
    pub can_prepare: bool,
    pub can_unprepare: bool,
    pub can_submit: bool,
    pub can_correct: bool,
    pub is_withdrawn: bool,
    /// Whether the recorded channel is meaningful: only after a confirmed
    /// submission (or on a closed attempt) does the channel say anything.
    pub show_channel: bool,
    /// Employer outcomes exist only on submitted attempts.
    pub can_record_outcome: bool,
    /// Whether a concrete outcome is recorded (anything but `unknown`).
    pub has_outcome: bool,
    /// Reporting obligations exist only on submitted attempts.
    pub can_record_report: bool,
    /// Whether a reporting record exists (anything saved, including `unknown`).
    pub has_report: bool,
    pub report_display: String,
    /// Whether the submitted-to-draft correction is blocked because a
    /// reporting obligation of this attempt is pending or reported.
    pub correction_blocked: bool,
    /// User-curated follow-up actions of this attempt.
    pub actions: Vec<ActionVm>,
}

impl ApplicationAttemptVm {
    pub fn from_application(
        application: &db::applications::Application,
        report: Option<&db::obligations::Obligation>,
        correction_blocked: bool,
        actions: Vec<ActionVm>,
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
            actions,
        }
    }
}

/// One follow-up action as displayed on the details page.
pub struct ActionVm {
    pub id: i64,
    pub title: String,
    /// Whether the action is still open.
    pub is_open: bool,
    pub due_display: Option<String>,
    pub completed_display: Option<String>,
    pub notes_display: Option<String>,
}

impl ActionVm {
    pub fn from_action(action: &db::actions::Action) -> Self {
        let completed_display = action.completed_at.as_deref().map(|moment| {
            // The completion moment is stored as a timestamp; the list
            // shows its calendar date.
            format!(
                "Done · {}",
                format_date_human(moment.get(..10).unwrap_or(moment))
            )
        });
        Self {
            id: action.id,
            title: action.title.clone(),
            is_open: action.completed_at.is_none(),
            due_display: action
                .due_at
                .as_deref()
                .map(|date| format!("Due {}", format_date_human(date))),
            completed_display,
            notes_display: action.notes.clone(),
        }
    }
}

/// One recorded history entry as displayed on the details page.
pub struct ApplicationEventVm {
    pub event_label: String,
    pub created_at: String,
    pub detail: Option<String>,
}

impl ApplicationEventVm {
    pub fn from_event(event: &db::application_events::ApplicationEvent) -> Self {
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

/// `POST /opportunities/{id}/applications` — starts a draft application for
/// the opportunity. Submitting the form again while an attempt is still
/// active simply returns to the details page without creating a duplicate.
pub async fn create_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
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
pub async fn prepare_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    apply_stage_transition(&state, id, RequestedTransition::Prepare).await
}

/// `POST /applications/{id}/unprepare` — moves a preparing attempt back to
/// draft.
pub async fn unprepare_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
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
pub async fn submit_application_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
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
pub async fn record_application_submission(
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
pub async fn correct_application_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
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
pub async fn correct_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    apply_stage_transition(&state, id, RequestedTransition::CorrectToDraft).await
}

/// `GET /applications/{id}/withdraw` — the confirmation page for
/// withdrawing a submitted application.
pub async fn withdraw_application_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
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
pub async fn withdraw_application(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
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
pub async fn outcome_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
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
pub async fn record_outcome(
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
pub async fn report_form(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
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
pub async fn record_report(
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

/// POST /applications/{id}/actions — adds a user-curated follow-up action
/// to an application attempt. An action enters the world only through this
/// form: no other workflow creates one, and none writes a history entry.
pub async fn add_application_action(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let title = form_field(&pairs, "title");
    let due_at = form_field(&pairs, "due_at");
    let notes = form_field(&pairs, "notes");

    let application = match db::applications::get_application(&state.db, id).await {
        Ok(Some(application)) => application,
        Ok(None) => return (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let new = db::actions::NewAction {
        title: &title,
        due_at: Some(&due_at),
        notes: Some(&notes),
    };
    match db::actions::add_action(&state.db, id, &new).await {
        Ok(_) => {
            Redirect::to(&format!("/opportunities/{}", application.opportunity_id)).into_response()
        }
        Err(db::actions::ActionError::Validation(err)) => {
            render_opportunity_details(
                &state,
                application.opportunity_id,
                Some(format!(
                    "Follow-up was not saved: {} {}.",
                    err.field, err.reason
                )),
            )
            .await
        }
        Err(db::actions::ActionError::ApplicationMissing) => {
            (StatusCode::NOT_FOUND, "No such application").into_response()
        }
        Err(db::actions::ActionError::Database(err)) => {
            tracing::error!(%err, "failed to store the follow-up action");
            render_opportunity_details(
                &state,
                application.opportunity_id,
                Some("The follow-up could not be stored. Please try again.".to_string()),
            )
            .await
        }
    }
}

/// POST /actions/{id}/complete — marks a follow-up action done. Completion
/// is a plain flag on a user-curated entry: no stage, obligation, or
/// history change follows from it.
pub async fn complete_application_action(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    set_action_done(state, id, true).await
}

/// POST /actions/{id}/reopen — clears the completion flag again.
pub async fn reopen_application_action(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    set_action_done(state, id, false).await
}

/// Shared body of the complete and reopen handlers.
async fn set_action_done(state: AppState, action_id: i64, completed: bool) -> Response {
    let Some(action) = load_action(&state, action_id).await else {
        return (StatusCode::NOT_FOUND, "No such follow-up").into_response();
    };
    match db::actions::set_action_completed(&state.db, action_id, completed).await {
        Ok(_) => redirect_for_action(&state, &action).await,
        Err(err) => {
            tracing::error!(%err, "failed to update the follow-up action");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// POST /actions/{id}/delete — removes a follow-up action outright.
/// Actions carry no evidence, so removal leaves nothing behind.
pub async fn delete_application_action(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    let Some(action) = load_action(&state, id).await else {
        return (StatusCode::NOT_FOUND, "No such follow-up").into_response();
    };
    match db::actions::delete_action(&state.db, id).await {
        Ok(_) => redirect_for_action(&state, &action).await,
        Err(err) => {
            tracing::error!(%err, "failed to delete the follow-up action");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Reads one follow-up action, treating database failures as missing.
async fn load_action(state: &AppState, action_id: i64) -> Option<db::actions::Action> {
    match db::actions::get_action(&state.db, action_id).await {
        Ok(action) => action,
        Err(err) => {
            tracing::error!(%err, "failed to load the follow-up action");
            None
        }
    }
}

/// Redirects to the opportunity page that owns the attempt the action
/// belongs to.
async fn redirect_for_action(state: &AppState, action: &db::actions::Action) -> Response {
    match db::applications::get_application(&state.db, action.application_id).await {
        Ok(Some(application)) => {
            Redirect::to(&format!("/opportunities/{}", application.opportunity_id)).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No such application").into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to load application");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

//! Application persistence: attempts per opportunity, their stages, the
//! confirmed submission facts, and the stage transition rules. An
//! opportunity may carry several attempts over time, but never two active
//! ones. Submission, submission corrections, and withdrawal are recorded
//! in the event history; in-progress corrections between draft and
//! preparing are not.

use super::*;

use sqlx::SqliteConnection;

/// Application stages of the submission workflow. Movement between the
/// stages is governed by the transition rules, not this list; the list
/// exists for display labels.
#[allow(dead_code)]
pub const STAGES: [&str; 4] = ["draft", "preparing", "submitted", "withdrawn"];

/// Channels through which an application can be sent.
pub const SUBMISSION_CHANNELS: [&str; 5] = [
    "email",
    "employer_portal",
    "employment_agency",
    "other",
    "unknown",
];

/// Employer outcomes, tracked independently of the stage. The outcome
/// controls are not implemented yet.
#[allow(dead_code)]
pub const EMPLOYER_OUTCOMES: [&str; 6] = [
    "unknown",
    "pending",
    "interview",
    "offer",
    "rejected",
    "withdrawn",
];

/// A stored application attempt as read from the `applications` table.
#[derive(Debug)]
pub struct Application {
    pub id: i64,
    /// Not displayed anywhere yet; kept so user-scoped queries stay
    /// possible without schema work. Read by the data-layer tests.
    #[allow(dead_code)]
    pub user_id: i64,
    pub opportunity_id: i64,
    pub stage: String,
    /// Date-only calendar submission date (`YYYY-MM-DD`) or `None`
    /// while the attempt is not submitted.
    pub submitted_at: Option<String>,
    pub submission_channel: String,
    pub employer_outcome: String,
    /// Date-only calendar date of the recorded employer outcome, or
    /// `None` while the outcome is `unknown`. Read by the
    /// employer-outcome controls.
    #[allow(dead_code)]
    pub outcome_at: Option<String>,
    /// Not displayed anywhere yet; read by the data-layer tests.
    #[allow(dead_code)]
    pub created_at: String,
}

/// A single field-level validation failure on submission input.
#[derive(Debug, PartialEq, Eq)]
pub struct SubmissionValidationError {
    pub field: &'static str,
    pub reason: &'static str,
}

impl std::fmt::Display for SubmissionValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid {}: {}", self.field, self.reason)
    }
}

impl std::error::Error for SubmissionValidationError {}

/// Why an application could not be created.
#[derive(Debug)]
pub enum ApplicationInsertError {
    /// The opportunity does not exist.
    OpportunityMissing,
    /// The opportunity already has a draft or preparing attempt.
    ActiveAttemptExists,
    /// SQLite rejected the row, for example on a foreign-key constraint.
    Database(sqlx::Error),
}

impl std::fmt::Display for ApplicationInsertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpportunityMissing => write!(f, "no such opportunity"),
            Self::ActiveAttemptExists => write!(f, "an active application already exists"),
            Self::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for ApplicationInsertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(err) => Some(err),
            _ => None,
        }
    }
}

/// Why a stage transition could not be applied.
#[derive(Debug)]
pub enum StageTransitionError {
    /// No application row with that id exists.
    ApplicationMissing,
    /// The current stage does not allow the requested transition.
    InvalidTransition { from: String, to: String },
    /// The submitted-to-draft correction is blocked because a
    /// reporting obligation of this application is pending or
    /// reported: the submission is the basis of those records and
    /// cannot be silently undone beneath them.
    ReportingBlocked,
    /// SQLite rejected the change, for example on a check constraint.
    Database(sqlx::Error),
}

impl std::fmt::Display for StageTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplicationMissing => write!(f, "no such application"),
            Self::InvalidTransition { from, to } => {
                write!(f, "cannot move an application from {from} to {to}")
            }
            Self::ReportingBlocked => write!(
                f,
                "the submission cannot be corrected while a report for it is pending or reported"
            ),
            Self::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for StageTransitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(err) => Some(err),
            _ => None,
        }
    }
}

/// Why a submission could not be recorded.
#[derive(Debug)]
pub enum SubmissionError {
    /// No application row with that id exists.
    ApplicationMissing,
    /// The current stage does not allow recording a submission; an
    /// application reaches `submitted` only from `preparing`.
    InvalidTransition { from: String, to: &'static str },
    /// The submitted input was rejected before any SQL ran.
    Validation(SubmissionValidationError),
    /// SQLite rejected the change, for example on a check constraint.
    Database(sqlx::Error),
}

impl std::fmt::Display for SubmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplicationMissing => write!(f, "no such application"),
            Self::InvalidTransition { from, to } => {
                write!(f, "cannot move an application from {from} to {to}")
            }
            Self::Validation(err) => write!(f, "{err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for SubmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(err) => Some(err),
            Self::Database(err) => Some(err),
            _ => None,
        }
    }
}

/// Maps one `applications` row into an [`Application`].
fn application_from_row(row: &SqliteRow) -> Result<Application, sqlx::Error> {
    Ok(Application {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        opportunity_id: row.try_get("opportunity_id")?,
        stage: row.try_get("stage")?,
        submitted_at: row.try_get("submitted_at")?,
        submission_channel: row.try_get("submission_channel")?,
        employer_outcome: row.try_get("employer_outcome")?,
        outcome_at: row.try_get("outcome_at")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Reads the id and stage of one application inside a transaction, or
/// `None` when no such row exists.
async fn stage_of(
    conn: &mut SqliteConnection,
    application_id: i64,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    sqlx::query_as("SELECT id, stage FROM applications WHERE id = ?")
        .bind(application_id)
        .fetch_optional(conn)
        .await
}

/// Creates a draft application for an opportunity and returns its id.
/// An opportunity may hold several attempts over time, but never two
/// active ones (draft or preparing), so repeated form submissions do
/// not create duplicate attempts.
pub async fn insert_application(
    pool: &SqlitePool,
    user_id: i64,
    opportunity_id: i64,
) -> Result<i64, ApplicationInsertError> {
    let active = sqlx::query(
        "SELECT id FROM applications \
             WHERE opportunity_id = ? AND stage IN ('draft', 'preparing') LIMIT 1",
    )
    .bind(opportunity_id)
    .fetch_optional(pool)
    .await
    .map_err(ApplicationInsertError::Database)?;
    if active.is_some() {
        return Err(ApplicationInsertError::ActiveAttemptExists);
    }

    let opportunity = sqlx::query("SELECT id FROM opportunities WHERE id = ?")
        .bind(opportunity_id)
        .fetch_optional(pool)
        .await
        .map_err(ApplicationInsertError::Database)?;
    if opportunity.is_none() {
        return Err(ApplicationInsertError::OpportunityMissing);
    }

    sqlx::query_scalar(
        "INSERT INTO applications (user_id, opportunity_id) VALUES (?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(opportunity_id)
    .fetch_one(pool)
    .await
    .map_err(ApplicationInsertError::Database)
}

/// Fetches one application by id, or `None` when no such row exists.
pub async fn get_application(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<Application>, sqlx::Error> {
    sqlx::query(
        "SELECT id, user_id, opportunity_id, stage, submitted_at, submission_channel, \
             employer_outcome, outcome_at, created_at \
             FROM applications WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(|row| application_from_row(&row))
    .transpose()
}

/// Lists the attempts of one opportunity, newest first.
pub async fn list_applications_for_opportunity(
    pool: &SqlitePool,
    opportunity_id: i64,
) -> Result<Vec<Application>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, user_id, opportunity_id, stage, submitted_at, submission_channel, \
             employer_outcome, outcome_at, created_at \
             FROM applications WHERE opportunity_id = ? ORDER BY id DESC",
    )
    .bind(opportunity_id)
    .fetch_all(pool)
    .await?;

    rows.iter().map(application_from_row).collect()
}

/// Returns the active (draft or preparing) attempt of an opportunity,
/// if any.
pub async fn active_application_for_opportunity(
    pool: &SqlitePool,
    opportunity_id: i64,
) -> Result<Option<Application>, sqlx::Error> {
    sqlx::query(
        "SELECT id, user_id, opportunity_id, stage, submitted_at, submission_channel, \
             employer_outcome, outcome_at, created_at \
             FROM applications \
             WHERE opportunity_id = ? AND stage IN ('draft', 'preparing') \
             ORDER BY id DESC LIMIT 1",
    )
    .bind(opportunity_id)
    .fetch_optional(pool)
    .await?
    .map(|row| application_from_row(&row))
    .transpose()
}

/// Moves a draft attempt to preparing, or a preparing attempt back to
/// draft. These in-progress corrections are not recorded in the event
/// history; only submission, submission corrections, and withdrawal are.
pub async fn set_in_progress_stage(
    pool: &SqlitePool,
    application_id: i64,
    to_stage: &str,
) -> Result<(), StageTransitionError> {
    let mut tx = pool.begin().await.map_err(StageTransitionError::Database)?;
    let Some((_, from)) = stage_of(&mut tx, application_id)
        .await
        .map_err(StageTransitionError::Database)?
    else {
        return Err(StageTransitionError::ApplicationMissing);
    };

    let allowed = (from == "draft" && to_stage == "preparing")
        || (from == "preparing" && to_stage == "draft");
    if !allowed {
        return Err(StageTransitionError::InvalidTransition {
            from,
            to: to_stage.to_string(),
        });
    }

    sqlx::query("UPDATE applications SET stage = ? WHERE id = ? AND stage = ?")
        .bind(to_stage)
        .bind(application_id)
        .bind(&from)
        .execute(&mut *tx)
        .await
        .map_err(StageTransitionError::Database)?;
    tx.commit().await.map_err(StageTransitionError::Database)?;
    Ok(())
}

/// Records a user-confirmed submission: channel and date-only calendar
/// date. An application reaches `submitted` only from `preparing`.
/// The stage change and the event are written in one
/// transaction so they succeed or fail together.
pub async fn record_submission(
    pool: &SqlitePool,
    application_id: i64,
    submitted_at: &str,
    submission_channel: &str,
) -> Result<(), SubmissionError> {
    let trimmed_date = submitted_at.trim();
    if trimmed_date.is_empty() {
        return Err(SubmissionError::Validation(SubmissionValidationError {
            field: "submitted_at",
            reason: "must not be empty",
        }));
    }
    if !super::opportunities::is_iso_date(trimmed_date) {
        return Err(SubmissionError::Validation(SubmissionValidationError {
            field: "submitted_at",
            reason: "must be a calendar date formatted YYYY-MM-DD",
        }));
    }
    let trimmed_channel = submission_channel.trim();
    if !SUBMISSION_CHANNELS.contains(&trimmed_channel) {
        return Err(SubmissionError::Validation(SubmissionValidationError {
            field: "submission_channel",
            reason: "is not a recognized value",
        }));
    }

    let mut tx = pool.begin().await.map_err(SubmissionError::Database)?;
    let Some((id, user_id, from)) =
        sqlx::query_as("SELECT id, user_id, stage FROM applications WHERE id = ?")
            .bind(application_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(SubmissionError::Database)?
    else {
        return Err(SubmissionError::ApplicationMissing);
    };
    if from != "preparing" {
        return Err(SubmissionError::InvalidTransition {
            from,
            to: "submitted",
        });
    }

    sqlx::query(
        "UPDATE applications \
             SET stage = 'submitted', submitted_at = ?, submission_channel = ? \
             WHERE id = ? AND stage = 'preparing'",
    )
    .bind(trimmed_date)
    .bind(trimmed_channel)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(SubmissionError::Database)?;

    super::application_events::insert_event(
        &mut tx,
        super::application_events::NewEvent {
            user_id,
            event_type: super::application_events::EVENT_SUBMITTED,
            entity_type: "application",
            entity_id: id,
            old_value: Some("preparing"),
            new_value: Some("submitted"),
            note: Some(&format!(
                "submitted_at={trimmed_date} channel={trimmed_channel}"
            )),
        },
    )
    .await
    .map_err(SubmissionError::Database)?;

    tx.commit().await.map_err(SubmissionError::Database)?;
    Ok(())
}

/// Corrects a mistakenly recorded submission back to `draft`:
/// clears the submission date and channel and resets the employer
/// outcome to `unknown`. The event log keeps the evidence of the
/// recorded submission, so the correction erases nothing. The
/// correction is blocked while any reporting obligation of the
/// application is `pending` or `reported`: set the reporting status
/// aside first.
pub async fn correct_submission_to_draft(
    pool: &SqlitePool,
    application_id: i64,
) -> Result<(), StageTransitionError> {
    let mut tx = pool.begin().await.map_err(StageTransitionError::Database)?;
    let Some((id, user_id, from, submitted_at, channel, outcome)): Option<(
        i64,
        i64,
        String,
        Option<String>,
        String,
        String,
    )> = sqlx::query_as(
        "SELECT id, user_id, stage, submitted_at, submission_channel, employer_outcome \
             FROM applications WHERE id = ?",
    )
    .bind(application_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(StageTransitionError::Database)?
    else {
        return Err(StageTransitionError::ApplicationMissing);
    };
    if from != "submitted" {
        return Err(StageTransitionError::InvalidTransition {
            from,
            to: "draft".to_string(),
        });
    }

    let blocking = sqlx::query(
        "SELECT 1 FROM reporting_obligations \
             WHERE application_id = ? AND status IN ('pending', 'reported') LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(StageTransitionError::Database)?;
    if blocking.is_some() {
        return Err(StageTransitionError::ReportingBlocked);
    }

    sqlx::query(
        "UPDATE applications \
             SET stage = 'draft', submitted_at = NULL, submission_channel = 'unknown', \
                 employer_outcome = 'unknown', outcome_at = NULL \
             WHERE id = ? AND stage = 'submitted'",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(StageTransitionError::Database)?;

    let note = format!(
        "submission cleared: submitted_at={} channel={}; employer_outcome={} reset to unknown",
        submitted_at.unwrap_or_default(),
        channel,
        outcome,
    );
    super::application_events::insert_event(
        &mut tx,
        super::application_events::NewEvent {
            user_id,
            event_type: super::application_events::EVENT_SUBMISSION_CORRECTED,
            entity_type: "application",
            entity_id: id,
            old_value: Some("submitted"),
            new_value: Some("draft"),
            note: Some(&note),
        },
    )
    .await
    .map_err(StageTransitionError::Database)?;

    tx.commit().await.map_err(StageTransitionError::Database)?;
    Ok(())
}

/// Records the genuine retraction of a submitted application.
/// `withdrawn` is reachable only from `submitted` and is terminal; a
/// re-application is a new attempt row.
pub async fn withdraw(pool: &SqlitePool, application_id: i64) -> Result<(), StageTransitionError> {
    let mut tx = pool.begin().await.map_err(StageTransitionError::Database)?;
    let Some((id, user_id, from)) =
        sqlx::query_as("SELECT id, user_id, stage FROM applications WHERE id = ?")
            .bind(application_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(StageTransitionError::Database)?
    else {
        return Err(StageTransitionError::ApplicationMissing);
    };
    if from != "submitted" {
        return Err(StageTransitionError::InvalidTransition {
            from,
            to: "withdrawn".to_string(),
        });
    }

    sqlx::query(
        "UPDATE applications SET stage = 'withdrawn' \
             WHERE id = ? AND stage = 'submitted'",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(StageTransitionError::Database)?;

    super::application_events::insert_event(
        &mut tx,
        super::application_events::NewEvent {
            user_id,
            event_type: super::application_events::EVENT_WITHDRAWN,
            entity_type: "application",
            entity_id: id,
            old_value: Some("submitted"),
            new_value: Some("withdrawn"),
            note: None,
        },
    )
    .await
    .map_err(StageTransitionError::Database)?;

    tx.commit().await.map_err(StageTransitionError::Database)?;
    Ok(())
}

/// Why an employer outcome could not be recorded.
#[derive(Debug)]
pub enum OutcomeError {
    /// No application row with that id exists.
    ApplicationMissing,
    /// Employer outcomes exist only on submitted attempts: an
    /// interview does not undo the submission, and a withdrawn
    /// attempt receives no outcome.
    NotSubmitted { stage: String },
    /// The submitted input was rejected before any SQL ran.
    Validation(SubmissionValidationError),
    /// SQLite rejected the change, for example on a check constraint.
    Database(sqlx::Error),
}

impl std::fmt::Display for OutcomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplicationMissing => write!(f, "no such application"),
            Self::NotSubmitted { stage } => write!(
                f,
                "an employer outcome requires a submitted application; this one is {stage}"
            ),
            Self::Validation(err) => write!(f, "{err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for OutcomeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(err) => Some(err),
            Self::Database(err) => Some(err),
            _ => None,
        }
    }
}

/// Records or corrects the employer outcome of a submitted application.
/// Outcomes exist only on submitted attempts; choosing `unknown`
/// clears the outcome and its date. Corrections are allowed and keep
/// the previous value in the event history; the change and its event
/// are written in one transaction.
pub async fn record_employer_outcome(
    pool: &SqlitePool,
    application_id: i64,
    employer_outcome: &str,
    outcome_at: Option<&str>,
) -> Result<(), OutcomeError> {
    let trimmed_outcome = employer_outcome.trim();
    if !EMPLOYER_OUTCOMES.contains(&trimmed_outcome) {
        return Err(OutcomeError::Validation(SubmissionValidationError {
            field: "employer_outcome",
            reason: "is not a recognized value",
        }));
    }

    // A concrete outcome carries the date it was learned; `unknown`
    // clears both.
    let trimmed_date = outcome_at.map(str::trim).filter(|date| !date.is_empty());
    if trimmed_outcome != "unknown" {
        match trimmed_date {
            None => {
                return Err(OutcomeError::Validation(SubmissionValidationError {
                    field: "outcome_at",
                    reason: "must not be empty",
                }));
            }
            Some(date) if !super::opportunities::is_iso_date(date) => {
                return Err(OutcomeError::Validation(SubmissionValidationError {
                    field: "outcome_at",
                    reason: "must be a calendar date formatted YYYY-MM-DD",
                }));
            }
            Some(_) => {}
        }
    }

    let mut tx = pool.begin().await.map_err(OutcomeError::Database)?;
    let Some((id, user_id, stage, previous)): Option<(i64, i64, String, String)> = sqlx::query_as(
        "SELECT id, user_id, stage, employer_outcome FROM applications WHERE id = ?",
    )
    .bind(application_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(OutcomeError::Database)?
    else {
        return Err(OutcomeError::ApplicationMissing);
    };
    if stage != "submitted" {
        return Err(OutcomeError::NotSubmitted { stage });
    }

    let cleared = trimmed_outcome == "unknown";
    if cleared {
        sqlx::query(
            "UPDATE applications \
                 SET employer_outcome = 'unknown', outcome_at = NULL \
                 WHERE id = ? AND stage = 'submitted'",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(OutcomeError::Database)?;
    } else {
        sqlx::query(
            "UPDATE applications \
                 SET employer_outcome = ?, outcome_at = ? \
                 WHERE id = ? AND stage = 'submitted'",
        )
        .bind(trimmed_outcome)
        .bind(trimmed_date)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(OutcomeError::Database)?;
    }

    let note = if cleared {
        "outcome cleared".to_string()
    } else {
        format!("outcome_at={}", trimmed_date.unwrap_or_default())
    };
    super::application_events::insert_event(
        &mut tx,
        super::application_events::NewEvent {
            user_id,
            event_type: super::application_events::EVENT_OUTCOME_CHANGED,
            entity_type: "application",
            entity_id: id,
            old_value: Some(&previous),
            new_value: Some(trimmed_outcome),
            note: Some(&note),
        },
    )
    .await
    .map_err(OutcomeError::Database)?;

    tx.commit().await.map_err(OutcomeError::Database)?;
    Ok(())
}

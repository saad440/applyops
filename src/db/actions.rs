//! User-curated follow-up actions per application. Deliberately separate
//! from the reporting obligations: an action is convenience the user
//! invents for themselves — it carries no evidence, is never created or
//! completed by another workflow, and writes no history entries.

use super::*;

/// One stored follow-up action as read from `application_actions`.
#[derive(Debug)]
pub struct Action {
    pub id: i64,
    /// Read by the data-layer tests.
    #[allow(dead_code)]
    pub user_id: i64,
    pub application_id: i64,
    pub title: String,
    /// Optional calendar date the action should happen by; a due date
    /// is not a completion date.
    pub due_at: Option<String>,
    /// The moment the action was marked done, or `None` while open.
    pub completed_at: Option<String>,
    pub notes: Option<String>,
    /// Read by the data-layer tests.
    #[allow(dead_code)]
    pub created_at: String,
}

/// Why an action could not be added.
#[derive(Debug)]
pub enum ActionError {
    /// No application row with that id exists.
    ApplicationMissing,
    /// The submitted input was rejected before any SQL ran.
    Validation(applications::SubmissionValidationError),
    /// SQLite rejected the change, for example on a check constraint.
    Database(sqlx::Error),
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplicationMissing => write!(f, "no such application"),
            Self::Validation(err) => write!(f, "{err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for ActionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(err) => Some(err),
            Self::Database(err) => Some(err),
            _ => None,
        }
    }
}

/// User-supplied data for one new follow-up action, validated before
/// any SQL runs.
#[derive(Debug)]
pub struct NewAction<'a> {
    pub title: &'a str,
    pub due_at: Option<&'a str>,
    pub notes: Option<&'a str>,
}

/// Maps one `application_actions` row into an [`Action`].
fn action_from_row(row: &SqliteRow) -> Result<Action, sqlx::Error> {
    Ok(Action {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        application_id: row.try_get("application_id")?,
        title: row.try_get("title")?,
        due_at: row.try_get("due_at")?,
        completed_at: row.try_get("completed_at")?,
        notes: row.try_get("notes")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Adds a follow-up action to an application, carrying the
/// application's user onto the new row. Actions enter the world only
/// through this function: nothing else in the app creates one.
pub async fn add_action(
    pool: &SqlitePool,
    application_id: i64,
    new: &NewAction<'_>,
) -> Result<i64, ActionError> {
    let title = new.title.trim();
    if title.is_empty() {
        return Err(ActionError::Validation(
            applications::SubmissionValidationError {
                field: "title",
                reason: "must not be empty",
            },
        ));
    }
    let due_at = new.due_at.map(str::trim).filter(|value| !value.is_empty());
    if let Some(due_at) = due_at
        && !super::opportunities::is_iso_date(due_at)
    {
        return Err(ActionError::Validation(
            applications::SubmissionValidationError {
                field: "due_at",
                reason: "must be a calendar date formatted YYYY-MM-DD",
            },
        ));
    }
    let notes = new.notes.map(str::trim).filter(|value| !value.is_empty());

    // One statement: the row is born with the application's user, and
    // an unknown application inserts nothing at all.
    let inserted = sqlx::query_scalar::<_, i64>(
        "INSERT INTO application_actions \
             (user_id, application_id, title, due_at, notes) \
             SELECT user_id, ?, ?, ?, ? FROM applications WHERE id = ? \
             RETURNING id",
    )
    .bind(application_id)
    .bind(title)
    .bind(due_at)
    .bind(notes)
    .bind(application_id)
    .fetch_optional(pool)
    .await
    .map_err(ActionError::Database)?;
    inserted.ok_or(ActionError::ApplicationMissing)
}

/// Lists an application's follow-up actions: open ones first in
/// due-date order (undated last), completed ones at the end.
pub async fn list_actions_for_application(
    pool: &SqlitePool,
    application_id: i64,
) -> Result<Vec<Action>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, user_id, application_id, title, due_at, completed_at, notes, created_at \
             FROM application_actions WHERE application_id = ? \
             ORDER BY completed_at IS NOT NULL, due_at IS NULL, due_at, id",
    )
    .bind(application_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(action_from_row).collect()
}

/// Reads one follow-up action by id.
pub async fn get_action(pool: &SqlitePool, action_id: i64) -> Result<Option<Action>, sqlx::Error> {
    sqlx::query(
        "SELECT id, user_id, application_id, title, due_at, completed_at, notes, created_at \
             FROM application_actions WHERE id = ?",
    )
    .bind(action_id)
    .fetch_optional(pool)
    .await?
    .map(|row| action_from_row(&row))
    .transpose()
}

/// Marks a follow-up action done or reopens it. Completion is a plain
/// flag on a user-curated entry: it changes neither the application's
/// stage nor any reporting obligation, and writes no history entry.
/// Returns whether a row was found.
pub async fn set_action_completed(
    pool: &SqlitePool,
    action_id: i64,
    completed: bool,
) -> Result<bool, sqlx::Error> {
    let statement = if completed {
        "UPDATE application_actions \
             SET completed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?"
    } else {
        "UPDATE application_actions SET completed_at = NULL WHERE id = ?"
    };
    let result = sqlx::query(statement).bind(action_id).execute(pool).await?;
    Ok(result.rows_affected() > 0)
}

/// Deletes a follow-up action outright. Actions carry no evidence, so
/// removal leaves nothing behind and writes no history entry. Returns
/// whether a row was found.
pub async fn delete_action(pool: &SqlitePool, action_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM application_actions WHERE id = ?")
        .bind(action_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

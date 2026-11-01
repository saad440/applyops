//! Append-only event history for the key moments in an application's life.
//! Rows are never updated or deleted; the trail starts when a change is recorded,
//! with no backfilled events for records that predate the trail.

use super::*;

use sqlx::SqliteConnection;

/// A confirmed submission was recorded.
pub const EVENT_SUBMITTED: &str = "application_submitted";
/// A recorded submission was corrected back to `draft`.
pub const EVENT_SUBMISSION_CORRECTED: &str = "application_submission_corrected";
/// A submitted application was genuinely retracted.
pub const EVENT_WITHDRAWN: &str = "application_withdrawn";
/// An employer outcome was recorded or changed on a submitted attempt.
pub const EVENT_OUTCOME_CHANGED: &str = "employer_outcome_changed";
/// A reporting status was assigned or corrected on an obligation.
pub const EVENT_REPORT_STATUS_CHANGED: &str = "obligation_status_changed";

/// One key moment as read from `application_events`.
#[derive(Debug)]
pub struct ApplicationEvent {
    /// Not displayed anywhere yet; read by the data-layer tests.
    #[allow(dead_code)]
    pub id: i64,
    /// Not displayed anywhere yet; kept so user-scoped queries stay
    /// possible without schema work. Read by the data-layer tests.
    #[allow(dead_code)]
    pub user_id: i64,
    pub event_type: String,
    /// Not displayed anywhere yet; kept so entity-scoped queries stay
    /// possible. Read by the data-layer tests.
    #[allow(dead_code)]
    pub entity_type: String,
    /// The application (or, later, opportunity/obligation) the event
    /// belongs to. Read by the data-layer tests.
    #[allow(dead_code)]
    pub entity_id: i64,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
}

/// The fields of one event-history entry.
pub(super) struct NewEvent<'a> {
    pub user_id: i64,
    pub event_type: &'a str,
    pub entity_type: &'a str,
    pub entity_id: i64,
    pub old_value: Option<&'a str>,
    pub new_value: Option<&'a str>,
    pub note: Option<&'a str>,
}

/// Inserts an event inside the caller's transaction. Old and new values
/// are sparse; the note carries the surrounding facts.
pub(super) async fn insert_event(
    conn: &mut SqliteConnection,
    event: NewEvent<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO application_events \
             (user_id, event_type, entity_type, entity_id, old_value, new_value, note) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(event.user_id)
    .bind(event.event_type)
    .bind(event.entity_type)
    .bind(event.entity_id)
    .bind(event.old_value)
    .bind(event.new_value)
    .bind(event.note)
    .execute(conn)
    .await?;
    Ok(())
}

/// Maps one `application_events` row into an [`ApplicationEvent`].
fn event_from_row(row: &SqliteRow) -> Result<ApplicationEvent, sqlx::Error> {
    Ok(ApplicationEvent {
        id: row.try_get("event_id")?,
        user_id: row.try_get("user_id")?,
        event_type: row.try_get("event_type")?,
        entity_type: row.try_get("entity_type")?,
        entity_id: row.try_get("entity_id")?,
        old_value: row.try_get("old_value")?,
        new_value: row.try_get("new_value")?,
        note: row.try_get("note")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Lists the events recorded for one opportunity's applications,
/// newest first. Application events join through their application
/// rows; reporting-obligation events join through the obligation of
/// one of those attempts.
pub async fn list_events_for_opportunity(
    pool: &SqlitePool,
    opportunity_id: i64,
    limit: i64,
) -> Result<Vec<ApplicationEvent>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT e.id AS event_id, e.user_id, e.event_type, e.entity_type, e.entity_id, \
                    e.old_value, e.new_value, e.note, e.created_at \
             FROM application_events e \
             JOIN applications a ON a.id = e.entity_id \
             WHERE e.entity_type = 'application' AND a.opportunity_id = ?1 \
             UNION ALL \
             SELECT e.id AS event_id, e.user_id, e.event_type, e.entity_type, e.entity_id, \
                    e.old_value, e.new_value, e.note, e.created_at \
             FROM application_events e \
             JOIN reporting_obligations o ON o.id = e.entity_id \
             JOIN applications a ON a.id = o.application_id \
             WHERE e.entity_type = 'obligation' AND a.opportunity_id = ?1 \
             ORDER BY event_id DESC LIMIT ?2",
    )
    .bind(opportunity_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.iter().map(event_from_row).collect()
}

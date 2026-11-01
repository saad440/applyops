//! Reporting obligations held toward an authority for a submitted
//! application: one workflow per application per kind, with an explicitly
//! assigned status that a submission never moves on its own. Unknown
//! (nothing reliably recorded) and pending (known to be outstanding) are
//! different states. Corrections are allowed and keep the previous value
//! in the event history; while an obligation is pending or reported, the
//! submitted-to-draft correction of its application is blocked.

use super::*;

/// Kinds of reporting obligations. The forms create and list only the
/// default kind; the other values are reserved for later use.
#[allow(dead_code)]
pub const KINDS: [&str; 4] = [
    "employment_agency",
    "jobcenter",
    "foreign_authority",
    "other",
];

/// The kind the forms work with.
pub const DEFAULT_KIND: &str = "employment_agency";

/// States of a reporting obligation. A submission is never marked as
/// reported on its own; only an explicit user confirmation moves a
/// status.
#[allow(dead_code)]
pub const STATUSES: [&str; 4] = ["pending", "reported", "not_required", "unknown"];

/// Ways a report was filed, when it was filed at all.
#[allow(dead_code)]
pub const METHODS: [&str; 3] = ["portal", "manual", "other"];

/// A stored reporting obligation as read from `reporting_obligations`.
#[derive(Debug)]
pub struct Obligation {
    /// Read by the data-layer tests.
    #[allow(dead_code)]
    pub id: i64,
    /// Read by the data-layer tests.
    #[allow(dead_code)]
    pub user_id: i64,
    pub application_id: i64,
    /// Read by the data-layer tests.
    #[allow(dead_code)]
    pub kind: String,
    pub status: String,
    /// Date-only calendar date the report was filed, or `None` while
    /// the status is not `reported`.
    pub reported_at: Option<String>,
    pub method: Option<String>,
    pub reference: Option<String>,
    pub recipient_name: Option<String>,
    pub notes: Option<String>,
    /// Read by the data-layer tests.
    #[allow(dead_code)]
    pub created_at: String,
}

/// Why a reporting status could not be recorded.
#[derive(Debug)]
pub enum ReportError {
    /// No application row with that id exists.
    ApplicationMissing,
    /// A reporting status exists only for submitted applications: a
    /// report describes a submission that actually happened.
    NotSubmitted { stage: String },
    /// The submitted input was rejected before any SQL ran.
    Validation(applications::SubmissionValidationError),
    /// SQLite rejected the change, for example on a check constraint.
    Database(sqlx::Error),
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplicationMissing => write!(f, "no such application"),
            Self::NotSubmitted { stage } => write!(
                f,
                "a reporting status requires a submitted application; this one is {stage}"
            ),
            Self::Validation(err) => write!(f, "{err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for ReportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(err) => Some(err),
            Self::Database(err) => Some(err),
            _ => None,
        }
    }
}

/// User-supplied data for one reporting-status save, validated before
/// any SQL runs.
#[derive(Debug)]
pub struct NewReport<'a> {
    pub status: &'a str,
    pub reported_at: Option<&'a str>,
    pub method: Option<&'a str>,
    pub reference: Option<&'a str>,
    pub recipient_name: Option<&'a str>,
    pub notes: Option<&'a str>,
}

/// Maps one `reporting_obligations` row into an [`Obligation`].
fn obligation_from_row(row: &SqliteRow) -> Result<Obligation, sqlx::Error> {
    Ok(Obligation {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        application_id: row.try_get("application_id")?,
        kind: row.try_get("kind")?,
        status: row.try_get("status")?,
        reported_at: row.try_get("reported_at")?,
        method: row.try_get("method")?,
        reference: row.try_get("reference")?,
        recipient_name: row.try_get("recipient_name")?,
        notes: row.try_get("notes")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Records or corrects the reporting status of a submitted application,
/// creating the obligation row of the given kind on first save.
/// Marking a report `reported` requires the calendar date it was filed;
/// moving away from `reported` clears the date from the row while the
/// event history keeps the previous facts. The change and its event are
/// written in one transaction.
pub async fn record_report(
    pool: &SqlitePool,
    application_id: i64,
    kind: &str,
    new: &NewReport<'_>,
) -> Result<(), ReportError> {
    if !KINDS.contains(&kind) {
        return Err(ReportError::Validation(
            applications::SubmissionValidationError {
                field: "kind",
                reason: "is not a recognized value",
            },
        ));
    }
    let status = new.status.trim();
    if !STATUSES.contains(&status) {
        return Err(ReportError::Validation(
            applications::SubmissionValidationError {
                field: "status",
                reason: "is not a recognized value",
            },
        ));
    }

    let method = new.method.map(str::trim).filter(|value| !value.is_empty());
    if let Some(method) = &method
        && !METHODS.contains(method)
    {
        return Err(ReportError::Validation(
            applications::SubmissionValidationError {
                field: "method",
                reason: "is not a recognized value",
            },
        ));
    }

    let reference = new
        .reference
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let notes = new.notes.map(str::trim).filter(|value| !value.is_empty());
    let recipient = new
        .recipient_name
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if kind == "other" && recipient.is_none() {
        return Err(ReportError::Validation(
            applications::SubmissionValidationError {
                field: "recipient_name",
                reason: "must not be empty for this kind",
            },
        ));
    }

    let submitted_date = new
        .reported_at
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let reported_at = if status == "reported" {
        match submitted_date {
            None => {
                return Err(ReportError::Validation(
                    applications::SubmissionValidationError {
                        field: "reported_at",
                        reason: "must not be empty",
                    },
                ));
            }
            Some(date) if !super::opportunities::is_iso_date(date) => {
                return Err(ReportError::Validation(
                    applications::SubmissionValidationError {
                        field: "reported_at",
                        reason: "must be a calendar date formatted YYYY-MM-DD",
                    },
                ));
            }
            Some(date) => Some(date),
        }
    } else {
        // A filed date without a reported status would contradict the
        // row; clearing it is recorded in the event note.
        None
    };

    let mut tx = pool.begin().await.map_err(ReportError::Database)?;
    let Some((user_id, stage)) =
        sqlx::query_as::<_, (i64, String)>("SELECT user_id, stage FROM applications WHERE id = ?")
            .bind(application_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(ReportError::Database)?
    else {
        return Err(ReportError::ApplicationMissing);
    };
    if stage != "submitted" {
        return Err(ReportError::NotSubmitted { stage });
    }

    let previous: Option<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, status, reported_at, recipient_name FROM reporting_obligations \
             WHERE application_id = ? AND kind = ?",
    )
    .bind(application_id)
    .bind(kind)
    .fetch_optional(&mut *tx)
    .await
    .map_err(ReportError::Database)?;

    let obligation_id = match &previous {
        Some((id, _, _, _)) => {
            sqlx::query(
                "UPDATE reporting_obligations \
                     SET status = ?, reported_at = ?, method = ?, reference = ?, \
                         recipient_name = ?, notes = ? \
                     WHERE id = ?",
            )
            .bind(status)
            .bind(reported_at)
            .bind(method)
            .bind(reference)
            .bind(recipient)
            .bind(notes)
            .bind(*id)
            .execute(&mut *tx)
            .await
            .map_err(ReportError::Database)?;
            *id
        }
        None => sqlx::query_scalar(
            "INSERT INTO reporting_obligations \
                     (user_id, application_id, kind, status, reported_at, method, \
                      reference, recipient_name, notes) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(application_id)
        .bind(kind)
        .bind(status)
        .bind(reported_at)
        .bind(method)
        .bind(reference)
        .bind(recipient)
        .bind(notes)
        .fetch_one(&mut *tx)
        .await
        .map_err(ReportError::Database)?,
    };

    // The note carries the surrounding facts: a date lost by leaving
    // the reported state, and recipient changes.
    let mut facts: Vec<String> = Vec::new();
    let recipient_changed = match &previous {
        Some((_, _, _, old_recipient)) => old_recipient.as_deref() != recipient,
        None => recipient.is_some(),
    };
    if let Some((_, old_status, old_reported_at, _)) = &previous
        && old_status != status
        && let Some(old_date) = old_reported_at
    {
        facts.push(format!("was reported_at={old_date}"));
    }
    if status == "reported" {
        let mut fact = format!("reported_at={}", reported_at.unwrap_or_default());
        if let Some(method) = &method {
            fact.push_str(&format!(" method={method}"));
        }
        facts.push(fact);
    }
    if recipient_changed {
        match recipient {
            Some(name) => facts.push(format!("recipient={name}")),
            None => facts.push("recipient cleared".to_string()),
        }
    }
    let note = if facts.is_empty() {
        None
    } else {
        Some(facts.join("; "))
    };

    super::application_events::insert_event(
        &mut tx,
        super::application_events::NewEvent {
            user_id,
            event_type: super::application_events::EVENT_REPORT_STATUS_CHANGED,
            entity_type: "obligation",
            entity_id: obligation_id,
            old_value: previous.as_ref().map(|(_, status, _, _)| status.as_str()),
            new_value: Some(status),
            note: note.as_deref(),
        },
    )
    .await
    .map_err(ReportError::Database)?;

    tx.commit().await.map_err(ReportError::Database)?;
    Ok(())
}

/// Fetches the reporting obligation of one application and kind, or
/// `None` when none exists yet.
pub async fn get_obligation(
    pool: &SqlitePool,
    application_id: i64,
    kind: &str,
) -> Result<Option<Obligation>, sqlx::Error> {
    sqlx::query(
        "SELECT id, user_id, application_id, kind, status, reported_at, method, \
                    reference, recipient_name, notes, created_at \
             FROM reporting_obligations WHERE application_id = ? AND kind = ?",
    )
    .bind(application_id)
    .bind(kind)
    .fetch_optional(pool)
    .await?
    .map(|row| obligation_from_row(&row))
    .transpose()
}

/// Lists the reporting obligations of all attempts of one opportunity,
/// newest first.
pub async fn list_obligations_for_opportunity(
    pool: &SqlitePool,
    opportunity_id: i64,
) -> Result<Vec<Obligation>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT o.id, o.user_id, o.application_id, o.kind, o.status, o.reported_at, \
                    o.method, o.reference, o.recipient_name, o.notes, o.created_at \
             FROM reporting_obligations o \
             JOIN applications a ON a.id = o.application_id \
             WHERE a.opportunity_id = ? ORDER BY o.id DESC",
    )
    .bind(opportunity_id)
    .fetch_all(pool)
    .await?;

    rows.iter().map(obligation_from_row).collect()
}

/// Backlog counts over all submitted attempts, computed from SQL data.
/// A submitted attempt with no obligation row counts as `unknown`,
/// because nothing reliable is recorded for it.
#[derive(Debug, Default)]
pub struct BacklogCounts {
    pub pending: i64,
    pub unknown: i64,
    pub reported: i64,
    pub not_required: i64,
}

/// One submitted attempt in the report backlog: outstanding work
/// (pending or unknown) with the parent opportunity for context.
#[derive(Debug)]
pub struct BacklogEntry {
    /// Read by the data-layer tests.
    #[allow(dead_code)]
    pub application_id: i64,
    pub opportunity_id: i64,
    pub organization: String,
    pub title: String,
    pub submitted_at: Option<String>,
    /// `None` when no obligation row exists yet; the backlog reads it
    /// as `unknown`.
    pub status: Option<String>,
}

/// Counts the reporting states of all submitted attempts, with separate
/// pending and unknown numbers.
pub async fn reporting_counts(pool: &SqlitePool) -> Result<BacklogCounts, sqlx::Error> {
    let row = sqlx::query(
        "SELECT \
                COALESCE(SUM(CASE WHEN ob.status = 'pending' THEN 1 ELSE 0 END), 0) AS pending, \
                COALESCE(SUM(CASE WHEN ob.status IS NULL OR ob.status = 'unknown' \
                    THEN 1 ELSE 0 END), 0) AS unknown, \
                COALESCE(SUM(CASE WHEN ob.status = 'reported' THEN 1 ELSE 0 END), 0) AS reported, \
                COALESCE(SUM(CASE WHEN ob.status = 'not_required' THEN 1 ELSE 0 END), 0) \
                    AS not_required \
             FROM applications a \
             LEFT JOIN reporting_obligations ob \
                    ON ob.application_id = a.id AND ob.kind = ? \
             WHERE a.stage = 'submitted'",
    )
    .bind(DEFAULT_KIND)
    .fetch_one(pool)
    .await?;

    Ok(BacklogCounts {
        pending: row.try_get("pending")?,
        unknown: row.try_get("unknown")?,
        reported: row.try_get("reported")?,
        not_required: row.try_get("not_required")?,
    })
}

/// Lists the submitted attempts whose report is still outstanding
/// (pending or unknown), oldest submission first. Closed attempts are
/// never listed.
pub async fn reporting_backlog(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<BacklogEntry>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT a.id AS application_id, a.opportunity_id, op.organization, op.title, \
                    a.submitted_at, ob.status \
             FROM applications a \
             JOIN opportunities op ON op.id = a.opportunity_id \
             LEFT JOIN reporting_obligations ob \
                    ON ob.application_id = a.id AND ob.kind = ? \
             WHERE a.stage = 'submitted' \
               AND (ob.status IS NULL OR ob.status IN ('pending', 'unknown')) \
             ORDER BY a.submitted_at ASC, a.id ASC LIMIT ?",
    )
    .bind(DEFAULT_KIND)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(BacklogEntry {
                application_id: row.try_get("application_id")?,
                opportunity_id: row.try_get("opportunity_id")?,
                organization: row.try_get("organization")?,
                title: row.try_get("title")?,
                submitted_at: row.try_get("submitted_at")?,
                status: row.try_get("status")?,
            })
        })
        .collect()
}

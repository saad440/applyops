//! Opportunity persistence: data model, vocabularies, and queries for the
//! `opportunities` table.

use super::*;

/// Opportunity stages of the discovery workflow.
pub const STAGES: [&str; 4] = ["discovered", "researching", "preparing", "archived"];

/// User-set priority labels. Priority records how important an
/// opportunity is to the user; it is independent of deadline urgency,
/// which is computed from the deadline date at render time and never
/// stored on the opportunity.
pub const PRIORITIES: [&str; 3] = ["p1", "p2", "p3"];

/// Estimated preparation effort levels. An unrated opportunity stores
/// `None`, never a placeholder value.
pub const EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// Kinds a recorded deadline can have. `None` means no verified closing
/// date; a kind is stored only together with the date it describes.
pub const DEADLINE_KINDS: [&str; 4] = [
    "hard_close",
    "priority_review",
    "first_review",
    "soft_target",
];

/// How reliably the recorded deadline is known. `None` means the claim
/// has not been assessed yet.
pub const DEADLINE_CONFIDENCES: [&str; 3] = ["verified", "unverified", "conflicting"];

/// A stored opportunity as read from the `opportunities` table.
#[derive(Debug)]
pub struct Opportunity {
    pub id: i64,
    /// Not displayed anywhere yet; kept so user-scoped queries stay
    /// possible without schema work. Read by the data-layer tests.
    #[allow(dead_code)]
    pub user_id: i64,
    pub organization: String,
    pub title: String,
    /// User-defined primary stream label, stored verbatim; see
    /// [`DEFAULT_STREAMS`] for the shipped suggestions.
    pub stream: String,
    pub city: Option<String>,
    pub country: Option<String>,
    pub priority: String,
    /// Optional self-assessed fit as a range within 1 to 5; `None` on
    /// both ends means the opportunity is unrated.
    pub fit_min: Option<f64>,
    pub fit_max: Option<f64>,
    /// Estimated preparation effort, or `None` while not estimated.
    pub effort: Option<String>,
    /// One- or two-sentence summary of the strongest fit and the
    /// biggest gap, or `None`.
    pub rationale: Option<String>,
    /// Date-only calendar deadline in `YYYY-MM-DD` form, or `None` when
    /// genuinely unknown; no cutoff time is invented for it.
    pub deadline_date: Option<String>,
    /// What the recorded deadline means, or `None` when there is no
    /// deadline date to describe.
    pub deadline_kind: Option<String>,
    /// How reliably the deadline is known, or `None` while it has not
    /// been assessed.
    pub deadline_confidence: Option<String>,
    /// The original advert or lead source; the external link shown for
    /// the opportunity.
    pub source_url: Option<String>,
    /// Fallback page of the employer for speculative leads without a
    /// concrete advert.
    pub recruitment_page_url: Option<String>,
    /// Notes on how the source and its dates were verified.
    pub source_notes: Option<String>,
    pub notes: Option<String>,
    pub stage: String,
    pub created_at: String,
}

/// User-supplied data for creating an opportunity, validated before
/// insert.
#[derive(Debug)]
pub struct NewOpportunity {
    pub user_id: i64,
    pub organization: String,
    pub title: String,
    /// User-defined primary stream label, stored verbatim; see
    /// [`DEFAULT_STREAMS`] for the shipped suggestions.
    pub stream: String,
    pub city: Option<String>,
    pub country: Option<String>,
    pub priority: String,
    pub fit_min: Option<f64>,
    pub fit_max: Option<f64>,
    pub effort: Option<String>,
    pub rationale: Option<String>,
    pub deadline_date: Option<String>,
    pub deadline_kind: Option<String>,
    pub deadline_confidence: Option<String>,
    pub source_url: Option<String>,
    pub recruitment_page_url: Option<String>,
    pub source_notes: Option<String>,
    pub notes: Option<String>,
    pub stage: String,
}

/// A single field-level validation failure on [`NewOpportunity`].
#[derive(Debug, PartialEq, Eq)]
pub struct OpportunityValidationError {
    pub field: &'static str,
    pub reason: &'static str,
}

impl std::fmt::Display for OpportunityValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid {}: {}", self.field, self.reason)
    }
}

impl std::error::Error for OpportunityValidationError {}

/// Why an opportunity could not be stored.
#[derive(Debug)]
pub enum OpportunityInsertError {
    /// The input was rejected before any SQL ran.
    Validation(OpportunityValidationError),
    /// SQLite rejected the row, for example on a foreign-key or check
    /// constraint.
    Database(sqlx::Error),
}

impl std::fmt::Display for OpportunityInsertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(err) => write!(f, "{err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for OpportunityInsertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(err) => Some(err),
            Self::Database(err) => Some(err),
        }
    }
}

/// Checks that a value is a plain `YYYY-MM-DD` calendar date. The month
/// and day ranges are checked here; full calendar validity is additionally
/// enforced by a CHECK constraint on the column itself. Shared with the
/// applications module for the confirmed submission date.
pub(super) fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }

    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if !digits(0..4) || !digits(5..7) || !digits(8..10) {
        return false;
    }

    let month: u32 = value[5..7].parse().unwrap_or(0);
    let day: u32 = value[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// Checks that a value looks like a web address: an `http` or `https`
/// scheme followed by something beyond the scheme itself. The rest of
/// the address is not parsed further.
fn is_http_url(value: &str) -> bool {
    let lowercase = value.to_lowercase();
    for scheme in ["http://", "https://"] {
        if let Some(rest) = lowercase.strip_prefix(scheme) {
            return !rest.is_empty();
        }
    }
    false
}

/// Validates user-supplied opportunity data. Required text must be
/// non-empty after trimming; the stream is a user-defined label and is
/// only checked for emptiness, while priority, stage, effort, deadline
/// kind, and deadline confidence are validated against their generic
/// vocabularies. Optional fields stay optional, a missing deadline is
/// `None` never a placeholder date, recorded URLs must be http or
/// https web addresses, the fit values must lie within 1 to 5 with the
/// minimum not above the maximum, and a deadline kind or confidence is
/// only accepted together with the deadline date it describes.
pub fn validate_new_opportunity(new: &NewOpportunity) -> Result<(), OpportunityValidationError> {
    fn recognized_code(
        value: &str,
        field: &'static str,
        allowed: &[&str],
    ) -> Result<(), OpportunityValidationError> {
        if allowed.contains(&value) {
            Ok(())
        } else {
            Err(OpportunityValidationError {
                field,
                reason: "is not a recognized value",
            })
        }
    }

    if new.organization.trim().is_empty() {
        return Err(OpportunityValidationError {
            field: "organization",
            reason: "must not be empty",
        });
    }
    if new.title.trim().is_empty() {
        return Err(OpportunityValidationError {
            field: "title",
            reason: "must not be empty",
        });
    }

    if new.stream.trim().is_empty() {
        return Err(OpportunityValidationError {
            field: "stream",
            reason: "must not be empty",
        });
    }

    recognized_code(&new.priority, "priority", &PRIORITIES)?;
    recognized_code(&new.stage, "stage", &STAGES)?;

    for (field, value) in [("fit_min", new.fit_min), ("fit_max", new.fit_max)] {
        if let Some(value) = value
            && !(1.0..=5.0).contains(&value)
        {
            return Err(OpportunityValidationError {
                field,
                reason: "must be a number between 1 and 5",
            });
        }
    }
    if let (Some(min), Some(max)) = (new.fit_min, new.fit_max)
        && min > max
    {
        return Err(OpportunityValidationError {
            field: "fit_max",
            reason: "must not be below the fit minimum",
        });
    }

    if let Some(effort) = &new.effort {
        recognized_code(effort, "effort", &EFFORTS)?;
    }

    // The rationale is meant as one or two sentences, not a second
    // notes column.
    if let Some(rationale) = &new.rationale
        && rationale.chars().count() > 500
    {
        return Err(OpportunityValidationError {
            field: "rationale",
            reason: "must be 500 characters or fewer",
        });
    }

    if let Some(date) = &new.deadline_date
        && !is_iso_date(date)
    {
        return Err(OpportunityValidationError {
            field: "deadline_date",
            reason: "must be a calendar date formatted YYYY-MM-DD",
        });
    }

    // A deadline kind or confidence describes the recorded deadline
    // date; without a date there is nothing to describe, and dropping
    // the value silently would hide the mismatch from the user.
    if new.deadline_kind.is_some() && new.deadline_date.is_none() {
        return Err(OpportunityValidationError {
            field: "deadline_kind",
            reason: "requires a deadline date",
        });
    }
    if new.deadline_confidence.is_some() && new.deadline_date.is_none() {
        return Err(OpportunityValidationError {
            field: "deadline_confidence",
            reason: "requires a deadline date",
        });
    }
    if let Some(kind) = &new.deadline_kind {
        recognized_code(kind, "deadline_kind", &DEADLINE_KINDS)?;
    }
    if let Some(confidence) = &new.deadline_confidence {
        recognized_code(confidence, "deadline_confidence", &DEADLINE_CONFIDENCES)?;
    }

    if let Some(url) = &new.source_url
        && !is_http_url(url)
    {
        return Err(OpportunityValidationError {
            field: "source_url",
            reason: "must be an http or https web address",
        });
    }

    if let Some(url) = &new.recruitment_page_url
        && !is_http_url(url)
    {
        return Err(OpportunityValidationError {
            field: "recruitment_page_url",
            reason: "must be an http or https web address",
        });
    }

    Ok(())
}

/// Maps one `opportunities` row into an [`Opportunity`].
pub(super) fn opportunity_from_row(row: &SqliteRow) -> Result<Opportunity, sqlx::Error> {
    Ok(Opportunity {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        organization: row.try_get("organization")?,
        title: row.try_get("title")?,
        stream: row.try_get("stream")?,
        city: row.try_get("city")?,
        country: row.try_get("country")?,
        priority: row.try_get("priority")?,
        fit_min: row.try_get("fit_min")?,
        fit_max: row.try_get("fit_max")?,
        effort: row.try_get("effort")?,
        rationale: row.try_get("rationale")?,
        deadline_date: row.try_get("deadline_date")?,
        deadline_kind: row.try_get("deadline_kind")?,
        deadline_confidence: row.try_get("deadline_confidence")?,
        source_url: row.try_get("source_url")?,
        recruitment_page_url: row.try_get("recruitment_page_url")?,
        source_notes: row.try_get("source_notes")?,
        notes: row.try_get("notes")?,
        stage: row.try_get("stage")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Stores a new opportunity and returns its generated id. Input is
/// validated first, so invalid priority or stage values never reach SQL;
/// organization, title, and the user-defined stream are stored trimmed.
pub async fn insert_opportunity(
    pool: &SqlitePool,
    new: &NewOpportunity,
) -> Result<i64, OpportunityInsertError> {
    validate_new_opportunity(new).map_err(OpportunityInsertError::Validation)?;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO opportunities \
             (user_id, organization, title, stream, city, country, priority, \
              fit_min, fit_max, effort, rationale, deadline_date, deadline_kind, \
              deadline_confidence, source_url, recruitment_page_url, source_notes, \
              notes, stage) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             RETURNING id",
    )
    .bind(new.user_id)
    .bind(new.organization.trim())
    .bind(new.title.trim())
    .bind(new.stream.trim())
    .bind(&new.city)
    .bind(&new.country)
    .bind(&new.priority)
    .bind(new.fit_min)
    .bind(new.fit_max)
    .bind(&new.effort)
    .bind(&new.rationale)
    .bind(&new.deadline_date)
    .bind(&new.deadline_kind)
    .bind(&new.deadline_confidence)
    .bind(&new.source_url)
    .bind(&new.recruitment_page_url)
    .bind(&new.source_notes)
    .bind(&new.notes)
    .bind(&new.stage)
    .fetch_one(pool)
    .await
    .map_err(OpportunityInsertError::Database)?;

    Ok(id)
}

/// Fetches one opportunity by id, or `None` when no such row exists.
pub async fn get_opportunity(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<Opportunity>, sqlx::Error> {
    sqlx::query(
        "SELECT id, user_id, organization, title, stream, city, country, priority, \
             fit_min, fit_max, effort, rationale, deadline_date, deadline_kind, \
             deadline_confidence, source_url, recruitment_page_url, source_notes, \
             notes, stage, created_at \
             FROM opportunities WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(|row| opportunity_from_row(&row))
    .transpose()
}

/// Lists opportunities newest first, up to `limit` rows.
pub async fn list_opportunities(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<Opportunity>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, user_id, organization, title, stream, city, country, priority, \
             fit_min, fit_max, effort, rationale, deadline_date, deadline_kind, \
             deadline_confidence, source_url, recruitment_page_url, source_notes, \
             notes, stage, created_at \
             FROM opportunities ORDER BY created_at DESC, id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.iter().map(opportunity_from_row).collect()
}

/// Escapes SQL LIKE wildcards in user-supplied search text so a search
/// for a literal `%` or `_` matches itself, not any character.
fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Lists opportunities whose organization, title, notes, or stream
/// contain the search text, case-insensitively for ASCII letters,
/// newest first.
pub async fn search_opportunities(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> Result<Vec<Opportunity>, sqlx::Error> {
    let pattern = format!("%{}%", escape_like(query.trim()));
    let rows = sqlx::query(
        "SELECT id, user_id, organization, title, stream, city, country, priority, \
             fit_min, fit_max, effort, rationale, deadline_date, deadline_kind, \
             deadline_confidence, source_url, recruitment_page_url, source_notes, \
             notes, stage, created_at \
             FROM opportunities \
             WHERE organization LIKE ?1 ESCAPE '\\' \
                OR title LIKE ?1 ESCAPE '\\' \
                OR notes LIKE ?1 ESCAPE '\\' \
                OR stream LIKE ?1 ESCAPE '\\' \
             ORDER BY created_at DESC, id DESC LIMIT ?2",
    )
    .bind(&pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.iter().map(opportunity_from_row).collect()
}

/// Updates an opportunity with new values and returns its id, or `None`
/// when no row with that id exists. Input is validated first, so
/// invalid priority or stage values never reach SQL.
pub async fn update_opportunity(
    pool: &SqlitePool,
    id: i64,
    new: &NewOpportunity,
) -> Result<Option<i64>, OpportunityInsertError> {
    validate_new_opportunity(new).map_err(OpportunityInsertError::Validation)?;

    let result = sqlx::query(
        "UPDATE opportunities \
             SET user_id = ?, organization = ?, title = ?, stream = ?, city = ?, \
                 country = ?, priority = ?, fit_min = ?, fit_max = ?, effort = ?, \
                 rationale = ?, deadline_date = ?, deadline_kind = ?, \
                 deadline_confidence = ?, source_url = ?, recruitment_page_url = ?, \
                 source_notes = ?, notes = ?, stage = ? \
             WHERE id = ?",
    )
    .bind(new.user_id)
    .bind(new.organization.trim())
    .bind(new.title.trim())
    .bind(new.stream.trim())
    .bind(&new.city)
    .bind(&new.country)
    .bind(&new.priority)
    .bind(new.fit_min)
    .bind(new.fit_max)
    .bind(&new.effort)
    .bind(&new.rationale)
    .bind(&new.deadline_date)
    .bind(&new.deadline_kind)
    .bind(&new.deadline_confidence)
    .bind(&new.source_url)
    .bind(&new.recruitment_page_url)
    .bind(&new.source_notes)
    .bind(&new.notes)
    .bind(&new.stage)
    .bind(id)
    .execute(pool)
    .await
    .map_err(OpportunityInsertError::Database)?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }
    Ok(Some(id))
}

/// Returns the total number of stored opportunities.
// Used as an assertion helper by the tests for now.
// The dashboard counts will call it once the overview lands.
#[allow(dead_code)]
pub async fn count_opportunities(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM opportunities")
        .fetch_one(pool)
        .await?;
    Ok(count)
}

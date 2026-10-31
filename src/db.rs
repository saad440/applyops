//! SQLite data access for ApplyOps. Every SQL statement lives in this module
//! so queries stay in one place and can later be scoped per user in one pass.

use std::path::Path;

use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
};

/// Builds the connect options for the local database file: creates the file
/// when missing, uses WAL journaling, and enforces foreign keys.
pub fn connect_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
}

/// Opens the connection pool for the database at `path`.
pub async fn connect(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .connect_with(connect_options(path))
        .await
}

/// Directory of SQL migration files, resolved relative to the working directory.
const MIGRATIONS_DIR: &str = "migrations";

/// Loads the SQL migrations from `MIGRATIONS_DIR` and applies them to the
/// database.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate::Migrator::new(Path::new(MIGRATIONS_DIR))
        .await?
        .run(pool)
        .await
}

/// A diagnostic note as stored in `diagnostic_notes`.
#[derive(Debug)]
pub struct DiagnosticNote {
    /// Not displayed anywhere yet; kept so user-scoped queries stay possible
    /// without schema work. Read by the data-layer tests.
    #[allow(dead_code)]
    pub user_id: i64,
    pub note: String,
    pub created_at: String,
}

/// Stores a diagnostic note.
pub async fn insert_note(pool: &SqlitePool, user_id: i64, note: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO diagnostic_notes (user_id, note) VALUES (?, ?)")
        .bind(user_id)
        .bind(note)
        .execute(pool)
        .await?;
    Ok(())
}

/// Returns the most recent diagnostic notes, newest first.
pub async fn recent_notes(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<DiagnosticNote>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT user_id, note, created_at \
         FROM diagnostic_notes ORDER BY id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(DiagnosticNote {
                user_id: row.try_get("user_id")?,
                note: row.try_get("note")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

/// Returns the total number of diagnostic notes.
pub async fn count_notes(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM diagnostic_notes")
        .fetch_one(pool)
        .await?;
    Ok(count)
}

/// Opportunity persistence: data model, vocabularies, and queries for the
/// `opportunities` table.
pub mod opportunities {
    use super::*;

    /// Opportunity stages of the discovery workflow.
    pub const STAGES: [&str; 4] = ["discovered", "researching", "preparing", "archived"];

    /// User-set priority labels. Priority records how important an
    /// opportunity is to the user; it is independent of deadline urgency,
    /// which is computed from the deadline date at render time and never
    /// stored on the opportunity.
    pub const PRIORITIES: [&str; 3] = ["p1", "p2", "p3"];

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
        /// Date-only calendar deadline in `YYYY-MM-DD` form, or `None` when
        /// genuinely unknown; no cutoff time is invented for it.
        pub deadline_date: Option<String>,
        pub source_url: Option<String>,
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
        pub deadline_date: Option<String>,
        pub source_url: Option<String>,
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
    /// only checked for emptiness, while priority and stage are validated
    /// against their generic vocabularies. Optional fields stay optional, a
    /// missing deadline is `None` never a placeholder date, and a recorded
    /// source URL must be an http or https web address.
    pub fn validate_new_opportunity(
        new: &NewOpportunity,
    ) -> Result<(), OpportunityValidationError> {
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

        if let Some(date) = &new.deadline_date
            && !is_iso_date(date)
        {
            return Err(OpportunityValidationError {
                field: "deadline_date",
                reason: "must be a calendar date formatted YYYY-MM-DD",
            });
        }

        if let Some(url) = &new.source_url
            && !is_http_url(url)
        {
            return Err(OpportunityValidationError {
                field: "source_url",
                reason: "must be an http or https web address",
            });
        }

        Ok(())
    }

    /// Maps one `opportunities` row into an [`Opportunity`].
    fn opportunity_from_row(row: &SqliteRow) -> Result<Opportunity, sqlx::Error> {
        Ok(Opportunity {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            organization: row.try_get("organization")?,
            title: row.try_get("title")?,
            stream: row.try_get("stream")?,
            city: row.try_get("city")?,
            country: row.try_get("country")?,
            priority: row.try_get("priority")?,
            deadline_date: row.try_get("deadline_date")?,
            source_url: row.try_get("source_url")?,
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
              deadline_date, source_url, notes, stage) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             RETURNING id",
        )
        .bind(new.user_id)
        .bind(new.organization.trim())
        .bind(new.title.trim())
        .bind(new.stream.trim())
        .bind(&new.city)
        .bind(&new.country)
        .bind(&new.priority)
        .bind(&new.deadline_date)
        .bind(&new.source_url)
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
             deadline_date, source_url, notes, stage, created_at \
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
             deadline_date, source_url, notes, stage, created_at \
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
             deadline_date, source_url, notes, stage, created_at \
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
                 country = ?, priority = ?, deadline_date = ?, source_url = ?, \
                 notes = ?, stage = ? \
             WHERE id = ?",
        )
        .bind(new.user_id)
        .bind(new.organization.trim())
        .bind(new.title.trim())
        .bind(new.stream.trim())
        .bind(&new.city)
        .bind(&new.country)
        .bind(&new.priority)
        .bind(&new.deadline_date)
        .bind(&new.source_url)
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
}

/// User-defined stream vocabulary. Streams are plain labels the user creates;
/// this table remembers them so forms can offer the known ones back, and the
/// shipped defaults seed a fresh installation only while the table is empty.
pub mod streams {
    use super::*;

    /// Stream labels shipped as defaults for a fresh installation. They are
    /// bootstrap suggestions only and are never enforced.
    pub const DEFAULT_STREAMS: [&str; 3] = ["Physics", "Scientific Computing", "IT"];

    /// A stored stream label as read from the `streams` table.
    #[derive(Debug)]
    pub struct Stream {
        /// Not displayed anywhere yet; read by the data-layer tests.
        #[allow(dead_code)]
        pub id: i64,
        /// Not displayed anywhere yet; kept so user-scoped queries stay
        /// possible without schema work.
        #[allow(dead_code)]
        pub user_id: i64,
        pub name: String,
        /// Not displayed anywhere yet; read by the data-layer tests.
        #[allow(dead_code)]
        pub created_at: String,
    }

    /// Maps one `streams` row into a [`Stream`].
    fn stream_from_row(row: &SqliteRow) -> Result<Stream, sqlx::Error> {
        Ok(Stream {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            name: row.try_get("name")?,
            created_at: row.try_get("created_at")?,
        })
    }

    /// Lists the known stream labels of a user, alphabetical and
    /// case-insensitive.
    pub async fn list_streams(pool: &SqlitePool, user_id: i64) -> Result<Vec<Stream>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, user_id, name, created_at \
             FROM streams WHERE user_id = ? ORDER BY name COLLATE NOCASE",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;

        rows.iter().map(stream_from_row).collect()
    }

    /// Stores a stream label and returns its id. An identical label that
    /// already exists for the user is left untouched and its id returned.
    pub async fn insert_stream(
        pool: &SqlitePool,
        user_id: i64,
        name: &str,
    ) -> Result<i64, sqlx::Error> {
        let name = name.trim();
        if let Some(id) = stream_id(pool, user_id, name).await? {
            return Ok(id);
        }

        sqlx::query_scalar("INSERT INTO streams (user_id, name) VALUES (?, ?) RETURNING id")
            .bind(user_id)
            .bind(name)
            .fetch_one(pool)
            .await
    }

    /// Returns the id of an existing stream label, if present.
    async fn stream_id(
        pool: &SqlitePool,
        user_id: i64,
        name: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar("SELECT id FROM streams WHERE user_id = ? AND name = ?")
            .bind(user_id)
            .bind(name)
            .fetch_optional(pool)
            .await
    }

    /// Seeds the shipped default streams while the user has none, returning
    /// how many rows were inserted.
    pub async fn seed_streams_if_empty(
        pool: &SqlitePool,
        user_id: i64,
        defaults: &[&str],
    ) -> Result<usize, sqlx::Error> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM streams WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await?;
        if count > 0 {
            return Ok(0);
        }

        let mut inserted = 0;
        for name in defaults {
            insert_stream(pool, user_id, name).await?;
            inserted += 1;
        }
        Ok(inserted)
    }
}

/// Per-user vocabulary of known authority names behind the report form's
/// selector: shipped defaults seed a fresh installation, names typed on
/// first use are remembered, and no obligation row references a label —
/// removing one affects future suggestions only.
pub mod authorities {
    use super::*;

    /// Authority names shipped as defaults for a fresh installation. They
    /// are bootstrap suggestions only.
    pub const DEFAULT_AUTHORITIES: [&str; 2] = ["Agentur für Arbeit", "Jobcenter"];

    /// A stored authority label as read from `known_authorities`.
    #[derive(Debug)]
    pub struct Authority {
        /// Read by the data-layer tests.
        #[allow(dead_code)]
        pub id: i64,
        /// Read by the data-layer tests.
        #[allow(dead_code)]
        pub user_id: i64,
        pub name: String,
        /// Whether the label is a shipped default rather than a learned
        /// one. Read by the data-layer tests.
        #[allow(dead_code)]
        pub is_seed: Option<bool>,
    }

    /// Maps one `known_authorities` row into an [`Authority`].
    fn authority_from_row(row: &SqliteRow) -> Result<Authority, sqlx::Error> {
        Ok(Authority {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            name: row.try_get("name")?,
            is_seed: row.try_get("is_seed")?,
        })
    }

    /// Lists the known authority labels of a user, alphabetical and
    /// case-insensitive.
    pub async fn list_authorities(
        pool: &SqlitePool,
        user_id: i64,
    ) -> Result<Vec<Authority>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, user_id, name, is_seed \
             FROM known_authorities WHERE user_id = ? ORDER BY name",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;

        rows.iter().map(authority_from_row).collect()
    }

    /// Stores an authority label and returns its id, or `None` when the
    /// label is empty. A label differing only in ASCII letter case counts
    /// as the same one; the most recently used spelling is kept, so
    /// re-typing a corrected capitalisation replaces the stored one.
    pub async fn insert_authority(
        pool: &SqlitePool,
        user_id: i64,
        name: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(None);
        }

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO known_authorities (user_id, name) VALUES (?, ?) \
             ON CONFLICT (user_id, name) DO UPDATE SET name = excluded.name \
             RETURNING id",
        )
        .bind(user_id)
        .bind(name)
        .fetch_one(pool)
        .await?;
        Ok(Some(id))
    }

    /// Seeds the shipped default authority labels while the user has none,
    /// returning how many rows were inserted. Because seeding triggers on
    /// an empty table, deleting every label restores the defaults on next
    /// use: an intentional reset path.
    pub async fn seed_authorities_if_empty(
        pool: &SqlitePool,
        user_id: i64,
        defaults: &[&str],
    ) -> Result<usize, sqlx::Error> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM known_authorities WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(pool)
                .await?;
        if count > 0 {
            return Ok(0);
        }

        let mut inserted = 0;
        for name in defaults {
            sqlx::query("INSERT INTO known_authorities (user_id, name, is_seed) VALUES (?, ?, 1)")
                .bind(user_id)
                .bind(name)
                .execute(pool)
                .await?;
            inserted += 1;
        }
        Ok(inserted)
    }
}

/// Application persistence: attempts per opportunity, their stages, the
/// confirmed submission facts, and the stage transition rules. An
/// opportunity may carry several attempts over time, but never two active
/// ones. Submission, submission corrections, and withdrawal are recorded
/// in the event history; in-progress corrections between draft and
/// preparing are not.
pub mod applications {
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
    pub async fn withdraw(
        pool: &SqlitePool,
        application_id: i64,
    ) -> Result<(), StageTransitionError> {
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
        let Some((id, user_id, stage, previous)): Option<(i64, i64, String, String)> =
            sqlx::query_as(
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
}

/// Reporting obligations held toward an authority for a submitted
/// application: one workflow per application per kind, with an explicitly
/// assigned status that a submission never moves on its own. Unknown
/// (nothing reliably recorded) and pending (known to be outstanding) are
/// different states. Corrections are allowed and keep the previous value
/// in the event history; while an obligation is pending or reported, the
/// submitted-to-draft correction of its application is blocked.
pub mod obligations {
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
        let Some((user_id, stage)) = sqlx::query_as::<_, (i64, String)>(
            "SELECT user_id, stage FROM applications WHERE id = ?",
        )
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
}

/// Append-only event history for the key moments in an application's life.
/// Rows are never updated or deleted; the trail starts when a change is recorded,
/// with no backfilled events for records that predate the trail.
pub mod application_events {
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
}

/// Opens a single-connection pool over an in-memory database, for tests.
/// In-memory SQLite databases are per connection, so the pool must never
/// open a second one. Foreign keys are enabled to match file-based connects.
#[cfg(test)]
pub async fn connect_in_memory() -> Result<SqlitePool, sqlx::Error> {
    let options = "sqlite::memory:"
        .parse::<SqliteConnectOptions>()?
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
}

#[cfg(test)]
mod tests;

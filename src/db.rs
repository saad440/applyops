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
    /// enforced by a CHECK constraint on the column itself.
    fn is_iso_date(value: &str) -> bool {
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

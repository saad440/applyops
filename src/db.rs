//! SQLite data access for ApplyOps. Every SQL statement lives in this module
//! so queries stay in one place and can later be scoped per user in one pass.

use std::path::Path;

use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

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

/// Opens a single-connection pool over an in-memory database, for tests.
/// In-memory SQLite databases are per connection, so the pool must never
/// open a second one.
#[cfg(test)]
pub async fn connect_in_memory() -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
}

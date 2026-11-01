//! User-defined stream vocabulary. Streams are plain labels the user creates;
//! this table remembers them so forms can offer the known ones back, and the
//! shipped defaults seed a fresh installation only while the table is empty.

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

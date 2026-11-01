//! Per-user vocabulary of known authority names behind the report form's
//! selector: shipped defaults seed a fresh installation, names typed on
//! first use are remembered, and no obligation row references a label —
//! removing one affects future suggestions only.

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
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM known_authorities WHERE user_id = ?")
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

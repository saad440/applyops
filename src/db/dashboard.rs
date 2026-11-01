//! Dashboard read models. These queries shape the pages that summarize
//! across concepts; single-concept pages read from their own modules.

use super::*;

/// Lists actionable opportunities that have a recorded deadline, soonest
/// first. Archived opportunities and undated leads are not deadline
/// material, and an opportunity whose latest application attempt is
/// `submitted` or `withdrawn` has left the actionable deadline view; a
/// correction back to `draft` returns it. Entries on the same date order
/// by priority, then organization and title case-insensitively, so the
/// order never depends on insertion luck.
pub async fn upcoming_deadlines(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<super::opportunities::Opportunity>, sqlx::Error> {
    let rows = sqlx::query(
            "SELECT o.id, o.user_id, o.organization, o.title, o.stream, o.city, o.country, \
             o.priority, o.fit_min, o.fit_max, o.effort, o.rationale, o.deadline_date, \
             o.deadline_kind, o.deadline_confidence, o.source_url, o.recruitment_page_url, \
             o.source_notes, o.notes, o.stage, o.created_at \
             FROM opportunities o \
             WHERE o.deadline_date IS NOT NULL \
               AND o.stage <> 'archived' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM applications latest \
                   WHERE latest.opportunity_id = o.id \
                     AND latest.id = (SELECT MAX(newer.id) FROM applications newer \
                                      WHERE newer.opportunity_id = o.id) \
                     AND latest.stage IN ('submitted', 'withdrawn') \
               ) \
             ORDER BY o.deadline_date ASC, \
                      CASE o.priority WHEN 'p1' THEN 1 WHEN 'p2' THEN 2 WHEN 'p3' THEN 3 ELSE 4 END ASC, \
                      o.organization COLLATE NOCASE ASC, \
                      o.title COLLATE NOCASE ASC, \
                      o.id ASC \
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;

    rows.iter()
        .map(super::opportunities::opportunity_from_row)
        .collect()
}

/// One current submitted attempt with its opportunity context, for the
/// dashboard's submitted summary.
pub struct SubmittedRow {
    pub opportunity_id: i64,
    pub organization: String,
    pub title: String,
    pub stream: String,
    pub submitted_at: Option<String>,
    pub employer_outcome: String,
    pub outcome_at: Option<String>,
}

/// Lists the attempts currently in `submitted` whose opportunity's latest
/// attempt is this one — the entries awaiting a response or an outcome,
/// newest submission first. A correction or withdrawal moves the
/// opportunity out of the list; its history stays on the details pages.
pub async fn submitted_latest(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<SubmittedRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT o.id AS opportunity_id, o.organization, o.title, o.stream, \
                a.submitted_at, a.employer_outcome, a.outcome_at \
             FROM applications a \
             JOIN opportunities o ON o.id = a.opportunity_id \
             WHERE a.stage = 'submitted' \
               AND a.id = (SELECT MAX(newer.id) FROM applications newer \
                           WHERE newer.opportunity_id = a.opportunity_id) \
             ORDER BY a.submitted_at DESC, a.id DESC \
             LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(SubmittedRow {
                opportunity_id: row.try_get("opportunity_id")?,
                organization: row.try_get("organization")?,
                title: row.try_get("title")?,
                stream: row.try_get("stream")?,
                submitted_at: row.try_get("submitted_at")?,
                employer_outcome: row.try_get("employer_outcome")?,
                outcome_at: row.try_get("outcome_at")?,
            })
        })
        .collect()
}

/// Lists the open-ended leads: actionable opportunities without a recorded
/// deadline whose latest application attempt is not submitted or
/// withdrawn. Priority, then organization and title, decide the order.
pub async fn open_leads(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<super::opportunities::Opportunity>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT o.id, o.user_id, o.organization, o.title, o.stream, o.city, o.country, \
                o.priority, o.fit_min, o.fit_max, o.effort, o.rationale, o.deadline_date, \
                o.deadline_kind, o.deadline_confidence, o.source_url, o.recruitment_page_url, \
                o.source_notes, o.notes, o.stage, o.created_at \
             FROM opportunities o \
             WHERE o.deadline_date IS NULL \
               AND o.stage <> 'archived' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM applications latest \
                   WHERE latest.opportunity_id = o.id \
                     AND latest.id = (SELECT MAX(newer.id) FROM applications newer \
                                      WHERE newer.opportunity_id = o.id) \
                     AND latest.stage IN ('submitted', 'withdrawn') \
               ) \
             ORDER BY CASE o.priority WHEN 'p1' THEN 1 WHEN 'p2' THEN 2 WHEN 'p3' THEN 3 ELSE 4 END ASC, \
                      o.organization COLLATE NOCASE ASC, o.title COLLATE NOCASE ASC, o.id ASC \
             LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(super::opportunities::opportunity_from_row)
        .collect()
}

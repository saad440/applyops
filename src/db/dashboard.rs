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

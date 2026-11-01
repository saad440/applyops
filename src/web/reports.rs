//! The reporting backlog page over submitted applications and their
//! reporting obligations.

use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::{AppState, HtmlTemplate, db};

use super::display::{display_label, format_date_human};

/// The report backlog page: submitted attempts whose report is pending or
/// not yet recorded, with separate counts.
#[derive(Template)]
#[template(path = "report_backlog.html")]
struct ReportBacklogTemplate {
    counts: db::obligations::BacklogCounts,
    entries: Vec<BacklogEntryVm>,
}

/// One backlog row prepared for display.
struct BacklogEntryVm {
    opportunity_id: i64,
    heading: String,
    submitted_display: String,
    status_display: String,
}

/// `GET /reports` — the report backlog: submitted attempts whose report is
/// pending or not yet recorded, with separate counts from SQL data.
pub async fn report_backlog(State(state): State<AppState>) -> Response {
    let counts = match db::obligations::reporting_counts(&state.db).await {
        Ok(counts) => counts,
        Err(err) => {
            tracing::error!(%err, "failed to count reporting states");
            db::obligations::BacklogCounts::default()
        }
    };

    let entries = match db::obligations::reporting_backlog(&state.db, 100).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::error!(%err, "failed to load the report backlog");
            Vec::new()
        }
    };

    let entry_views: Vec<BacklogEntryVm> = entries
        .iter()
        .map(|entry| BacklogEntryVm {
            opportunity_id: entry.opportunity_id,
            heading: format!("{} — {}", entry.organization, entry.title),
            submitted_display: entry
                .submitted_at
                .as_deref()
                .map(format_date_human)
                .unwrap_or_else(|| "Unknown date".to_string()),
            status_display: entry
                .status
                .as_deref()
                .map(display_label)
                .unwrap_or_else(|| "Unknown".to_string()),
        })
        .collect();

    HtmlTemplate(ReportBacklogTemplate {
        counts,
        entries: entry_views,
    })
    .into_response()
}

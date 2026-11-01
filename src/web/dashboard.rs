//! The dashboard page: a deadline timeline whose columns come from the
//! streams that actually have deadline entries. The page stores nothing of
//! its own: every request re-derives what it shows from the same stored
//! records the other pages edit, so no second copy can drift out of date.

use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::{AppState, HtmlTemplate, db};

use super::display::{
    civil_from_days, countdown_label, display_label, format_date_human, format_deadline_detail,
    format_fit, format_location, parse_iso_day_number, today_day_number,
};

/// Width of the fixed date gutter in the timeline grid, in rem.
const DATE_GUTTER_REM: i32 = 7;

/// Dashboard page: the deadline timeline, one row per deadline date and one
/// column per stream that has deadline entries.
#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    as_of_display: String,
    entry_count: i64,
    /// CSS `grid-template-columns` custom property for the timeline: the
    /// date gutter plus one flexible column per stream.
    grid_template: String,
    /// Stream labels of the timeline columns, case-insensitively sorted.
    /// A user-defined column order can replace this later.
    columns: Vec<String>,
    rows: Vec<TimelineRowVm>,
}

/// One date row of the timeline: the date, its countdown, and one cell per
/// stream column in column order. Empty cells stay in place so the grid
/// lines up across rows.
pub struct TimelineRowVm {
    /// Day number of the row's deadline, for grouping while assembling.
    pub day: i64,
    pub date_display: String,
    pub countdown_display: String,
    pub overdue: bool,
    pub cells: Vec<TimelineCellVm>,
}

/// One stream column of a date row.
pub struct TimelineCellVm {
    pub entries: Vec<DeadlineEntryVm>,
}

/// One deadline card on the timeline, prepared for display. The internal
/// link and the external source link stay separate, and the countdown is
/// computed against a caller-supplied day so tests can pin urgency to a
/// known date.
pub struct DeadlineEntryVm {
    pub opportunity_id: i64,
    /// The stream label, shown as a chip when the narrow layout stacks the
    /// columns away.
    pub stream: String,
    pub heading: String,
    /// Priority and location in one line; the stream lives on the column
    /// header, so it is not repeated here.
    pub subtitle: String,
    /// The stored priority code; validated vocabulary, used as a CSS class.
    pub priority_class: String,
    /// The deadline's kind and confidence, e.g. `Hard close · Verified`.
    pub deadline_detail: Option<String>,
    rationale: Option<String>,
    /// Fit and effort in one line, or `None` while both are unrated.
    pub meta_display: Option<String>,
    pub external_link: Option<ExternalLinkVm>,
}

/// The one external link on a timeline card: the recorded advert, or the
/// recruitment page as a fallback, or `None` when neither exists. It stays
/// separate from the internal details link.
pub struct ExternalLinkVm {
    pub label: String,
    pub url: String,
}

impl DeadlineEntryVm {
    /// Prepares one timeline card from stored data; `as_of` is the day
    /// number the countdown is measured against.
    pub fn from_opportunity(opportunity: &db::opportunities::Opportunity) -> Self {
        let mut subtitle = opportunity.priority.to_uppercase();
        if let Some(place) =
            format_location(opportunity.city.as_deref(), opportunity.country.as_deref())
        {
            subtitle.push_str(&format!(" · {place}"));
        }

        let mut meta_parts = Vec::new();
        if let Some(fit) = format_fit(opportunity.fit_min, opportunity.fit_max) {
            meta_parts.push(format!("Fit {fit}"));
        }
        if let Some(effort) = opportunity.effort.as_deref().map(display_label) {
            meta_parts.push(format!("Effort {effort}"));
        }

        // The external link is the recorded advert when present; the
        // recruitment page is the fallback for speculative leads. Without
        // either, no link is shown rather than an invented one.
        let external = opportunity
            .source_url
            .as_deref()
            .map(|url| ("Source", url))
            .or_else(|| {
                opportunity
                    .recruitment_page_url
                    .as_deref()
                    .map(|url| ("Recruitment page", url))
            });

        Self {
            opportunity_id: opportunity.id,
            stream: opportunity.stream.clone(),
            heading: format!("{} — {}", opportunity.organization, opportunity.title),
            subtitle,
            priority_class: opportunity.priority.clone(),
            deadline_detail: format_deadline_detail(
                opportunity.deadline_kind.as_deref(),
                opportunity.deadline_confidence.as_deref(),
            ),
            rationale: opportunity.rationale.clone(),
            meta_display: if meta_parts.is_empty() {
                None
            } else {
                Some(meta_parts.join(" · "))
            },
            external_link: external.map(|(label, url)| ExternalLinkVm {
                label: label.to_string(),
                url: url.to_string(),
            }),
        }
    }
}

/// Groups the ordered deadline entries into timeline columns and date rows.
/// Columns are the stream labels that actually occur among the entries,
/// case-insensitively sorted; rows are the distinct deadline dates in
/// ascending order. The query orders entries by date, then priority, then
/// organization and title, and that order is preserved inside each cell.
pub fn assemble_timeline(
    opportunities: &[db::opportunities::Opportunity],
    as_of: i64,
) -> (Vec<String>, Vec<TimelineRowVm>) {
    let mut columns: Vec<String> = Vec::new();
    for opportunity in opportunities {
        if !columns.iter().any(|known| known == &opportunity.stream) {
            columns.push(opportunity.stream.clone());
        }
    }
    columns.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));

    let mut rows: Vec<TimelineRowVm> = Vec::new();
    for opportunity in opportunities {
        let Some(day) = opportunity
            .deadline_date
            .as_deref()
            .and_then(parse_iso_day_number)
        else {
            continue;
        };
        let date_display = format_date_human(opportunity.deadline_date.as_deref().unwrap_or(""));

        let row = match rows.last_mut() {
            // The query sorts by deadline date, so equal dates are adjacent.
            Some(last) if last.day == day => last,
            _ => {
                rows.push(TimelineRowVm {
                    day,
                    date_display,
                    countdown_display: countdown_label(day, as_of),
                    overdue: day < as_of,
                    cells: columns
                        .iter()
                        .map(|_| TimelineCellVm {
                            entries: Vec::new(),
                        })
                        .collect(),
                });
                rows.last_mut().expect("row was just pushed")
            }
        };

        if let Some(index) = columns
            .iter()
            .position(|known| known == &opportunity.stream)
        {
            row.cells[index]
                .entries
                .push(DeadlineEntryVm::from_opportunity(opportunity));
        }
    }

    (columns, rows)
}

/// `GET /dashboard` — the deadline timeline: every actionable opportunity
/// that has a recorded deadline, grouped by stream column and date row. A
/// confirmed submission or withdrawal moves the card out of the timeline
/// while the deadline facts on the opportunity stay untouched.
pub async fn dashboard(State(state): State<AppState>) -> Response {
    let as_of = today_day_number();
    let (year, month, day) = civil_from_days(as_of);
    let as_of_display = format_date_human(&format!("{year:04}-{month:02}-{day:02}"));

    let opportunities = match db::dashboard::upcoming_deadlines(&state.db, 100).await {
        Ok(opportunities) => opportunities,
        Err(err) => {
            tracing::error!(%err, "failed to load dashboard deadlines");
            Vec::new()
        }
    };

    let (columns, rows) = assemble_timeline(&opportunities, as_of);
    let grid_template = format!(
        "--timeline-columns: {DATE_GUTTER_REM}rem repeat({}, minmax(15rem, 1fr))",
        columns.len()
    );

    HtmlTemplate(DashboardTemplate {
        entry_count: opportunities.len() as i64,
        as_of_display,
        grid_template,
        columns,
        rows,
    })
    .into_response()
}

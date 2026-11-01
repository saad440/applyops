//! The dashboard page: a deadline timeline whose columns come from the
//! streams that actually have deadline entries. The page stores nothing of
//! its own: every request re-derives what it shows from the same stored
//! records the other pages edit, so no second copy can drift out of date.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};

use crate::{AppState, DEFAULT_USER_ID, HtmlTemplate, db};

use super::display::{
    civil_from_days, countdown_label, display_label, format_date_human, format_deadline_detail,
    format_fit, format_location, parse_iso_day_number, today_day_number,
};

/// Width of the fixed date gutter in the timeline grid, in rem. Wide
/// enough for a full date on one line.
const DATE_GUTTER_REM: i32 = 8;

/// The timeline filters, parsed from GET query parameters so filter state
/// survives bookmarks, reloads, and the browser back button. Unknown or
/// missing values fall back to the defaults, so a stale link never becomes
/// an error page.
#[derive(Debug)]
pub struct DashboardFilters {
    /// Stream labels to show; empty means every stream.
    pub streams: Vec<String>,
    /// `all` (default), `p1`, or `p1p2`.
    pub priority: String,
    /// Deadline window in days from the as-of day; `None` means all
    /// upcoming.
    pub horizon_days: Option<i64>,
    /// Whether entries before the as-of day stay visible. Off by default:
    /// triaged past-due records must not clutter the active view.
    pub show_past_due: bool,
}

impl Default for DashboardFilters {
    fn default() -> Self {
        Self {
            streams: Vec::new(),
            priority: "all".to_string(),
            horizon_days: None,
            show_past_due: false,
        }
    }
}

impl DashboardFilters {
    /// Parses the filter parameters from raw form pairs.
    pub fn from_pairs(pairs: &[(String, String)]) -> Self {
        let mut filters = Self::default();
        let mut priority_seen = false;
        let mut horizon_seen = false;
        for (key, value) in pairs {
            match key.as_str() {
                "stream" => {
                    let label = value.trim().to_string();
                    if !label.is_empty() && !filters.streams.contains(&label) {
                        filters.streams.push(label);
                    }
                }
                "priority" if !priority_seen => {
                    priority_seen = true;
                    if value == "p1" || value == "p1p2" {
                        filters.priority = value.clone();
                    }
                }
                "horizon" if !horizon_seen => {
                    horizon_seen = true;
                    filters.horizon_days = match value.as_str() {
                        "14" => Some(14),
                        "30" => Some(30),
                        "90" => Some(90),
                        _ => None,
                    };
                }
                "past_due" => filters.show_past_due = value == "1",
                _ => {}
            }
        }
        filters
    }

    /// Whether any filter deviates from the defaults.
    pub fn is_active(&self) -> bool {
        !self.streams.is_empty()
            || self.priority != "all"
            || self.horizon_days.is_some()
            || self.show_past_due
    }

    /// Whether an entry with the given stream, priority, and deadline day
    /// stays visible under the filters.
    pub fn keeps(&self, stream: &str, priority: &str, day: i64, as_of: i64) -> bool {
        if !self.streams.is_empty() && !self.streams.iter().any(|known| known == stream) {
            return false;
        }
        match self.priority.as_str() {
            "p1" if priority != "p1" => return false,
            "p1p2" if priority == "p3" => return false,
            _ => {}
        }
        if day < as_of && !self.show_past_due {
            return false;
        }
        if let Some(horizon) = self.horizon_days
            && day > as_of + horizon
        {
            return false;
        }
        true
    }
}

/// One stream label of the filter form with its checked state.
struct FilterStreamVm {
    name: String,
    checked: bool,
}

/// Dashboard page: the deadline timeline, one row per deadline date and one
/// column per stream that has deadline entries.
#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    as_of_display: String,
    entry_count: i64,
    /// Whether any filter is active; drives the reset hint.
    filters_active: bool,
    filter_streams: Vec<FilterStreamVm>,
    filter_priority_all: bool,
    filter_priority_p1: bool,
    filter_priority_p1p2: bool,
    filter_horizon_all: bool,
    filter_horizon_14: bool,
    filter_horizon_30: bool,
    filter_horizon_90: bool,
    show_past_due: bool,
    /// CSS `grid-template-columns` custom property for the timeline: the
    /// date gutter plus one flexible column per stream.
    grid_template: String,
    /// Stream labels of the timeline columns, case-insensitively sorted.
    /// A user-defined column order can replace this later.
    columns: Vec<String>,
    rows: Vec<TimelineRowVm>,
    /// Stream groups of the submitted summary.
    submitted_groups: Vec<SubmittedGroupVm>,
    /// Open-ended leads without a deadline.
    leads: Vec<LeadVm>,
    /// Reporting states of all submitted attempts, with separate pending
    /// and unknown numbers.
    report_counts: db::obligations::BacklogCounts,
    /// The outstanding reports, most urgent surface for the workflow.
    report_items: Vec<ReportItemVm>,
    /// The small deterministic set of urgent or high-priority entries.
    focus: Vec<DeadlineEntryVm>,
    /// Submitted applications whose report is pending or unknown.
    pending_reports: i64,
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

/// One submitted entry in the dashboard's submitted summary.
pub struct SubmittedEntryVm {
    pub opportunity_id: i64,
    pub heading: String,
    /// The confirmed submission date.
    pub submitted_display: String,
    /// The employer outcome, with its date when one is recorded.
    pub outcome_display: String,
}

/// One stream group of the submitted summary.
pub struct SubmittedGroupVm {
    pub name: String,
    pub entries: Vec<SubmittedEntryVm>,
}

/// One open-ended lead on the dashboard.
pub struct LeadVm {
    pub opportunity_id: i64,
    pub heading: String,
    pub stream: String,
    /// The workflow stage; it says what the next step on this lead is.
    pub stage: String,
    pub effort_display: Option<String>,
}

/// One outstanding report in the dashboard's reporting section, linked to
/// its report form.
pub struct ReportItemVm {
    pub application_id: i64,
    pub heading: String,
    pub submitted_display: String,
    pub status_display: String,
}

/// Groups the submitted attempts by stream, case-insensitively sorted,
/// preserving the query's newest-first order inside each group.
fn assemble_submitted(rows: &[db::dashboard::SubmittedRow]) -> Vec<SubmittedGroupVm> {
    let mut names: Vec<String> = Vec::new();
    for row in rows {
        if !names.iter().any(|known| known == &row.stream) {
            names.push(row.stream.clone());
        }
    }
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));

    let mut groups: Vec<SubmittedGroupVm> = names
        .iter()
        .map(|name| SubmittedGroupVm {
            name: name.clone(),
            entries: Vec::new(),
        })
        .collect();
    for row in rows {
        if let Some(group) = groups.iter_mut().find(|group| group.name == row.stream) {
            // An unknown outcome is stated as such; a recorded one carries
            // its date.
            let outcome_display = match row.outcome_at.as_deref() {
                Some(date) if row.employer_outcome != "unknown" => format!(
                    "{} · {}",
                    display_label(&row.employer_outcome),
                    format_date_human(date)
                ),
                _ => display_label(&row.employer_outcome),
            };
            group.entries.push(SubmittedEntryVm {
                opportunity_id: row.opportunity_id,
                heading: format!("{} — {}", row.organization, row.title),
                submitted_display: row
                    .submitted_at
                    .as_deref()
                    .map(format_date_human)
                    .unwrap_or_else(|| "Unknown date".to_string()),
                outcome_display,
            });
        }
    }
    groups.retain(|group| !group.entries.is_empty());
    groups
}

/// Prepares the open-ended leads for display; the query's priority and
/// name order is kept.
fn assemble_leads(opportunities: &[db::opportunities::Opportunity]) -> Vec<LeadVm> {
    opportunities
        .iter()
        .map(|opportunity| LeadVm {
            opportunity_id: opportunity.id,
            heading: format!("{} — {}", opportunity.organization, opportunity.title),
            stream: opportunity.stream.clone(),
            stage: opportunity.stage.clone(),
            effort_display: opportunity.effort.as_deref().map(display_label),
        })
        .collect()
}

/// Selects the focus entries: deadline entries due today or overdue, or
/// priority P1 — at most five, in the query's order (date, then priority,
/// then organization and title). A deterministic small set; nothing fancier
/// is needed yet.
pub fn assemble_focus(
    opportunities: &[db::opportunities::Opportunity],
    as_of: i64,
) -> Vec<DeadlineEntryVm> {
    opportunities
        .iter()
        .filter(|opportunity| {
            let due_or_overdue = opportunity
                .deadline_date
                .as_deref()
                .and_then(parse_iso_day_number)
                .is_some_and(|day| day <= as_of);
            due_or_overdue || opportunity.priority == "p1"
        })
        .take(5)
        .map(DeadlineEntryVm::from_opportunity)
        .collect()
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
/// that has a recorded deadline, grouped by stream column and date row,
/// narrowed by the GET filters. A confirmed submission or withdrawal moves
/// the card out of the timeline while the deadline facts on the opportunity
/// stay untouched.
pub async fn dashboard(
    State(state): State<AppState>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let as_of = today_day_number();
    let (year, month, day) = civil_from_days(as_of);
    let as_of_display = format_date_human(&format!("{year:04}-{month:02}-{day:02}"));

    let filters = DashboardFilters::from_pairs(&pairs);

    let opportunities = match db::dashboard::upcoming_deadlines(&state.db, 100).await {
        Ok(opportunities) => opportunities,
        Err(err) => {
            tracing::error!(%err, "failed to load dashboard deadlines");
            Vec::new()
        }
    };

    let visible: Vec<db::opportunities::Opportunity> = opportunities
        .into_iter()
        .filter(|opportunity| {
            opportunity
                .deadline_date
                .as_deref()
                .and_then(parse_iso_day_number)
                .is_some_and(|day| {
                    filters.keeps(&opportunity.stream, &opportunity.priority, day, as_of)
                })
        })
        .collect();

    let (columns, rows) = assemble_timeline(&visible, as_of);
    let grid_template = format!(
        "--timeline-columns: {DATE_GUTTER_REM}rem repeat({}, minmax(15rem, 1fr))",
        columns.len()
    );

    let submitted_rows = match db::dashboard::submitted_latest(&state.db, 20).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(%err, "failed to load submitted attempts");
            Vec::new()
        }
    };
    let submitted_groups = assemble_submitted(&submitted_rows);

    let leads_rows = match db::dashboard::open_leads(&state.db, 20).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(%err, "failed to load open-ended leads");
            Vec::new()
        }
    };
    let leads = assemble_leads(&leads_rows);

    let report_counts = match db::obligations::reporting_counts(&state.db).await {
        Ok(counts) => counts,
        Err(err) => {
            tracing::error!(%err, "failed to count reporting states");
            Default::default()
        }
    };
    let report_entries = match db::obligations::reporting_backlog(&state.db, 10).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::error!(%err, "failed to load the report backlog");
            Vec::new()
        }
    };
    let report_items: Vec<ReportItemVm> = report_entries
        .iter()
        .map(|entry| ReportItemVm {
            application_id: entry.application_id,
            heading: format!("{} — {}", entry.organization, entry.title),
            submitted_display: entry
                .submitted_at
                .as_deref()
                .map(format_date_human)
                .unwrap_or_else(|| "Unknown date".to_string()),
            status_display: display_label(entry.status.as_deref().unwrap_or("unknown")),
        })
        .collect();
    let pending_reports = report_counts.pending + report_counts.unknown;

    let focus = assemble_focus(&visible, as_of);

    let stream_labels = match db::streams::list_streams(&state.db, DEFAULT_USER_ID).await {
        Ok(streams) => streams.into_iter().map(|stream| stream.name).collect(),
        Err(err) => {
            tracing::error!(%err, "failed to load stream labels");
            Vec::new()
        }
    };
    let filter_streams: Vec<FilterStreamVm> = stream_labels
        .iter()
        .map(|name| FilterStreamVm {
            name: name.clone(),
            checked: filters.streams.contains(name),
        })
        .collect();

    HtmlTemplate(DashboardTemplate {
        entry_count: visible.len() as i64,
        as_of_display,
        filters_active: filters.is_active(),
        filter_streams,
        filter_priority_all: filters.priority == "all",
        filter_priority_p1: filters.priority == "p1",
        filter_priority_p1p2: filters.priority == "p1p2",
        filter_horizon_all: filters.horizon_days.is_none(),
        filter_horizon_14: filters.horizon_days == Some(14),
        filter_horizon_30: filters.horizon_days == Some(30),
        filter_horizon_90: filters.horizon_days == Some(90),
        show_past_due: filters.show_past_due,
        grid_template,
        columns,
        rows,
        submitted_groups,
        leads,
        report_counts,
        report_items,
        focus,
        pending_reports,
    })
    .into_response()
}

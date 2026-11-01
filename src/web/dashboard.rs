//! The dashboard page: an ordered next-deadlines view over actionable
//! opportunities.

use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::{AppState, HtmlTemplate, db};

use super::display::{
    civil_from_days, countdown_label, display_label, format_date_human, format_deadline_detail,
    format_fit, format_location, parse_iso_day_number, today_day_number,
};

/// Dashboard page: the ordered next-deadlines view. The page stores nothing
/// of its own: every request re-derives what it shows from the same stored
/// records the other pages edit, so no second copy can drift out of date.
#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    as_of_display: String,
    entry_count: i64,
    entries: Vec<DeadlineEntryVm>,
}

/// One deadline row on the dashboard, prepared for display. The internal
/// link and the external source link stay separate, and the countdown is
/// computed against a caller-supplied day so tests can pin urgency to a
/// known date.
pub struct DeadlineEntryVm {
    pub opportunity_id: i64,
    pub heading: String,
    /// Stream, priority, and location in one line.
    pub subtitle: String,
    /// The stored priority code; validated vocabulary, used as a CSS class.
    pub priority_class: String,
    pub deadline_display: String,
    /// The deadline's kind and confidence, e.g. `Hard close · Verified`.
    pub deadline_detail: Option<String>,
    pub countdown_display: Option<String>,
    pub overdue: bool,
    pub rationale: Option<String>,
    /// Fit and effort in one line, or `None` while both are unrated.
    pub meta_display: Option<String>,
    pub external_link: Option<ExternalLinkVm>,
}

/// The one external link on a dashboard row: the recorded advert, or the
/// recruitment page as a fallback, or `None` when neither exists. It stays
/// separate from the internal details link.
pub struct ExternalLinkVm {
    pub label: String,
    pub url: String,
}

impl DeadlineEntryVm {
    /// Prepares one dashboard row from stored data; `as_of` is the day
    /// number the countdown is measured against.
    pub fn from_opportunity(opportunity: &db::opportunities::Opportunity, as_of: i64) -> Self {
        let deadline_day = opportunity
            .deadline_date
            .as_deref()
            .and_then(parse_iso_day_number);

        let mut subtitle = format!(
            "{} · {}",
            opportunity.stream,
            opportunity.priority.to_uppercase()
        );
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
            heading: format!("{} — {}", opportunity.organization, opportunity.title),
            subtitle,
            priority_class: opportunity.priority.clone(),
            deadline_display: opportunity
                .deadline_date
                .as_deref()
                .map(format_date_human)
                .unwrap_or_default(),
            deadline_detail: format_deadline_detail(
                opportunity.deadline_kind.as_deref(),
                opportunity.deadline_confidence.as_deref(),
            ),
            countdown_display: deadline_day.map(|day| countdown_label(day, as_of)),
            overdue: deadline_day.is_some_and(|day| day < as_of),
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

/// `GET /dashboard` — the ordered next-deadlines view: every actionable
/// opportunity that has a recorded deadline, soonest first. A confirmed
/// submission or withdrawal moves the entry out of this list while the
/// deadline facts on the opportunity stay untouched.
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

    let entries: Vec<DeadlineEntryVm> = opportunities
        .iter()
        .map(|opportunity| DeadlineEntryVm::from_opportunity(opportunity, as_of))
        .collect();

    HtmlTemplate(DashboardTemplate {
        entry_count: entries.len() as i64,
        as_of_display,
        entries,
    })
    .into_response()
}

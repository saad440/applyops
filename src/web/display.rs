//! Presentation helpers shared by the page modules: stored codes become
//! readable labels, stored dates become display text, and deadline urgency
//! is computed here for display only.

use std::time::{SystemTime, UNIX_EPOCH};

/// Turns a stored code such as `employment_agency` into a readable label such
/// as `Employment agency`; unknown codes pass through unchanged.
pub fn display_label(code: &str) -> String {
    const LABELS: [(&str, &str); 26] = [
        ("email", "Email"),
        ("employer_portal", "Employer portal"),
        ("employment_agency", "Employment agency"),
        ("other", "Other"),
        ("unknown", "Unknown"),
        ("pending", "Pending"),
        ("interview", "Interview"),
        ("offer", "Offer"),
        ("rejected", "Rejected"),
        ("withdrawn", "Withdrawn"),
        ("reported", "Reported"),
        ("not_required", "Not required"),
        ("jobcenter", "Jobcenter"),
        ("foreign_authority", "Foreign authority"),
        ("portal", "Authority portal"),
        ("manual", "Manual"),
        ("hard_close", "Hard close"),
        ("priority_review", "Priority consideration"),
        ("first_review", "First review"),
        ("soft_target", "Soft target"),
        ("verified", "Verified"),
        ("unverified", "Unverified"),
        ("conflicting", "Conflicting dates"),
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
    ];

    LABELS
        .iter()
        .find(|(stored, _)| *stored == code)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| code.to_string())
}

/// Combines the stored city and country into one place such as
/// `Snorndorf, Testland`; a lone part appears alone.
pub fn format_location(city: Option<&str>, country: Option<&str>) -> Option<String> {
    match (city, country) {
        (Some(city), Some(country)) => Some(format!("{city}, {country}")),
        (Some(place), None) | (None, Some(place)) => Some(place.to_string()),
        (None, None) => None,
    }
}

/// Combines the deadline's kind and confidence into one label such as
/// `Hard close · Verified`; a lone part appears alone, and a deadline with
/// neither stays unlabeled.
pub fn format_deadline_detail(kind: Option<&str>, confidence: Option<&str>) -> Option<String> {
    let kind = kind.map(display_label);
    let confidence = confidence.map(display_label);
    match (kind, confidence) {
        (Some(kind), Some(confidence)) => Some(format!("{kind} · {confidence}")),
        (Some(kind), None) => Some(kind),
        (None, Some(confidence)) => Some(confidence),
        (None, None) => None,
    }
}

/// Formats an ISO `YYYY-MM-DD` date as e.g. `15 Jan 2027` for display.
/// Values that are not ten characters long are passed through unchanged.
pub fn format_date_human(iso_date: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    if iso_date.len() != 10 {
        return iso_date.to_string();
    }

    let year = &iso_date[0..4];
    let day = iso_date[8..10].trim_start_matches('0');
    let month: usize = iso_date[5..7].parse().unwrap_or(0);
    match month {
        1..=12 => format!("{day} {} {year}", MONTHS[month - 1]),
        _ => iso_date.to_string(),
    }
}

/// Formats a fit range as e.g. `4/5` or `3.5–4/5`. A range with only one
/// stored end shows that end; no fit at all is `None`, never a placeholder.
pub fn format_fit(min: Option<f64>, max: Option<f64>) -> Option<String> {
    match (min, max) {
        (Some(min), Some(max)) if min == max => Some(format!("{min}/5")),
        (Some(min), Some(max)) => Some(format!("{min}–{max}/5")),
        (Some(value), None) | (None, Some(value)) => Some(format!("{value}/5")),
        (None, None) => None,
    }
}

/// Converts a calendar date to the number of days since 1970-01-01
/// (proleptic Gregorian). Deadlines carry no time of day, so everything the
/// dashboard computes about them is whole-day arithmetic.
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let shifted_year = i64::from(if month <= 2 { year - 1 } else { year });
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * i64::from(month_index) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Inverse of [`days_from_civil`]: converts days since 1970-01-01 back to
/// a `(year, month, day)` calendar date.
pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let shifted_days = days + 719_468;
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

/// Strictly parses `YYYY-MM-DD` into a day number, rejecting anything else,
/// including impossible calendar dates. Stored deadlines are validated at
/// the data layer, so a parse failure here only costs the countdown label.
pub fn parse_iso_day_number(iso_date: &str) -> Option<i64> {
    let (year, rest) = iso_date.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    let year: i32 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    let day: u32 = day.parse().ok()?;
    let days = days_from_civil(year, month, day);
    if civil_from_days(days) != (year, month, day) {
        return None;
    }
    Some(days)
}

/// The calendar day number of a moment in the given time zone: the day
/// the dashboard's countdowns and urgency labels are measured against.
/// Zone lookup and conversion are delegated to `jiff`; the calendar
/// arithmetic stays in this module.
pub fn day_number_in(moment_unix_seconds: i64, time_zone: &jiff::tz::TimeZone) -> i64 {
    let zoned = jiff::Timestamp::from_second(moment_unix_seconds)
        .expect("a moment within the supported range")
        .to_zoned(time_zone.clone())
        .date();
    days_from_civil(
        i32::from(zoned.year()),
        u32::try_from(zoned.month()).unwrap_or_default(),
        u32::try_from(zoned.day()).unwrap_or_default(),
    )
}

/// Today's day number in the app's configured time zone.
pub fn today_day_number_in(time_zone: &jiff::tz::TimeZone) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    day_number_in(now, time_zone)
}

/// The countdown text for a deadline measured against the current day.
/// Past deadlines stay visible and say by how much.
pub fn countdown_label(deadline_day: i64, as_of_day: i64) -> String {
    match deadline_day - as_of_day {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        n if n > 1 => format!("in {n} days"),
        -1 => "1 day overdue".to_string(),
        n => format!("{} days overdue", -n),
    }
}

//! The diagnostics page and its note form, kept as a persistence smoke test.

use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};

use crate::{AppState, DEFAULT_USER_ID, HtmlTemplate, db};

/// Diagnostics page, rendered from `templates/diagnostics.html` at compile time.
#[derive(Template)]
#[template(path = "diagnostics.html")]
struct DiagnosticsTemplate {
    note_count: i64,
    notes: Vec<db::DiagnosticNote>,
}

/// `GET /diagnostics` — the persistence smoke-test notes page.
pub async fn diagnostics(State(state): State<AppState>) -> Response {
    let count = match db::count_notes(&state.db).await {
        Ok(count) => count,
        Err(err) => {
            tracing::error!(%err, "failed to count diagnostic notes");
            0
        }
    };

    let notes = match db::recent_notes(&state.db, 10).await {
        Ok(notes) => notes,
        Err(err) => {
            tracing::error!(%err, "failed to load diagnostic notes");
            Vec::new()
        }
    };

    HtmlTemplate(DiagnosticsTemplate {
        note_count: count,
        notes,
    })
    .into_response()
}

/// `POST /diagnostics/notes` — stores a diagnostic note, then redirects back
/// to the diagnostics page.
pub async fn create_note(
    State(state): State<AppState>,
    Form(form): Form<Vec<(String, String)>>,
) -> impl IntoResponse {
    let note = form
        .iter()
        .find(|(field, _)| field == "note")
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default();

    if !note.is_empty()
        && let Err(err) = db::insert_note(&state.db, DEFAULT_USER_ID, &note).await
    {
        tracing::error!(%err, "failed to store diagnostic note");
    }

    Redirect::to("/diagnostics")
}

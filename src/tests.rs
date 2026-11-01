//! Tests for the HTTP layer: routes, list and search, form handling, and
//! display helpers, exercised directly against in-memory SQLite.

use super::*;

use axum::extract::{Form, Path, Query, State};

use crate::web::dashboard::DeadlineEntryVm;
use crate::web::display::{
    civil_from_days, countdown_label, days_from_civil, format_date_human, parse_iso_day_number,
};
use crate::web::opportunities::OpportunityListTemplate;
use crate::web::*;

/// Application state over a fresh in-memory database with migrations and
/// default streams, mirroring server startup.
async fn test_state() -> AppState {
    let pool = db::connect_in_memory().await.expect("in-memory pool");
    db::run_migrations(&pool).await.expect("migrations run");
    db::streams::seed_streams_if_empty(&pool, DEFAULT_USER_ID, &db::streams::DEFAULT_STREAMS)
        .await
        .expect("default streams seeded");
    db::authorities::seed_authorities_if_empty(
        &pool,
        DEFAULT_USER_ID,
        &db::authorities::DEFAULT_AUTHORITIES,
    )
    .await
    .expect("default authorities seeded");
    AppState { db: pool }
}

/// Reads a response body as text.
async fn body_string(response: Response) -> String {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

/// Asserts that a flow step returned the redirect of the POST/redirect/GET
/// baseline.
fn assert_redirect(response: Response) {
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

/// The form pairs for the fictional university position.
fn university_pairs() -> Vec<(String, String)> {
    vec![
        (
            "organization".to_string(),
            "Universität Schneckenhausen".to_string(),
        ),
        (
            "title".to_string(),
            "PhD position, low-temperature physics".to_string(),
        ),
        ("stream".to_string(), "Physics".to_string()),
        ("priority".to_string(), "p1".to_string()),
        ("stage".to_string(), "discovered".to_string()),
        ("deadline_date".to_string(), "2027-01-15".to_string()),
        ("fit_min".to_string(), "3.5".to_string()),
        ("fit_max".to_string(), "4".to_string()),
        ("effort".to_string(), "medium".to_string()),
        (
            "rationale".to_string(),
            "Good refrigeration; lack of direct experience in making it work.".to_string(),
        ),
        ("deadline_kind".to_string(), "hard_close".to_string()),
        ("deadline_confidence".to_string(), "verified".to_string()),
        (
            "recruitment_page_url".to_string(),
            "https://www.example.org/research".to_string(),
        ),
        (
            "source_notes".to_string(),
            "Deadline confirmed on the department page.".to_string(),
        ),
    ]
}

/// The form pairs for the fictional IT posting.
fn it_posting_pairs() -> Vec<(String, String)> {
    vec![
        (
            "organization".to_string(),
            "SynerTech Buzzworks GmbH".to_string(),
        ),
        (
            "title".to_string(),
            "Senior AI-Blockchain Synergy Engineer (m/w/d)".to_string(),
        ),
        ("stream".to_string(), "IT".to_string()),
        ("priority".to_string(), "p3".to_string()),
        ("stage".to_string(), "discovered".to_string()),
    ]
}

#[tokio::test]
async fn root_responds_with_html() {
    let state = test_state().await;
    let response = root(State(state), Query(Vec::new())).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&"text/html; charset=utf-8".parse().unwrap())
    );
}

#[tokio::test]
async fn empty_list_shows_the_empty_state() {
    let state = test_state().await;
    let response = root(State(state), Query(Vec::new())).await.into_response();

    let body = body_string(response).await;
    assert!(body.contains("No opportunities recorded yet"));
    assert!(body.contains("Add the first opportunity"));
}

#[tokio::test]
async fn created_opportunity_appears_in_the_list() {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let response = root(State(state), Query(Vec::new())).await.into_response();
    let body = body_string(response).await;
    assert!(body.contains("Universität Schneckenhausen"));
    assert!(body.contains(r#"href="/opportunities/1""#));
    assert!(body.contains("1 shown"));
}

#[tokio::test]
async fn search_filters_the_list() {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = create_opportunity(State(state.clone()), Form(it_posting_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let query = Query(vec![("q".to_string(), "Buzzworks".to_string())]);
    let response = root(State(state.clone()), query).await.into_response();
    let body = body_string(response).await;
    assert!(body.contains("SynerTech Buzzworks GmbH"));
    assert!(!body.contains("Schneckenhausen"));
    assert!(body.contains("1 shown"));

    // A search without hits shows the no-match state.
    let query = Query(vec![("q".to_string(), "quantum".to_string())]);
    let response = root(State(state), query).await.into_response();
    let body = body_string(response).await;
    assert!(body.contains("Nothing matches"));
}

#[tokio::test]
async fn index_links_to_the_opportunity_form() {
    let state = test_state().await;
    let response = root(State(state), Query(Vec::new())).await.into_response();
    let body = body_string(response).await;
    assert!(body.contains(r#"href="/opportunities/new""#));
}

#[tokio::test]
async fn diagnostics_page_shows_stored_note() {
    let state = test_state().await;
    db::insert_note(&state.db, DEFAULT_USER_ID, "round trip")
        .await
        .unwrap();

    let response = diagnostics(State(state)).await.into_response();
    let body = body_string(response).await;
    assert!(body.contains("round trip"));
    assert!(body.contains("Persistent records"));
}

#[test]
fn index_template_links_stylesheet() {
    let template = OpportunityListTemplate {
        search_query: String::new(),
        opportunities: Vec::new(),
        list_count: 0,
    };
    let html = template.render().expect("index template renders");
    assert!(html.contains(r#"href="/static/css/app.css""#));
    assert!(html.contains("ApplyOps"));
}

#[tokio::test]
async fn diagnostic_notes_round_trip() {
    let pool = db::connect_in_memory().await.expect("in-memory pool");
    db::run_migrations(&pool).await.expect("migrations run");
    assert_eq!(db::count_notes(&pool).await.unwrap(), 0);

    db::insert_note(&pool, DEFAULT_USER_ID, "smoke note")
        .await
        .unwrap();

    let notes = db::recent_notes(&pool, 10).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].note, "smoke note");
    assert_eq!(notes[0].user_id, DEFAULT_USER_ID);
    assert_eq!(db::count_notes(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn form_page_offers_default_streams() {
    let state = test_state().await;
    let response = new_opportunity_form(State(state)).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_string(response).await;
    assert!(body.contains("New opportunity"));
    assert!(body.contains("Physics"));
    assert!(body.contains("Scientific Computing"));
    assert!(body.contains("IT"));
}

#[tokio::test]
async fn create_opportunity_redirects_to_details() {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION),
        Some(&"/opportunities/1".parse().unwrap())
    );

    let details = opportunity_details(State(state), Path(1))
        .await
        .into_response();
    assert_eq!(details.status(), StatusCode::OK);
    let body = body_string(details).await;
    assert!(body.contains("Universität Schneckenhausen"));
    assert!(body.contains("15 Jan 2027"));
    assert!(body.contains("P1"));
    assert!(body.contains(r#"/opportunities/1/edit"#));
}

#[tokio::test]
async fn invalid_form_is_redisplayed_with_error_and_values() {
    let state = test_state().await;
    let mut pairs = university_pairs();
    pairs[0] = ("organization".to_string(), String::new());

    let response = create_opportunity(State(state.clone()), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_string(response).await;
    assert!(body.contains("must not be empty"));
    assert!(
        body.contains("A title worth keeping")
            || body.contains("PhD position, low-temperature physics")
    );
    assert_eq!(
        db::opportunities::count_opportunities(&state.db)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn malformed_deadline_is_reported_on_the_form() {
    let state = test_state().await;
    let mut pairs = university_pairs();
    pairs[5] = ("deadline_date".to_string(), "19.01.2027".to_string());

    let response = create_opportunity(State(state), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_string(response).await;
    assert!(body.contains("must be a calendar date formatted YYYY-MM-DD"));
}

#[tokio::test]
async fn new_stream_label_is_remembered_for_future_forms() {
    let state = test_state().await;
    let mut pairs = it_posting_pairs();
    pairs[2] = ("stream".to_string(), "Marine Acoustics".to_string());

    let response = create_opportunity(State(state.clone()), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let streams = db::streams::list_streams(&state.db, DEFAULT_USER_ID)
        .await
        .unwrap();
    assert!(
        streams
            .iter()
            .any(|stream| stream.name == "Marine Acoustics")
    );
}

#[tokio::test]
async fn edit_form_is_prefilled_with_stored_values() {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let response = edit_opportunity_form(State(state), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_string(response).await;
    assert!(body.contains("Edit opportunity"));
    assert!(body.contains(r#"action="/opportunities/1/edit""#));
    assert!(body.contains(r#"value="Universität Schneckenhausen""#));
    assert!(body.contains(r#"value="2027-01-15""#));
}

#[tokio::test]
async fn update_opportunity_redirects_and_persists() {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let mut edited = university_pairs();
    edited[1] = (
        "title".to_string(),
        "PhD position, cryogenic detectors".to_string(),
    );
    edited[4] = ("stage".to_string(), "researching".to_string());
    edited[5] = ("deadline_date".to_string(), String::new());
    // A deadline type without the date it describes is rejected, so
    // clearing the deadline clears it as well.
    edited[10] = ("deadline_kind".to_string(), String::new());
    edited[11] = ("deadline_confidence".to_string(), String::new());

    let response = update_opportunity(State(state.clone()), Path(1), Form(edited))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION),
        Some(&"/opportunities/1".parse().unwrap())
    );

    let details = opportunity_details(State(state.clone()), Path(1))
        .await
        .into_response();
    let body = body_string(details).await;
    assert!(body.contains("cryogenic detectors"));
    assert!(body.contains("researching"));
    assert!(body.contains("No deadline recorded"));

    // The edited values appear in the list as well.
    let response = root(State(state), Query(Vec::new())).await.into_response();
    let body = body_string(response).await;
    assert!(body.contains("cryogenic detectors"));
}

#[tokio::test]
async fn opportunity_metadata_round_trips_through_form_and_details() {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    // The details page shows the stored metadata.
    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("3.5–4/5"));
    assert!(body.contains("Medium"));
    assert!(body.contains("Hard close · Verified"));
    assert!(body.contains("Good refrigeration"));
    assert!(body.contains(r#"href="https://www.example.org/research""#));
    assert!(body.contains("Deadline confirmed on the department page."));

    // The edit form is prefilled with the stored values.
    let body = body_string(
        edit_opportunity_form(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains(r#"value="3.5""#));
    assert!(body.contains(r#"value="4""#));
    assert!(body.contains(r#"<option value="medium" selected>"#));
    assert!(body.contains(r#"<option value="hard_close" selected>"#));
    assert!(body.contains(r#"<option value="verified" selected>"#));

    // Clearing the metadata on edit removes it from the page again.
    let mut cleared = university_pairs();
    cleared[6] = ("fit_min".to_string(), String::new());
    cleared[7] = ("fit_max".to_string(), String::new());
    cleared[8] = ("effort".to_string(), String::new());
    cleared[9] = ("rationale".to_string(), String::new());
    cleared[10] = ("deadline_kind".to_string(), String::new());
    cleared[11] = ("deadline_confidence".to_string(), String::new());
    cleared[12] = ("recruitment_page_url".to_string(), String::new());
    cleared[13] = ("source_notes".to_string(), String::new());
    assert_redirect(update_opportunity(State(state.clone()), Path(1), Form(cleared)).await);

    let body = body_string(
        opportunity_details(State(state), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(!body.contains("3.5–4/5"));
    assert!(!body.contains("Hard close"));
}

#[tokio::test]
async fn an_out_of_range_fit_is_reported_on_its_field() {
    let state = test_state().await;
    let mut pairs = university_pairs();
    pairs[7] = ("fit_max".to_string(), "9".to_string());

    let response = create_opportunity(State(state.clone()), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Fit maximum must be a number between 1 and 5"));
    assert_eq!(
        db::opportunities::count_opportunities(&state.db)
            .await
            .unwrap(),
        0
    );

    // A value that is not a number at all is named the same way.
    let mut pairs = university_pairs();
    pairs[6] = ("fit_min".to_string(), "soon".to_string());
    let response = create_opportunity(State(state.clone()), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Fit minimum must be a number between 1 and 5"));

    // The maximum below the minimum is reported on the maximum.
    let mut pairs = university_pairs();
    pairs[6] = ("fit_min".to_string(), "4".to_string());
    pairs[7] = ("fit_max".to_string(), "2".to_string());
    let response = create_opportunity(State(state), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Fit maximum must not be below the fit minimum"));
}

#[tokio::test]
async fn a_deadline_type_without_a_date_is_reported_on_the_form() {
    let state = test_state().await;
    let mut pairs = university_pairs();
    pairs[5] = ("deadline_date".to_string(), String::new());

    let response = create_opportunity(State(state.clone()), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Deadline type requires a deadline date"));
    assert_eq!(
        db::opportunities::count_opportunities(&state.db)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn editing_a_missing_opportunity_returns_not_found() {
    let state = test_state().await;

    let response = edit_opportunity_form(State(state.clone()), Path(999))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = update_opportunity(State(state), Path(999), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn missing_opportunity_returns_not_found() {
    let state = test_state().await;
    let response = opportunity_details(State(state), Path(999))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn dates_display_as_day_month_year() {
    assert_eq!(format_date_human("2026-10-19"), "19 Oct 2026");
    assert_eq!(format_date_human("2027-01-15"), "15 Jan 2027");
    assert_eq!(format_date_human("2026-10-05"), "5 Oct 2026");
}

#[tokio::test]
async fn health_responds_with_ok() {
    let response = health().await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn bind_addr_defaults_to_loopback() {
    assert_eq!(resolve_bind_addr(None), "127.0.0.1:8080");
}

#[test]
fn bind_addr_env_overrides_default() {
    assert_eq!(
        resolve_bind_addr(Some("0.0.0.0:9000".to_string())),
        "0.0.0.0:9000"
    );
}

#[test]
fn db_path_defaults_to_data_dir() {
    assert_eq!(resolve_db_path(None), "data/applyops.db");
}

#[test]
fn db_path_env_overrides_default() {
    assert_eq!(
        resolve_db_path(Some("/tmp/other.db".to_string())),
        "/tmp/other.db"
    );
}

#[test]
fn loopback_addresses_are_detected() {
    assert!(is_loopback_addr("127.0.0.1:8080"));
    assert!(is_loopback_addr("localhost:8080"));
    assert!(is_loopback_addr("[::1]:9000"));
}

#[test]
fn non_loopback_addresses_are_detected() {
    assert!(!is_loopback_addr("0.0.0.0:8080"));
    assert!(!is_loopback_addr("192.168.1.10:8080"));
}

// --- Application workflow ---

/// Fresh state with one opportunity and one draft application for it; the
/// ids on the empty database are deterministic (opportunity 1, attempt 1).
async fn state_with_draft_application() -> AppState {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = create_application(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    state
}

/// The confirmed submission form pairs.
fn submission_pairs() -> Vec<(String, String)> {
    vec![
        ("submitted_at".to_string(), "2026-10-19".to_string()),
        (
            "submission_channel".to_string(),
            "employment_agency".to_string(),
        ),
    ]
}

#[tokio::test]
async fn details_page_shows_the_application_section() {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let response = opportunity_details(State(state), Path(1))
        .await
        .into_response();
    let body = body_string(response).await;
    assert!(body.contains("No application yet"));
    assert!(body.contains("Start an application"));
    assert!(body.contains("Nothing recorded yet"));
}

#[tokio::test]
async fn starting_an_application_creates_a_draft() {
    let state = state_with_draft_application().await;
    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Attempt · draft"));
    assert!(body.contains("Mark as preparing"));
    assert!(body.contains("History"));

    // A repeated submit does not create a duplicate attempt.
    let response = create_application(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let attempts = db::applications::list_applications_for_opportunity(&state.db, 1)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
}

#[tokio::test]
async fn starting_an_application_for_a_missing_opportunity_404s() {
    let state = test_state().await;
    let response = create_application(State(state), Path(999))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn draft_moves_to_preparing_and_back() {
    let state = state_with_draft_application().await;
    let response = prepare_application(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Record submission"));
    assert!(body.contains("Back to draft"));

    assert_redirect(unprepare_application(State(state.clone()), Path(1)).await);
    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "draft");
}

#[tokio::test]
async fn submission_flow_records_confirmed_facts_and_history() {
    let state = state_with_draft_application().await;
    assert_redirect(prepare_application(State(state.clone()), Path(1)).await);

    // The submission form is an explicit confirmation step.
    let response = submit_application_form(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Record submission"));
    assert!(body.contains("Employment agency"));

    let response =
        record_application_submission(State(state.clone()), Path(1), Form(submission_pairs()))
            .await
            .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION),
        Some(&"/opportunities/1".parse().unwrap())
    );

    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("19 Oct 2026"));
    assert!(body.contains("Employment agency"));
    assert!(body.contains("Submission recorded"));
    assert!(body.contains("Correct to draft"));
    assert!(body.contains("Withdraw"));

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "submitted");
    assert_eq!(stored.submitted_at.as_deref(), Some("2026-10-19"));
    assert_eq!(stored.submission_channel, "employment_agency");
}

#[tokio::test]
async fn submission_without_a_date_is_reported_on_the_form() {
    let state = state_with_draft_application().await;
    assert_redirect(prepare_application(State(state.clone()), Path(1)).await);

    let pairs = vec![
        ("submitted_at".to_string(), String::new()),
        ("submission_channel".to_string(), "email".to_string()),
    ];
    let response = record_application_submission(State(state.clone()), Path(1), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("must not be empty"));

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "preparing");
}

#[tokio::test]
async fn submission_from_a_draft_attempt_is_rejected_with_feedback() {
    let state = state_with_draft_application().await;
    let response =
        record_application_submission(State(state.clone()), Path(1), Form(submission_pairs()))
            .await
            .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("only an application that is preparing can be recorded as submitted"));

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "draft");
}

#[tokio::test]
async fn correction_clears_submission_and_keeps_history() {
    let state = state_with_draft_application().await;
    assert_redirect(prepare_application(State(state.clone()), Path(1)).await);
    let mut pairs = submission_pairs();
    pairs[0] = ("submitted_at".to_string(), "2026-10-12".to_string());
    pairs[1] = ("submission_channel".to_string(), "email".to_string());
    assert_redirect(
        record_application_submission(State(state.clone()), Path(1), Form(pairs)).await,
    );

    let response = correct_application_form(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Correct the recorded submission"));
    assert!(body.contains("the correction is recorded, not erased"));

    let response = correct_application(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "draft");
    assert!(stored.submitted_at.is_none());

    // The history still shows the original submission next to the
    // correction, including the cleared date.
    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Submission corrected"));
    assert!(body.contains("Submission recorded"));
    assert!(body.contains("2026-10-12"));
}

#[tokio::test]
async fn withdrawal_closes_the_attempt_and_allows_a_new_one() {
    let state = state_with_draft_application().await;
    assert_redirect(prepare_application(State(state.clone()), Path(1)).await);
    assert_redirect(
        record_application_submission(State(state.clone()), Path(1), Form(submission_pairs()))
            .await,
    );

    let response = withdraw_application(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Attempt · withdrawn"));
    assert!(body.contains("closed for good"));
    assert!(body.contains("Start another application"));

    // Starting a new attempt after withdrawal is possible.
    let response = create_application(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let attempts = db::applications::list_applications_for_opportunity(&state.db, 1)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].stage, "draft");
    assert_eq!(attempts[1].stage, "withdrawn");
}

#[tokio::test]
async fn missing_applications_return_not_found() {
    let state = test_state().await;
    let responses = vec![
        submit_application_form(State(state.clone()), Path(999))
            .await
            .into_response(),
        prepare_application(State(state.clone()), Path(999))
            .await
            .into_response(),
        correct_application(State(state.clone()), Path(999))
            .await
            .into_response(),
        withdraw_application(State(state), Path(999))
            .await
            .into_response(),
    ];
    for response in responses {
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

// --- Employer outcome ---

/// Fresh state with one submitted application (opportunity 1, attempt 1).
async fn state_with_submitted_application() -> AppState {
    let state = state_with_draft_application().await;
    assert_redirect(prepare_application(State(state.clone()), Path(1)).await);
    assert_redirect(
        record_application_submission(State(state.clone()), Path(1), Form(submission_pairs()))
            .await,
    );
    state
}

#[tokio::test]
async fn submitted_attempt_offers_the_outcome_form() {
    let state = state_with_submitted_application().await;
    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Record employer outcome"));

    let response = outcome_form(State(state), Path(1)).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Employer outcome"));
    assert!(body.contains("Pending"));
    assert!(body.contains("Interview"));
    assert!(body.contains("Unknown — clear the outcome"));
}

#[tokio::test]
async fn recording_an_outcome_persists_facts_and_history() {
    let state = state_with_submitted_application().await;
    let pairs = vec![
        ("employer_outcome".to_string(), "interview".to_string()),
        ("outcome_at".to_string(), "2026-10-25".to_string()),
    ];
    let response = record_outcome(State(state.clone()), Path(1), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Interview · 25 Oct 2026"));
    assert!(body.contains("Update employer outcome"));
    assert!(body.contains("Employer outcome changed"));

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.employer_outcome, "interview");
    assert_eq!(stored.outcome_at.as_deref(), Some("2026-10-25"));
}

#[tokio::test]
async fn outcome_without_a_date_is_reported_on_the_form() {
    let state = state_with_submitted_application().await;
    let pairs = vec![
        ("employer_outcome".to_string(), "offer".to_string()),
        ("outcome_at".to_string(), String::new()),
    ];
    let response = record_outcome(State(state.clone()), Path(1), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("must not be empty"));

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.employer_outcome, "unknown");
}

#[tokio::test]
async fn clearing_the_outcome_from_the_form() {
    let state = state_with_submitted_application().await;
    let pairs = vec![
        ("employer_outcome".to_string(), "rejected".to_string()),
        ("outcome_at".to_string(), "2026-10-25".to_string()),
    ];
    assert_redirect(record_outcome(State(state.clone()), Path(1), Form(pairs)).await);

    let pairs = vec![
        ("employer_outcome".to_string(), "unknown".to_string()),
        ("outcome_at".to_string(), String::new()),
    ];
    let response = record_outcome(State(state.clone()), Path(1), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.employer_outcome, "unknown");
    assert!(stored.outcome_at.is_none());
}

#[tokio::test]
async fn outcome_on_a_draft_attempt_is_rejected_with_feedback() {
    let state = state_with_draft_application().await;
    let pairs = vec![
        ("employer_outcome".to_string(), "offer".to_string()),
        ("outcome_at".to_string(), "2026-10-25".to_string()),
    ];
    let response = record_outcome(State(state.clone()), Path(1), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("only a submitted application can receive an employer outcome"));

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.employer_outcome, "unknown");
}

#[tokio::test]
async fn outcome_form_for_a_missing_application_404s() {
    let state = test_state().await;
    let response = outcome_form(State(state), Path(999)).await.into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// --- Reporting status and the report backlog ---

/// The form pairs for marking a report pending.
fn report_pending_pairs() -> Vec<(String, String)> {
    vec![("status".to_string(), "pending".to_string())]
}

#[tokio::test]
async fn submitted_attempt_offers_the_report_form() {
    let state = state_with_submitted_application().await;
    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Record reporting status"));
    assert!(body.contains("No record yet"));

    let response = report_form(State(state), Path(1)).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Reporting status"));
    assert!(body.contains("Pending"));
    assert!(body.contains("Reported"));
    assert!(body.contains("Not required"));
    assert!(body.contains("Unknown — nothing reliably recorded"));
    // The seeded authority labels are offered as suggestions.
    assert!(body.contains("Agentur für Arbeit"));
}

#[tokio::test]
async fn recording_a_report_persists_facts_history_and_labels() {
    let state = state_with_submitted_application().await;
    let pairs = vec![
        ("status".to_string(), "reported".to_string()),
        ("reported_at".to_string(), "2026-11-02".to_string()),
        ("method".to_string(), "portal".to_string()),
        (
            "recipient_name".to_string(),
            "District Office Musterstadt".to_string(),
        ),
    ];
    assert_redirect(record_report(State(state.clone()), Path(1), Form(pairs)).await);

    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Reported · 2 Nov 2026"));
    assert!(body.contains("Update reporting status"));
    assert!(body.contains("Reporting status changed"));

    let stored = db::obligations::get_obligation(&state.db, 1, db::obligations::DEFAULT_KIND)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "reported");
    assert_eq!(stored.reported_at.as_deref(), Some("2026-11-02"));
    assert_eq!(stored.method.as_deref(), Some("portal"));

    // The authority label typed on first use is remembered.
    let labels = db::authorities::list_authorities(&state.db, DEFAULT_USER_ID)
        .await
        .unwrap();
    assert!(
        labels
            .iter()
            .any(|authority| authority.name == "District Office Musterstadt")
    );
}

#[tokio::test]
async fn report_without_a_date_is_rejected_on_the_form() {
    let state = state_with_submitted_application().await;
    let pairs = vec![("status".to_string(), "reported".to_string())];
    let response = record_report(State(state.clone()), Path(1), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("must not be empty"));

    let stored = db::obligations::get_obligation(&state.db, 1, db::obligations::DEFAULT_KIND)
        .await
        .unwrap();
    assert!(stored.is_none());
}

#[tokio::test]
async fn report_on_a_draft_attempt_is_rejected_with_feedback() {
    let state = state_with_draft_application().await;
    let response = record_report(State(state.clone()), Path(1), Form(report_pending_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("reporting status exists only for submitted applications"));

    let stored = db::obligations::get_obligation(&state.db, 1, db::obligations::DEFAULT_KIND)
        .await
        .unwrap();
    assert!(stored.is_none());
}

#[tokio::test]
async fn correction_is_blocked_until_the_report_is_set_aside() {
    let state = state_with_submitted_application().await;
    assert_redirect(
        record_report(State(state.clone()), Path(1), Form(report_pending_pairs())).await,
    );

    // The details page states the block instead of offering the link.
    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("blocked while the report is pending or reported"));
    assert!(!body.contains("/applications/1/correct"));

    // The correction rejects with an explanatory message and changes nothing.
    let response = correct_application(State(state.clone()), Path(1))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("cannot be corrected while a report"));
    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "submitted");

    // Setting the report to not required unblocks the correction.
    let pairs = vec![("status".to_string(), "not_required".to_string())];
    assert_redirect(record_report(State(state.clone()), Path(1), Form(pairs)).await);
    assert_redirect(correct_application(State(state.clone()), Path(1)).await);

    let stored = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "draft");

    let body = body_string(
        opportunity_details(State(state), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Submission corrected"));
}

#[tokio::test]
async fn the_report_backlog_counts_and_lists_outstanding_work() {
    let state = state_with_submitted_application().await;

    // The submitted attempt without any record counts as unknown.
    let response = report_backlog(State(state.clone())).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("0 pending · 1 unknown"));
    assert!(body.contains("Report: Unknown"));

    assert_redirect(
        record_report(State(state.clone()), Path(1), Form(report_pending_pairs())).await,
    );
    let body = body_string(report_backlog(State(state.clone())).await.into_response()).await;
    assert!(body.contains("1 pending · 0 unknown"));
    assert!(body.contains("Report: Pending"));

    // Marking the report done empties the backlog.
    let pairs = vec![
        ("status".to_string(), "reported".to_string()),
        ("reported_at".to_string(), "2026-11-02".to_string()),
    ];
    assert_redirect(record_report(State(state.clone()), Path(1), Form(pairs)).await);
    let body = body_string(report_backlog(State(state)).await.into_response()).await;
    assert!(body.contains("Nothing outstanding"));
    assert!(body.contains("1 reported"));
}

// --- Follow-up actions ---

/// The form pairs for adding a follow-up action; everything optional stays
/// empty when `None`.
fn follow_up_pairs(
    title: &str,
    due_at: Option<&str>,
    notes: Option<&str>,
) -> Vec<(String, String)> {
    let mut pairs = vec![("title".to_string(), title.to_string())];
    if let Some(due_at) = due_at {
        pairs.push(("due_at".to_string(), due_at.to_string()));
    }
    if let Some(notes) = notes {
        pairs.push(("notes".to_string(), notes.to_string()));
    }
    pairs
}

#[tokio::test]
async fn the_details_page_offers_the_follow_ups_form() {
    let state = state_with_submitted_application().await;
    let body = body_string(
        opportunity_details(State(state), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Follow-ups"));
    assert!(body.contains("No follow-ups recorded"));
    assert!(body.contains(r#"action="/applications/1/actions""#));
}

#[tokio::test]
async fn adding_a_follow_up_persists_and_shows_it() {
    let state = state_with_submitted_application().await;
    let pairs = follow_up_pairs(
        "Ask for feedback",
        Some("2026-11-05"),
        Some("Reply to the recruiter"),
    );
    assert_redirect(add_application_action(State(state.clone()), Path(1), Form(pairs)).await);

    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Ask for feedback"));
    assert!(body.contains("Due 5 Nov 2026"));
    assert!(body.contains("Reply to the recruiter"));
    assert!(body.contains("Mark done"));
    assert!(body.contains("Delete"));

    let stored = db::actions::list_actions_for_application(&state.db, 1)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].due_at.as_deref(), Some("2026-11-05"));
}

#[tokio::test]
async fn an_empty_follow_up_title_is_rejected_with_feedback() {
    let state = state_with_submitted_application().await;
    let pairs = follow_up_pairs("   ", None, None);
    let response = add_application_action(State(state.clone()), Path(1), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Follow-up was not saved: title must not be empty"));
    assert!(
        db::actions::list_actions_for_application(&state.db, 1)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_malformed_follow_up_due_date_is_rejected_with_feedback() {
    let state = state_with_submitted_application().await;
    let pairs = follow_up_pairs("Call back", Some("soon"), None);
    let response = add_application_action(State(state.clone()), Path(1), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Follow-up was not saved: due_at must be a calendar date"));
}

#[tokio::test]
async fn completing_and_reopening_a_follow_up_changes_nothing_else() {
    let state = state_with_submitted_application().await;
    let pairs = follow_up_pairs("Thank the contact", None, None);
    assert_redirect(add_application_action(State(state.clone()), Path(1), Form(pairs)).await);

    // Completion shows the done marker and the reopen affordance.
    assert_redirect(complete_application_action(State(state.clone()), Path(1)).await);
    let body = body_string(
        opportunity_details(State(state.clone()), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(body.contains("Done ·"));
    assert!(body.contains("Reopen"));

    // Completion changed neither the stage nor the outcome, and the
    // history still holds only the submission event. History entries
    // render as <li><strong> on one line, action rows do not.
    let application = db::applications::get_application(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(application.stage, "submitted");
    assert_eq!(application.employer_outcome, "unknown");
    assert_eq!(body.matches("<li><strong>").count(), 1);

    // Reopening clears the marker again.
    assert_redirect(reopen_application_action(State(state.clone()), Path(1)).await);
    let body = body_string(
        opportunity_details(State(state), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(!body.contains("Done ·"));
    assert!(body.contains("Mark done"));
}

#[tokio::test]
async fn deleting_a_follow_up_removes_it_from_the_page() {
    let state = state_with_submitted_application().await;
    let pairs = follow_up_pairs("Book travel", None, None);
    assert_redirect(add_application_action(State(state.clone()), Path(1), Form(pairs)).await);
    assert_redirect(delete_application_action(State(state.clone()), Path(1)).await);

    let body = body_string(
        opportunity_details(State(state), Path(1))
            .await
            .into_response(),
    )
    .await;
    assert!(!body.contains("Book travel"));
    assert!(body.contains("No follow-ups recorded"));
}

#[tokio::test]
async fn follow_up_changes_for_a_missing_row_404() {
    let state = state_with_submitted_application().await;
    let complete = complete_application_action(State(state.clone()), Path(99))
        .await
        .into_response();
    let reopen = reopen_application_action(State(state.clone()), Path(99))
        .await
        .into_response();
    let delete = delete_application_action(State(state), Path(99))
        .await
        .into_response();
    for response in [complete, reopen, delete] {
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

// --- Dashboard ---

/// A stored opportunity for view-model tests, modeled on the fictional
/// university used elsewhere in this file.
fn stored_opportunity_with(deadline: Option<&str>) -> db::opportunities::Opportunity {
    db::opportunities::Opportunity {
        id: 7,
        user_id: 1,
        organization: "Universität Schneckenhausen".to_string(),
        title: "Doctoral researcher".to_string(),
        stream: "QI".to_string(),
        city: Some("Schneckenhausen".to_string()),
        country: Some("Testland".to_string()),
        priority: "p2".to_string(),
        fit_min: Some(3.5),
        fit_max: Some(4.0),
        effort: Some("medium".to_string()),
        rationale: Some("Strong theory fit, lacks computing background.".to_string()),
        deadline_date: deadline.map(str::to_string),
        deadline_kind: deadline.map(|_| "hard_close".to_string()),
        deadline_confidence: deadline.map(|_| "verified".to_string()),
        source_url: Some("https://www.example.org/vacancy".to_string()),
        recruitment_page_url: None,
        source_notes: None,
        notes: None,
        stage: "discovered".to_string(),
        created_at: "2026-10-01 10:00:00".to_string(),
    }
}

#[test]
fn civil_day_arithmetic_round_trips_across_eras_and_leap_years() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    // 19 Oct 2026 is 4 days after 15 Oct 2026.
    assert_eq!(
        days_from_civil(2026, 10, 19) - days_from_civil(2026, 10, 15),
        4
    );
    // Leap day exists, but not in century years that are not divisible by 400.
    assert_eq!(
        days_from_civil(2028, 3, 1) - days_from_civil(2028, 2, 28),
        2
    );
    assert_eq!(
        days_from_civil(1900, 3, 1) - days_from_civil(1900, 2, 28),
        1
    );
    assert_eq!(
        days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 28),
        2
    );

    // Round trips across 130 years, sampled.
    let start = days_from_civil(1970, 1, 1);
    let end = days_from_civil(2100, 1, 1);
    for day in (start..=end).step_by(97) {
        let (year, month, date) = civil_from_days(day);
        assert_eq!(days_from_civil(year, month, date), day);
    }
}

#[test]
fn countdown_labels_are_honest_in_both_directions() {
    let deadline = days_from_civil(2027, 1, 15);
    assert_eq!(countdown_label(deadline, deadline), "today");
    assert_eq!(countdown_label(deadline, deadline - 1), "tomorrow");
    assert_eq!(countdown_label(deadline, deadline - 2), "in 2 days");
    assert_eq!(countdown_label(deadline, deadline - 9), "in 9 days");
    assert_eq!(countdown_label(deadline, deadline + 1), "1 day overdue");
    assert_eq!(countdown_label(deadline, deadline + 4), "4 days overdue");
}

#[test]
fn iso_day_parsing_rejects_non_dates() {
    assert_eq!(
        parse_iso_day_number("2027-01-15"),
        Some(days_from_civil(2027, 1, 15))
    );
    assert_eq!(parse_iso_day_number("19.01.2027"), None);
    assert_eq!(parse_iso_day_number("2027-02-30"), None);
    assert_eq!(parse_iso_day_number("2027-13-01"), None);
    assert_eq!(parse_iso_day_number(""), None);
    assert_eq!(parse_iso_day_number("15 Jan 2027"), None);
}

#[test]
fn dashboard_entries_carry_display_fields_and_urgency() {
    // Fixed as-of date so the displayed urgency is deterministic.
    let as_of = days_from_civil(2026, 10, 19);

    let overdue =
        DeadlineEntryVm::from_opportunity(&stored_opportunity_with(Some("2026-10-15")), as_of);
    assert_eq!(
        overdue.heading,
        "Universität Schneckenhausen — Doctoral researcher"
    );
    assert_eq!(overdue.subtitle, "QI · P2 · Schneckenhausen, Testland");
    assert_eq!(overdue.deadline_display, "15 Oct 2026");
    assert_eq!(
        overdue.deadline_detail.as_deref(),
        Some("Hard close · Verified")
    );
    assert_eq!(overdue.countdown_display.as_deref(), Some("4 days overdue"));
    assert!(overdue.overdue);
    assert_eq!(overdue.priority_class, "p2");
    assert_eq!(
        overdue.meta_display.as_deref(),
        Some("Fit 3.5–4/5 · Effort Medium")
    );
    let external = overdue.external_link.as_ref().unwrap();
    assert_eq!(external.label, "Source");
    assert_eq!(external.url, "https://www.example.org/vacancy");

    let upcoming =
        DeadlineEntryVm::from_opportunity(&stored_opportunity_with(Some("2026-10-23")), as_of);
    assert_eq!(upcoming.countdown_display.as_deref(), Some("in 4 days"));
    assert!(!upcoming.overdue);

    // Without a date there is no countdown and no invented one.
    let undated = DeadlineEntryVm::from_opportunity(&stored_opportunity_with(None), as_of);
    assert_eq!(undated.countdown_display, None);
    assert!(!undated.overdue);
    assert_eq!(undated.deadline_detail, None);
}

#[tokio::test]
async fn dashboard_lists_deadlines_and_moves_submitted_out() {
    let state = test_state().await;
    let response = create_opportunity(State(state.clone()), Form(university_pairs()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let body = body_string(dashboard(State(state.clone())).await.into_response()).await;
    assert!(body.contains("Universität Schneckenhausen — PhD position, low-temperature physics"));
    assert!(body.contains("15 Jan 2027"));
    assert!(body.contains("/opportunities/1"));
    // No advert URL is recorded, so the recruitment page is the external
    // link rather than an invented one.
    assert!(body.contains("example.org/research"));
    assert!(body.contains("Recruitment page"));

    // Confirming the submission moves the entry out of the deadline view
    // while the opportunity itself keeps its recorded deadline.
    assert_redirect(create_application(State(state.clone()), Path(1)).await);
    assert_redirect(prepare_application(State(state.clone()), Path(1)).await);
    assert_redirect(
        record_application_submission(State(state.clone()), Path(1), Form(submission_pairs()))
            .await,
    );

    let body = body_string(dashboard(State(state.clone())).await.into_response()).await;
    assert!(!body.contains("Schneckenhausen"));
    let stored = db::opportunities::get_opportunity(&state.db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.deadline_date.as_deref(), Some("2027-01-15"));
}

#[tokio::test]
async fn the_empty_dashboard_explains_itself_and_links_back() {
    let state = test_state().await;
    let body = body_string(dashboard(State(state.clone())).await.into_response()).await;
    assert!(body.contains("No upcoming deadlines"));
    assert!(body.contains("opportunity list"));
    assert!(body.contains("as of "));
}

#[tokio::test]
async fn the_opportunity_list_links_to_the_dashboard() {
    let state = test_state().await;
    let body = body_string(root(State(state), Query(Vec::new())).await.into_response()).await;
    assert!(body.contains("<a href=\"/dashboard\">Dashboard</a>"));
}

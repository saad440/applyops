//! Tests for the HTTP layer: routes, list and search, form handling, and
//! display helpers, exercised directly against in-memory SQLite.

use super::*;

/// Application state over a fresh in-memory database with migrations and
/// default streams, mirroring server startup.
async fn test_state() -> AppState {
    let pool = db::connect_in_memory().await.expect("in-memory pool");
    db::run_migrations(&pool).await.expect("migrations run");
    db::streams::seed_streams_if_empty(&pool, DEFAULT_USER_ID, &db::streams::DEFAULT_STREAMS)
        .await
        .expect("default streams seeded");
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

//! Tests for the HTTP layer: routes, form handling, and display helpers,
//! exercised directly against in-memory SQLite.

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

#[tokio::test]
async fn root_responds_with_html() {
    let state = test_state().await;
    let response = root(State(state)).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&"text/html; charset=utf-8".parse().unwrap())
    );
}

#[tokio::test]
async fn stored_note_appears_on_index_page() {
    let state = test_state().await;
    db::insert_note(&state.db, DEFAULT_USER_ID, "round trip")
        .await
        .unwrap();

    let response = root(State(state)).await.into_response();
    let body = body_string(response).await;
    assert!(body.contains("round trip"));
    assert!(body.contains("Persistent records"));
}

#[tokio::test]
async fn index_links_to_the_opportunity_form() {
    let state = test_state().await;
    let response = root(State(state)).await.into_response();
    let body = body_string(response).await;
    assert!(body.contains(r#"href="/opportunities/new""#));
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

#[test]
fn index_template_links_stylesheet() {
    let template = IndexTemplate {
        note_count: 0,
        notes: Vec::new(),
    };
    let html = template.render().expect("index template renders");
    assert!(html.contains(r#"href="/static/css/app.css""#));
    assert!(html.contains("ApplyOps"));
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
    let pairs = vec![
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
    ];

    let response = create_opportunity(State(state.clone()), Form(pairs))
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
}

#[tokio::test]
async fn invalid_form_is_redisplayed_with_error_and_values() {
    let state = test_state().await;
    let pairs = vec![
        ("organization".to_string(), String::new()),
        ("title".to_string(), "A title worth keeping".to_string()),
        ("stream".to_string(), "Physics".to_string()),
        ("priority".to_string(), "p1".to_string()),
        ("stage".to_string(), "discovered".to_string()),
    ];

    let response = create_opportunity(State(state.clone()), Form(pairs))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_string(response).await;
    assert!(body.contains("must not be empty"));
    assert!(body.contains("A title worth keeping"));
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
    let pairs = vec![
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
        ("deadline_date".to_string(), "19.01.2027".to_string()),
    ];

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
    let pairs = vec![
        (
            "organization".to_string(),
            "SynerTech Buzzworks GmbH".to_string(),
        ),
        (
            "title".to_string(),
            "Senior AI-Blockchain Synergy Engineer (m/w/d)".to_string(),
        ),
        ("stream".to_string(), "Marine Acoustics".to_string()),
        ("priority".to_string(), "p3".to_string()),
        ("stage".to_string(), "discovered".to_string()),
    ];

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

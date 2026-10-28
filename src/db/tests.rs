//! Data-layer tests for the `db` module, exercising migrations, the
//! opportunity persistence API, and the diagnostic notes path on in-memory
//! SQLite.

use super::opportunities::*;
use super::streams::{self, DEFAULT_STREAMS};
use super::*;

const USER_ID: i64 = 1;

async fn migrated_pool() -> SqlitePool {
    let pool = connect_in_memory().await.expect("in-memory pool");
    run_migrations(&pool).await.expect("migrations run");
    pool
}

/// Fictional university in a made-up village.
fn sample_opportunity() -> NewOpportunity {
    NewOpportunity {
        user_id: USER_ID,
        organization: "Universität Schneckenhausen".to_string(),
        title: "PhD position, low-temperature physics".to_string(),
        stream: DEFAULT_STREAMS[0].to_string(),
        city: Some("Schneckenhausen".to_string()),
        country: Some("Germany".to_string()),
        priority: "p1".to_string(),
        deadline_date: Some("2027-01-15".to_string()),
        source_url: Some("https://www.example.org/vacancy".to_string()),
        notes: Some(
            "Solid measurement background expected; the main gap is cryogenic experience."
                .to_string(),
        ),
        stage: "discovered".to_string(),
    }
}

/// Buzzword-driven IT posting. Hopefully fictional.
fn sample_it_opportunity() -> NewOpportunity {
    NewOpportunity {
        user_id: USER_ID,
        organization: "SynerTech Buzzworks GmbH".to_string(),
        title: "Senior AI-Blockchain Synergy Engineer (m/w/d)".to_string(),
        stream: "IT".to_string(),
        city: Some("Musterstadt".to_string()),
        country: Some("Germany".to_string()),
        priority: "p3".to_string(),
        deadline_date: Some("2027-03-01".to_string()),
        source_url: Some("https://www.example.org/jobs/synergy-engineer".to_string()),
        notes: Some(
            "Promises to disrupt everything; the posting never says what is actually built."
                .to_string(),
        ),
        stage: "discovered".to_string(),
    }
}

#[tokio::test]
async fn opportunity_round_trip_preserves_all_fields() {
    let pool = migrated_pool().await;
    let new = sample_opportunity();

    let id = insert_opportunity(&pool, &new).await.unwrap();
    assert!(id > 0);

    let stored = get_opportunity(&pool, id).await.unwrap().unwrap();
    assert_eq!(stored.user_id, USER_ID);
    assert_eq!(stored.organization, "Universität Schneckenhausen");
    assert_eq!(stored.title, "PhD position, low-temperature physics");
    assert_eq!(stored.stream, DEFAULT_STREAMS[0]);
    assert_eq!(stored.city.as_deref(), Some("Schneckenhausen"));
    assert_eq!(stored.country.as_deref(), Some("Germany"));
    assert_eq!(stored.priority, "p1");
    assert_eq!(stored.deadline_date.as_deref(), Some("2027-01-15"));
    assert_eq!(
        stored.source_url.as_deref(),
        Some("https://www.example.org/vacancy")
    );
    assert_eq!(
        stored.notes.as_deref(),
        Some("Solid measurement background expected; the main gap is cryogenic experience.")
    );
    assert_eq!(stored.stage, "discovered");
    assert!(!stored.created_at.is_empty());
}

#[tokio::test]
async fn absent_optional_fields_stay_absent() {
    let pool = migrated_pool().await;
    let mut new = sample_opportunity();
    new.city = None;
    new.country = None;
    new.deadline_date = None;
    new.source_url = None;
    new.notes = None;

    let id = insert_opportunity(&pool, &new).await.unwrap();
    let stored = get_opportunity(&pool, id).await.unwrap().unwrap();

    assert!(stored.city.is_none());
    assert!(stored.country.is_none());
    assert!(stored.deadline_date.is_none());
    assert!(stored.source_url.is_none());
    assert!(stored.notes.is_none());
}

#[tokio::test]
async fn custom_stream_labels_are_accepted_verbatim() {
    let pool = migrated_pool().await;

    // A label outside the shipped defaults is a valid user-defined
    // stream and is stored unchanged.
    let mut new = sample_opportunity();
    new.stream = "public_art_residency".to_string();
    assert!(validate_new_opportunity(&new).is_ok());

    let id = insert_opportunity(&pool, &new).await.unwrap();
    let stored = get_opportunity(&pool, id).await.unwrap().unwrap();
    assert_eq!(stored.stream, "public_art_residency");

    // A shipped default is equally accepted.
    let mut with_default = sample_opportunity();
    with_default.stream = DEFAULT_STREAMS[0].to_string();
    let default_id = insert_opportunity(&pool, &with_default).await.unwrap();
    let stored_default = get_opportunity(&pool, default_id).await.unwrap().unwrap();
    assert_eq!(stored_default.stream, DEFAULT_STREAMS[0]);
}

#[tokio::test]
async fn users_table_is_seeded_with_the_local_owner() {
    let pool = migrated_pool().await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let owner: i64 = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, USER_ID);
}

#[tokio::test]
async fn unknown_user_id_is_rejected() {
    let pool = migrated_pool().await;
    let mut new = sample_opportunity();
    new.user_id = 42;

    let err = insert_opportunity(&pool, &new).await.unwrap_err();
    assert!(matches!(err, OpportunityInsertError::Database(_)));
    assert_eq!(count_opportunities(&pool).await.unwrap(), 0);
}

#[tokio::test]
async fn unrecognized_priority_or_stage_values_are_rejected() {
    let pool = migrated_pool().await;

    let mut new = sample_opportunity();
    new.priority = "p4".to_string();
    assert_eq!(
        validate_new_opportunity(&new).unwrap_err().field,
        "priority"
    );
    new = sample_opportunity();
    new.stage = "applied".to_string();
    assert_eq!(validate_new_opportunity(&new).unwrap_err().field, "stage");

    let err = insert_opportunity(&pool, &new).await.unwrap_err();
    assert!(matches!(err, OpportunityInsertError::Validation(_)));
    assert_eq!(count_opportunities(&pool).await.unwrap(), 0);
}

#[tokio::test]
async fn malformed_deadlines_are_rejected() {
    let pool = migrated_pool().await;

    // A wrong format is caught by code validation before any SQL runs.
    let mut new = sample_opportunity();
    new.deadline_date = Some("19.01.2027".to_string());
    assert_eq!(
        validate_new_opportunity(&new).unwrap_err().field,
        "deadline_date"
    );

    // A well-formed but impossible calendar date passes code validation
    // and is rejected by the CHECK constraint on the column.
    new.deadline_date = Some("2027-02-30".to_string());
    assert!(validate_new_opportunity(&new).is_ok());
    let err = insert_opportunity(&pool, &new).await.unwrap_err();
    assert!(matches!(err, OpportunityInsertError::Database(_)));

    assert_eq!(count_opportunities(&pool).await.unwrap(), 0);
}

#[tokio::test]
async fn list_returns_newest_first() {
    let pool = migrated_pool().await;

    insert_opportunity(&pool, &sample_opportunity())
        .await
        .unwrap();
    insert_opportunity(&pool, &sample_it_opportunity())
        .await
        .unwrap();

    let listed = list_opportunities(&pool, 10).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].organization, "SynerTech Buzzworks GmbH");
    assert_eq!(listed[1].organization, "Universität Schneckenhausen");
}

#[tokio::test]
async fn get_missing_opportunity_returns_none() {
    let pool = migrated_pool().await;
    assert!(get_opportunity(&pool, 999).await.unwrap().is_none());
}

#[tokio::test]
async fn diagnostic_and_opportunity_tables_coexist() {
    let pool = migrated_pool().await;
    insert_note(&pool, USER_ID, "still works").await.unwrap();
    insert_opportunity(&pool, &sample_opportunity())
        .await
        .unwrap();

    assert_eq!(count_notes(&pool).await.unwrap(), 1);
    assert_eq!(count_opportunities(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn default_streams_seed_only_while_empty() {
    let pool = migrated_pool().await;

    let first = streams::seed_streams_if_empty(&pool, USER_ID, &DEFAULT_STREAMS)
        .await
        .unwrap();
    assert_eq!(first, DEFAULT_STREAMS.len());

    // A second run finds the table populated and adds nothing.
    let second = streams::seed_streams_if_empty(&pool, USER_ID, &DEFAULT_STREAMS)
        .await
        .unwrap();
    assert_eq!(second, 0);

    let listed = streams::list_streams(&pool, USER_ID).await.unwrap();
    assert_eq!(listed.len(), DEFAULT_STREAMS.len());
    let names: Vec<&str> = listed.iter().map(|stream| stream.name.as_str()).collect();
    assert_eq!(names, ["IT", "Physics", "Scientific Computing"]);
}

#[tokio::test]
async fn inserting_an_existing_stream_returns_the_same_row() {
    let pool = migrated_pool().await;
    streams::seed_streams_if_empty(&pool, USER_ID, &DEFAULT_STREAMS)
        .await
        .unwrap();

    let seeded_id = streams::insert_stream(&pool, USER_ID, "Physics")
        .await
        .unwrap();
    let again = streams::insert_stream(&pool, USER_ID, "  Physics  ")
        .await
        .unwrap();
    assert_eq!(seeded_id, again);

    let custom = streams::insert_stream(&pool, USER_ID, "Marine Acoustics")
        .await
        .unwrap();
    assert_ne!(seeded_id, custom);

    let listed = streams::list_streams(&pool, USER_ID).await.unwrap();
    assert_eq!(listed.len(), DEFAULT_STREAMS.len() + 1);
}

#[tokio::test]
async fn streams_require_a_known_user() {
    let pool = migrated_pool().await;

    let err = streams::insert_stream(&pool, 42, "Physics")
        .await
        .unwrap_err();
    assert!(matches!(err, sqlx::Error::Database(_)));
}

#[tokio::test]
async fn source_urls_must_be_web_addresses() {
    let pool = migrated_pool().await;

    let mut new = sample_opportunity();
    new.source_url = Some("ftp://files.example.org/vacancy".to_string());
    assert_eq!(
        validate_new_opportunity(&new).unwrap_err().field,
        "source_url"
    );

    new.source_url = Some("https://".to_string());
    assert_eq!(
        validate_new_opportunity(&new).unwrap_err().field,
        "source_url"
    );

    // The scheme check is case-insensitive; the address is stored verbatim.
    new.source_url = Some("HTTPS://Example.org/Vacancy".to_string());
    assert!(validate_new_opportunity(&new).is_ok());

    let id = insert_opportunity(&pool, &new).await.unwrap();
    let stored = get_opportunity(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        stored.source_url.as_deref(),
        Some("HTTPS://Example.org/Vacancy")
    );
}

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

#[tokio::test]
async fn update_changes_stored_values() {
    let pool = migrated_pool().await;
    let id = insert_opportunity(&pool, &sample_opportunity())
        .await
        .unwrap();

    let mut edited = sample_opportunity();
    edited.title = "PhD position, cryogenic detectors".to_string();
    edited.stage = "researching".to_string();
    edited.deadline_date = None;
    update_opportunity(&pool, id, &edited)
        .await
        .unwrap()
        .unwrap();

    let stored = get_opportunity(&pool, id).await.unwrap().unwrap();
    assert_eq!(stored.title, "PhD position, cryogenic detectors");
    assert_eq!(stored.stage, "researching");
    assert!(stored.deadline_date.is_none());
    assert_eq!(stored.organization, "Universität Schneckenhausen");
}

#[tokio::test]
async fn update_missing_row_returns_none() {
    let pool = migrated_pool().await;
    let result = update_opportunity(&pool, 999, &sample_opportunity())
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn update_rejects_invalid_values_without_touching_the_row() {
    let pool = migrated_pool().await;
    let id = insert_opportunity(&pool, &sample_opportunity())
        .await
        .unwrap();

    let mut edited = sample_opportunity();
    edited.stage = "applied".to_string();
    let err = update_opportunity(&pool, id, &edited).await.unwrap_err();
    assert!(matches!(err, OpportunityInsertError::Validation(_)));

    let stored = get_opportunity(&pool, id).await.unwrap().unwrap();
    assert_eq!(stored.stage, "discovered");
}

#[tokio::test]
async fn search_matches_fields_and_escapes_wildcards() {
    let pool = migrated_pool().await;

    let mut university = sample_opportunity();
    university.notes = Some("Fond of literals like a_b and 100%".to_string());
    insert_opportunity(&pool, &university).await.unwrap();

    insert_opportunity(&pool, &sample_it_opportunity())
        .await
        .unwrap();

    // Matches by organization substring, newest first.
    let hits = search_opportunities(&pool, "Buzzworks", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].organization, "SynerTech Buzzworks GmbH");

    // Matches by title.
    let hits = search_opportunities(&pool, "low-temperature", 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);

    // Wildcards in the query are literals, not patterns.
    let hits = search_opportunities(&pool, "a_b", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    let hits = search_opportunities(&pool, "100%", 10).await.unwrap();
    assert_eq!(hits.len(), 1);

    // ASCII case-insensitivity.
    let hits = search_opportunities(&pool, "buzzworks", 10).await.unwrap();
    assert_eq!(hits.len(), 1);

    // No match yields an empty list.
    let hits = search_opportunities(&pool, "quantum", 10).await.unwrap();
    assert!(hits.is_empty());
}

// --- Applications: attempts, confirmed submissions, corrections ---

use super::application_events;
use super::applications;

/// Inserts the sample opportunity and returns its id.
async fn opportunity_id(pool: &SqlitePool) -> i64 {
    insert_opportunity(pool, &sample_opportunity())
        .await
        .unwrap()
}

#[tokio::test]
async fn application_starts_as_draft_with_defaults() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;

    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(stored.user_id, USER_ID);
    assert_eq!(stored.opportunity_id, opportunity_id);
    assert_eq!(stored.stage, "draft");
    assert!(stored.submitted_at.is_none());
    assert_eq!(stored.submission_channel, "unknown");
    assert_eq!(stored.employer_outcome, "unknown");
    assert!(stored.outcome_at.is_none());
    assert!(
        applications::active_application_for_opportunity(&pool, opportunity_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn application_requires_an_existing_opportunity() {
    let pool = migrated_pool().await;
    let err = applications::insert_application(&pool, USER_ID, 999)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::ApplicationInsertError::OpportunityMissing
    ));
}

#[tokio::test]
async fn only_one_active_attempt_per_opportunity() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;

    applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    let err = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::ApplicationInsertError::ActiveAttemptExists
    ));

    // A second attempt becomes possible once the first one is withdrawn.
    let first = applications::active_application_for_opportunity(&pool, opportunity_id)
        .await
        .unwrap()
        .unwrap();
    applications::set_in_progress_stage(&pool, first.id, "preparing")
        .await
        .unwrap();
    applications::record_submission(&pool, first.id, "2026-10-19", "email")
        .await
        .unwrap();
    applications::withdraw(&pool, first.id).await.unwrap();
    assert!(
        applications::active_application_for_opportunity(&pool, opportunity_id)
            .await
            .unwrap()
            .is_none()
    );

    applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    assert_eq!(
        applications::list_applications_for_opportunity(&pool, opportunity_id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn in_progress_transitions_only_move_between_draft_and_preparing() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();

    // draft -> preparing and back are legal in-progress corrections.
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();
    assert_eq!(
        applications::get_application(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .stage,
        "preparing"
    );
    applications::set_in_progress_stage(&pool, id, "draft")
        .await
        .unwrap();

    // `submitted` is reached only from `preparing`.
    let err = applications::record_submission(&pool, id, "2026-10-19", "email")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::SubmissionError::InvalidTransition { .. }
    ));

    // An in-progress move may target only draft or preparing.
    let err = applications::set_in_progress_stage(&pool, id, "submitted")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::StageTransitionError::InvalidTransition { .. }
    ));

    // In-progress corrections are not recorded in the event history.
    let events = application_events::list_events_for_opportunity(&pool, opportunity_id, 10)
        .await
        .unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn submission_is_confirmed_with_date_and_channel() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();

    applications::record_submission(&pool, id, "2026-10-19", "employment_agency")
        .await
        .unwrap();
    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "submitted");
    assert_eq!(stored.submitted_at.as_deref(), Some("2026-10-19"));
    assert_eq!(stored.submission_channel, "employment_agency");

    // The event history records the change with old and new values and the
    // submission facts.
    let events = application_events::list_events_for_opportunity(&pool, opportunity_id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, application_events::EVENT_SUBMITTED);
    assert_eq!(events[0].entity_type, "application");
    assert_eq!(events[0].entity_id, id);
    assert_eq!(events[0].old_value.as_deref(), Some("preparing"));
    assert_eq!(events[0].new_value.as_deref(), Some("submitted"));
    let note = events[0].note.as_deref().unwrap_or_default();
    assert!(note.contains("submitted_at=2026-10-19"));
    assert!(note.contains("channel=employment_agency"));
}

#[tokio::test]
async fn submission_validation_rejects_bad_dates_and_channels() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();

    let err = applications::record_submission(&pool, id, "", "email")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::SubmissionError::Validation(applications::SubmissionValidationError {
            field: "submitted_at",
            ..
        })
    ));

    let err = applications::record_submission(&pool, id, "19.01.2027", "email")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::SubmissionError::Validation(applications::SubmissionValidationError {
            field: "submitted_at",
            ..
        })
    ));

    // Well-formed but impossible: code validation passes, the CHECK
    // constraint on the column rejects it.
    let err = applications::record_submission(&pool, id, "2027-02-30", "email")
        .await
        .unwrap_err();
    assert!(matches!(err, applications::SubmissionError::Database(_)));

    let err = applications::record_submission(&pool, id, "2026-10-19", "carrier_pigeon")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::SubmissionError::Validation(applications::SubmissionValidationError {
            field: "submission_channel",
            ..
        })
    ));

    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "preparing");
}

#[tokio::test]
async fn correction_returns_to_draft_and_keeps_the_evidence() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();
    applications::record_submission(&pool, id, "2026-10-12", "employer_portal")
        .await
        .unwrap();

    applications::correct_submission_to_draft(&pool, id)
        .await
        .unwrap();
    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "draft");
    assert!(stored.submitted_at.is_none());
    assert_eq!(stored.submission_channel, "unknown");
    assert_eq!(stored.employer_outcome, "unknown");

    // Both the recorded submission and the correction remain in the trail:
    // the correction is recorded, not erased.
    let events = application_events::list_events_for_opportunity(&pool, opportunity_id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].event_type,
        application_events::EVENT_SUBMISSION_CORRECTED
    );
    assert_eq!(events[0].old_value.as_deref(), Some("submitted"));
    assert_eq!(events[0].new_value.as_deref(), Some("draft"));
    let note = events[0].note.as_deref().unwrap_or_default();
    assert!(note.contains("submitted_at=2026-10-12"));
    assert!(note.contains("reset to unknown"));

    // A correction is only possible from `submitted`.
    let err = applications::correct_submission_to_draft(&pool, id)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::StageTransitionError::InvalidTransition { .. }
    ));
}

#[tokio::test]
async fn withdrawal_is_terminal_and_evented() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();
    applications::record_submission(&pool, id, "2026-10-19", "email")
        .await
        .unwrap();

    applications::withdraw(&pool, id).await.unwrap();
    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "withdrawn");

    // Terminal: a withdrawn attempt cannot be withdrawn again or corrected.
    let err = applications::withdraw(&pool, id).await.unwrap_err();
    assert!(matches!(
        err,
        applications::StageTransitionError::InvalidTransition { .. }
    ));
    let err = applications::correct_submission_to_draft(&pool, id)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::StageTransitionError::InvalidTransition { .. }
    ));

    let events = application_events::list_events_for_opportunity(&pool, opportunity_id, 10)
        .await
        .unwrap();
    assert_eq!(events[0].event_type, application_events::EVENT_WITHDRAWN);
}

#[tokio::test]
async fn missing_applications_report_application_missing() {
    let pool = migrated_pool().await;
    assert!(matches!(
        applications::set_in_progress_stage(&pool, 999, "preparing").await,
        Err(applications::StageTransitionError::ApplicationMissing)
    ));
    assert!(matches!(
        applications::record_submission(&pool, 999, "2026-10-19", "email").await,
        Err(applications::SubmissionError::ApplicationMissing)
    ));
    assert!(matches!(
        applications::correct_submission_to_draft(&pool, 999).await,
        Err(applications::StageTransitionError::ApplicationMissing)
    ));
    assert!(matches!(
        applications::withdraw(&pool, 999).await,
        Err(applications::StageTransitionError::ApplicationMissing)
    ));
    assert!(
        applications::get_application(&pool, 999)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn employer_outcome_is_recorded_with_date_and_event() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();
    applications::record_submission(&pool, id, "2026-10-19", "email")
        .await
        .unwrap();

    applications::record_employer_outcome(&pool, id, "interview", Some("2026-10-25"))
        .await
        .unwrap();
    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "submitted");
    assert_eq!(stored.employer_outcome, "interview");
    assert_eq!(stored.outcome_at.as_deref(), Some("2026-10-25"));

    let events = application_events::list_events_for_opportunity(&pool, opportunity_id, 10)
        .await
        .unwrap();
    assert_eq!(
        events[0].event_type,
        application_events::EVENT_OUTCOME_CHANGED
    );
    assert_eq!(events[0].old_value.as_deref(), Some("unknown"));
    assert_eq!(events[0].new_value.as_deref(), Some("interview"));
    assert_eq!(events[0].note.as_deref(), Some("outcome_at=2026-10-25"));
}

#[tokio::test]
async fn employer_outcome_requires_a_submitted_attempt() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();

    let err = applications::record_employer_outcome(&pool, id, "interview", Some("2026-10-25"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::OutcomeError::NotSubmitted { .. }
    ));
}

#[tokio::test]
async fn employer_outcome_validation_rejects_bad_values_and_dates() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();
    applications::record_submission(&pool, id, "2026-10-19", "email")
        .await
        .unwrap();

    let err = applications::record_employer_outcome(&pool, id, "hired", Some("2026-10-25"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::OutcomeError::Validation(applications::SubmissionValidationError {
            field: "employer_outcome",
            ..
        })
    ));

    let err = applications::record_employer_outcome(&pool, id, "offer", None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::OutcomeError::Validation(applications::SubmissionValidationError {
            field: "outcome_at",
            ..
        })
    ));

    let err = applications::record_employer_outcome(&pool, id, "offer", Some("19.10.2027"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        applications::OutcomeError::Validation(applications::SubmissionValidationError {
            field: "outcome_at",
            ..
        })
    ));

    // Well-formed but impossible: the CHECK constraint rejects it.
    let err = applications::record_employer_outcome(&pool, id, "offer", Some("2027-02-30"))
        .await
        .unwrap_err();
    assert!(matches!(err, applications::OutcomeError::Database(_)));

    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.employer_outcome, "unknown");
}

#[tokio::test]
async fn clearing_the_outcome_with_unknown_removes_the_date() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();
    applications::record_submission(&pool, id, "2026-10-19", "email")
        .await
        .unwrap();
    applications::record_employer_outcome(&pool, id, "rejected", Some("2026-10-25"))
        .await
        .unwrap();

    applications::record_employer_outcome(&pool, id, "unknown", None)
        .await
        .unwrap();
    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.employer_outcome, "unknown");
    assert!(stored.outcome_at.is_none());

    let events = application_events::list_events_for_opportunity(&pool, opportunity_id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].old_value.as_deref(), Some("rejected"));
    assert_eq!(events[0].new_value.as_deref(), Some("unknown"));
    assert_eq!(events[0].note.as_deref(), Some("outcome cleared"));
}

#[tokio::test]
async fn outcome_corrections_append_events() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();
    applications::record_submission(&pool, id, "2026-10-19", "email")
        .await
        .unwrap();

    applications::record_employer_outcome(&pool, id, "pending", Some("2026-10-20"))
        .await
        .unwrap();
    applications::record_employer_outcome(&pool, id, "offer", Some("2026-11-01"))
        .await
        .unwrap();

    let events = application_events::list_events_for_opportunity(&pool, opportunity_id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].old_value.as_deref(), Some("pending"));
    assert_eq!(events[0].new_value.as_deref(), Some("offer"));
    assert_eq!(events[0].note.as_deref(), Some("outcome_at=2026-11-01"));
}

#[tokio::test]
async fn missing_application_outcome_reports_application_missing() {
    let pool = migrated_pool().await;
    assert!(matches!(
        applications::record_employer_outcome(&pool, 999, "offer", Some("2026-10-25")).await,
        Err(applications::OutcomeError::ApplicationMissing)
    ));
}

// --- Reporting obligations and known authorities ---

use super::authorities;
use super::obligations;

/// Inserts the sample opportunity and a submitted application attempt,
/// returning the opportunity and application ids.
async fn submitted_attempt(pool: &SqlitePool) -> (i64, i64) {
    let opportunity_id = opportunity_id(pool).await;
    let id = applications::insert_application(pool, USER_ID, opportunity_id)
        .await
        .unwrap();
    applications::set_in_progress_stage(pool, id, "preparing")
        .await
        .unwrap();
    applications::record_submission(pool, id, "2026-10-19", "email")
        .await
        .unwrap();
    (opportunity_id, id)
}

/// Report input with everything optional left empty.
fn report(
    status: &'static str,
    reported_at: Option<&'static str>,
) -> obligations::NewReport<'static> {
    obligations::NewReport {
        status,
        reported_at,
        method: None,
        reference: None,
        recipient_name: None,
        notes: None,
    }
}

#[tokio::test]
async fn report_saves_update_one_row_per_kind() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;

    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("pending", None),
    )
    .await
    .unwrap();

    let stored = obligations::get_obligation(&pool, id, obligations::DEFAULT_KIND)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.user_id, USER_ID);
    assert_eq!(stored.application_id, id);
    assert_eq!(stored.kind, obligations::DEFAULT_KIND);
    assert_eq!(stored.status, "pending");
    assert!(stored.reported_at.is_none());

    // A correction updates the same row rather than adding one.
    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &obligations::NewReport {
            status: "reported",
            reported_at: Some("2026-11-02"),
            method: Some("portal"),
            reference: Some("AB-123"),
            recipient_name: Some("District Office Musterstadt"),
            notes: Some("confirmed online"),
        },
    )
    .await
    .unwrap();

    let stored = obligations::get_obligation(&pool, id, obligations::DEFAULT_KIND)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "reported");
    assert_eq!(stored.reported_at.as_deref(), Some("2026-11-02"));
    assert_eq!(stored.method.as_deref(), Some("portal"));
    assert_eq!(stored.reference.as_deref(), Some("AB-123"));
    assert_eq!(
        stored.recipient_name.as_deref(),
        Some("District Office Musterstadt")
    );
    assert_eq!(stored.notes.as_deref(), Some("confirmed online"));

    // Only one row may exist per application and kind.
    let err = sqlx::query(
        "INSERT INTO reporting_obligations (user_id, application_id, kind) \
         VALUES (?, ?, 'employment_agency')",
    )
    .bind(USER_ID)
    .bind(id)
    .execute(&pool)
    .await;
    assert!(err.is_err());
}

#[tokio::test]
async fn reported_requires_a_calendar_date() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;

    let err = obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("reported", None),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        obligations::ReportError::Validation(applications::SubmissionValidationError {
            field: "reported_at",
            ..
        })
    ));

    let err = obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("reported", Some("02.11.2026")),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, obligations::ReportError::Validation(_)));

    // Well-formed but impossible: code validation passes, the CHECK
    // constraint on the column rejects it.
    let err = obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("reported", Some("2026-02-30")),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, obligations::ReportError::Database(_)));

    // Nothing was stored along the way.
    assert!(
        obligations::get_obligation(&pool, id, obligations::DEFAULT_KIND)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn leaving_reported_clears_the_date_and_keeps_the_evidence() {
    let pool = migrated_pool().await;
    let (opportunity_id, id) = submitted_attempt(&pool).await;

    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &obligations::NewReport {
            status: "reported",
            reported_at: Some("2026-11-02"),
            method: Some("manual"),
            reference: None,
            recipient_name: None,
            notes: None,
        },
    )
    .await
    .unwrap();

    // Correcting an accidental report back to pending clears the filed
    // date from the row.
    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("pending", None),
    )
    .await
    .unwrap();

    let stored = obligations::get_obligation(&pool, id, obligations::DEFAULT_KIND)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "pending");
    assert!(stored.reported_at.is_none());
    assert_eq!(stored.method, None);

    // The history keeps the old facts: the obligation event appears next
    // to the application events of the same attempt.
    let events = application_events::list_events_for_opportunity(&pool, opportunity_id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].event_type,
        application_events::EVENT_REPORT_STATUS_CHANGED
    );
    assert_eq!(events[0].entity_type, "obligation");
    assert_eq!(events[0].old_value.as_deref(), Some("reported"));
    assert_eq!(events[0].new_value.as_deref(), Some("pending"));
    let note = events[0].note.as_deref().unwrap_or_default();
    assert!(note.contains("was reported_at=2026-11-02"));
}

#[tokio::test]
async fn report_requires_a_submitted_application() {
    let pool = migrated_pool().await;
    let opportunity_id = opportunity_id(&pool).await;
    let id = applications::insert_application(&pool, USER_ID, opportunity_id)
        .await
        .unwrap();

    // A draft attempt has no submission to report.
    assert!(matches!(
        obligations::record_report(
            &pool,
            id,
            obligations::DEFAULT_KIND,
            &report("pending", None)
        )
        .await,
        Err(obligations::ReportError::NotSubmitted { .. })
    ));

    applications::set_in_progress_stage(&pool, id, "preparing")
        .await
        .unwrap();
    assert!(matches!(
        obligations::record_report(&pool, id, obligations::DEFAULT_KIND, &report("unknown", None))
            .await,
        Err(obligations::ReportError::NotSubmitted { stage })
            if stage == "preparing"
    ));

    // Once submitted, the reporting state can be recorded as unknown or
    // pending.
    applications::record_submission(&pool, id, "2026-10-19", "email")
        .await
        .unwrap();
    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("unknown", None),
    )
    .await
    .unwrap();
    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("pending", None),
    )
    .await
    .unwrap();

    let stored = obligations::get_obligation(&pool, id, obligations::DEFAULT_KIND)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "pending");
}

#[tokio::test]
async fn other_kind_requires_a_recipient_name() {
    let pool = migrated_pool().await;
    let (opportunity_id, id) = submitted_attempt(&pool).await;

    let err = obligations::record_report(&pool, id, "other", &report("pending", None))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        obligations::ReportError::Validation(applications::SubmissionValidationError {
            field: "recipient_name",
            ..
        })
    ));

    // The default kind carries no recipient requirement.
    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("pending", None),
    )
    .await
    .unwrap();

    // With a recipient, the reserved kind is accepted as its own row.
    obligations::record_report(
        &pool,
        id,
        "other",
        &obligations::NewReport {
            status: "pending",
            reported_at: None,
            method: None,
            reference: None,
            recipient_name: Some("District Office Musterstadt"),
            notes: None,
        },
    )
    .await
    .unwrap();

    let obligations_list = obligations::list_obligations_for_opportunity(&pool, opportunity_id)
        .await
        .unwrap();
    assert_eq!(obligations_list.len(), 2);
}

#[tokio::test]
async fn correction_is_blocked_while_a_report_is_open() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;

    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("pending", None),
    )
    .await
    .unwrap();
    assert!(matches!(
        applications::correct_submission_to_draft(&pool, id).await,
        Err(applications::StageTransitionError::ReportingBlocked)
    ));

    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("reported", Some("2026-11-02")),
    )
    .await
    .unwrap();
    assert!(matches!(
        applications::correct_submission_to_draft(&pool, id).await,
        Err(applications::StageTransitionError::ReportingBlocked)
    ));

    // Setting the report aside unblocks the correction; the submission is
    // then corrected with the usual clearing and history entry.
    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("not_required", None),
    )
    .await
    .unwrap();
    applications::correct_submission_to_draft(&pool, id)
        .await
        .unwrap();

    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "draft");
    assert!(stored.submitted_at.is_none());
}

#[tokio::test]
async fn backlog_counts_and_entries_reflect_sql_data() {
    let pool = migrated_pool().await;

    /// Creates an opportunity and its submitted attempt.
    async fn submit_against(pool: &SqlitePool, variant: u8) -> i64 {
        let mut new = sample_opportunity();
        new.title = format!("Position {variant}");
        let opportunity_id = insert_opportunity(pool, &new).await.unwrap();
        let id = applications::insert_application(pool, USER_ID, opportunity_id)
            .await
            .unwrap();
        applications::set_in_progress_stage(pool, id, "preparing")
            .await
            .unwrap();
        applications::record_submission(pool, id, "2026-10-19", "email")
            .await
            .unwrap();
        id
    }

    let pending_id = submit_against(&pool, 1).await;
    let unknown_id = submit_against(&pool, 2).await;
    let reported_id = submit_against(&pool, 3).await;
    let withdrawn_id = submit_against(&pool, 4).await;

    obligations::record_report(
        &pool,
        pending_id,
        obligations::DEFAULT_KIND,
        &report("pending", None),
    )
    .await
    .unwrap();
    obligations::record_report(
        &pool,
        reported_id,
        obligations::DEFAULT_KIND,
        &report("reported", Some("2026-11-02")),
    )
    .await
    .unwrap();
    obligations::record_report(
        &pool,
        withdrawn_id,
        obligations::DEFAULT_KIND,
        &report("pending", None),
    )
    .await
    .unwrap();
    // A withdrawn attempt leaves the backlog even with a pending report.
    applications::withdraw(&pool, withdrawn_id).await.unwrap();

    let counts = obligations::reporting_counts(&pool).await.unwrap();
    assert_eq!(counts.pending, 1);
    // The attempt without any row counts as unknown.
    assert_eq!(counts.unknown, 1);
    assert_eq!(counts.reported, 1);
    assert_eq!(counts.not_required, 0);

    let entries = obligations::reporting_backlog(&pool, 10).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].application_id, pending_id);
    assert_eq!(entries[0].status.as_deref(), Some("pending"));
    assert_eq!(entries[1].application_id, unknown_id);
    assert!(entries[1].status.is_none());
}

#[tokio::test]
async fn authorities_seed_and_learn_labels() {
    let pool = migrated_pool().await;

    assert_eq!(
        authorities::seed_authorities_if_empty(&pool, USER_ID, &authorities::DEFAULT_AUTHORITIES)
            .await
            .unwrap(),
        2
    );
    // Seeding again changes nothing.
    assert_eq!(
        authorities::seed_authorities_if_empty(&pool, USER_ID, &authorities::DEFAULT_AUTHORITIES)
            .await
            .unwrap(),
        0
    );

    let stored = authorities::list_authorities(&pool, USER_ID).await.unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].name, "Agentur für Arbeit");
    assert_eq!(stored[0].is_seed, Some(true));

    // A learned label gains its own row, marked as not a seed.
    let learned = authorities::insert_authority(&pool, USER_ID, "District Office Musterstadt")
        .await
        .unwrap()
        .expect("a non-empty label is stored");
    // ASCII case differences resolve to the same label; the most recently
    // used spelling is kept, so a corrected capitalisation replaces the
    // stored one.
    let case_variant = authorities::insert_authority(&pool, USER_ID, "district office musterstadt")
        .await
        .unwrap()
        .expect("the label exists");
    assert_eq!(learned, case_variant);
    let existing = authorities::insert_authority(&pool, USER_ID, "Jobcenter")
        .await
        .unwrap()
        .expect("the seeded label exists");
    assert_ne!(existing, learned);

    // An empty label is not stored.
    assert_eq!(
        authorities::insert_authority(&pool, USER_ID, "   ")
            .await
            .unwrap(),
        None
    );

    let stored = authorities::list_authorities(&pool, USER_ID).await.unwrap();
    assert_eq!(stored.len(), 3);
    let learned_label = stored
        .iter()
        .find(|authority| {
            authority
                .name
                .eq_ignore_ascii_case("district office musterstadt")
        })
        .unwrap();
    assert_eq!(learned_label.name, "district office musterstadt");
    assert_eq!(learned_label.is_seed, None);

    // Labels are advisory: removing one affects suggestions only and
    // leaves the obligation rows untouched.
    sqlx::query("DELETE FROM known_authorities WHERE id = ?")
        .bind(learned)
        .execute(&pool)
        .await
        .unwrap();
    let remaining = authorities::list_authorities(&pool, USER_ID).await.unwrap();
    assert_eq!(remaining.len(), 2);
}

use super::actions;

/// Follow-up input with everything optional left empty.
fn action(
    title: &'static str,
    due_at: Option<&'static str>,
    notes: Option<&'static str>,
) -> actions::NewAction<'static> {
    actions::NewAction {
        title,
        due_at,
        notes,
    }
}

#[tokio::test]
async fn follow_up_round_trip_preserves_fields() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;

    let action_id = actions::add_action(
        &pool,
        id,
        &action(
            "Follow up with the employer",
            Some("2026-11-05"),
            Some("Ask about the interview panel "),
        ),
    )
    .await
    .unwrap();

    let stored = actions::get_action(&pool, action_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.user_id, USER_ID);
    assert_eq!(stored.application_id, id);
    assert_eq!(stored.title, "Follow up with the employer");
    assert_eq!(stored.due_at.as_deref(), Some("2026-11-05"));
    assert!(stored.completed_at.is_none());
    assert_eq!(
        stored.notes.as_deref(),
        Some("Ask about the interview panel")
    );
    assert!(!stored.created_at.is_empty());
}

#[tokio::test]
async fn follow_up_validation_rejects_empty_title_and_bad_dates() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;

    let err = actions::add_action(&pool, id, &action("   ", None, None))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        actions::ActionError::Validation(ref err) if err.field == "title"
    ));

    let err = actions::add_action(&pool, id, &action("Call", Some("soon"), None))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        actions::ActionError::Validation(ref err) if err.field == "due_at"
    ));

    // A calendar-impossible date passes the format check but is rejected
    // by the column's CHECK constraint.
    let err = actions::add_action(&pool, id, &action("Call", Some("2027-02-30"), None))
        .await
        .unwrap_err();
    assert!(matches!(err, actions::ActionError::Database(_)));

    // The attempt still holds no actions after the rejected saves.
    assert!(
        actions::list_actions_for_application(&pool, id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn follow_up_requires_an_existing_application() {
    let pool = migrated_pool().await;
    let err = actions::add_action(&pool, 999, &action("Call back", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, actions::ActionError::ApplicationMissing));
}

#[tokio::test]
async fn an_attempt_holds_zero_or_many_follow_ups() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;

    assert!(
        actions::list_actions_for_application(&pool, id)
            .await
            .unwrap()
            .is_empty()
    );

    for title in [
        "Ask for feedback",
        "Prepare for the interview",
        "Send portfolio",
    ] {
        actions::add_action(&pool, id, &action(title, None, None))
            .await
            .unwrap();
    }
    let stored = actions::list_actions_for_application(&pool, id)
        .await
        .unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[0].title, "Ask for feedback");
}

#[tokio::test]
async fn open_follow_ups_sort_by_due_date_with_completed_last() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;

    actions::add_action(&pool, id, &action("Undated", None, None))
        .await
        .unwrap();
    actions::add_action(&pool, id, &action("Later", Some("2026-12-01"), None))
        .await
        .unwrap();
    actions::add_action(&pool, id, &action("Sooner", Some("2026-11-05"), None))
        .await
        .unwrap();

    // Completing the undated one moves it to the end even though it was
    // created first.
    let stored = actions::list_actions_for_application(&pool, id)
        .await
        .unwrap();
    let undated = stored.iter().find(|a| a.title == "Undated").unwrap().id;
    actions::set_action_completed(&pool, undated, true)
        .await
        .unwrap();

    let stored = actions::list_actions_for_application(&pool, id)
        .await
        .unwrap();
    let titles: Vec<&str> = stored.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, ["Sooner", "Later", "Undated"]);
    assert!(stored[2].completed_at.is_some());
}

#[tokio::test]
async fn completing_a_follow_up_changes_no_stage_or_report_state_and_writes_no_event() {
    let pool = migrated_pool().await;
    let (opportunity_id, id) = submitted_attempt(&pool).await;

    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("pending", None),
    )
    .await
    .unwrap();
    let action_id = actions::add_action(&pool, id, &action("Thank the contact", None, None))
        .await
        .unwrap();
    let events_before = application_events::list_events_for_opportunity(&pool, opportunity_id, 100)
        .await
        .unwrap()
        .len();

    actions::set_action_completed(&pool, action_id, true)
        .await
        .unwrap();

    let stored = applications::get_application(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.stage, "submitted");
    let obligation = obligations::get_obligation(&pool, id, obligations::DEFAULT_KIND)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(obligation.status, "pending");
    let events_after = application_events::list_events_for_opportunity(&pool, opportunity_id, 100)
        .await
        .unwrap();
    assert_eq!(events_after.len(), events_before);
}

#[tokio::test]
async fn reopening_clears_the_completion_moment() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;
    let action_id = actions::add_action(&pool, id, &action("Check deadline extension", None, None))
        .await
        .unwrap();

    actions::set_action_completed(&pool, action_id, true)
        .await
        .unwrap();
    let stored = actions::get_action(&pool, action_id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.completed_at.is_some());

    actions::set_action_completed(&pool, action_id, false)
        .await
        .unwrap();
    let stored = actions::get_action(&pool, action_id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.completed_at.is_none());
}

#[tokio::test]
async fn deleting_a_follow_up_removes_the_row() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;
    let action_id = actions::add_action(&pool, id, &action("Book travel", None, None))
        .await
        .unwrap();

    assert!(actions::delete_action(&pool, action_id).await.unwrap());
    assert!(
        actions::get_action(&pool, action_id)
            .await
            .unwrap()
            .is_none()
    );
    // Deleting again reports that nothing was found.
    assert!(!actions::delete_action(&pool, action_id).await.unwrap());
}

#[tokio::test]
async fn no_other_workflow_creates_or_touches_follow_ups() {
    let pool = migrated_pool().await;
    let (_, id) = submitted_attempt(&pool).await;

    // The whole lifecycle around submissions, outcomes, and reporting
    // runs without creating, completing, or deleting any follow-up.
    applications::record_employer_outcome(&pool, id, "interview", Some("2026-10-25"))
        .await
        .unwrap();
    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("reported", Some("2026-11-02")),
    )
    .await
    .unwrap();
    assert!(
        actions::list_actions_for_application(&pool, id)
            .await
            .unwrap()
            .is_empty()
    );

    obligations::record_report(
        &pool,
        id,
        obligations::DEFAULT_KIND,
        &report("not_required", None),
    )
    .await
    .unwrap();
    applications::correct_submission_to_draft(&pool, id)
        .await
        .unwrap();
    assert!(
        actions::list_actions_for_application(&pool, id)
            .await
            .unwrap()
            .is_empty()
    );
}

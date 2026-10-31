-- Fourth migration: applications as first-class records separate from
-- opportunities, plus the append-only event history. An opportunity
-- can carry more than one attempt over time, so there is no unique
-- constraint on opportunity_id; every row carries user_id from birth.
-- Submission and outcome dates are plain calendar dates; only the event
-- history records timestamps. Stage transitions are validated in code
-- before any SQL runs.

CREATE TABLE applications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    opportunity_id INTEGER NOT NULL REFERENCES opportunities(id),
    stage TEXT NOT NULL DEFAULT 'draft' CHECK (
        stage IN ('draft', 'preparing', 'submitted', 'withdrawn')
    ),
    -- Date-only calendar submission date or NULL while the attempt is not
    -- submitted; strftime rejects impossible dates such as 2027-02-30.
    submitted_at TEXT CHECK (
        submitted_at IS NULL OR submitted_at = strftime('%Y-%m-%d', submitted_at)
    ),
    submission_channel TEXT NOT NULL DEFAULT 'unknown' CHECK (
        submission_channel IN ('email', 'employer_portal', 'employment_agency', 'other', 'unknown')
    ),
    -- Tracked independently of the stage: submitted is not an outcome, and
    -- an interview does not undo the submission.
    employer_outcome TEXT NOT NULL DEFAULT 'unknown' CHECK (
        employer_outcome IN ('unknown', 'pending', 'interview', 'offer', 'rejected', 'withdrawn')
    ),
    outcome_at TEXT CHECK (
        outcome_at IS NULL OR outcome_at = strftime('%Y-%m-%d', outcome_at)
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_applications_user_id ON applications (user_id);
CREATE INDEX idx_applications_opportunity_id ON applications (opportunity_id);

-- Append-only event history for the key moments in an application's life:
-- submissions, corrections, withdrawals, outcome changes, agency report
-- marks and corrections, and later archive and metadata changes. old_value
-- and new_value are sparse human-readable fields; the event type identifies
-- the change and the note carries the surrounding facts. The history starts
-- when a change is recorded; older records get no backfilled events.
CREATE TABLE application_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    event_type TEXT NOT NULL,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('application', 'opportunity', 'obligation')),
    entity_id INTEGER NOT NULL,
    old_value TEXT,
    new_value TEXT,
    note TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_application_events_user_id ON application_events (user_id);
CREATE INDEX idx_application_events_entity ON application_events (entity_type, entity_id);

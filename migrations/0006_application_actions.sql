-- Sixth migration: user-curated follow-up actions per application. Each
-- row is convenience the user invents for themselves: it carries no
-- evidence, writes no history entries, and is never created or completed
-- automatically by any other workflow. Every row carries user_id from
-- birth.
CREATE TABLE application_actions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    application_id INTEGER NOT NULL REFERENCES applications(id),
    title TEXT NOT NULL,
    -- Optional calendar date the action should happen by; a due date is
    -- not a completion date. strftime rejects impossible dates such as
    -- 2027-02-30.
    due_at TEXT CHECK (due_at IS NULL OR due_at = strftime('%Y-%m-%d', due_at)),
    -- The moment the action was marked done; NULL while it is open.
    completed_at TEXT,
    notes TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_application_actions_user_id ON application_actions (user_id);
CREATE INDEX idx_application_actions_application_id ON application_actions (application_id);

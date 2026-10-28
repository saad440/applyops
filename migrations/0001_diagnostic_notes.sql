-- First migration: diagnostic table used by the initial persistence smoke
-- test. `user_id` is present from the first migration so introducing real
-- user management later does not require rewriting stored rows.
CREATE TABLE diagnostic_notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    note TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

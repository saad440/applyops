-- Third migration: user-defined streams. The stream label on an opportunity
-- is free text; this table remembers the known vocabulary so forms can offer
-- it back. The shipped defaults are seeded at startup only while a user has
-- no streams yet.
CREATE TABLE streams (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (user_id, name)
);

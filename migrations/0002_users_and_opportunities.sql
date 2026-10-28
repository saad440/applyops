-- Second migration: the minimal users table with the single local owner
-- seeded, and the opportunities table for real leads. Every domain table
-- carries user_id from birth so a later multi-user decision stays additive.
-- Priority and stage are constrained here and validated in code before any
-- insert runs; the stream label is free user-defined text.

CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- The single local owner of the local-first, single-user phase.
INSERT INTO users (id) VALUES (1);

CREATE TABLE opportunities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    organization TEXT NOT NULL,
    title TEXT NOT NULL,
    -- User-defined primary stream label; shipped defaults live in the
    -- application, not in a database constraint.
    stream TEXT NOT NULL,
    city TEXT,
    country TEXT,
    priority TEXT NOT NULL CHECK (priority IN ('p1', 'p2', 'p3')),
    -- Date-only calendar deadline (YYYY-MM-DD) or NULL when genuinely
    -- unknown; strftime rejects impossible dates such as 2027-02-30.
    deadline_date TEXT CHECK (
        deadline_date IS NULL OR deadline_date = strftime('%Y-%m-%d', deadline_date)
    ),
    source_url TEXT,
    notes TEXT,
    stage TEXT NOT NULL DEFAULT 'discovered' CHECK (
        stage IN ('discovered', 'researching', 'preparing', 'archived')
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_opportunities_user_id ON opportunities (user_id);

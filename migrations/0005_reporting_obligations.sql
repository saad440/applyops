-- Fifth migration: reporting obligations held toward an authority for each
-- submitted application, plus the per-user vocabulary of known authority
-- names. One workflow per application per kind; the reporting state is
-- assigned explicitly and never inferred from a submission. Every row
-- carries user_id from birth.
CREATE TABLE reporting_obligations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    application_id INTEGER NOT NULL REFERENCES applications(id),
    kind TEXT NOT NULL DEFAULT 'employment_agency' CHECK (
        kind IN ('employment_agency', 'jobcenter', 'foreign_authority', 'other')
    ),
    -- Independent of the employer outcome: a report is assigned explicitly,
    -- and unknown (nothing reliably recorded) is not the same as pending
    -- (known to be outstanding).
    status TEXT NOT NULL DEFAULT 'unknown' CHECK (
        status IN ('pending', 'reported', 'not_required', 'unknown')
    ),
    -- Date-only calendar date the report was filed. Code validation
    -- requires it whenever the status is reported; strftime rejects
    -- impossible dates such as 2027-02-30.
    reported_at TEXT CHECK (
        reported_at IS NULL OR reported_at = strftime('%Y-%m-%d', reported_at)
    ),
    method TEXT CHECK (
        method IS NULL OR method IN ('portal', 'manual', 'other')
    ),
    reference TEXT,
    notes TEXT,
    -- Human-readable authority name. Advisory free text with no foreign
    -- key; code validation requires it for kind = 'other'.
    recipient_name TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (application_id, kind)
);

CREATE INDEX idx_reporting_obligations_user_id ON reporting_obligations (user_id);
CREATE INDEX idx_reporting_obligations_status ON reporting_obligations (status);

-- Advisory selector vocabulary behind the report form's authority field.
-- Labels are remembered per user and offered back as suggestions; no
-- obligation row references them, so removing one affects future
-- suggestions only. Name comparisons are ASCII case-insensitive.
CREATE TABLE known_authorities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    name TEXT NOT NULL COLLATE NOCASE,
    is_seed INTEGER,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (user_id, name)
);

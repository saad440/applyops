-- Seventh migration: richer opportunity metadata for prioritisation and
-- provenance: the self-assessed fit range, the estimated preparation
-- effort, a short rationale, the kind and confidence of the recorded
-- deadline, a fallback recruitment page, and notes on how the source was
-- verified. Everything is optional; an unrated opportunity stores NULLs
-- and never a placeholder number. Enum-like columns are constrained here
-- and validated in code before any SQL runs. The fit bounds are enforced
-- per column; the ordering fit_min <= fit_max spans two columns, which
-- ALTER TABLE cannot add to an existing table, so it lives in the code
-- validation. A deadline kind or confidence is likewise checked in code
-- to always accompany a deadline date, since it describes that date.

ALTER TABLE opportunities ADD COLUMN fit_min REAL
    CHECK (fit_min IS NULL OR (fit_min >= 1.0 AND fit_min <= 5.0));

ALTER TABLE opportunities ADD COLUMN fit_max REAL
    CHECK (fit_max IS NULL OR (fit_max >= 1.0 AND fit_max <= 5.0));

ALTER TABLE opportunities ADD COLUMN effort TEXT
    CHECK (effort IS NULL OR effort IN ('low', 'medium', 'high'));

ALTER TABLE opportunities ADD COLUMN rationale TEXT;

ALTER TABLE opportunities ADD COLUMN deadline_kind TEXT
    CHECK (deadline_kind IS NULL OR deadline_kind IN
        ('hard_close', 'priority_review', 'first_review', 'soft_target'));

ALTER TABLE opportunities ADD COLUMN deadline_confidence TEXT
    CHECK (deadline_confidence IS NULL OR deadline_confidence IN
        ('verified', 'unverified', 'conflicting'));

ALTER TABLE opportunities ADD COLUMN recruitment_page_url TEXT;

ALTER TABLE opportunities ADD COLUMN source_notes TEXT;

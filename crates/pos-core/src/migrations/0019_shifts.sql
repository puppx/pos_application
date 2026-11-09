-- Shifts and cash-up: the float at the start, money put in or taken out of
-- the drawer, and the count at the end against what should be there.
CREATE TABLE shifts (
    id             INTEGER PRIMARY KEY,
    opened_by      INTEGER NOT NULL REFERENCES users (id),
    opened_at      INTEGER NOT NULL DEFAULT (unixepoch()),
    float_cents    INTEGER NOT NULL CHECK (float_cents >= 0),
    closed_by      INTEGER REFERENCES users (id),
    closed_at      INTEGER,
    counted_cents  INTEGER,
    expected_cents INTEGER,
    note           TEXT
);
-- At most one open shift (one drawer).
CREATE UNIQUE INDEX shifts_one_open ON shifts ((closed_at IS NULL)) WHERE closed_at IS NULL;

CREATE TABLE cash_moves (
    id           INTEGER PRIMARY KEY,
    shift_id     INTEGER NOT NULL REFERENCES shifts (id),
    kind         TEXT NOT NULL CHECK (kind IN ('in', 'out')),
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
    reason       TEXT,
    user_id      INTEGER NOT NULL REFERENCES users (id),
    at           INTEGER NOT NULL DEFAULT (unixepoch())
);

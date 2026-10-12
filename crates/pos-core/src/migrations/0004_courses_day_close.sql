-- v0.6.0: starters (course holding), deletable option groups, end-of-day close.

-- Starters go to the kitchen first; the rest of a dine-in order waits for
-- "Call mains".
ALTER TABLE products ADD COLUMN starter INTEGER NOT NULL DEFAULT 0;
-- Snapshot of products.starter when the line was added.
ALTER TABLE order_items ADD COLUMN starter INTEGER NOT NULL DEFAULT 0;
-- The line was held back behind the starters. While it is also unsent it is
-- waiting for "Call mains"; once sent it marks the "MAINS AWAY" round.
ALTER TABLE order_items ADD COLUMN held INTEGER NOT NULL DEFAULT 0;

-- Deleted groups stay in the table because past order lines reference their
-- options, but they are hidden everywhere.
ALTER TABLE option_groups ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;

-- One row per closed business day ("Z report"). The summary is stored as
-- JSON so a reprint shows exactly what was printed at close.
CREATE TABLE day_closes (
    id            INTEGER PRIMARY KEY,
    business_date TEXT NOT NULL UNIQUE,
    from_ts       INTEGER NOT NULL,
    to_ts         INTEGER NOT NULL,
    closed_by     INTEGER NOT NULL REFERENCES users (id),
    summary       TEXT NOT NULL
);

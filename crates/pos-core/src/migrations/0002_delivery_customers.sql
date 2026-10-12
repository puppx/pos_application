-- v0.2.0: delivery platforms (Uber Eats, Just Eat, Deliveroo...) and saved
-- customers. Additive only: existing v0.1.0 databases upgrade in place.

CREATE TABLE delivery_channels (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL COLLATE NOCASE,
    color      TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    active     INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX delivery_channels_name ON delivery_channels (name) WHERE active = 1;
INSERT INTO delivery_channels (name, color, sort_order) VALUES
    ('Uber Eats', '#06c167', 1),
    ('Just Eat',  '#ff8000', 2),
    ('Deliveroo', '#00ccbc', 3);

CREATE TABLE customers (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    phone         TEXT,
    -- Digits only, UK numbers as 0xxxxxxxxxx, for search and duplicates.
    phone_digits  TEXT,
    address_line1 TEXT,
    address_line2 TEXT,
    town          TEXT,
    -- Normalised, e.g. "SW1A 2AA".
    postcode      TEXT,
    notes         TEXT,
    active        INTEGER NOT NULL DEFAULT 1,
    order_count   INTEGER NOT NULL DEFAULT 0,
    last_order_at INTEGER,
    created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at    INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX customers_phone ON customers (phone_digits);
CREATE INDEX customers_postcode ON customers (postcode);
CREATE INDEX customers_name ON customers (name COLLATE NOCASE);

ALTER TABLE orders ADD COLUMN channel_id INTEGER REFERENCES delivery_channels (id);
-- The platform's own order number, e.g. Uber Eats "#A1B2C".
ALTER TABLE orders ADD COLUMN external_ref TEXT;
ALTER TABLE orders ADD COLUMN customer_id INTEGER REFERENCES customers (id);
CREATE INDEX orders_customer ON orders (customer_id, opened_at) WHERE customer_id IS NOT NULL;

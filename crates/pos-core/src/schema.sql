-- Schema v1. Money is stored as integer cents, tax rates as basis points
-- (1500 = 15.00%), timestamps as unix seconds (UTC).

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
) WITHOUT ROWID;

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin', 'staff')),
    -- Argon2id(pin, install salt). Deterministic so login is one hash plus
    -- one indexed lookup; UNIQUE because the PIN alone identifies the user.
    pin_hash      BLOB NOT NULL UNIQUE,
    active        INTEGER NOT NULL DEFAULT 1,
    created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    last_login_at INTEGER
);

CREATE TABLE products (
    id          INTEGER PRIMARY KEY,
    sku         TEXT UNIQUE,
    barcode     TEXT UNIQUE,
    name        TEXT NOT NULL COLLATE NOCASE,
    price_cents INTEGER NOT NULL CHECK (price_cents >= 0),
    cost_cents  INTEGER NOT NULL DEFAULT 0 CHECK (cost_cents >= 0),
    tax_rate_bp INTEGER NOT NULL DEFAULT 0 CHECK (tax_rate_bp BETWEEN 0 AND 10000),
    stock_qty   INTEGER NOT NULL DEFAULT 0,
    track_stock INTEGER NOT NULL DEFAULT 1,
    active      INTEGER NOT NULL DEFAULT 1,
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);
-- NOCASE index lets `name LIKE 'abc%'` use the index for short queries.
CREATE INDEX products_name ON products (name);

-- Substring search for 3+ character queries without full table scans.
CREATE VIRTUAL TABLE products_fts USING fts5 (
    name, sku, barcode,
    content = 'products', content_rowid = 'id', tokenize = 'trigram'
);
CREATE TRIGGER products_ai AFTER INSERT ON products BEGIN
    INSERT INTO products_fts (rowid, name, sku, barcode)
    VALUES (new.id, new.name, new.sku, new.barcode);
END;
CREATE TRIGGER products_ad AFTER DELETE ON products BEGIN
    INSERT INTO products_fts (products_fts, rowid, name, sku, barcode)
    VALUES ('delete', old.id, old.name, old.sku, old.barcode);
END;
CREATE TRIGGER products_au AFTER UPDATE OF name, sku, barcode ON products BEGIN
    INSERT INTO products_fts (products_fts, rowid, name, sku, barcode)
    VALUES ('delete', old.id, old.name, old.sku, old.barcode);
    INSERT INTO products_fts (rowid, name, sku, barcode)
    VALUES (new.id, new.name, new.sku, new.barcode);
END;

CREATE TABLE sales (
    id             INTEGER PRIMARY KEY,
    user_id        INTEGER NOT NULL REFERENCES users (id),
    status         TEXT NOT NULL DEFAULT 'completed' CHECK (status IN ('completed', 'voided')),
    subtotal_cents INTEGER NOT NULL,
    discount_cents INTEGER NOT NULL,
    tax_cents      INTEGER NOT NULL,
    total_cents    INTEGER NOT NULL,
    payment_method TEXT NOT NULL CHECK (payment_method IN ('cash', 'card', 'other')),
    tendered_cents INTEGER NOT NULL,
    change_cents   INTEGER NOT NULL,
    created_at     INTEGER NOT NULL DEFAULT (unixepoch()),
    voided_by      INTEGER REFERENCES users (id),
    voided_at      INTEGER,
    void_reason    TEXT
);
CREATE INDEX sales_created_at ON sales (created_at);
CREATE INDEX sales_user ON sales (user_id, created_at);

CREATE TABLE sale_items (
    id               INTEGER PRIMARY KEY,
    sale_id          INTEGER NOT NULL REFERENCES sales (id),
    product_id       INTEGER REFERENCES products (id),
    -- Snapshot so receipts stay correct after product edits.
    name             TEXT NOT NULL,
    unit_price_cents INTEGER NOT NULL,
    qty              INTEGER NOT NULL CHECK (qty > 0),
    discount_cents   INTEGER NOT NULL DEFAULT 0,
    tax_rate_bp      INTEGER NOT NULL,
    tax_cents        INTEGER NOT NULL,
    line_total_cents INTEGER NOT NULL
);
CREATE INDEX sale_items_sale ON sale_items (sale_id);
CREATE INDEX sale_items_product ON sale_items (product_id);

-- Inventory ledger: every stock change, from sales, voids or manual adjustment.
CREATE TABLE stock_movements (
    id         INTEGER PRIMARY KEY,
    product_id INTEGER NOT NULL REFERENCES products (id),
    delta      INTEGER NOT NULL,
    reason     TEXT NOT NULL,
    sale_id    INTEGER REFERENCES sales (id),
    user_id    INTEGER REFERENCES users (id),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX stock_movements_product ON stock_movements (product_id, created_at);

CREATE TABLE audit_log (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER REFERENCES users (id),
    action     TEXT NOT NULL,
    detail     TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX audit_log_created_at ON audit_log (created_at);

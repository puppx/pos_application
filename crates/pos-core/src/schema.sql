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

-- ---------------------------------------------------------------- menu

CREATE TABLE categories (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL COLLATE NOCASE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    color      TEXT,
    active     INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX categories_name ON categories (name) WHERE active = 1;

CREATE TABLE products (
    id          INTEGER PRIMARY KEY,
    category_id INTEGER REFERENCES categories (id),
    sku         TEXT UNIQUE,
    barcode     TEXT UNIQUE,
    name        TEXT NOT NULL COLLATE NOCASE,
    price_cents INTEGER NOT NULL CHECK (price_cents >= 0),
    cost_cents  INTEGER NOT NULL DEFAULT 0 CHECK (cost_cents >= 0),
    tax_rate_bp INTEGER NOT NULL DEFAULT 0 CHECK (tax_rate_bp BETWEEN 0 AND 10000),
    stock_qty   INTEGER NOT NULL DEFAULT 0,
    track_stock INTEGER NOT NULL DEFAULT 0,
    active      INTEGER NOT NULL DEFAULT 1,
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);
-- NOCASE index lets `name LIKE 'abc%'` use the index for short queries.
CREATE INDEX products_name ON products (name);
CREATE INDEX products_category ON products (category_id, name);

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

-- Option groups are reusable sets of choices attached to menu items, e.g.
-- "Burger size" (choose exactly 1) or "Extras" (choose up to 5). An option's
-- price is added to the item price (it may be negative, e.g. "Small").
CREATE TABLE option_groups (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL COLLATE NOCASE,
    min_select INTEGER NOT NULL DEFAULT 0,
    max_select INTEGER NOT NULL DEFAULT 1,
    sort_order INTEGER NOT NULL DEFAULT 0,
    active     INTEGER NOT NULL DEFAULT 1,
    CHECK (min_select >= 0 AND max_select >= 1 AND min_select <= max_select)
);
CREATE UNIQUE INDEX option_groups_name ON option_groups (name) WHERE active = 1;

CREATE TABLE options (
    id          INTEGER PRIMARY KEY,
    group_id    INTEGER NOT NULL REFERENCES option_groups (id),
    name        TEXT NOT NULL,
    price_cents INTEGER NOT NULL DEFAULT 0,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    active      INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX options_group ON options (group_id, sort_order);

CREATE TABLE product_option_groups (
    product_id INTEGER NOT NULL REFERENCES products (id),
    group_id   INTEGER NOT NULL REFERENCES option_groups (id),
    sort_order INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (product_id, group_id)
) WITHOUT ROWID;
CREATE INDEX product_option_groups_group ON product_option_groups (group_id);

-- ---------------------------------------------------------- floor plan

CREATE TABLE floors (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL COLLATE NOCASE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    active     INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX floors_name ON floors (name) WHERE active = 1;

-- Position and size are in plan units on a fixed 1000 x 700 canvas, so the
-- layout scales to any screen. Removed tables are deactivated, not deleted,
-- because past orders still reference them.
CREATE TABLE dining_tables (
    id       INTEGER PRIMARY KEY,
    floor_id INTEGER NOT NULL REFERENCES floors (id),
    name     TEXT NOT NULL COLLATE NOCASE,
    shape    TEXT NOT NULL DEFAULT 'rect' CHECK (shape IN ('rect', 'round')),
    x        INTEGER NOT NULL,
    y        INTEGER NOT NULL,
    w        INTEGER NOT NULL,
    h        INTEGER NOT NULL,
    active   INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX dining_tables_name ON dining_tables (floor_id, name) WHERE active = 1;

-- -------------------------------------------------------------- orders

-- An order is opened, filled and sent to the kitchen over time, then paid
-- (or cancelled). Paid orders are the sales record; a paid order can later
-- be voided by an admin.
CREATE TABLE orders (
    id               INTEGER PRIMARY KEY,
    order_type       TEXT NOT NULL CHECK (order_type IN ('dine_in', 'takeaway', 'collection', 'delivery')),
    status           TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'paid', 'cancelled', 'voided')),
    table_id         INTEGER REFERENCES dining_tables (id),
    customer_name    TEXT,
    customer_phone   TEXT,
    delivery_address TEXT,
    notes            TEXT,
    opened_by        INTEGER NOT NULL REFERENCES users (id),
    opened_at        INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Filled in at payment.
    subtotal_cents   INTEGER,
    discount_cents   INTEGER,
    tax_cents        INTEGER,
    total_cents      INTEGER,
    payment_method   TEXT CHECK (payment_method IN ('cash', 'card', 'other')),
    tendered_cents   INTEGER,
    change_cents     INTEGER,
    paid_by          INTEGER REFERENCES users (id),
    paid_at          INTEGER,
    -- Filled in on cancel (open order) or void (paid order).
    closed_by        INTEGER REFERENCES users (id),
    closed_at        INTEGER,
    close_reason     TEXT,
    CHECK ((order_type = 'dine_in') = (table_id IS NOT NULL))
);
-- At most one open order per table.
CREATE UNIQUE INDEX orders_open_table ON orders (table_id) WHERE status = 'open';
CREATE INDEX orders_open ON orders (opened_at) WHERE status = 'open';
CREATE INDEX orders_opened_at ON orders (opened_at);
CREATE INDEX orders_paid_at ON orders (paid_at) WHERE paid_at IS NOT NULL;
CREATE INDEX orders_opened_by ON orders (opened_by, opened_at);

CREATE TABLE order_items (
    id               INTEGER PRIMARY KEY,
    order_id         INTEGER NOT NULL REFERENCES orders (id),
    product_id       INTEGER NOT NULL REFERENCES products (id),
    -- Snapshot at the time the item was added, so later menu edits do not
    -- change open bills or past receipts. Includes the chosen options.
    name             TEXT NOT NULL,
    unit_price_cents INTEGER NOT NULL CHECK (unit_price_cents >= 0),
    tax_rate_bp      INTEGER NOT NULL,
    qty              INTEGER NOT NULL CHECK (qty > 0),
    discount_cents   INTEGER NOT NULL DEFAULT 0,
    note             TEXT,
    sent_at          INTEGER,
    added_by         INTEGER NOT NULL REFERENCES users (id),
    added_at         INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX order_items_order ON order_items (order_id);
CREATE INDEX order_items_product ON order_items (product_id);

-- Options chosen on an order line, snapshotted like the line itself. The
-- line's unit_price_cents already includes their prices.
CREATE TABLE order_item_options (
    id          INTEGER PRIMARY KEY,
    item_id     INTEGER NOT NULL REFERENCES order_items (id) ON DELETE CASCADE,
    option_id   INTEGER NOT NULL REFERENCES options (id),
    group_name  TEXT NOT NULL,
    name        TEXT NOT NULL,
    price_cents INTEGER NOT NULL
);
CREATE INDEX order_item_options_item ON order_item_options (item_id);

-- Inventory ledger: every stock change, from orders, voids or manual adjustment.
CREATE TABLE stock_movements (
    id         INTEGER PRIMARY KEY,
    product_id INTEGER NOT NULL REFERENCES products (id),
    delta      INTEGER NOT NULL,
    reason     TEXT NOT NULL,
    order_id   INTEGER REFERENCES orders (id),
    user_id    INTEGER REFERENCES users (id),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX stock_movements_product ON stock_movements (product_id, created_at);
CREATE INDEX stock_movements_order ON stock_movements (order_id) WHERE order_id IS NOT NULL;

CREATE TABLE audit_log (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER REFERENCES users (id),
    action     TEXT NOT NULL,
    detail     TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX audit_log_created_at ON audit_log (created_at);

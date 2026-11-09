-- Payments: an order can be paid in parts (split bill, cash + card) and
-- with a tip. One row per payment; the order closes once they cover it.
CREATE TABLE payments (
    id             INTEGER PRIMARY KEY,
    order_id       INTEGER NOT NULL REFERENCES orders (id),
    method         TEXT NOT NULL CHECK (method IN ('cash', 'card', 'other')),
    -- Towards the bill (never more than what was left to pay).
    amount_cents   INTEGER NOT NULL CHECK (amount_cents >= 0),
    -- On top of the bill, kept out of sales and tax.
    tip_cents      INTEGER NOT NULL DEFAULT 0 CHECK (tip_cents >= 0),
    -- Handed over: amount + tip + change.
    tendered_cents INTEGER NOT NULL,
    change_cents   INTEGER NOT NULL DEFAULT 0,
    -- "Pay for these dishes": JSON [[order_item_id, qty], ...]; NULL otherwise.
    items          TEXT,
    paid_by        INTEGER NOT NULL REFERENCES users (id),
    paid_at        INTEGER NOT NULL DEFAULT (unixepoch()),
    shift_id       INTEGER
);
CREATE INDEX payments_order ON payments (order_id);
CREATE INDEX payments_paid_at ON payments (paid_at);

-- Orders paid before this version: one payment each.
INSERT INTO payments (order_id, method, amount_cents, tendered_cents, change_cents, paid_by, paid_at)
SELECT id, payment_method, COALESCE(total_cents, 0), COALESCE(tendered_cents, total_cents, 0), COALESCE(change_cents, 0), paid_by, paid_at
FROM orders
WHERE status IN ('paid', 'voided') AND payment_method IS NOT NULL AND paid_by IS NOT NULL AND paid_at IS NOT NULL;

ALTER TABLE orders ADD COLUMN tip_cents INTEGER NOT NULL DEFAULT 0;

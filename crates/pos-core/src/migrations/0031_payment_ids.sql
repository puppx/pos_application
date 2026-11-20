-- Payment ids are never handed out again: a payment taken back left its id
-- free for the next payment, so "payment 231" in the audit log could mean
-- two payments, and an undo from a stale screen on another till could take
-- back a newer payment on the same order. AUTOINCREMENT needs the table
-- rebuilt; the rows, their ids, the indexes and the foreign keys stay as
-- they were (no other table refers to payments, and it has no triggers).
CREATE TABLE payments_new (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
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
INSERT INTO payments_new (id, order_id, method, amount_cents, tip_cents, tendered_cents, change_cents, items, paid_by, paid_at, shift_id)
SELECT id, order_id, method, amount_cents, tip_cents, tendered_cents, change_cents, items, paid_by, paid_at, shift_id FROM payments;
DROP TABLE payments;
ALTER TABLE payments_new RENAME TO payments;
CREATE INDEX payments_order ON payments (order_id);
CREATE INDEX payments_paid_at ON payments (paid_at);

-- Not the ids of payments already taken back either: the highest of the
-- payments left and of those the audit log names
-- ("order 12 payment 231 cash ...").
DELETE FROM sqlite_sequence WHERE name = 'payments';
INSERT INTO sqlite_sequence (name, seq)
SELECT 'payments', MAX(
    COALESCE((SELECT MAX(id) FROM payments), 0),
    COALESCE((SELECT MAX(CAST(substr(detail, instr(detail, ' payment ') + 9) AS INTEGER)) FROM audit_log
              WHERE action = 'order.payment_removed' AND instr(detail, ' payment ') > 0), 0));

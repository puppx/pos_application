-- v0.7.0: service charge. A percentage of the food (after discounts, before
-- tax) set per order: taken from Settings when the order is opened (for the
-- chosen order types) and changeable on the order. Not taxed.
ALTER TABLE orders ADD COLUMN service_bp INTEGER NOT NULL DEFAULT 0;
-- Filled in at payment.
ALTER TABLE orders ADD COLUMN service_cents INTEGER;

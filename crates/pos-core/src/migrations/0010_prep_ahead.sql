-- v0.7.9: "prepare ahead" dishes. On a dine-in order whose mains wait for
-- "Call mains", these are printed on a PREPARE NOW ticket straight away so
-- the kitchen can start them, then served with the mains as usual.
ALTER TABLE products ADD COLUMN prep INTEGER NOT NULL DEFAULT 0;
-- Category default for new items.
ALTER TABLE categories ADD COLUMN prep INTEGER NOT NULL DEFAULT 0;
-- Snapshot on the order line, and when its PREPARE NOW ticket went out.
ALTER TABLE order_items ADD COLUMN prep INTEGER NOT NULL DEFAULT 0;
ALTER TABLE order_items ADD COLUMN prep_sent_at INTEGER;

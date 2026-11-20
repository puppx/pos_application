-- When an open order last changed: a dish added, changed, moved or removed,
-- a payment taken or removed, or the order's own details edited. NULL until
-- the first change (opened_at counts then). Kept up to date by TEMP triggers
-- (orders.rs ACTIVITY_TRIGGERS); forgotten empty orders are swept by it.
ALTER TABLE orders ADD COLUMN touched_at INTEGER;

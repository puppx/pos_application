-- Kitchen screen: when the kitchen marked a dish as done.
ALTER TABLE order_items ADD COLUMN kitchen_done_at INTEGER;

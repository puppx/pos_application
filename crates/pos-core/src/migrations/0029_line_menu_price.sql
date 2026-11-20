-- The price a line had before its price was changed on the order
-- (set_item_price); NULL = never changed. More of a dish made cheaper needs
-- the same manager's say-so as the cheaper price itself.
ALTER TABLE order_items ADD COLUMN menu_price_cents INTEGER;

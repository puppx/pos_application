-- v0.7.4: a second name in Thai for menu items, item choices, options and
-- categories, printed under the English name on kitchen tickets. NULL = none.
ALTER TABLE products ADD COLUMN name_alt TEXT;
ALTER TABLE product_variants ADD COLUMN name_alt TEXT;
ALTER TABLE options ADD COLUMN name_alt TEXT;
ALTER TABLE categories ADD COLUMN name_alt TEXT;
-- Snapshots on order lines, like the English name.
ALTER TABLE order_items ADD COLUMN name_alt TEXT;
ALTER TABLE order_item_options ADD COLUMN name_alt TEXT;

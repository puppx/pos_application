-- v0.9.6: set menus. A set is a product (its own tile, price and tax) made of
-- sections (Starter, Main, Side...), each listing the menu items to choose
-- from. On an order it is one priced line with a free line per chosen dish
-- under it; bills show only the set, the kitchen gets the dishes.
ALTER TABLE products ADD COLUMN is_set INTEGER NOT NULL DEFAULT 0;
-- Made for sets only: not shown on the order screen by itself.
ALTER TABLE products ADD COLUMN set_only INTEGER NOT NULL DEFAULT 0;
CREATE TABLE set_sections (
    id         INTEGER PRIMARY KEY,
    set_id     INTEGER NOT NULL REFERENCES products (id),
    name       TEXT NOT NULL,
    name_alt   TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    -- How many dishes the customer chooses from this section.
    pick       INTEGER NOT NULL DEFAULT 1,
    -- Sent with the starters on dine-in orders (the rest waits for the mains).
    starter    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX set_sections_set ON set_sections (set_id);
CREATE TABLE set_section_items (
    section_id INTEGER NOT NULL REFERENCES set_sections (id),
    product_id INTEGER NOT NULL REFERENCES products (id),
    sort_order INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (section_id, product_id)
);
-- Order lines: a set's dishes point at the set's line and keep their section.
ALTER TABLE order_items ADD COLUMN parent_id INTEGER REFERENCES order_items (id);
ALTER TABLE order_items ADD COLUMN section_name TEXT;
ALTER TABLE order_items ADD COLUMN section_alt TEXT;
ALTER TABLE order_items ADD COLUMN is_set INTEGER NOT NULL DEFAULT 0;
CREATE INDEX order_items_parent ON order_items (parent_id) WHERE parent_id IS NOT NULL;

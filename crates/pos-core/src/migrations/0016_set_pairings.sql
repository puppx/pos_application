-- v0.10.0: a set section can say what its dishes come with (rice, noodles...).
-- Empty = nothing to pair. When ordering, each chosen dish gets one of these,
-- printed under it on kitchen tickets ("+ with Jasmine rice").
CREATE TABLE set_section_with (
    section_id INTEGER NOT NULL REFERENCES set_sections (id),
    product_id INTEGER NOT NULL REFERENCES products (id),
    sort_order INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (section_id, product_id)
);
ALTER TABLE order_items ADD COLUMN with_name TEXT;
ALTER TABLE order_items ADD COLUMN with_alt TEXT;

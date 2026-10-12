-- v0.3.0: item choices ("variants"), e.g. a soup sold as Chicken 6.50,
-- Beef 7.50, Prawn 8.50. Each choice has its own full price; the item's
-- own price is not used while it has choices.

CREATE TABLE product_variants (
    id          INTEGER PRIMARY KEY,
    product_id  INTEGER NOT NULL REFERENCES products (id),
    name        TEXT NOT NULL,
    price_cents INTEGER NOT NULL CHECK (price_cents >= 0),
    sort_order  INTEGER NOT NULL DEFAULT 0,
    active      INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX product_variants_product ON product_variants (product_id, sort_order);

-- Which choice an order line was (its name is also in the line's name).
ALTER TABLE order_items ADD COLUMN variant_id INTEGER REFERENCES product_variants (id);

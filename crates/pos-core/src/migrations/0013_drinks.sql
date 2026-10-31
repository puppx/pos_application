-- v0.9.2: drinks. Like starters they go to the kitchen straight away (never
-- held for the mains), and they print on their own DRINKS ticket.
ALTER TABLE products ADD COLUMN drink INTEGER NOT NULL DEFAULT 0;
-- Default for items in a category; new items take it (trigger below).
ALTER TABLE categories ADD COLUMN drink INTEGER NOT NULL DEFAULT 0;
CREATE TRIGGER products_drink_default AFTER INSERT ON products WHEN NEW.category_id IS NOT NULL
BEGIN
    UPDATE products SET drink = (SELECT drink FROM categories WHERE id = NEW.category_id) WHERE id = NEW.id;
END;
-- Categories already called "Drinks" (or similar) start as drinks.
UPDATE categories SET drink = 1
 WHERE lower(name) LIKE 'drink%' OR lower(name) LIKE '%beverage%' OR name LIKE '%เครื่องดื่ม%';
UPDATE products SET drink = 1 WHERE category_id IN (SELECT id FROM categories WHERE drink = 1);

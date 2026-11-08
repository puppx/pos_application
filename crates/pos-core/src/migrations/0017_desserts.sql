-- v0.10.5: desserts, like drinks: printed on their own DESSERTS ticket
-- (switchable in Settings). Unlike drinks they wait for the mains as usual.
ALTER TABLE products ADD COLUMN dessert INTEGER NOT NULL DEFAULT 0;
ALTER TABLE categories ADD COLUMN dessert INTEGER NOT NULL DEFAULT 0;
CREATE TRIGGER products_dessert_default AFTER INSERT ON products WHEN NEW.category_id IS NOT NULL
BEGIN
    UPDATE products SET dessert = (SELECT dessert FROM categories WHERE id = NEW.category_id) WHERE id = NEW.id;
END;
-- Categories already called "Desserts" (or similar) start as desserts.
UPDATE categories SET dessert = 1
 WHERE lower(name) LIKE 'dessert%' OR lower(name) LIKE 'sweet%' OR name LIKE '%ของหวาน%';
UPDATE products SET dessert = 1 WHERE category_id IN (SELECT id FROM categories WHERE dessert = 1);

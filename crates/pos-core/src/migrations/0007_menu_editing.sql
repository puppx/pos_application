-- v0.7.3: faster menu editing.
-- Category defaults for new items: tax rate (NULL = the main default in
-- Settings) and "items here are starters".
ALTER TABLE categories ADD COLUMN tax_rate_bp INTEGER CHECK (tax_rate_bp BETWEEN 0 AND 10000);
ALTER TABLE categories ADD COLUMN starter INTEGER NOT NULL DEFAULT 0;
-- Item position within its category (drag to reorder); ties sort by name.
ALTER TABLE products ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;

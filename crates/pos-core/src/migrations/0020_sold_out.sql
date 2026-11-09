-- Sold out for today: the item cannot be added; cleared at End of day.
ALTER TABLE products ADD COLUMN sold_out INTEGER NOT NULL DEFAULT 0;

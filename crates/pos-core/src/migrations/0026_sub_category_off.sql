-- v0.12.1: a sub-category can switch off a default its main category has on.
-- Bits: 1 starter, 2 prepare ahead, 4 drink, 8 dessert. A bit only counts for
-- a sub-category whose own flag is off; 0 = take the main category's (as before).
ALTER TABLE categories ADD COLUMN flags_off INTEGER NOT NULL DEFAULT 0;

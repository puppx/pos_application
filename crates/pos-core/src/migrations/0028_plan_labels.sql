-- A shape on the floor plan that only shows where something is (a bar, the
-- kitchen door, a window): drawn on the plan, never takes an order, not counted.
ALTER TABLE dining_tables ADD COLUMN is_table INTEGER NOT NULL DEFAULT 1;

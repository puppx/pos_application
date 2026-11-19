-- v0.12.9: a dish on an order can be moved to another kitchen team ("Which
-- team makes this?"). NULL = the menu item's team, as before.
ALTER TABLE order_items ADD COLUMN team_id INTEGER REFERENCES teams (id);

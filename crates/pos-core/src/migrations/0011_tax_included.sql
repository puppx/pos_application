-- v0.8.1: prices that already include tax. Each order remembers which way it
-- was rung up: 0 = tax added on top (every order before this version),
-- 1 = menu prices include the tax. Changing the setting only affects new orders.
ALTER TABLE orders ADD COLUMN tax_incl INTEGER NOT NULL DEFAULT 0;

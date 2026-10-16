-- v0.7.2: menu items deleted for good. An item that past orders still name
-- cannot lose its row (the order lines point to it), so it is marked deleted
-- and hidden everywhere, even from "Show removed". Unused items are removed.
ALTER TABLE products ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;

-- When the bill was printed for the customer before payment: the receipt is
-- then not printed again automatically when the order is paid.
ALTER TABLE orders ADD COLUMN bill_printed_at INTEGER;

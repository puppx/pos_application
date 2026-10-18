-- v0.7.7: tax rate chosen on one order (e.g. 0% for a tax-free customer),
-- applied to all its lines. NULL = each item's own menu rate.
ALTER TABLE orders ADD COLUMN tax_bp INTEGER CHECK (tax_bp BETWEEN 0 AND 10000);

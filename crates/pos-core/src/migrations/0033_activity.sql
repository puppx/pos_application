-- Activity log: what each row is about, for fast filters and paging.
-- order_id: the order the row is about; approver_id: whose PIN approved it
-- (NULL = none or self); terminal: 'local' (main till) or 'web-<id>'.
-- Old rows are filled in from their detail text once (see activity.rs).
ALTER TABLE audit_log ADD COLUMN order_id INTEGER;
ALTER TABLE audit_log ADD COLUMN approver_id INTEGER;
ALTER TABLE audit_log ADD COLUMN terminal TEXT;
CREATE INDEX audit_log_order ON audit_log (order_id) WHERE order_id IS NOT NULL;
CREATE INDEX audit_log_user ON audit_log (user_id, created_at);
CREATE INDEX audit_log_action ON audit_log (action, created_at);

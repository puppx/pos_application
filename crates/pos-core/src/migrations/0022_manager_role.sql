-- A third role, "manager", between admin and staff. SQLite cannot change a
-- CHECK constraint in place, so the users table is rebuilt: copied aside,
-- recreated and filled again (references to it are checked at commit).
PRAGMA defer_foreign_keys = ON;
CREATE TEMP TABLE users_copy AS SELECT id, name, role, pin_hash, active, created_at, last_login_at, pin_len FROM users;
DROP TABLE users;
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin', 'manager', 'staff')),
    pin_hash      BLOB NOT NULL UNIQUE,
    active        INTEGER NOT NULL DEFAULT 1,
    created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    last_login_at INTEGER,
    pin_len       INTEGER
);
INSERT INTO users (id, name, role, pin_hash, active, created_at, last_login_at, pin_len)
SELECT id, name, role, pin_hash, active, created_at, last_login_at, pin_len FROM users_copy;
DROP TABLE users_copy;

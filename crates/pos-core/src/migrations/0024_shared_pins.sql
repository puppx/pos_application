-- PINs no longer have to be unique: staff pick their name, then type their
-- PIN. The users table is rebuilt without the UNIQUE rule on pin_hash
-- (references to it are checked at commit, once it is filled again).
PRAGMA defer_foreign_keys = ON;
CREATE TEMP TABLE users_copy AS SELECT id, name, role, pin_hash, active, created_at, last_login_at, pin_len FROM users;
DROP TABLE users;
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin', 'manager', 'staff')),
    pin_hash      BLOB NOT NULL,
    active        INTEGER NOT NULL DEFAULT 1,
    created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    last_login_at INTEGER,
    pin_len       INTEGER
);
CREATE INDEX users_pin ON users (pin_hash);
INSERT INTO users (id, name, role, pin_hash, active, created_at, last_login_at, pin_len)
SELECT id, name, role, pin_hash, active, created_at, last_login_at, pin_len FROM users_copy;
DROP TABLE users_copy;

-- Tickets ("Contact admin"): staff ask a manager or admin for help, with
-- the context sent along. Status changes are messages of kind 'status'.
CREATE TABLE tickets (
    id              INTEGER PRIMARY KEY,
    created_at      INTEGER NOT NULL DEFAULT (unixepoch()),
    created_by      INTEGER REFERENCES users (id) ON DELETE SET NULL,
    created_by_name TEXT NOT NULL,
    terminal        TEXT NOT NULL,
    device          TEXT NOT NULL DEFAULT '',
    category        TEXT NOT NULL CHECK (category IN ('printer', 'payment', 'order', 'menu', 'till', 'other')),
    urgency         TEXT NOT NULL DEFAULT 'normal' CHECK (urgency IN ('normal', 'urgent')),
    status          TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'in_progress', 'done')),
    order_id        INTEGER,
    table_name      TEXT,
    context         TEXT NOT NULL DEFAULT '{}',
    updated_at      INTEGER NOT NULL DEFAULT (unixepoch()),
    last_msg_id     INTEGER,
    last_msg_by     INTEGER,
    last_msg_at     INTEGER,
    closed_at       INTEGER,
    closed_by       INTEGER REFERENCES users (id) ON DELETE SET NULL
);
CREATE INDEX tickets_status ON tickets (status, urgency, last_msg_at);
CREATE INDEX tickets_creator ON tickets (created_by, last_msg_at);
CREATE INDEX tickets_closed ON tickets (closed_at) WHERE status = 'done';

CREATE TABLE ticket_messages (
    id         INTEGER PRIMARY KEY,
    ticket_id  INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    user_id    INTEGER REFERENCES users (id) ON DELETE SET NULL,
    user_name  TEXT NOT NULL,
    kind       TEXT NOT NULL DEFAULT 'message' CHECK (kind IN ('message', 'status')),
    body       TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX ticket_messages_ticket ON ticket_messages (ticket_id, id);

CREATE TABLE ticket_reads (
    ticket_id    INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    last_read_id INTEGER NOT NULL,
    PRIMARY KEY (ticket_id, user_id)
) WITHOUT ROWID;

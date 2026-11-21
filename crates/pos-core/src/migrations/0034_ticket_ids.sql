-- Ticket and ticket message ids are never handed out again: a ticket
-- deleted (by hand or by the cleanup) left its id free for the next one, so
-- "#28" in the Activity log could mean two tickets, and a till that had seen
-- message 41 would skip a new reply that got id 41 again. AUTOINCREMENT
-- needs the tables rebuilt; the rows, their ids, the indexes and the
-- foreign keys stay as they were (no triggers use these tables).
--
-- Foreign keys are on while migrations run, and dropping a table deletes its
-- rows first: dropping the old tickets while a table that cascades from
-- "tickets" exists would delete those rows too. So the new message and read
-- tables first refer to tickets_new; renaming tickets_new to tickets then
-- points them at "tickets", as SQLite renames references with the table.
CREATE TABLE tickets_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
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
INSERT INTO tickets_new (id, created_at, created_by, created_by_name, terminal, device, category, urgency, status, order_id, table_name,
                         context, updated_at, last_msg_id, last_msg_by, last_msg_at, closed_at, closed_by)
SELECT id, created_at, created_by, created_by_name, terminal, device, category, urgency, status, order_id, table_name,
       context, updated_at, last_msg_id, last_msg_by, last_msg_at, closed_at, closed_by FROM tickets;

CREATE TABLE ticket_messages_new (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id  INTEGER NOT NULL REFERENCES tickets_new (id) ON DELETE CASCADE,
    user_id    INTEGER REFERENCES users (id) ON DELETE SET NULL,
    user_name  TEXT NOT NULL,
    kind       TEXT NOT NULL DEFAULT 'message' CHECK (kind IN ('message', 'status')),
    body       TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
INSERT INTO ticket_messages_new (id, ticket_id, user_id, user_name, kind, body, created_at)
SELECT id, ticket_id, user_id, user_name, kind, body, created_at FROM ticket_messages;

CREATE TABLE ticket_reads_new (
    ticket_id    INTEGER NOT NULL REFERENCES tickets_new (id) ON DELETE CASCADE,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    last_read_id INTEGER NOT NULL,
    PRIMARY KEY (ticket_id, user_id)
) WITHOUT ROWID;
INSERT INTO ticket_reads_new (ticket_id, user_id, last_read_id) SELECT ticket_id, user_id, last_read_id FROM ticket_reads;

-- Children first: then nothing cascades from the old tickets.
DROP TABLE ticket_reads;
DROP TABLE ticket_messages;
DROP TABLE tickets;
ALTER TABLE tickets_new RENAME TO tickets;
ALTER TABLE ticket_messages_new RENAME TO ticket_messages;
ALTER TABLE ticket_reads_new RENAME TO ticket_reads;
CREATE INDEX tickets_status ON tickets (status, urgency, last_msg_at);
CREATE INDEX tickets_creator ON tickets (created_by, last_msg_at);
CREATE INDEX tickets_closed ON tickets (closed_at) WHERE status = 'done';
CREATE INDEX ticket_messages_ticket ON ticket_messages (ticket_id, id);

-- Not the ids of tickets already deleted either: the highest of the tickets
-- left and of those the log names ("#28 printer urgent"); for messages, the
-- highest any ticket or read marker still remembers.
DELETE FROM sqlite_sequence WHERE name IN ('tickets', 'ticket_messages');
INSERT INTO sqlite_sequence (name, seq)
SELECT 'tickets', MAX(
    COALESCE((SELECT MAX(id) FROM tickets), 0),
    COALESCE((SELECT MAX(CAST(substr(detail, 2) AS INTEGER)) FROM audit_log
              WHERE action LIKE 'ticket.%' AND detail LIKE '#%'), 0));
INSERT INTO sqlite_sequence (name, seq)
SELECT 'ticket_messages', MAX(
    COALESCE((SELECT MAX(id) FROM ticket_messages), 0),
    COALESCE((SELECT MAX(last_msg_id) FROM tickets), 0),
    COALESCE((SELECT MAX(last_read_id) FROM ticket_reads), 0));

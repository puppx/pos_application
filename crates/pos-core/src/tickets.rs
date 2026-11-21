//! Tickets ("Contact admin"): staff ask a manager or admin for help from
//! any screen; managers and admins (who have [`Permission::AnswerTickets`])
//! see, answer and close them. What the ticket is about (who, terminal,
//! order and table, app version, recent print failures) is filled in here,
//! never taken from the client, except the screen it was sent from and the
//! device's own name.
//!
//! A ticket the caller may not see answers "not found", never "forbidden",
//! so ids cannot be probed. Read markers are per user (every device).

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{Permission, Pos, PosError, Result, Session};

pub const TICKET_CATEGORIES: &[&str] = &["printer", "payment", "order", "menu", "till", "other"];
pub const MAX_TICKET_BODY: usize = 500;
/// Tickets one person may open per hour.
pub const TICKETS_PER_HOUR: i64 = 20;
/// Tickets not done yet one person may have at once.
pub const MAX_OPEN_TICKETS: i64 = 50;
/// The creator may reopen a done ticket this long after it was closed.
pub const REOPEN_SECS: i64 = 7 * 86_400;
/// Done tickets show in the Done tab (and staff's own list) this long.
pub const DONE_SHOWN_SECS: i64 = 30 * 86_400;
/// Print failures this recent go with a ticket.
const PRINT_FAILURES_SECS: i64 = 30 * 60;
const MAX_CONTEXT: usize = 4096;

/// What the client sends with a new ticket.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTicket {
    pub category: String,
    #[serde(default)]
    pub urgency: Option<String>,
    pub body: String,
    #[serde(default)]
    pub order_id: Option<i64>,
    /// `state.orderId ? 'order' : state.view`.
    #[serde(default)]
    pub screen: Option<String>,
    /// e.g. "Order #742" or "Tables".
    #[serde(default)]
    pub screen_label: Option<String>,
    /// This device's name ("Bar tablet", "iPad · Safari").
    #[serde(default)]
    pub device: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TicketMessage {
    pub id: i64,
    pub user_id: Option<i64>,
    pub user_name: String,
    /// "message", or "status" (body: "open", "in_progress" or "done").
    pub kind: String,
    pub body: String,
    pub created_at: i64,
    /// Written by the caller.
    pub mine: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ticket {
    pub id: i64,
    pub created_at: i64,
    pub created_by: Option<i64>,
    pub created_by_name: String,
    /// "local" (main till) or "web" (a browser).
    pub terminal: String,
    pub device: String,
    pub category: String,
    pub urgency: String,
    /// "open" (New), "in_progress" or "done".
    pub status: String,
    pub order_id: Option<i64>,
    pub table_name: Option<String>,
    /// The order's status now ("open", "paid"…); None once it was deleted.
    pub order_status: Option<String>,
    /// {screen, screen_label, version, terminal_label, print_failures: [{at, message}], print_failure_count}.
    pub context: serde_json::Value,
    pub updated_at: i64,
    pub last_msg_at: Option<i64>,
    pub last_msg_by: Option<i64>,
    pub last_msg_by_name: Option<String>,
    pub closed_at: Option<i64>,
    pub closed_by: Option<i64>,
    pub closed_by_name: Option<String>,
    /// The caller opened it.
    pub mine: bool,
    /// Not done and the last message is from someone other than its creator.
    pub waiting: bool,
    /// The caller may reopen it now (done; an answerer, or its creator within 7 days).
    pub can_reopen: bool,
    pub messages: Vec<TicketMessage>,
}

/// One row of a ticket list.
#[derive(Debug, Clone, Serialize)]
pub struct TicketRow {
    pub id: i64,
    pub category: String,
    pub urgency: String,
    pub status: String,
    pub created_by: Option<i64>,
    pub created_by_name: String,
    pub device: String,
    /// The last message (up to 120 characters).
    pub preview: String,
    pub last_msg_at: Option<i64>,
    pub last_msg_by_name: Option<String>,
    /// Last change of any kind (message or status).
    pub updated_at: i64,
    pub unread: bool,
    pub created_at: i64,
    pub closed_at: Option<i64>,
    pub order_id: Option<i64>,
    pub table_name: Option<String>,
    pub waiting: bool,
    pub mine: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UrgentTicket {
    pub id: i64,
    pub category: String,
    pub preview: String,
    pub by: String,
    pub device: String,
    pub created_at: i64,
    pub table_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatestReply {
    pub ticket_id: i64,
    pub by: String,
    pub preview: String,
    pub msg_id: i64,
    pub category: String,
}

/// `app_status.tickets`: what the 10 s poll needs for the badges.
#[derive(Debug, Clone, Serialize)]
pub struct TicketBadges {
    /// Tickets with a message from someone else newer than my read marker
    /// (answerers: every ticket not done, plus my own; staff: my own).
    pub unread: i64,
    /// Unread among my own tickets (the ? Help badge for anyone).
    pub unread_mine: i64,
    /// Answerers: the Open tab's count; staff: 0.
    pub open: i64,
    /// Answerers: the Waiting tab's count; staff: 0.
    pub waiting: i64,
    /// Answerers: urgent tickets not done and unread by me (newest 3).
    pub urgent: Vec<UrgentTicket>,
    /// Answerers: how many such urgent tickets there are in all (0 for staff).
    pub urgent_count: i64,
    /// The newest unread message from someone else on my own tickets.
    pub latest: Option<LatestReply>,
    /// The highest message id the caller can see (changes = something new).
    pub seq: i64,
    /// The caller answers tickets (managers and admins).
    pub answerer: bool,
}

fn preview(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 120 {
        one.chars().take(119).collect::<String>() + "…"
    } else {
        one
    }
}

fn clean_text(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>().trim().to_string()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// "Visible to `?2` (user id), who answers tickets when `?3`."
const VISIBLE: &str = "(?3 OR k.created_by = ?2)";

const ROW_SQL: &str = "SELECT k.id, k.category, k.urgency, k.status, k.created_by, k.created_by_name, k.device,
        COALESCE((SELECT body FROM ticket_messages WHERE id = k.last_msg_id), ''), k.last_msg_at, lu.name, k.updated_at,
        (k.last_msg_by IS NOT ?2 AND k.last_msg_id > COALESCE(r.last_read_id, 0)), k.created_at, k.closed_at, k.order_id, k.table_name,
        (k.status <> 'done' AND k.last_msg_by IS NOT k.created_by), k.created_by IS ?2
     FROM tickets k
     LEFT JOIN ticket_reads r ON r.ticket_id = k.id AND r.user_id = ?2
     LEFT JOIN users lu ON lu.id = k.last_msg_by";

fn row_from(r: &rusqlite::Row) -> rusqlite::Result<TicketRow> {
    let body: String = r.get(7)?;
    let last_name: Option<String> = r.get(9)?;
    Ok(TicketRow {
        id: r.get(0)?,
        category: r.get(1)?,
        urgency: r.get(2)?,
        status: r.get(3)?,
        created_by: r.get(4)?,
        created_by_name: r.get(5)?,
        device: r.get(6)?,
        preview: preview(&body),
        last_msg_at: r.get(8)?,
        last_msg_by_name: last_name,
        updated_at: r.get(10)?,
        unread: r.get(11)?,
        created_at: r.get(12)?,
        closed_at: r.get(13)?,
        order_id: r.get(14)?,
        table_name: r.get(15)?,
        waiting: r.get(16)?,
        mine: r.get(17)?,
    })
}

impl Pos {
    fn answers(s: &Session) -> bool {
        s.role.can(Permission::AnswerTickets)
    }

    /// The ticket's creator and status, if `s` may see it; else not found.
    fn visible_ticket(&self, s: &Session, id: i64) -> Result<(Option<i64>, String, Option<i64>)> {
        self.conn
            .prepare_cached(&format!("SELECT k.created_by, k.status, k.closed_at FROM tickets k WHERE k.id = ?1 AND {VISIBLE}"))?
            .query_row(params![id, s.user_id, Self::answers(s)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?
            .ok_or(PosError::NotFound)
    }

    /// Open a ticket (anyone who sells). The server adds who, the terminal,
    /// the order's table, the app version and recent print failures.
    pub fn create_ticket(&mut self, s: &Session, t: &NewTicket) -> Result<Ticket> {
        s.require(Permission::Sell)?;
        if !TICKET_CATEGORIES.contains(&t.category.as_str()) {
            return invalid("Pick what it's about");
        }
        let urgency = t.urgency.as_deref().unwrap_or("normal");
        if !matches!(urgency, "normal" | "urgent") {
            return invalid("urgency: normal or urgent");
        }
        let body = clean_text(&t.body, MAX_TICKET_BODY + 1);
        if body.is_empty() {
            return invalid("Add a few words");
        }
        if body.chars().count() > MAX_TICKET_BODY {
            return invalid(format!("Up to {MAX_TICKET_BODY} characters"));
        }
        let (last_hour, not_done): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*) FILTER (WHERE created_at > unixepoch() - 3600), COUNT(*) FILTER (WHERE status <> 'done') FROM tickets WHERE created_by = ?1",
            [s.user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if last_hour >= TICKETS_PER_HOUR || not_done >= MAX_OPEN_TICKETS {
            return invalid("Too many tickets, try again later");
        }
        // The order and its table from the database, never from the client.
        let order: Option<(i64, Option<String>, String)> = match t.order_id {
            Some(id) => self
                .conn
                .query_row(
                    "SELECT o.id, t.name, o.status FROM orders o LEFT JOIN dining_tables t ON t.id = o.table_id WHERE o.id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?,
            None => None,
        };
        let local = self.terminal.as_deref() == Some("local");
        let since = now() - PRINT_FAILURES_SECS;
        let failures: Vec<_> = crate::printing::print_failures(Some(0)).failures.into_iter().filter(|f| f.at >= since).collect();
        let count = failures.len();
        let mut list: Vec<serde_json::Value> =
            failures.iter().rev().take(10).map(|f| serde_json::json!({ "at": f.at, "message": f.message.chars().take(200).collect::<String>() })).collect();
        let short = |v: &Option<String>, n: usize| v.as_deref().map(|x| clean_text(x, n)).unwrap_or_default();
        let device = short(&t.device, 60);
        let device = if device.is_empty() { (if local { "Main till" } else { "Browser" }).to_string() } else { device };
        let context = loop {
            let c = serde_json::json!({
                "screen": short(&t.screen, 40),
                "screen_label": short(&t.screen_label, 80),
                "version": env!("CARGO_PKG_VERSION"),
                "terminal_label": if local { "Main till" } else { "Browser" },
                "print_failures": list,
                "print_failure_count": count,
            })
            .to_string();
            if c.len() <= MAX_CONTEXT || list.is_empty() {
                break c;
            }
            list.pop();
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO tickets (created_by, created_by_name, terminal, device, category, urgency, order_id, table_name, context)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                s.user_id,
                s.name,
                if local { "local" } else { "web" },
                device,
                t.category,
                urgency,
                order.as_ref().map(|o| o.0),
                order.as_ref().and_then(|o| o.1.clone()),
                context
            ],
        )?;
        let id = tx.last_insert_rowid();
        add_message(&tx, id, s, "message", &body)?;
        let detail = format!("#{id} {} {urgency}", t.category);
        crate::activity::insert_full(&tx, Some(s.user_id), "ticket.create", &detail, self.terminal.as_deref(), order.as_ref().map(|o| o.0))?;
        tx.commit()?;
        self.get_ticket(s, id)
    }

    /// A ticket list. `tab`: "mine" (anyone: my own, done ones for 30
    /// days), or for answerers "open", "waiting", "done" (staff asking for
    /// those get "mine").
    pub fn list_tickets(&self, s: &Session, tab: &str, limit: i64, offset: i64) -> Result<Vec<TicketRow>> {
        s.require(Permission::Sell)?;
        let tab = if Self::answers(s) { tab } else { "mine" };
        let (cond, order) = match tab {
            "mine" => (
                "k.created_by = ?2 AND (k.status <> 'done' OR k.closed_at > unixepoch() - ?4)",
                "k.status = 'done', k.updated_at DESC, k.id DESC",
            ),
            "open" => ("k.status <> 'done' AND k.last_msg_by IS k.created_by", "k.urgency = 'urgent' DESC, k.updated_at ASC, k.id"),
            "waiting" => ("k.status <> 'done' AND k.last_msg_by IS NOT k.created_by", "k.urgency = 'urgent' DESC, k.updated_at ASC, k.id"),
            "done" => ("k.status = 'done' AND k.closed_at > unixepoch() - ?4", "k.closed_at DESC, k.id DESC"),
            _ => return invalid("tab: open, waiting, done or mine"),
        };
        let sql = format!("{ROW_SQL} WHERE {cond} AND {VISIBLE} ORDER BY {order} LIMIT ?5 OFFSET ?6");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![0, s.user_id, Self::answers(s), DONE_SHOWN_SECS, limit.clamp(1, 500), offset.max(0)], row_from)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One ticket with its messages; marks it read for the caller.
    pub fn get_ticket(&self, s: &Session, id: i64) -> Result<Ticket> {
        s.require(Permission::Sell)?;
        self.visible_ticket(s, id)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, user_id, user_name, kind, body, created_at FROM ticket_messages WHERE ticket_id = ?1 ORDER BY id",
        )?;
        let messages = stmt
            .query_map([id], |r| {
                let user_id: Option<i64> = r.get(1)?;
                Ok(TicketMessage {
                    id: r.get(0)?,
                    user_id,
                    user_name: r.get(2)?,
                    kind: r.get(3)?,
                    body: r.get(4)?,
                    created_at: r.get(5)?,
                    mine: user_id == Some(s.user_id),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let answerer = Self::answers(s);
        let mut t = self.conn.query_row(
            "SELECT k.id, k.created_at, k.created_by, k.created_by_name, k.terminal, k.device, k.category, k.urgency, k.status,
                    k.order_id, k.table_name, o.status, k.context, k.updated_at, k.last_msg_at, k.last_msg_by, lu.name,
                    k.closed_at, k.closed_by, cu.name
             FROM tickets k LEFT JOIN orders o ON o.id = k.order_id
             LEFT JOIN users lu ON lu.id = k.last_msg_by LEFT JOIN users cu ON cu.id = k.closed_by
             WHERE k.id = ?1",
            [id],
            |r| {
                let created_by: Option<i64> = r.get(2)?;
                let status: String = r.get(8)?;
                let last_by: Option<i64> = r.get(15)?;
                let closed_at: Option<i64> = r.get(17)?;
                let context: String = r.get(12)?;
                let mine = created_by == Some(s.user_id);
                Ok(Ticket {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    created_by,
                    created_by_name: r.get(3)?,
                    terminal: r.get(4)?,
                    device: r.get(5)?,
                    category: r.get(6)?,
                    urgency: r.get(7)?,
                    waiting: status != "done" && last_by != created_by,
                    can_reopen: status == "done" && (answerer || (mine && closed_at.is_some_and(|c| now() - c <= REOPEN_SECS))),
                    status,
                    order_id: r.get(9)?,
                    table_name: r.get(10)?,
                    order_status: r.get(11)?,
                    context: serde_json::from_str(&context).unwrap_or(serde_json::Value::Null),
                    updated_at: r.get(13)?,
                    last_msg_at: r.get(14)?,
                    last_msg_by: last_by,
                    last_msg_by_name: r.get(16)?,
                    closed_at,
                    closed_by: r.get(18)?,
                    closed_by_name: r.get(19)?,
                    mine,
                    messages: Vec::new(),
                })
            },
        )?;
        if let Some(last) = messages.last() {
            self.conn
                .prepare_cached(
                    "INSERT INTO ticket_reads (ticket_id, user_id, last_read_id) VALUES (?1, ?2, ?3)
                     ON CONFLICT (ticket_id, user_id) DO UPDATE SET last_read_id = MAX(last_read_id, excluded.last_read_id)",
                )?
                .execute(params![id, s.user_id, last.id])?;
        }
        t.messages = messages;
        Ok(t)
    }

    /// May `s` set ticket `id` to `status`? Answerers always; its creator
    /// may close it, and reopen it within 7 days of closing.
    fn may_set_status(s: &Session, creator: Option<i64>, cur: &str, closed_at: Option<i64>, status: &str) -> Result<()> {
        if Self::answers(s) {
            return Ok(());
        }
        if creator != Some(s.user_id) {
            return Err(PosError::NotFound);
        }
        match status {
            "done" => Ok(()),
            "open" if cur == "done" => {
                if closed_at.is_some_and(|c| now() - c <= REOPEN_SECS) {
                    Ok(())
                } else {
                    invalid("This ticket was closed over 7 days ago: open a new one")
                }
            }
            "open" => Ok(()),
            _ => Err(PosError::Forbidden),
        }
    }

    fn change_status(&self, tx: &rusqlite::Transaction, s: &Session, id: i64, cur: &str, status: &str) -> Result<()> {
        if cur == status {
            return Ok(());
        }
        if status == "done" {
            tx.execute("UPDATE tickets SET status = 'done', closed_at = unixepoch(), closed_by = ?2, updated_at = unixepoch() WHERE id = ?1", params![id, s.user_id])?;
        } else {
            tx.execute("UPDATE tickets SET status = ?2, closed_at = NULL, closed_by = NULL, updated_at = unixepoch() WHERE id = ?1", params![id, status])?;
        }
        add_message(tx, id, s, "status", status)?;
        let order: Option<i64> = tx.query_row("SELECT order_id FROM tickets WHERE id = ?1", [id], |r| r.get(0))?;
        crate::activity::insert_full(tx, Some(s.user_id), "ticket.status", &format!("#{id} {cur} -> {status}"), self.terminal.as_deref(), order)
    }

    /// Reply to a ticket (its creator, or an answerer). An answerer's reply
    /// to a New ticket makes it In progress. `set_status` changes the
    /// status too, by the rules of [`Pos::set_ticket_status`].
    pub fn reply_ticket(&mut self, s: &Session, id: i64, body: &str, set_status: Option<&str>) -> Result<Ticket> {
        s.require(Permission::Sell)?;
        let (creator, cur, closed_at) = self.visible_ticket(s, id)?;
        let body = clean_text(body, MAX_TICKET_BODY + 1);
        if body.is_empty() {
            return invalid("Add a few words");
        }
        if body.chars().count() > MAX_TICKET_BODY {
            return invalid(format!("Up to {MAX_TICKET_BODY} characters"));
        }
        let mut status = set_status.map(str::to_string);
        if let Some(st) = &status {
            if !matches!(st.as_str(), "open" | "in_progress" | "done") {
                return invalid("status: open, in_progress or done");
            }
            Self::may_set_status(s, creator, &cur, closed_at, st)?;
        } else if cur == "open" && Self::answers(s) && creator != Some(s.user_id) {
            status = Some("in_progress".into());
        }
        let tx = self.conn.unchecked_transaction()?;
        let msg = add_message(&tx, id, s, "message", &body)?;
        tx.execute("INSERT INTO ticket_reads (ticket_id, user_id, last_read_id) VALUES (?1, ?2, ?3) ON CONFLICT (ticket_id, user_id) DO UPDATE SET last_read_id = MAX(last_read_id, excluded.last_read_id)", params![id, s.user_id, msg])?;
        if let Some(st) = &status {
            self.change_status(&tx, s, id, &cur, st)?;
        }
        tx.commit()?;
        self.get_ticket(s, id)
    }

    /// Set a ticket New ("open"), In progress or Done (answerers; its
    /// creator may close it, or reopen it within 7 days).
    pub fn set_ticket_status(&mut self, s: &Session, id: i64, status: &str) -> Result<Ticket> {
        s.require(Permission::Sell)?;
        let (creator, cur, closed_at) = self.visible_ticket(s, id)?;
        if !matches!(status, "open" | "in_progress" | "done") {
            return invalid("status: open, in_progress or done");
        }
        Self::may_set_status(s, creator, &cur, closed_at, status)?;
        let tx = self.conn.unchecked_transaction()?;
        self.change_status(&tx, s, id, &cur, status)?;
        tx.commit()?;
        self.get_ticket(s, id)
    }

    /// Delete a ticket for good, with its messages and read markers (admins).
    pub fn delete_ticket(&mut self, s: &Session, id: i64) -> Result<()> {
        s.require(Permission::Delete)?;
        let (category, by, order, msgs): (String, String, Option<i64>, i64) = self
            .conn
            .query_row(
                "SELECT category, created_by_name, order_id, (SELECT COUNT(*) FROM ticket_messages WHERE ticket_id = tickets.id AND kind = 'message')
                 FROM tickets WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
            .ok_or(PosError::NotFound)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM tickets WHERE id = ?1", [id])?;
        let detail = format!("#{id} {category} by {by}, {msgs} message{}", if msgs == 1 { "" } else { "s" });
        crate::activity::insert_full(&tx, Some(s.user_id), "ticket.delete", &detail, self.terminal.as_deref(), order)?;
        tx.commit()?;
        Ok(())
    }

    /// The badge counts for `app_status.tickets`.
    pub fn ticket_badges(&self, s: &Session) -> Result<TicketBadges> {
        let answerer = Self::answers(s);
        let unread_cond = "k.last_msg_by IS NOT ?1 AND k.last_msg_id > COALESCE((SELECT last_read_id FROM ticket_reads WHERE ticket_id = k.id AND user_id = ?1), 0)";
        let (unread, unread_mine, open, waiting): (i64, i64, i64, i64) = self.conn.prepare_cached(&format!(
            "SELECT COUNT(*) FILTER (WHERE {unread_cond} AND ((?2 AND k.status <> 'done') OR k.created_by = ?1)),
                    COUNT(*) FILTER (WHERE {unread_cond} AND k.created_by = ?1),
                    COUNT(*) FILTER (WHERE ?2 AND k.status <> 'done' AND k.last_msg_by IS k.created_by),
                    COUNT(*) FILTER (WHERE ?2 AND k.status <> 'done' AND k.last_msg_by IS NOT k.created_by)
             FROM tickets k WHERE ?2 OR k.created_by = ?1"
        ))?
        .query_row(params![s.user_id, answerer], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        let urgent_count: i64 = if answerer {
            self.conn
                .prepare_cached(&format!("SELECT COUNT(*) FROM tickets k WHERE k.status <> 'done' AND k.urgency = 'urgent' AND {unread_cond}"))?
                .query_row(params![s.user_id], |r| r.get(0))?
        } else {
            0
        };
        let urgent = if answerer {
            let mut stmt = self.conn.prepare_cached(&format!(
                "SELECT k.id, k.category, COALESCE((SELECT body FROM ticket_messages WHERE id = k.last_msg_id), ''), k.created_by_name, k.device, k.created_at, k.table_name
                 FROM tickets k WHERE k.status <> 'done' AND k.urgency = 'urgent' AND {unread_cond} ORDER BY k.id DESC LIMIT 3"
            ))?;
            let rows = stmt.query_map(params![s.user_id], |r| {
                Ok(UrgentTicket {
                    id: r.get(0)?,
                    category: r.get(1)?,
                    preview: preview(&r.get::<_, String>(2)?),
                    by: r.get(3)?,
                    device: r.get(4)?,
                    created_at: r.get(5)?,
                    table_name: r.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        } else {
            Vec::new()
        };
        let latest = self
            .conn
            .prepare_cached(
                "SELECT m.ticket_id, m.user_name, m.body, m.id, k.category FROM ticket_messages m JOIN tickets k ON k.id = m.ticket_id
                 WHERE k.created_by = ?1 AND m.kind = 'message' AND m.user_id IS NOT ?1
                   AND m.id > COALESCE((SELECT last_read_id FROM ticket_reads WHERE ticket_id = k.id AND user_id = ?1), 0)
                 ORDER BY m.id DESC LIMIT 1",
            )?
            .query_row([s.user_id], |r| {
                Ok(LatestReply { ticket_id: r.get(0)?, by: r.get(1)?, preview: preview(&r.get::<_, String>(2)?), msg_id: r.get(3)?, category: r.get(4)? })
            })
            .optional()?;
        let seq: i64 = self
            .conn
            .prepare_cached("SELECT COALESCE(MAX(m.id), 0) FROM ticket_messages m JOIN tickets k ON k.id = m.ticket_id WHERE ?2 OR k.created_by = ?1")?
            .query_row(params![s.user_id, answerer], |r| r.get(0))?;
        Ok(TicketBadges { unread, unread_mine, open, waiting, urgent, urgent_count, latest, seq, answerer })
    }

    /// Ids of the users logged in somewhere who answer tickets (active managers and admins).
    pub fn answerers_among(&self, user_ids: &[i64]) -> Result<usize> {
        let mut n = 0;
        let mut stmt = self.conn.prepare_cached("SELECT EXISTS (SELECT 1 FROM users WHERE id = ?1 AND active = 1 AND role IN ('manager', 'admin'))")?;
        let mut seen = std::collections::HashSet::new();
        for id in user_ids {
            if seen.insert(*id) && stmt.query_row([id], |r| r.get::<_, bool>(0))? {
                n += 1;
            }
        }
        Ok(n)
    }
}

/// Append a message (or status line) and keep `tickets.last_msg_*` up to date.
fn add_message(tx: &rusqlite::Transaction, id: i64, s: &Session, kind: &str, body: &str) -> Result<i64> {
    tx.execute("INSERT INTO ticket_messages (ticket_id, user_id, user_name, kind, body) VALUES (?1, ?2, ?3, ?4, ?5)", params![id, s.user_id, s.name, kind, body])?;
    let msg = tx.last_insert_rowid();
    if kind == "message" {
        tx.execute(
            "UPDATE tickets SET last_msg_id = ?2, last_msg_by = ?3, last_msg_at = unixepoch(), updated_at = unixepoch() WHERE id = ?1",
            params![id, msg, s.user_id],
        )?;
    }
    Ok(msg)
}

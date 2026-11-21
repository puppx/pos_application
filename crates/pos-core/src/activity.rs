//! The Activity log (Manage → Activity log): the audit rows with what they
//! are about (`order_id`, `approver_id`, `terminal`, filled in on every new
//! row and once for old rows), listed with filters and keyset paging, and
//! managed by admins: delete chosen rows, clear older ones, export as CSV,
//! and the automatic cleanup ("Keep for", setting `log_keep_days`).
//!
//! Rows that record a deletion (`log.cleared`, `log.auto_cleared`) can never
//! be deleted by any command: every DELETE here excludes them.

use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{Permission, Pos, PosError, Result, Session};

/// Actions whose `user_id` is the approver (the PIN typed), with the person
/// at the till named at the end of the detail after this word.
const APPROVER_IS_USER: &[(&str, &str)] = &[("order.price", " by "), ("order.discount_approved", " for "), ("order.price_qty_approved", " for ")];

/// Rows that record a deletion of log rows: never deleted.
pub const CLEARED_ACTIONS: &str = "('log.cleared', 'log.auto_cleared')";

/// `log_keep_days`: 30, 90, 180, 365, or 0 for Forever. Not set = 180.
pub const LOG_KEEP_KEY: &str = "log_keep_days";
pub const LOG_KEEP_DEFAULT: i64 = 180;
pub const LOG_KEEP_CHOICES: &[i64] = &[30, 90, 180, 365, 0];
const CLEANUP_AT_KEY: &str = "_log_cleanup_at";
const CLEANUP_COUNT_KEY: &str = "_log_cleanup_count";
/// The automatic cleanup runs at most this often.
pub const CLEANUP_EVERY_SECS: i64 = 20 * 60 * 60;
/// Rows deleted per batch (each batch is its own short transaction).
pub const CLEANUP_BATCH: usize = 2000;

/// A cancel that only closed an empty order (by hand, by the sweep or at day close).
const EMPTY_CANCEL: &str = "(a.action = 'order.cancel' AND (a.user_id IS NULL OR a.detail LIKE '% empty' OR a.detail LIKE '%empty order(s)%'))";

/// "Needs a look" (Activity log filter), as one SQL condition on `a`.
pub fn look_sql() -> String {
    format!(
        "(a.action IN ('order.void', 'order.delete', 'order.item_void', 'order.payment_removed', 'login.failed', 'pin.failed', 'reset.bad_pin', 'log.bad_pin')
          OR (a.action = 'order.cancel' AND NOT {EMPTY_CANCEL})
          OR (a.action = 'drawer.open' AND a.detail = 'no sale'))"
    )
}

fn kind_sql(kind: &str) -> Option<String> {
    let void = format!("(a.action IN ('order.void', 'order.delete', 'order.item_void') OR (a.action = 'order.cancel' AND NOT {EMPTY_CANCEL}))");
    let money = "(a.action IN ('order.payment_removed', 'order.discount', 'order.discount_approved', 'order.price', 'order.price_qty_approved', 'order.tax',
        'order.service', 'order.payment_other', 'order.open_item', 'order.part_payment', 'drawer.open'))"
        .to_string();
    let login = "(a.action IN ('login', 'logout', 'login.failed', 'pin.failed', 'reset.bad_pin', 'log.bad_pin', 'user.reset_pin', 'setup.admin_created'))".to_string();
    let setup = "(a.action IN ('setting.update', 'service.settings', 'server.settings', 'teams.save', 'channels.save', 'user.create', 'user.update', 'device.autostart')
        OR a.action LIKE 'printer.%' OR a.action LIKE 'product.%' OR a.action LIKE 'category.%' OR a.action LIKE 'options.%'
        OR a.action LIKE 'menu.%' OR a.action LIKE 'customer.%' OR a.action LIKE 'floor.%')"
        .to_string();
    let shift = "(a.action IN ('shift.open', 'shift.cash', 'shift.close', 'day.close'))".to_string();
    Some(match kind {
        "void" => void,
        "money" => money,
        "login" => login,
        "setup" => setup,
        "shift" => shift,
        "other" => format!("NOT ({void} OR {money} OR {login} OR {setup} OR {shift})"),
        _ => return None,
    })
}

/// "local", or "web-" and the first 6 characters of a browser's terminal id.
pub(crate) fn terminal_label(t: &str) -> String {
    if t == "local" {
        "local".into()
    } else {
        let id = t.strip_prefix("web-").unwrap_or(t);
        format!("web-{}", id.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).collect::<String>())
    }
}

/// A device's own name as sent by the browser ("Bar tablet"): one line, at
/// most 40 characters, no '|' (it separates it from the terminal in the column).
pub fn clean_device_label(s: &str) -> Option<String> {
    let v: String = s.chars().filter(|c| !c.is_control() && *c != '|').take(40).collect();
    let v = v.trim().to_string();
    (!v.is_empty()).then_some(v)
}

fn leading_int(s: &str) -> Option<i64> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// What an audit row is about, read from its action and detail text (the
/// same for new rows and for old rows filled in once): (order id, approver id).
pub(crate) fn refs(action: &str, detail: &str, user_id: Option<i64>) -> (Option<i64>, Option<i64>) {
    let order = if detail.contains("empty order(s)") {
        None
    } else {
        match action {
            "order.delete" => detail.strip_prefix('#').and_then(leading_int),
            "drawer.open" => detail.find("order ").and_then(|i| leading_int(&detail[i + 6..])),
            a if a.starts_with("order.") || a == "kitchen.main_course" => match detail.strip_prefix("order ") {
                Some(rest) => leading_int(rest),
                None => leading_int(detail),
            },
            _ => None,
        }
    };
    let approver = if APPROVER_IS_USER.iter().any(|(a, _)| *a == action) {
        user_id
    } else {
        pin_of(detail)
    };
    (order, approver)
}

/// The user id in a trailing "(PIN of NAME #ID, ROLE)".
fn pin_of(detail: &str) -> Option<i64> {
    let rest = &detail[detail.rfind("(PIN of ")? + 8..];
    let rest = rest.strip_suffix(')')?;
    let (_, id_role) = rest.rsplit_once(" #")?;
    let (id, _) = id_role.split_once(", ")?;
    id.parse().ok()
}

/// Write one audit row with what it is about.
pub(crate) fn insert(conn: &Connection, user_id: Option<i64>, action: &str, detail: &str, terminal: Option<&str>) -> Result<()> {
    insert_full(conn, user_id, action, detail, terminal, None)
}

/// [`insert`] naming the order itself (e.g. a ticket about an order).
pub(crate) fn insert_full(conn: &Connection, user_id: Option<i64>, action: &str, detail: &str, terminal: Option<&str>, order: Option<i64>) -> Result<()> {
    let (found, approver_id) = refs(action, detail, user_id);
    let order_id = order.or(found);
    conn.prepare_cached("INSERT INTO audit_log (user_id, action, detail, order_id, approver_id, terminal) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
        .execute(params![user_id, action, detail, order_id, approver_id, terminal])?;
    Ok(())
}

/// Fill in `order_id` / `approver_id` of the rows written before they
/// existed (run once, by the migration that adds them), 2,000 rows a step.
pub(crate) fn backfill(conn: &Connection) -> Result<()> {
    let mut after = 0i64;
    loop {
        let rows: Vec<(i64, String, String, Option<i64>)> = conn
            .prepare(
                "SELECT id, action, COALESCE(detail, ''), user_id FROM audit_log
                 WHERE id > ?1 AND (action LIKE 'order.%' OR action IN ('drawer.open', 'kitchen.main_course') OR detail LIKE '%(PIN of %')
                 ORDER BY id LIMIT 2000",
            )?
            .query_map([after], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let Some(last) = rows.last() else { return Ok(()) };
        after = last.0;
        let mut upd = conn.prepare("UPDATE audit_log SET order_id = ?2, approver_id = ?3 WHERE id = ?1")?;
        for (id, action, detail, user) in &rows {
            let (o, a) = refs(action, detail, *user);
            if o.is_some() || a.is_some() {
                upd.execute(params![id, o, a])?;
            }
        }
    }
}

/// The order a row is about, as far as the UI needs it for `orderTitle()`.
#[derive(Debug, Clone, Serialize)]
pub struct ActivityOrder {
    pub order_type: String,
    pub status: String,
    pub table_name: Option<String>,
    pub floor_name: Option<String>,
    pub customer_name: Option<String>,
    pub channel_name: Option<String>,
    pub external_ref: Option<String>,
    /// The order's total (cents) as stored: final once paid or voided.
    pub total_cents: i64,
}

/// One Activity log row. `user_name` is the person who did it (for price
/// changes and approved discounts, the person at the till, read from the
/// detail); `approver_name` whoever else's PIN approved it (None = nobody else).
#[derive(Debug, Clone, Serialize)]
pub struct ActivityRow {
    pub id: i64,
    /// Unix seconds.
    pub at: i64,
    pub action: String,
    pub detail: String,
    pub user_id: Option<i64>,
    pub user_name: Option<String>,
    pub approver_id: Option<i64>,
    pub approver_name: Option<String>,
    pub order_id: Option<i64>,
    /// The order, while it still exists.
    pub order: Option<ActivityOrder>,
    /// "local" (main till), "web-xxxxxx" (a browser), or None (older rows, the app itself).
    pub terminal: Option<String>,
    /// The browser's own device name ("Bar tablet") when it sent one.
    pub device: Option<String>,
    /// Current name of what the row names by id (menu item, customer, staff
    /// member, category, floor, or the table an order moved to), if it still exists.
    pub ref_name: Option<String>,
    /// "Needs a look".
    pub look: bool,
    /// A `log.cleared` / `log.auto_cleared` row (cannot be deleted).
    pub cleared: bool,
    /// Business day (YYYY-MM-DD) the row belongs to, as in History.
    pub day: String,
    /// Type group: void, money, login, setup, shift or other.
    pub kind: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityCursor {
    pub at: i64,
    pub id: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityFilter {
    /// Unix seconds, from (inclusive) and to (exclusive). Left out = no bound.
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub user_id: Option<i64>,
    /// void, money, login, setup, shift or other.
    pub kind: Option<String>,
    pub look: Option<bool>,
    pub q: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityDay {
    pub date: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityPage {
    pub rows: Vec<ActivityRow>,
    /// Pass as `before` for the next page; None at the end.
    pub next: Option<ActivityCursor>,
    /// Rows per business day for the whole filter (first page only).
    pub days: Option<Vec<ActivityDay>>,
    /// "Needs a look" rows in the date range (other filters too, except `look`).
    pub look_count: i64,
    /// Rows matching the filter (all pages).
    pub total: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityStaff {
    pub id: i64,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityCount {
    pub count: i64,
    pub from: Option<i64>,
    pub to: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogRetention {
    /// 30, 90, 180, 365, or 0 = Forever.
    pub keep_days: i64,
    pub count: i64,
    /// About how much the log takes: SUM(length(detail)) + 48 per row.
    pub approx_bytes: i64,
    /// Unix seconds of the oldest row.
    pub oldest: Option<i64>,
    pub last_cleanup_at: Option<i64>,
    pub last_cleanup_count: Option<i64>,
}

const ROW_SELECT: &str = "SELECT a.id, a.created_at, a.action, COALESCE(a.detail, ''), a.user_id, u.name, a.approver_id, ap.name, a.order_id, a.terminal,
        o.order_type, o.status, t.name, f.name, o.customer_name, ch.name, o.external_ref, o.total_cents,
        CASE
          WHEN a.action LIKE 'product.%' THEN (SELECT name FROM products WHERE id = CAST(a.detail AS INTEGER))
          WHEN a.action LIKE 'customer.%' THEN (SELECT name FROM customers WHERE id = CAST(a.detail AS INTEGER))
          WHEN a.action IN ('user.reset_pin', 'user.update', 'user.create') THEN (SELECT name FROM users WHERE id = CAST(a.detail AS INTEGER))
          WHEN a.action LIKE 'category.%' THEN (SELECT name FROM categories WHERE id = CAST(a.detail AS INTEGER))
          WHEN a.action LIKE 'floor.%' THEN (SELECT name FROM floors WHERE id = CAST(a.detail AS INTEGER))
          WHEN a.action = 'order.move' THEN (SELECT name FROM dining_tables WHERE id = CAST(substr(a.detail, instr(a.detail, '-> table ') + 9) AS INTEGER))
        END";

const ROW_FROM: &str = " FROM audit_log a
        LEFT JOIN users u ON u.id = a.user_id
        LEFT JOIN users ap ON ap.id = a.approver_id AND a.approver_id IS NOT a.user_id
        LEFT JOIN orders o ON o.id = a.order_id
        LEFT JOIN dining_tables t ON t.id = o.table_id
        LEFT JOIN floors f ON f.id = t.floor_id
        LEFT JOIN delivery_channels ch ON ch.id = o.channel_id";

/// The business day of `a.created_at` (?1 = the open business date).
fn day_sql(cur: usize) -> String {
    format!(
        "COALESCE((SELECT d.business_date FROM day_closes d WHERE a.created_at >= d.from_ts AND a.created_at < d.to_ts LIMIT 1),
           CASE WHEN a.created_at >= (SELECT MAX(to_ts) FROM day_closes) THEN ?{cur} ELSE date(a.created_at, 'unixepoch', 'localtime') END)"
    )
}

fn kind_case() -> String {
    let mut s = String::from("CASE");
    for k in ["void", "money", "login", "setup", "shift"] {
        s += &format!(" WHEN {} THEN '{k}'", kind_sql(k).unwrap_or_default());
    }
    s + " ELSE 'other' END"
}

/// WHERE conditions (on `a`, `u`, `t`) and their values for a filter.
fn where_sql(f: &ActivityFilter, with_look: bool, args: &mut Vec<Value>) -> Result<String> {
    let mut w = vec!["1".to_string()];
    let arg = |v: Value, args: &mut Vec<Value>| {
        args.push(v);
        format!("?{}", args.len())
    };
    if let Some(from) = f.from {
        w.push(format!("a.created_at >= {}", arg(Value::Integer(from), args)));
    }
    if let Some(to) = f.to {
        w.push(format!("a.created_at < {}", arg(Value::Integer(to), args)));
    }
    let mut keep = Vec::new();
    if let Some(uid) = f.user_id {
        let p = arg(Value::Integer(uid), args);
        let approved: Vec<String> = APPROVER_IS_USER
            .iter()
            .map(|(a, word)| format!("(a.action = '{a}' AND a.detail LIKE '%{word}' || (SELECT name FROM users WHERE id = {p}))"))
            .collect();
        keep.push(format!("(a.user_id = {p} OR a.approver_id = {p} OR {})", approved.join(" OR ")));
    }
    if let Some(kind) = f.kind.as_deref().filter(|k| !k.is_empty() && *k != "all") {
        keep.push(kind_sql(kind).ok_or_else(|| PosError::Validation("unknown type".into()))?);
    }
    if with_look && f.look == Some(true) {
        keep.push(look_sql());
    }
    if let Some(q) = f.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let num = q.strip_prefix('#').unwrap_or(q).trim();
        if !num.is_empty() && num.len() <= 12 && num.bytes().all(|b| b.is_ascii_digit()) {
            keep.push(format!("a.order_id = {}", arg(Value::Integer(num.parse().unwrap_or(0)), args)));
        } else {
            let like = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            let p = arg(Value::Text(like), args);
            keep.push(format!("(u.name LIKE {p} ESCAPE '\\' OR t.name LIKE {p} ESCAPE '\\' OR a.detail LIKE {p} ESCAPE '\\')"));
        }
    }
    if !keep.is_empty() {
        // Clearing rows show whatever the filter.
        w.push(format!("(({}) OR a.action IN {CLEARED_ACTIONS})", keep.join(" AND ")));
    }
    Ok(w.join(" AND "))
}

fn row_from(r: &rusqlite::Row) -> rusqlite::Result<ActivityRow> {
    let action: String = r.get(2)?;
    let detail: String = r.get(3)?;
    let mut user_name: Option<String> = r.get(5)?;
    let mut approver_name: Option<String> = r.get(7)?;
    if let Some((_, word)) = APPROVER_IS_USER.iter().find(|(a, _)| *a == action) {
        // user_id is the approver; the person at the till is named last.
        if let Some((_, staff)) = detail.rsplit_once(word) {
            let approver = user_name.take();
            approver_name = approver.filter(|a| a != staff);
            user_name = Some(staff.to_string());
        }
    }
    let order_type: Option<String> = r.get(10)?;
    let order = match order_type {
        Some(order_type) => Some(ActivityOrder {
            order_type,
            status: r.get(11)?,
            table_name: r.get(12)?,
            floor_name: r.get(13)?,
            customer_name: r.get(14)?,
            channel_name: r.get(15)?,
            external_ref: r.get(16)?,
            total_cents: r.get::<_, Option<i64>>(17)?.unwrap_or(0),
        }),
        None => None,
    };
    let cleared = action == "log.cleared" || action == "log.auto_cleared";
    let stored: Option<String> = r.get(9)?;
    let (terminal, device) = match stored.as_deref().and_then(|t| t.split_once('|')) {
        Some((t, d)) => (Some(t.to_string()), Some(d.to_string())),
        None => (stored, None),
    };
    Ok(ActivityRow {
        id: r.get(0)?,
        at: r.get(1)?,
        detail,
        user_id: r.get(4)?,
        user_name,
        approver_id: r.get(6)?,
        approver_name,
        order_id: r.get(8)?,
        order,
        terminal,
        device,
        ref_name: r.get(18)?,
        look: r.get(19)?,
        day: r.get(20)?,
        kind: r.get(21)?,
        cleared,
        action,
    })
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

impl Pos {
    fn activity_rows(&self, f: &ActivityFilter, before: Option<ActivityCursor>, limit: usize, oldest_first: bool) -> Result<Vec<ActivityRow>> {
        let mut args = vec![Value::Text(self.business_date()?.format("%Y-%m-%d").to_string())];
        let mut w = where_sql(f, true, &mut args)?;
        if let Some(c) = before {
            args.push(Value::Integer(c.at));
            let at = args.len();
            args.push(Value::Integer(c.id));
            w += &format!(" AND (a.created_at < ?{at} OR (a.created_at = ?{at} AND a.id < ?{}))", at + 1);
        }
        let order = if oldest_first { "ASC" } else { "DESC" };
        let sql = format!(
            "{ROW_SELECT}, {}, {}, {}{ROW_FROM} WHERE {w} ORDER BY a.created_at {order}, a.id {order} LIMIT {limit}",
            look_sql(),
            day_sql(1),
            kind_case()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), row_from)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// One page of the Activity log, newest first (managers and admins).
    pub fn list_activity(&self, s: &Session, f: &ActivityFilter, before: Option<ActivityCursor>, limit: Option<usize>) -> Result<ActivityPage> {
        s.require(Permission::ViewActivity)?;
        let limit = limit.unwrap_or(200).clamp(1, 500);
        let mut rows = self.activity_rows(f, before, limit + 1, false)?;
        let next = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(|r| ActivityCursor { at: r.at, id: r.id })
        } else {
            None
        };
        let mut args = Vec::new();
        let w = where_sql(f, true, &mut args)?;
        let total: i64 = self.conn.query_row(&format!("SELECT COUNT(*){ROW_FROM} WHERE {w}"), params_from_iter(args), |r| r.get(0))?;
        let mut args = Vec::new();
        let w = where_sql(f, false, &mut args)?;
        let look_count: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*){ROW_FROM} WHERE {w} AND {}", look_sql()),
            params_from_iter(args),
            |r| r.get(0),
        )?;
        let days = if before.is_none() {
            let mut args = vec![Value::Text(self.business_date()?.format("%Y-%m-%d").to_string())];
            let w = where_sql(f, true, &mut args)?;
            let sql = format!("SELECT d, COUNT(*) FROM (SELECT {} AS d{ROW_FROM} WHERE {w}) GROUP BY d ORDER BY d DESC", day_sql(1));
            let mut stmt = self.conn.prepare(&sql)?;
            let days = stmt
                .query_map(params_from_iter(args), |r| Ok(ActivityDay { date: r.get(0)?, count: r.get(1)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Some(days)
        } else {
            None
        };
        Ok(ActivityPage { rows, next, days, look_count, total })
    }

    /// The rows about one order (and its tickets), oldest first, at most 100.
    pub fn order_activity(&self, s: &Session, order_id: i64) -> Result<Vec<ActivityRow>> {
        s.require(Permission::ViewActivity)?;
        let args = vec![Value::Text(self.business_date()?.format("%Y-%m-%d").to_string()), Value::Integer(order_id)];
        let sql = format!(
            "{ROW_SELECT}, {}, {}, {}{ROW_FROM} WHERE a.order_id = ?2 ORDER BY a.created_at, a.id LIMIT 100",
            look_sql(),
            day_sql(1),
            kind_case()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), row_from)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Everyone for the "Everyone" filter: active first, then turned off.
    pub fn activity_staff(&self, s: &Session) -> Result<Vec<ActivityStaff>> {
        s.require(Permission::ViewActivity)?;
        let mut stmt = self.conn.prepare("SELECT id, name, active FROM users ORDER BY active DESC, name COLLATE NOCASE, id")?;
        let rows = stmt.query_map([], |r| Ok(ActivityStaff { id: r.get(0)?, name: r.get(1)?, active: r.get(2)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The caller's own PIN again, for deleting log rows. A wrong one is
    /// logged as `log.bad_pin` and counts towards the lockout.
    fn check_log_pin(&mut self, s: &Session, pin: &str) -> Result<()> {
        self.throttle.check(Instant::now())?;
        let ok = match self.hash_pin(pin) {
            Ok(hash) => self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM users WHERE id = ?1 AND pin_hash = ?2 AND active = 1)",
                params![s.user_id, hash],
                |r| r.get(0),
            )?,
            Err(_) => false,
        };
        if ok {
            return Ok(());
        }
        self.throttle.record_failure(Instant::now());
        self.audit(Some(s.user_id), "log.bad_pin", "")?;
        Err(PosError::InvalidPin)
    }

    fn write_cleared(&self, s: Option<&Session>, action: &str, detail: &serde_json::Value) -> Result<()> {
        self.audit(s.map(|s| s.user_id), action, &detail.to_string())
    }

    /// Delete chosen rows (admins, with their PIN). Rows that record a
    /// clearing are never deleted. One `log.cleared` row records it.
    pub fn delete_activity(&mut self, s: &Session, ids: &[i64], pin: &str) -> Result<usize> {
        s.require(Permission::Delete)?;
        if ids.is_empty() || ids.len() > 500 {
            return invalid("choose 1 to 500 entries");
        }
        self.check_log_pin(s, pin)?;
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let cond = format!("id IN ({list}) AND action NOT IN {CLEARED_ACTIONS}");
        let tx = self.conn.unchecked_transaction()?;
        let (from, to): (Option<i64>, Option<i64>) =
            tx.query_row(&format!("SELECT MIN(created_at), MAX(created_at) FROM audit_log WHERE {cond}"), [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let n = tx.execute(&format!("DELETE FROM audit_log WHERE {cond}"), [])?;
        tx.commit()?;
        if n > 0 {
            self.write_cleared(Some(s), "log.cleared", &serde_json::json!({ "kind": "selected", "count": n, "from": from, "to": to }))?;
        }
        Ok(n)
    }

    /// What "Clear older entries" before `before` would delete.
    pub fn activity_count(&self, s: &Session, before: i64) -> Result<ActivityCount> {
        s.require(Permission::Delete)?;
        Ok(self.conn.query_row(
            &format!("SELECT COUNT(*), MIN(created_at), MAX(created_at) FROM audit_log WHERE created_at < ?1 AND action NOT IN {CLEARED_ACTIONS}"),
            [before],
            |r| Ok(ActivityCount { count: r.get(0)?, from: r.get(1)?, to: r.get(2)? }),
        )?)
    }

    /// Delete every row older than `before` (admins, with their PIN and
    /// DELETE typed), except clearing rows. `csv`: a CSV copy was saved
    /// first (recorded in the `log.cleared` row).
    pub fn clear_activity(&mut self, s: &Session, before: i64, pin: &str, confirm: &str, csv: bool) -> Result<usize> {
        s.require(Permission::Delete)?;
        if confirm.trim() != "DELETE" {
            return invalid("type DELETE to confirm");
        }
        self.check_log_pin(s, pin)?;
        let c = self.activity_count(s, before)?;
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute(&format!("DELETE FROM audit_log WHERE created_at < ?1 AND action NOT IN {CLEARED_ACTIONS}"), [before])?;
        tx.commit()?;
        if n > 0 {
            self.write_cleared(Some(s), "log.cleared", &serde_json::json!({ "kind": "older", "count": n, "from": c.from, "to": c.to, "before": before, "csv": csv }))?;
        }
        Ok(n)
    }

    /// The rows of a filter (or every row older than `before`) as CSV, UTF-8
    /// with a BOM, oldest first (admins). Columns: time, who, approver,
    /// action, text, order, detail. Returns (csv, rows).
    pub fn export_activity(&self, s: &Session, f: &ActivityFilter, before: Option<i64>) -> Result<(String, usize)> {
        s.require(Permission::Delete)?;
        let f = match before {
            Some(b) => ActivityFilter { to: Some(b), ..Default::default() },
            None => f.clone(),
        };
        let rows = self.activity_rows(&f, None, 1_000_000, true)?;
        let mut out = String::from("\u{feff}time,who,approver,action,text,order,detail\r\n");
        for r in &rows {
            let time = chrono::DateTime::from_timestamp(r.at, 0)
                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default();
            let text = match (&r.ref_name, r.kind.as_str()) {
                (Some(n), _) => format!("{} {n}", r.action),
                (None, k) => format!("{} ({k})", r.action),
            };
            let fields = [
                time,
                r.user_name.clone().unwrap_or_default(),
                r.approver_name.clone().unwrap_or_default(),
                r.action.clone(),
                text,
                r.order_id.map(|o| format!("#{o}")).unwrap_or_default(),
                r.detail.clone(),
            ];
            out += &fields.iter().map(|x| csv_field(x)).collect::<Vec<_>>().join(",");
            out += "\r\n";
        }
        Ok((out, rows.len()))
    }

    /// "Keep for" in days (0 = Forever).
    pub fn log_keep_days(&self) -> Result<i64> {
        Ok(self
            .setting(LOG_KEEP_KEY)?
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|d| LOG_KEEP_CHOICES.contains(d))
            .unwrap_or(LOG_KEEP_DEFAULT))
    }

    fn raw_setting_i64(&self, key: &str) -> Result<Option<i64>> {
        let v: Option<String> =
            self.conn.query_row("SELECT CAST(value AS TEXT) FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?;
        Ok(v.and_then(|v| v.trim().parse().ok()))
    }

    fn set_raw_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// The "Keep for" row in Backup & data (admins).
    pub fn log_retention(&self, s: &Session) -> Result<LogRetention> {
        s.require(Permission::ManageSettings)?;
        let (count, approx_bytes, oldest) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(detail)), 0) + 48 * COUNT(*), MIN(created_at) FROM audit_log",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(LogRetention {
            keep_days: self.log_keep_days()?,
            count,
            approx_bytes,
            oldest,
            last_cleanup_at: self.raw_setting_i64(CLEANUP_AT_KEY)?,
            last_cleanup_count: self.raw_setting_i64(CLEANUP_COUNT_KEY)?,
        })
    }

    /// The cutoff (unix seconds) when the automatic cleanup is due at `now`:
    /// not Forever, and the last run over 20 hours ago. Marks it as run, so
    /// another start or day close does not start a second one meanwhile.
    pub fn log_cleanup_due(&self, now: i64) -> Result<Option<(i64, i64)>> {
        let days = self.log_keep_days()?;
        if days == 0 {
            return Ok(None);
        }
        if self.raw_setting_i64(CLEANUP_AT_KEY)?.is_some_and(|t| now - t < CLEANUP_EVERY_SECS && t <= now) {
            return Ok(None);
        }
        self.set_raw_setting(CLEANUP_AT_KEY, &now.to_string())?;
        Ok(Some((now - days * 86_400, days)))
    }

    /// One batch of the automatic cleanup: up to [`CLEANUP_BATCH`] log rows
    /// older than `cutoff` (never clearing rows), then, once none are left,
    /// done tickets closed before it (their messages and read markers go
    /// with them). Returns (log rows, tickets) deleted; (0, 0) = finished.
    pub fn log_cleanup_batch(&mut self, cutoff: i64) -> Result<(usize, usize)> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let n = tx.execute(
            &format!(
                "DELETE FROM audit_log WHERE id IN (SELECT id FROM audit_log WHERE created_at < ?1 AND action NOT IN {CLEARED_ACTIONS} LIMIT {CLEANUP_BATCH})"
            ),
            [cutoff],
        )?;
        let t = if n == 0 {
            tx.execute(
                &format!(
                    "DELETE FROM tickets WHERE id IN (SELECT id FROM tickets WHERE status = 'done' AND closed_at < ?1 LIMIT {})",
                    CLEANUP_BATCH / 20
                ),
                [cutoff],
            )?
        } else {
            0
        };
        tx.commit()?;
        Ok((n, t))
    }

    /// End of an automatic cleanup: one `log.auto_cleared` row when anything went.
    pub fn log_cleanup_finish(&self, cutoff: i64, days: i64, count: usize, tickets: usize) -> Result<()> {
        self.set_raw_setting(CLEANUP_COUNT_KEY, &(count + tickets).to_string())?;
        if count + tickets > 0 {
            let detail = serde_json::json!({ "count": count, "cutoff": cutoff, "tickets": tickets, "days": days });
            self.write_cleared(None, "log.auto_cleared", &detail)?;
        }
        Ok(())
    }

    /// The whole cleanup in one go (tests and small logs). The app runs the
    /// batches itself, letting go of the lock between them.
    pub fn log_cleanup_now(&mut self, now: i64) -> Result<Option<(usize, usize)>> {
        let Some((cutoff, days)) = self.log_cleanup_due(now)? else { return Ok(None) };
        let (mut count, mut tickets) = (0, 0);
        loop {
            let (n, t) = self.log_cleanup_batch(cutoff)?;
            if n + t == 0 {
                break;
            }
            count += n;
            tickets += t;
        }
        self.log_cleanup_finish(cutoff, days, count, tickets)?;
        Ok(Some((count, tickets)))
    }

    /// Who closed (cancelled / voided) an order at the till and whose PIN
    /// approved it, from its log row, when someone else's PIN did.
    pub(crate) fn close_approver(&self, order_id: i64) -> Result<Option<(Option<String>, String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT u.name, ap.name, ap.role FROM audit_log a JOIN users ap ON ap.id = a.approver_id LEFT JOIN users u ON u.id = a.user_id
                 WHERE a.order_id = ?1 AND a.action IN ('order.cancel', 'order.void') AND a.approver_id IS NOT a.user_id
                 ORDER BY a.id DESC LIMIT 1",
                [order_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;

    fn setup() -> (Pos, Session, Session, Session) {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1234").unwrap();
        pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        let quinn = pos.login_as(None, "4646", &mut Default::default()).unwrap();
        let sam = pos.login_as(None, "2222", &mut Default::default()).unwrap();
        (pos, admin, quinn, sam)
    }

    fn row(pos: &Pos, user: Option<i64>, action: &str, detail: &str, at: i64) {
        insert(&pos.conn, user, action, detail, None).unwrap();
        pos.conn.execute("UPDATE audit_log SET created_at = ?1 WHERE id = last_insert_rowid()", [at]).unwrap();
    }

    fn count(pos: &Pos, action: &str) -> i64 {
        pos.conn.query_row("SELECT COUNT(*) FROM audit_log WHERE action = ?1", [action], |r| r.get(0)).unwrap()
    }

    #[test]
    fn managers_and_admins_see_the_log_staff_do_not() {
        let (pos, admin, quinn, sam) = setup();
        let f = ActivityFilter::default();
        assert!(matches!(pos.list_activity(&sam, &f, None, None), Err(PosError::Forbidden)));
        assert!(matches!(pos.order_activity(&sam, 1), Err(PosError::Forbidden)));
        assert!(matches!(pos.activity_staff(&sam), Err(PosError::Forbidden)));
        assert!(pos.list_activity(&quinn, &f, None, None).unwrap().total > 0);
        assert!(pos.list_activity(&admin, &f, None, None).unwrap().days.is_some());
        // Managing it is for admins only.
        let mut pos = pos;
        assert!(matches!(pos.delete_activity(&quinn, &[1], "4646"), Err(PosError::Forbidden)));
        assert!(matches!(pos.clear_activity(&quinn, i64::MAX, "4646", "DELETE", false), Err(PosError::Forbidden)));
        assert!(matches!(pos.activity_count(&quinn, 0), Err(PosError::Forbidden)));
        assert!(matches!(pos.export_activity(&quinn, &f, None), Err(PosError::Forbidden)));
        assert!(matches!(pos.log_retention(&quinn), Err(PosError::Forbidden)));
    }

    #[test]
    fn keyset_pages_cover_every_row_once_newest_first() {
        let (pos, admin, ..) = setup();
        for i in 0..25 {
            row(&pos, Some(admin.user_id), "setting.update", &format!("k{i}"), 1_000 + i / 3);
        }
        let f = ActivityFilter { kind: Some("setup".into()), ..Default::default() };
        let mut seen = Vec::new();
        let mut before = None;
        loop {
            let page = pos.list_activity(&admin, &f, before, Some(10)).unwrap();
            assert_eq!(page.total, 27, "25 settings + the two staff added in setup");
            seen.extend(page.rows.iter().map(|r| (r.at, r.id)));
            match page.next {
                Some(n) => before = Some(n),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(seen, sorted);
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len());
    }

    #[test]
    fn needs_a_look_is_exactly_the_listed_set_and_text_search_and_orders_work() {
        let (pos, admin, quinn, sam) = setup();
        row(&pos, Some(sam.user_id), "order.cancel", &format!("42 changed mind (PIN of Quinn #{}, manager)", quinn.user_id), 5_000);
        row(&pos, Some(sam.user_id), "order.cancel", "43 empty", 5_001);
        row(&pos, None, "order.cancel", "2 empty order(s) untouched for over 30 minutes", 5_002);
        row(&pos, Some(sam.user_id), "drawer.open", "no sale", 5_003);
        row(&pos, Some(sam.user_id), "drawer.open", "cash payment, order 42", 5_004);
        row(&pos, None, "pin.failed", "", 5_005);
        row(&pos, Some(quinn.user_id), "order.price", "order 42 Pad thai 900 -> 800 by Sam", 5_006);
        let f = ActivityFilter { from: Some(5_000), to: Some(6_000), look: Some(true), ..Default::default() };
        let page = pos.list_activity(&admin, &f, None, None).unwrap();
        let acts: Vec<_> = page.rows.iter().map(|r| (r.action.as_str(), r.detail.as_str())).collect();
        assert_eq!(acts, [("pin.failed", ""), ("drawer.open", "no sale"), ("order.cancel", &*format!("42 changed mind (PIN of Quinn #{}, manager)", quinn.user_id))]);
        assert_eq!(page.look_count, 3);
        let cancel = page.rows.last().unwrap();
        assert_eq!((cancel.user_name.as_deref(), cancel.approver_name.as_deref(), cancel.order_id), (Some("Sam"), Some("Quinn"), Some(42)));
        // A price changed with Quinn's PIN: Sam did it, Quinn approved it.
        let f = ActivityFilter { from: Some(5_000), to: Some(6_000), q: Some("#42".into()), ..Default::default() };
        let page = pos.list_activity(&admin, &f, None, None).unwrap();
        assert_eq!(page.rows.len(), 3, "{:?}", page.rows);
        let price = &page.rows[0];
        assert_eq!((price.user_name.as_deref(), price.approver_name.as_deref(), price.kind.as_str()), (Some("Sam"), Some("Quinn"), "money"));
        // Sam's rows include the price he changed with Quinn's PIN.
        let f = ActivityFilter { from: Some(5_000), to: Some(6_000), user_id: Some(sam.user_id), ..Default::default() };
        assert_eq!(pos.list_activity(&admin, &f, None, None).unwrap().rows.len(), 5);
        let f = ActivityFilter { from: Some(5_000), to: Some(6_000), q: Some("changed".into()), ..Default::default() };
        assert_eq!(pos.list_activity(&admin, &f, None, None).unwrap().rows.len(), 1);
        let f = ActivityFilter { from: Some(5_000), to: Some(6_000), kind: Some("other".into()), ..Default::default() };
        assert_eq!(pos.list_activity(&admin, &f, None, None).unwrap().rows.len(), 2, "the two empty cancels");
    }

    #[test]
    fn clearing_rows_can_never_be_deleted_and_a_wrong_pin_is_logged() {
        let (mut pos, admin, ..) = setup();
        for i in 0..5 {
            row(&pos, Some(admin.user_id), "setting.update", "x", 100 + i);
        }
        assert!(matches!(pos.clear_activity(&admin, 200, "1234", "delete", true), Err(PosError::Validation(_))));
        assert!(matches!(pos.clear_activity(&admin, 200, "9999", "DELETE", true), Err(PosError::InvalidPin)));
        assert_eq!(count(&pos, "log.bad_pin"), 1);
        assert_eq!(pos.activity_count(&admin, 200).unwrap().count, 5);
        assert_eq!(pos.clear_activity(&admin, 200, "1234", "DELETE", true).unwrap(), 5);
        assert_eq!(count(&pos, "log.cleared"), 1);
        let cleared: i64 = pos.conn.query_row("SELECT id FROM audit_log WHERE action = 'log.cleared'", [], |r| r.get(0)).unwrap();
        // Back-date it, then try every way to delete it.
        pos.conn.execute("UPDATE audit_log SET created_at = 50 WHERE action = 'log.cleared'", []).unwrap();
        row(&pos, None, "log.auto_cleared", "{}", 40);
        assert_eq!(pos.delete_activity(&admin, &[cleared], "1234").unwrap(), 0);
        assert_eq!(pos.clear_activity(&admin, 1_000, "1234", "DELETE", false).unwrap(), 0);
        pos.set_setting(&admin, LOG_KEEP_KEY, "30").unwrap();
        pos.log_cleanup_now(10_000_000_000).unwrap();
        assert_eq!(count(&pos, "log.cleared"), 1);
        assert_eq!(count(&pos, "log.auto_cleared"), 2, "the old one and the new one");
        // Delete selected: one row recording it.
        row(&pos, Some(admin.user_id), "login", "", 20_000_000_000);
        let login: i64 = pos.conn.query_row("SELECT MAX(id) FROM audit_log WHERE action = 'login'", [], |r| r.get(0)).unwrap();
        assert_eq!(pos.delete_activity(&admin, &[login, cleared], "1234").unwrap(), 1);
        assert_eq!(count(&pos, "log.cleared"), 2);
        let csv = pos.export_activity(&admin, &ActivityFilter::default(), None).unwrap();
        assert!(csv.0.starts_with("\u{feff}time,who,approver,action,text,order,detail\r\n"));
        assert!(csv.0.contains("log.cleared"));
    }

    #[test]
    fn the_cleanup_runs_in_batches_at_most_every_20_hours_and_forever_keeps_all() {
        let (mut pos, admin, quinn, sam) = setup();
        let now = chrono::Utc::now().timestamp() + 60;
        let old = now - 200 * 86_400;
        pos.conn.execute_batch("BEGIN").unwrap();
        for i in 0..(CLEANUP_BATCH as i64 + 500) {
            row(&pos, Some(admin.user_id), "setting.update", "old", old + i);
        }
        pos.conn.execute_batch("COMMIT").unwrap();
        row(&pos, Some(admin.user_id), "setting.update", "new", now - 10);
        // A done ticket closed long ago, with its messages, and one still open.
        let t = pos.create_ticket(&sam, &crate::NewTicket { category: "printer".into(), urgency: None, body: "jam".into(), order_id: None, screen: None, screen_label: None, device: None }).unwrap();
        pos.set_ticket_status(&quinn, t.id, "done").unwrap();
        pos.conn.execute("UPDATE tickets SET closed_at = ?1 WHERE id = ?2", [old, t.id]).unwrap();
        let open = pos.create_ticket(&sam, &crate::NewTicket { category: "other".into(), urgency: None, body: "hi".into(), order_id: None, screen: None, screen_label: None, device: None }).unwrap();
        pos.conn.execute("UPDATE tickets SET created_at = ?1 WHERE id = ?2", [old, open.id]).unwrap();
        // Forever: nothing.
        pos.set_setting(&admin, LOG_KEEP_KEY, "0").unwrap();
        assert_eq!(pos.log_cleanup_now(now).unwrap(), None);
        pos.set_setting(&admin, LOG_KEEP_KEY, "180").unwrap();
        let (cutoff, days) = pos.log_cleanup_due(now).unwrap().unwrap();
        assert_eq!(days, 180);
        assert_eq!(pos.log_cleanup_batch(cutoff).unwrap(), (CLEANUP_BATCH, 0));
        assert_eq!(pos.log_cleanup_batch(cutoff).unwrap(), (500, 0));
        assert_eq!(pos.log_cleanup_batch(cutoff).unwrap(), (0, 1));
        assert_eq!(pos.log_cleanup_batch(cutoff).unwrap(), (0, 0));
        pos.log_cleanup_finish(cutoff, days, CLEANUP_BATCH + 500, 1).unwrap();
        assert_eq!(count(&pos, "log.auto_cleared"), 1);
        let msgs: i64 = pos.conn.query_row("SELECT COUNT(*) FROM ticket_messages WHERE ticket_id = ?1", [t.id], |r| r.get(0)).unwrap();
        assert_eq!(msgs, 0, "its messages went with it");
        assert!(pos.get_ticket(&quinn, open.id).is_ok(), "tickets not done are never cleaned up");
        // Within 20 hours: not again.
        assert_eq!(pos.log_cleanup_due(now + 3600).unwrap(), None);
        assert!(pos.log_cleanup_due(now + CLEANUP_EVERY_SECS + 1).unwrap().is_some());
        let r = pos.log_retention(&admin).unwrap();
        assert_eq!((r.keep_days, r.last_cleanup_count), (180, Some(CLEANUP_BATCH as i64 + 501)));
        assert!(r.count > 0 && r.approx_bytes > 0);
    }

    #[test]
    fn refs_read_every_detail_format() {
        assert_eq!(refs("order.item_void", "order 12 Tom yum x1 (PIN of Mia #4, manager)", Some(2)), (Some(12), Some(4)));
        assert_eq!(refs("order.cancel", "42 changed mind (PIN of A #B #7, admin)", Some(2)), (Some(42), Some(7)));
        assert_eq!(refs("order.cancel", "42 empty", Some(2)), (Some(42), None));
        assert_eq!(refs("order.cancel", "3 empty order(s) at day close", Some(2)), (None, None));
        assert_eq!(refs("order.delete", "#9 paid total 1200 oops", Some(1)), (Some(9), None));
        assert_eq!(refs("drawer.open", "cash payment, order 77", Some(1)), (Some(77), None));
        assert_eq!(refs("drawer.open", "no sale", Some(1)), (None, None));
        assert_eq!(refs("order.price", "order 5 Pad thai 900 -> 800 by Sam", Some(3)), (Some(5), Some(3)));
        assert_eq!(refs("order.service", "5 0 (was 1250) (no PIN, manager)", Some(3)), (Some(5), None));
        assert_eq!(refs("kitchen.main_course", "8", Some(3)), (Some(8), None));
        assert_eq!(refs("product.update", "8 Rice", Some(3)), (None, None));
        assert_eq!(terminal_label("local"), "local");
        assert_eq!(terminal_label("abcdef0123456789"), "web-abcdef");
        assert_eq!(terminal_label("web-abcdef0123456789"), "web-abcdef");
        assert_eq!(clean_device_label(" Bar | tablet\n"), Some("Bar  tablet".into()));
    }
}

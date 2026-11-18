//! Shifts and cash-up: open with a float, put money in or take it out of
//! the drawer during the shift, and close by counting the cash. The count is
//! compared with what should be in the drawer:
//!
//! float + cash taken (with cash tips, less change) + paid in - paid out
//! - cash refunded for orders voided during the shift.
//!
//! Card payments and card tips never touch the drawer. One shift is open at
//! a time (there is one cash drawer).

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::error::invalid;
use crate::printing::{render_escpos, render_text, Line, Size};
use crate::receipt::{local_time, money, two_col};
use crate::{PaymentNames, Permission, Pos, PosError, PrintJob, PrinterTarget, Result, Session};

#[derive(Debug, Clone, Serialize)]
pub struct CashMove {
    pub id: i64,
    /// "in" or "out".
    pub kind: String,
    pub amount_cents: i64,
    pub reason: Option<String>,
    pub user_name: String,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Shift {
    pub id: i64,
    pub opened_by_name: String,
    pub opened_at: i64,
    pub float_cents: i64,
    pub closed_by_name: Option<String>,
    pub closed_at: Option<i64>,
    /// Payments taken during the shift (a split bill counts each part).
    pub payment_count: i64,
    /// Cash towards bills, and cash tips.
    pub cash_cents: i64,
    pub cash_tip_cents: i64,
    pub card_cents: i64,
    pub card_tip_cents: i64,
    /// Delivery platforms and other payments that are not cash or card.
    pub other_cents: i64,
    pub paid_in_cents: i64,
    pub paid_out_cents: i64,
    /// Cash given back for orders voided during the shift.
    pub refund_cents: i64,
    /// What should be in the drawer (now, or at the close).
    pub expected_cents: i64,
    pub counted_cents: Option<i64>,
    /// counted - expected: negative = cash missing.
    pub difference_cents: Option<i64>,
    pub note: Option<String>,
    pub moves: Vec<CashMove>,
}

/// opened by, opened at, float, closed by, closed at, counted, expected, note.
type ShiftRow = (String, i64, i64, Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<String>);

impl Pos {
    fn load_shift(&self, id: i64) -> Result<Shift> {
        let (opened_by_name, opened_at, float_cents, closed_by_name, closed_at, counted_cents, stored_expected, note): ShiftRow = self
            .conn
            .prepare_cached(
                "SELECT u.name, s.opened_at, s.float_cents, c.name, s.closed_at, s.counted_cents, s.expected_cents, s.note
                 FROM shifts s JOIN users u ON u.id = s.opened_by LEFT JOIN users c ON c.id = s.closed_by WHERE s.id = ?1",
            )?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))
            .optional()?
            .ok_or(PosError::NotFound)?;
        let (payment_count, cash, cash_tip, card, card_tip, other): (i64, i64, i64, i64, i64, i64) = self.conn.prepare_cached(
            "SELECT COUNT(*),
                    COALESCE(SUM(amount_cents) FILTER (WHERE method = 'cash'), 0),
                    COALESCE(SUM(tip_cents) FILTER (WHERE method = 'cash'), 0),
                    COALESCE(SUM(amount_cents) FILTER (WHERE method = 'card'), 0),
                    COALESCE(SUM(tip_cents) FILTER (WHERE method = 'card'), 0),
                    COALESCE(SUM(amount_cents + tip_cents) FILTER (WHERE method = 'other'), 0)
             FROM payments WHERE shift_id = ?1",
        )?
        .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
        let end = closed_at.unwrap_or(i64::MAX);
        let refund: i64 = self.conn.prepare_cached(
            "SELECT COALESCE(SUM(p.amount_cents + p.tip_cents), 0) FROM payments p JOIN orders o ON o.id = p.order_id
             WHERE p.method = 'cash' AND o.status = 'voided' AND o.closed_at >= ?1 AND o.closed_at <= ?2",
        )?
        .query_row(params![opened_at, end], |r| r.get(0))?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, m.kind, m.amount_cents, m.reason, u.name, m.at FROM cash_moves m JOIN users u ON u.id = m.user_id
             WHERE m.shift_id = ?1 ORDER BY m.id",
        )?;
        let moves: Vec<CashMove> = stmt
            .query_map([id], |r| {
                Ok(CashMove { id: r.get(0)?, kind: r.get(1)?, amount_cents: r.get(2)?, reason: r.get(3)?, user_name: r.get(4)?, at: r.get(5)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let paid_in: i64 = moves.iter().filter(|m| m.kind == "in").map(|m| m.amount_cents).sum();
        let paid_out: i64 = moves.iter().filter(|m| m.kind == "out").map(|m| m.amount_cents).sum();
        let expected = stored_expected.unwrap_or(float_cents + cash + cash_tip + paid_in - paid_out - refund);
        Ok(Shift {
            id,
            opened_by_name,
            opened_at,
            float_cents,
            closed_by_name,
            closed_at,
            payment_count,
            cash_cents: cash,
            cash_tip_cents: cash_tip,
            card_cents: card,
            card_tip_cents: card_tip,
            other_cents: other,
            paid_in_cents: paid_in,
            paid_out_cents: paid_out,
            refund_cents: refund,
            expected_cents: expected,
            counted_cents,
            difference_cents: counted_cents.map(|c| c - expected),
            note,
            moves,
        })
    }

    fn open_shift_id(&self) -> Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT id FROM shifts WHERE closed_at IS NULL", [], |r| r.get(0)).optional()?)
    }

    /// The shift running now, if any (with what should be in the drawer).
    pub fn current_shift(&self, s: &Session) -> Result<Option<Shift>> {
        s.require(Permission::Sell)?;
        self.open_shift_id()?.map(|id| self.load_shift(id)).transpose()
    }

    /// Start a shift with `float_cents` in the drawer.
    pub fn open_shift(&self, s: &Session, float_cents: i64) -> Result<Shift> {
        s.require(Permission::Sell)?;
        if !(0..=100_000_000).contains(&float_cents) {
            return invalid("enter the cash in the drawer");
        }
        if self.open_shift_id()?.is_some() {
            return invalid("a shift is already open: close it first");
        }
        self.conn.execute("INSERT INTO shifts (opened_by, float_cents) VALUES (?1, ?2)", params![s.user_id, float_cents])?;
        let id = self.conn.last_insert_rowid();
        self.audit(Some(s.user_id), "shift.open", &format!("shift {id} float {float_cents}"))?;
        self.load_shift(id)
    }

    /// Money put into ("in") or taken out of ("out") the drawer during the
    /// shift, e.g. more change, or paying a supplier. A reason is needed to
    /// take money out.
    pub fn cash_move(&self, s: &Session, kind: &str, amount_cents: i64, reason: &str) -> Result<Shift> {
        s.require(Permission::Sell)?;
        let id = self.open_shift_id()?.ok_or_else(|| PosError::Validation("open a shift first".into()))?;
        if kind != "in" && kind != "out" {
            return invalid("choose money in or money out");
        }
        if !(1..=100_000_000).contains(&amount_cents) {
            return invalid("enter an amount");
        }
        let reason = reason.trim();
        if kind == "out" && reason.is_empty() {
            return invalid("say what the money was taken out for");
        }
        self.conn.execute(
            "INSERT INTO cash_moves (shift_id, kind, amount_cents, reason, user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, kind, amount_cents, (!reason.is_empty()).then_some(reason), s.user_id],
        )?;
        self.audit(Some(s.user_id), "shift.cash", &format!("shift {id} {kind} {amount_cents} {reason}"))?;
        self.load_shift(id)
    }

    /// End the shift: the cash counted in the drawer is kept with what was
    /// expected, so the difference shows in Reports and on the printout.
    pub fn close_shift(&self, s: &Session, counted_cents: i64, note: &str) -> Result<Shift> {
        s.require(Permission::ViewReports)?;
        let id = self.open_shift_id()?.ok_or_else(|| PosError::Validation("no shift is open".into()))?;
        if !(0..=100_000_000).contains(&counted_cents) {
            return invalid("enter the cash counted in the drawer");
        }
        let expected = self.load_shift(id)?.expected_cents;
        let note = note.trim();
        self.conn.execute(
            "UPDATE shifts SET closed_by = ?2, closed_at = unixepoch(), counted_cents = ?3, expected_cents = ?4, note = ?5 WHERE id = ?1",
            params![id, s.user_id, counted_cents, expected, (!note.is_empty()).then_some(note)],
        )?;
        self.audit(Some(s.user_id), "shift.close", &format!("shift {id} counted {counted_cents} expected {expected}"))?;
        self.load_shift(id)
    }

    /// Past shifts, newest first.
    pub fn list_shifts(&self, s: &Session, limit: i64) -> Result<Vec<Shift>> {
        s.require(Permission::ViewReports)?;
        let ids: Vec<i64> = self
            .conn
            .prepare_cached("SELECT id FROM shifts ORDER BY id DESC LIMIT ?1")?
            .query_map([limit.clamp(1, 500)], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        ids.into_iter().map(|id| self.load_shift(id)).collect()
    }

    fn shift_layout(&self, sh: &Shift, currency: &str, width: usize) -> Result<Vec<Line>> {
        let m = |c: i64| money(currency, c);
        let PaymentNames { cash, card } = self.payment_names()?;
        let mut v = Vec::new();
        let rule = |v: &mut Vec<Line>, c: char| v.push(Line::new(Size::Normal, false, false, c.to_string().repeat(width)));
        rule(&mut v, '=');
        v.push(Line::new(Size::Big, true, true, if sh.closed_at.is_some() { "SHIFT CLOSED" } else { "SHIFT (OPEN)" }));
        rule(&mut v, '=');
        two_col(&mut v, Size::Normal, false, "Opened", &format!("{} {}", local_time(sh.opened_at), sh.opened_by_name), width, "");
        if let (Some(at), Some(by)) = (sh.closed_at, &sh.closed_by_name) {
            two_col(&mut v, Size::Normal, false, "Closed", &format!("{} {by}", local_time(at)), width, "");
        }
        two_col(&mut v, Size::Normal, false, "Payments", &sh.payment_count.to_string(), width, "");
        rule(&mut v, '-');
        two_col(&mut v, Size::Normal, false, "Float", &m(sh.float_cents), width, "");
        two_col(&mut v, Size::Normal, false, &format!("{cash} sales"), &m(sh.cash_cents), width, "");
        if sh.cash_tip_cents != 0 {
            two_col(&mut v, Size::Normal, false, &format!("{cash} tips"), &m(sh.cash_tip_cents), width, "");
        }
        if sh.paid_in_cents != 0 {
            two_col(&mut v, Size::Normal, false, "Paid in", &m(sh.paid_in_cents), width, "");
        }
        if sh.paid_out_cents != 0 {
            two_col(&mut v, Size::Normal, false, "Paid out", &m(-sh.paid_out_cents), width, "");
        }
        if sh.refund_cents != 0 {
            two_col(&mut v, Size::Normal, false, &format!("{cash} refunds"), &m(-sh.refund_cents), width, "");
        }
        rule(&mut v, '-');
        two_col(&mut v, Size::Tall, true, "EXPECTED CASH", &m(sh.expected_cents), width, "");
        if let (Some(c), Some(d)) = (sh.counted_cents, sh.difference_cents) {
            two_col(&mut v, Size::Tall, true, "COUNTED", &m(c), width, "");
            let label = if d < 0 { "SHORT" } else if d > 0 { "OVER" } else { "DIFFERENCE" };
            two_col(&mut v, Size::Tall, true, label, &m(d), width, "");
        }
        rule(&mut v, '-');
        two_col(&mut v, Size::Normal, false, &card, &m(sh.card_cents), width, "");
        if sh.card_tip_cents != 0 {
            two_col(&mut v, Size::Normal, false, &format!("{card} tips"), &m(sh.card_tip_cents), width, "");
        }
        if sh.other_cents != 0 {
            two_col(&mut v, Size::Normal, false, "Platforms / other", &m(sh.other_cents), width, "");
        }
        for mv in &sh.moves {
            let label = format!("{} {}", if mv.kind == "in" { "In:" } else { "Out:" }, mv.reason.as_deref().unwrap_or(""));
            two_col(&mut v, Size::Normal, false, &label, &m(mv.amount_cents), width, "  ");
        }
        if let Some(n) = &sh.note {
            v.push(Line::new(Size::Normal, false, false, format!("Note: {n}")));
        }
        rule(&mut v, '=');
        Ok(v)
    }

    /// The shift report as text (Reports screen).
    pub fn shift_text(&self, sh: &Shift) -> Result<String> {
        let cfg = self.receipt_settings()?;
        Ok(render_text(&self.shift_layout(sh, &cfg.currency, cfg.printer.width)?, cfg.printer.width))
    }

    /// The shift report on the receipt printer, or None if there is none.
    pub fn shift_job(&self, s: &Session, sh: &Shift) -> Result<Option<PrintJob>> {
        s.require(Permission::Sell)?;
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return Ok(None);
        }
        let bytes = render_escpos(&self.shift_layout(sh, &cfg.currency, cfg.printer.width)?, cfg.printer.width, cfg.printer.bold);
        Ok(Some(PrintJob::new(cfg.printer.target, bytes)))
    }
}

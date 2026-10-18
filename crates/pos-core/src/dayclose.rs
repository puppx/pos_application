//! End of day ("Z report"): close the business day, print its sales summary
//! and move on to the next date.
//!
//! A business day runs from the previous close to this one, so a late
//! service that finishes after midnight still belongs to the day it started.

use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::error::invalid;
use crate::printing::{render_escpos, render_text, wrap, Line, Size};
use crate::receipt::{local_time, money, two_col};
use crate::{Permission, Pos, PosError, PrintJob, PrinterTarget, Result, SalesSummary, Session};

const DATE_KEY: &str = "_business_date";

#[derive(Debug, Clone, Serialize)]
pub struct BusinessDay {
    /// The open business day, "YYYY-MM-DD".
    pub date: String,
    /// Start of the day (unix seconds): the previous close, or local
    /// midnight of `date` before the first close.
    pub since: i64,
    /// Orders still open; the day cannot close until they are paid or cancelled.
    pub open_orders: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayClose {
    pub id: i64,
    pub business_date: String,
    pub from: i64,
    pub to: i64,
    pub closed_by_name: String,
    pub summary: SalesSummary,
}

pub(crate) fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

pub(crate) fn local_midnight(d: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default())
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

fn payment_label(key: &str) -> String {
    match key {
        "cash" => "Cash".into(),
        "card" => "Card".into(),
        "other" => "Other / platform".into(),
        k => k.into(),
    }
}

fn type_label(key: &str) -> String {
    match key {
        "dine_in" => "Dine-in".into(),
        "takeaway" => "Takeaway".into(),
        "collection" => "Collection".into(),
        "delivery" => "Delivery".into(),
        k => k.into(),
    }
}

impl DayClose {
    fn layout(&self, store: &str, currency: &str, width: usize) -> Vec<Line> {
        let m = |c: i64| money(currency, c);
        let big = width / 2;
        let sum = &self.summary;
        let mut v = Vec::new();
        for t in wrap(store, big, "", "") {
            v.push(Line::new(Size::Big, true, true, t));
        }
        v.push(Line::new(Size::Big, true, true, "END OF DAY"));
        let day = parse_date(&self.business_date)
            .map(|d| d.format("%a %d/%m/%Y").to_string())
            .unwrap_or_else(|| self.business_date.clone());
        v.push(Line::new(Size::Normal, true, true, day));
        v.push(Line::new(Size::Normal, false, false, ""));
        two_col(&mut v, Size::Normal, false, "From", &local_time(self.from), width, "");
        two_col(&mut v, Size::Normal, false, "To", &local_time(self.to), width, "");
        two_col(&mut v, Size::Normal, false, "Closed by", &self.closed_by_name, width, "");
        v.push(Line::new(Size::Normal, false, false, "-".repeat(width)));
        two_col(&mut v, Size::Normal, false, "Orders", &sum.order_count.to_string(), width, "");
        two_col(&mut v, Size::Normal, false, "Sales before tax", &m(sum.subtotal_cents), width, "");
        if sum.discount_cents != 0 {
            two_col(&mut v, Size::Normal, false, "Discounts", &m(-sum.discount_cents), width, "");
        }
        two_col(&mut v, Size::Normal, false, "Tax", &m(sum.tax_cents), width, "");
        if sum.service_cents != 0 {
            two_col(&mut v, Size::Normal, false, "Service charge", &m(sum.service_cents), width, "");
        }
        two_col(&mut v, Size::Big, true, "TOTAL", &m(sum.total_cents), big, "");
        let section = |v: &mut Vec<Line>, title: &str, rows: Vec<(String, i64, i64)>| {
            if rows.is_empty() {
                return;
            }
            v.push(Line::new(Size::Normal, false, false, ""));
            v.push(Line::new(Size::Normal, true, true, format!("-- {title} --")));
            for (label, count, cents) in rows {
                two_col(v, Size::Normal, false, &format!("{label} ({count})"), &m(cents), width, "  ");
            }
        };
        let rows = |g: &[crate::GroupTotal], label: fn(&str) -> String| {
            g.iter().map(|g| (label(&g.key), g.count, g.total_cents)).collect::<Vec<_>>()
        };
        section(&mut v, "PAYMENTS", rows(&sum.by_payment, payment_label));
        section(&mut v, "ORDER TYPES", rows(&sum.by_type, type_label));
        if sum.by_channel.iter().any(|c| c.key != "Direct") {
            section(&mut v, "PLATFORMS", rows(&sum.by_channel, str::to_string));
        }
        section(&mut v, "CATEGORIES (before tax)", rows(&sum.by_category, str::to_string));
        v.push(Line::new(Size::Normal, false, false, "-".repeat(width)));
        two_col(&mut v, Size::Normal, false, &format!("Voided ({})", sum.voided_count), &m(sum.voided_total_cents), width, "");
        two_col(&mut v, Size::Normal, false, "Cancelled orders", &sum.cancelled_count.to_string(), width, "");
        v
    }
}

impl Pos {
    fn business_date(&self) -> Result<NaiveDate> {
        let raw: Option<String> = self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([DATE_KEY], |r| r.get(0))
            .optional()?;
        Ok(raw.as_deref().and_then(parse_date).unwrap_or_else(|| Local::now().date_naive()))
    }

    fn day_start(&self, date: NaiveDate) -> Result<i64> {
        let last: Option<i64> = self.conn.query_row("SELECT MAX(to_ts) FROM day_closes", [], |r| r.get(0))?;
        Ok(last.unwrap_or_else(|| local_midnight(date)))
    }

    /// The business day currently open (shown in the header and Reports).
    pub fn business_day(&self, s: &Session) -> Result<BusinessDay> {
        s.require(Permission::Sell)?;
        let date = self.business_date()?;
        let open_orders = self.conn.query_row("SELECT COUNT(*) FROM orders WHERE status = 'open'", [], |r| r.get(0))?;
        Ok(BusinessDay { date: date.format("%Y-%m-%d").to_string(), since: self.day_start(date)?, open_orders })
    }

    /// Close the business day: record its sales summary and move to the
    /// next date (the day after, or today if that is later). Every order
    /// must be paid or cancelled first.
    pub fn close_day(&self, s: &Session) -> Result<DayClose> {
        s.require(Permission::ViewReports)?;
        let tx = self.conn.unchecked_transaction()?;
        let open: i64 = tx.query_row("SELECT COUNT(*) FROM orders WHERE status = 'open'", [], |r| r.get(0))?;
        if open > 0 {
            return invalid(format!(
                "{open} order{} still open: pay or cancel {} before closing the day",
                if open == 1 { " is" } else { "s are" },
                if open == 1 { "it" } else { "them" }
            ));
        }
        let date = self.business_date()?;
        let from = self.day_start(date)?;
        // Exclusive end, so an order paid this second is in this day.
        let to = Local::now().timestamp().max(from) + 1;
        let summary = self.sales_summary(s, from, to)?;
        let json = serde_json::to_string(&summary).map_err(|e| PosError::Internal(e.to_string()))?;
        let key = date.format("%Y-%m-%d").to_string();
        tx.execute(
            "INSERT INTO day_closes (business_date, from_ts, to_ts, closed_by, summary) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, from, to, s.user_id, json],
        )?;
        let id = tx.last_insert_rowid();
        let next = (date + chrono::Days::new(1)).max(Local::now().date_naive());
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [DATE_KEY, &next.format("%Y-%m-%d").to_string()],
        )?;
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'day.close', ?2)",
            params![s.user_id, format!("{key}: {} orders, total {}", summary.order_count, summary.total_cents)],
        )?;
        tx.commit()?;
        Ok(DayClose { id, business_date: key, from, to, closed_by_name: s.name.clone(), summary })
    }

    /// Past closes, newest first, for the Reports screen and reprints.
    pub fn day_closes(&self, s: &Session, limit: i64) -> Result<Vec<DayClose>> {
        s.require(Permission::ViewReports)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT d.id, d.business_date, d.from_ts, d.to_ts, u.name, d.summary
             FROM day_closes d JOIN users u ON u.id = d.closed_by
             ORDER BY d.to_ts DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.clamp(1, 366)], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, String>(5)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, business_date, from, to, closed_by_name, json) = row?;
            let summary = serde_json::from_str(&json).map_err(|e| PosError::Internal(e.to_string()))?;
            out.push(DayClose { id, business_date, from, to, closed_by_name, summary });
        }
        Ok(out)
    }

    pub fn day_close(&self, s: &Session, id: i64) -> Result<DayClose> {
        self.day_closes(s, 366)?.into_iter().find(|d| d.id == id).ok_or(PosError::NotFound)
    }

    /// The end-of-day report as plain text, laid out like the printout.
    pub fn day_close_text(&self, d: &DayClose) -> Result<String> {
        let cfg = self.receipt_settings()?;
        let store = self.setting("store_name")?.unwrap_or_else(|| "POS".into());
        Ok(render_text(&d.layout(&store, &cfg.currency, cfg.printer.width), cfg.printer.width))
    }

    /// The report on the receipt printer (one copy), or None if there is none.
    pub fn day_close_job(&self, s: &Session, d: &DayClose) -> Result<Option<PrintJob>> {
        s.require(Permission::ViewReports)?;
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return Ok(None);
        }
        let store = self.setting("store_name")?.unwrap_or_else(|| "POS".into());
        let bytes = render_escpos(&d.layout(&store, &cfg.currency, cfg.printer.width), cfg.printer.width, cfg.printer.bold);
        Ok(Some(PrintJob::new(cfg.printer.target, bytes)))
    }
}

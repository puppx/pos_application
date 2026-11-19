//! Customer receipts / bills and the cash drawer.
//!
//! The cash drawer plugs into the receipt printer (RJ11 "DK" port); the
//! printer opens it when it receives the ESC/POS drawer pulse, so "open the
//! drawer" is just a tiny print job.

use chrono::TimeZone;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::printing::{rule, render_escpos, render_escpos_in, render_text, wrap, Line, Size};
use crate::{Order, OrderItem, OrderType, Permission, Pos, PosError, PrintJob, PrintNames, PrinterConfig, PrinterTarget, Result, Session};

const SETTING_KEY: &str = "_receipt_printer";

/// A bill handed over (`orders.bill_printed_at`) stops the receipt printing
/// again at payment, but only while the bill is still right: any change to
/// what the order costs (dishes added, removed or moved in or out, quantity,
/// price, discount, tax, service charge) forgets it, so the customer gets a
/// receipt with the final amount. Done with TEMP triggers, created on every
/// connection and never stored in the database (no migration), so every
/// way an order can change is covered, now and later.
pub(crate) const BILL_STALE_TRIGGERS: &str = "
    CREATE TEMP TRIGGER IF NOT EXISTS bill_stale_item_added AFTER INSERT ON main.order_items
    BEGIN
        UPDATE orders SET bill_printed_at = NULL WHERE id = NEW.order_id AND bill_printed_at IS NOT NULL;
    END;
    CREATE TEMP TRIGGER IF NOT EXISTS bill_stale_item_removed AFTER DELETE ON main.order_items
    BEGIN
        UPDATE orders SET bill_printed_at = NULL WHERE id = OLD.order_id AND bill_printed_at IS NOT NULL;
    END;
    CREATE TEMP TRIGGER IF NOT EXISTS bill_stale_item_changed
    AFTER UPDATE OF qty, unit_price_cents, discount_cents, tax_rate_bp, order_id ON main.order_items
    WHEN OLD.qty IS NOT NEW.qty OR OLD.unit_price_cents IS NOT NEW.unit_price_cents OR OLD.discount_cents IS NOT NEW.discount_cents
      OR OLD.tax_rate_bp IS NOT NEW.tax_rate_bp OR OLD.order_id IS NOT NEW.order_id
    BEGIN
        UPDATE orders SET bill_printed_at = NULL WHERE id IN (OLD.order_id, NEW.order_id) AND bill_printed_at IS NOT NULL;
    END;
    CREATE TEMP TRIGGER IF NOT EXISTS bill_stale_order_changed AFTER UPDATE OF service_bp, tax_bp, tax_incl ON main.orders
    WHEN OLD.service_bp IS NOT NEW.service_bp OR OLD.tax_bp IS NOT NEW.tax_bp OR OLD.tax_incl IS NOT NEW.tax_incl
    BEGIN
        UPDATE orders SET bill_printed_at = NULL WHERE id = NEW.id AND bill_printed_at IS NOT NULL;
    END;";

/// ESC @ (reset) then ESC p 0 25 250: pulse drawer pin 2 for 50 ms on / 500 ms off.
const DRAWER_KICK: [u8; 7] = [0x1B, b'@', 0x1B, b'p', 0, 25, 250];

fn yes() -> bool {
    true
}

fn pound() -> String {
    "£".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptSettings {
    #[serde(flatten)]
    pub printer: PrinterConfig,
    /// Print the receipt automatically when an order is paid.
    #[serde(default = "yes")]
    pub auto_print: bool,
    /// Open the cash drawer when an order is paid in cash.
    #[serde(default = "yes")]
    pub drawer_on_cash: bool,
    /// Name printed at the top; empty means the restaurant name from Settings.
    #[serde(default)]
    pub shop_name: Option<String>,
    /// Lines under the restaurant name: address, phone, VAT number...
    #[serde(default)]
    pub header: Option<String>,
    /// Closing lines, e.g. "Thank you for your visit!".
    #[serde(default)]
    pub footer: Option<String>,
    #[serde(default = "pound")]
    pub currency: String,
    /// Print a second, staff-only copy when an order is paid, with the
    /// tax broken down by class (net, VAT, gross).
    #[serde(default = "yes")]
    pub staff_copy: bool,
    /// Customer bills and receipts also list the tax by rate (net, VAT,
    /// gross) under the total, like the staff copy.
    #[serde(default)]
    pub tax_breakdown: bool,
    /// "Served by <name>" on customer bills and receipts (the staff copy
    /// always has it).
    #[serde(default = "yes")]
    pub show_staff: bool,
}

impl Default for ReceiptSettings {
    fn default() -> Self {
        ReceiptSettings {
            printer: PrinterConfig::default(),
            auto_print: true,
            drawer_on_cash: true,
            shop_name: None,
            header: None,
            footer: Some("Thank you!".into()),
            currency: pound(),
            staff_copy: true,
            tax_breakdown: false,
            show_staff: true,
        }
    }
}

impl ReceiptSettings {
    fn validate(&self) -> Result<()> {
        self.printer.validate()?;
        if self.shop_name.as_deref().is_some_and(|n| n.chars().count() > 60) {
            return invalid("the name on the receipt is too long");
        }
        if self.header.as_deref().is_some_and(|h| h.chars().count() > 400) {
            return invalid("the receipt header is too long");
        }
        if self.footer.as_deref().is_some_and(|f| f.chars().count() > 300) {
            return invalid("the receipt footer is too long");
        }
        if self.currency.chars().count() > 3 {
            return invalid("the currency symbol is too long");
        }
        Ok(())
    }
}

pub struct ReceiptLine {
    pub qty: i64,
    pub name: String,
    /// Printed under the name (Thai), when receipts print both.
    pub name_alt: Option<String>,
    pub options: Vec<String>,
    /// qty x unit price, before discount.
    pub amount_cents: i64,
    pub discount_cents: i64,
}

/// One tax rate on an order: what was sold at it, and the tax in that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxClass {
    pub rate_bp: i64,
    pub net_cents: i64,
    pub vat_cents: i64,
    pub gross_cents: i64,
}

/// A bill (before payment) or receipt (after), laid out for a thermal printer.
pub struct Receipt {
    pub store_name: String,
    pub header: Vec<String>,
    pub footer: Vec<String>,
    pub currency: String,
    /// "BILL", "RECEIPT" or "VOID".
    pub title: String,
    pub order_id: i64,
    pub time: String,
    pub heading: String,
    pub details: Vec<String>,
    pub staff: String,
    pub lines: Vec<ReceiptLine>,
    pub subtotal_cents: i64,
    pub discount_cents: i64,
    pub tax_cents: i64,
    /// The prices already include `tax_cents` (it is not added to the total).
    pub tax_incl: bool,
    /// Net / VAT / gross per tax rate, lowest rate first (the staff copy).
    pub tax_classes: Vec<TaxClass>,
    /// ("Service charge 12.5%", 3.13), when the order has one.
    pub service: Option<(String, i64)>,
    /// The customer's copy lists `tax_classes` under the total too.
    pub tax_breakdown: bool,
    /// The customer's copy says who served.
    pub show_staff: bool,
    pub total_cents: i64,
    /// ("Cash", 40.00), ("Change", 5.35)...: what was handed over.
    pub payment: Vec<(String, i64)>,
    /// The staff copy's: what each payment paid, then for cash with change
    /// what was handed over and the change ("Paid: Cash", 5.00),
    /// ("  Tendered", 10.00), ("  Change", 5.00).
    pub staff_payment: Vec<(String, i64)>,
}

/// Left text and right-aligned amount on one line; long text wraps.
#[allow(clippy::too_many_arguments)]
/// "Service charge 12.5% (no tax)   £4.06"; on narrow paper "(no tax)"
/// goes to the next line with the amount rather than being split.
fn service_no_tax(v: &mut Vec<Line>, label: &str, amount: &str, w: usize) {
    let left = format!("{label} (no tax)");
    if left.chars().count() + amount.chars().count() < w {
        return two_col(v, Size::Normal, false, &left, amount, w, "");
    }
    for p in wrap(label, w, "", "") {
        v.push(Line::new(Size::Normal, false, false, p));
    }
    two_col(v, Size::Normal, false, "(no tax)", amount, w, "");
}

pub(crate) fn two_col(v: &mut Vec<Line>, size: Size, bold: bool, left: &str, right: &str, w: usize, indent: &str) {
    let room = w.saturating_sub(right.chars().count() + 1).max(8);
    let parts = wrap(left, room, "", indent);
    let last = parts.len() - 1;
    for (i, p) in parts.into_iter().enumerate() {
        let text = if i == last {
            let pad = w.saturating_sub(p.chars().count() + right.chars().count());
            format!("{p}{}{right}", " ".repeat(pad))
        } else {
            p
        };
        v.push(Line::new(size, bold, false, text));
    }
}

/// 1250 -> "12.5", 1000 -> "10".
pub(crate) fn percent(bp: i64) -> String {
    let s = format!("{}.{:02}", bp / 100, bp % 100);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub(crate) fn money(currency: &str, cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    format!("{sign}{currency}{}.{:02}", cents.abs() / 100, cents.abs() % 100)
}

impl Receipt {
    fn money(&self, cents: i64) -> String {
        money(&self.currency, cents)
    }

    fn layout(&self, width: usize) -> Vec<Line> {
        let mut v = Vec::new();
        let big = width / 2;
        for t in wrap(&self.store_name, big, "", "") {
            v.push(Line::new(Size::Big, true, true, t));
        }
        for h in &self.header {
            for t in wrap(h, width, "", "") {
                v.push(Line::new(Size::Normal, false, true, t));
            }
        }
        let line = |v: &mut Vec<Line>, ch| v.push(Line::new(Size::Normal, false, false, rule(ch, width)));
        // BILL / RECEIPT, number and time between double rules.
        line(&mut v, '=');
        two_col(&mut v, Size::Normal, true, &format!("{} #{}", self.title, self.order_id), &self.time, width, "");
        line(&mut v, '=');
        v.push(Line::new(Size::Normal, true, false, self.heading.clone()));
        for d in &self.details {
            for t in wrap(d, width, "", "  ") {
                v.push(Line::new(Size::Normal, false, false, t));
            }
        }
        if self.show_staff {
            v.push(Line::new(Size::Normal, false, false, format!("Served by {}", self.staff)));
        }
        line(&mut v, '-');
        two_col(&mut v, Size::Normal, true, "ITEM", "AMOUNT", width, "");
        line(&mut v, '-');

        for l in &self.lines {
            two_col(&mut v, Size::Normal, false, &format!("{} x {}", l.qty, l.name), &self.money(l.amount_cents), width, "    ");
            if let Some(alt) = &l.name_alt {
                for t in wrap(alt, width, "    ", "    ") {
                    v.push(Line::new(Size::Normal, false, false, t));
                }
            }
            for o in &l.options {
                for t in wrap(o, width, "   + ", "     ") {
                    v.push(Line::new(Size::Normal, false, false, t));
                }
            }
            if l.discount_cents != 0 {
                two_col(&mut v, Size::Normal, false, "   discount", &self.money(-l.discount_cents), width, "");
            }
        }
        line(&mut v, '-');
        two_col(&mut v, Size::Normal, false, "Subtotal", &self.money(self.subtotal_cents), width, "");
        if self.discount_cents != 0 {
            two_col(&mut v, Size::Normal, false, "Discount", &self.money(-self.discount_cents), width, "");
        }
        // The customer's copy shows no tax when it is inside the prices (the
        // staff copy has it by class); tax added on top is part of the sum.
        if !self.tax_incl {
            two_col(&mut v, Size::Normal, false, "Tax", &self.money(self.tax_cents), width, "");
        }
        if let Some((label, cents)) = &self.service {
            two_col(&mut v, Size::Normal, false, label, &self.money(*cents), width, "");
        }
        line(&mut v, '=');
        two_col(&mut v, Size::Big, true, "TOTAL", &self.money(self.total_cents), big, "");
        line(&mut v, '=');
        if !self.payment.is_empty() {
            for (label, amount) in &self.payment {
                two_col(&mut v, Size::Normal, false, label, &self.money(*amount), width, "");
            }
            line(&mut v, '-');
        }
        if self.tax_breakdown && !self.tax_classes.is_empty() {
            self.tax_table(&mut v, width);
            // As on the staff copy: the service charge is outside the table,
            // so GROSS plus this line is the TOTAL.
            if let Some((label, cents)) = &self.service {
                service_no_tax(&mut v, label, &self.money(*cents), width);
            }
            if self.tax_incl {
                v.push(Line::new(Size::Normal, false, true, "Prices include tax"));
            }
            line(&mut v, '-');
        }
        if !self.footer.is_empty() {
            v.push(Line::new(Size::Normal, false, false, ""));
            for f in &self.footer {
                for t in wrap(f, width, "", "") {
                    v.push(Line::new(Size::Normal, false, true, t));
                }
            }
        }
        v
    }

    /// The staff copy: who and when, what was sold, and the tax by class.
    fn staff_layout(&self, width: usize) -> Vec<Line> {
        let mut v = Vec::new();
        let line = |v: &mut Vec<Line>, ch| v.push(Line::new(Size::Normal, false, false, rule(ch, width)));
        for t in wrap(&self.store_name, width / 2, "", "") {
            v.push(Line::new(Size::Big, true, true, t));
        }
        line(&mut v, '=');
        v.push(Line::new(Size::Tall, true, true, "STAFF COPY"));
        v.push(Line::new(Size::Normal, false, true, "not for the customer"));
        line(&mut v, '=');
        two_col(&mut v, Size::Normal, true, &format!("{} #{}", self.title, self.order_id), &self.time, width, "");
        v.push(Line::new(Size::Normal, true, false, self.heading.clone()));
        v.push(Line::new(Size::Normal, false, false, format!("Served by {}", self.staff)));
        for (label, amount) in &self.staff_payment {
            two_col(&mut v, Size::Normal, false, label, &self.money(*amount), width, "");
        }
        line(&mut v, '-');
        two_col(&mut v, Size::Normal, true, "ITEM", "AMOUNT", width, "");
        line(&mut v, '-');
        for l in &self.lines {
            two_col(&mut v, Size::Normal, false, &format!("{} x {}", l.qty, l.name), &self.money(l.amount_cents - l.discount_cents), width, "    ");
        }
        line(&mut v, '=');
        self.tax_table(&mut v, width);
        if self.discount_cents != 0 {
            v.push(Line::new(Size::Normal, false, false, "(after discounts)"));
        }
        if let Some((label, cents)) = &self.service {
            service_no_tax(&mut v, label, &self.money(*cents), width);
        }
        line(&mut v, '=');
        two_col(&mut v, Size::Big, true, "TOTAL", &self.money(self.total_cents), width / 2, "");
        line(&mut v, '=');
        v.push(Line::new(Size::Normal, false, true, if self.tax_incl { "Prices include tax" } else { "Tax added on top of prices" }));
        v
    }

    /// TAX | NET | VAT | GROSS, a row per rate and the totals, the three
    /// amounts sharing what is left of the width.
    fn tax_table(&self, v: &mut Vec<Line>, width: usize) {
        let line = |v: &mut Vec<Line>, ch| v.push(Line::new(Size::Normal, false, false, rule(ch, width)));
        let mut rows: Vec<(String, [i64; 3])> = Vec::new();
        let mut totals = [0, 0, 0];
        for (i, c) in self.tax_classes.iter().enumerate() {
            let letter = char::from(b'A' + u8::try_from(i.min(25)).unwrap_or(0));
            rows.push((format!("{letter} {}%", percent(c.rate_bp)), [c.net_cents, c.vat_cents, c.gross_cents]));
            totals = [totals[0] + c.net_cents, totals[1] + c.vat_cents, totals[2] + c.gross_cents];
        }
        let all = rows.iter().map(|r| r.1).chain([totals]).flatten().collect::<Vec<_>>();
        // Usually: an 8-wide label and three equal columns. Big amounts drop
        // the currency sign, then the columns widen and the label gives way,
        // so every amount keeps a space before it and the row fits the paper.
        let mut label_w = 8;
        let mut col = width.saturating_sub(label_w) / 3;
        let widest = |sign: bool| all.iter().map(|&c| self.amount(c, sign).chars().count()).max().unwrap_or(0);
        let sign = widest(true) < col;
        if !sign {
            col = col.max(widest(false) + 1);
            label_w = width.saturating_sub(3 * col).max(5);
        }
        let cw = col.saturating_sub(1);
        let row = |a: &str, b: &str, c: &str, d: &str| {
            let a: String = a.chars().take(label_w).collect();
            format!("{a:<label_w$} {b:>cw$} {c:>cw$} {d:>cw$}")
        };
        let cells = |r: [i64; 3]| r.map(|c| self.amount(c, sign));
        v.push(Line::new(Size::Normal, true, false, row("TAX", "NET", "VAT", "GROSS")));
        line(v, '-');
        for (label, r) in &rows {
            let [b, c, d] = cells(*r);
            v.push(Line::new(Size::Normal, false, false, row(label, &b, &c, &d)));
        }
        line(v, '-');
        let [b, c, d] = cells(totals);
        v.push(Line::new(Size::Normal, true, false, row("Total", &b, &c, &d)));
    }

    /// An amount for the tax table, with or without the currency sign.
    fn amount(&self, cents: i64, sign: bool) -> String {
        if sign { self.money(cents) } else { money("", cents) }
    }

    pub fn staff_to_text(&self, width: usize) -> String {
        render_text(&self.staff_layout(width), width)
    }

    pub fn staff_to_escpos(&self, width: usize, bold: bool) -> Vec<u8> {
        render_escpos(&self.staff_layout(width), width, bold)
    }

    pub fn to_text(&self, width: usize) -> String {
        render_text(&self.layout(width), width)
    }

    pub fn to_escpos(&self, width: usize, bold: bool) -> Vec<u8> {
        render_escpos(&self.layout(width), width, bold)
    }

    /// The receipt as printer `p` prints it (paper width, bold, image mode).
    pub fn escpos_for(&self, p: &PrinterConfig) -> Vec<u8> {
        render_escpos_in(&self.layout(p.width), p.width, p.bold, false, p.image_mode)
    }

    /// The staff copy as printer `p` prints it.
    pub fn staff_escpos_for(&self, p: &PrinterConfig) -> Vec<u8> {
        render_escpos_in(&self.staff_layout(p.width), p.width, p.bold, false, p.image_mode)
    }
}

fn lines_of(s: &Option<String>) -> Vec<String> {
    s.as_deref()
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

pub(crate) fn local_time(ts: i64) -> String {
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%d/%m/%Y %H:%M").to_string())
        .unwrap_or_default()
}

impl Pos {
    pub fn receipt_settings(&self) -> Result<ReceiptSettings> {
        let raw: Option<String> = self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([SETTING_KEY], |r| r.get(0))
            .optional()?;
        Ok(raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
    }

    pub fn set_receipt_settings(&self, s: &Session, cfg: &ReceiptSettings) -> Result<()> {
        s.require(Permission::ManageSettings)?;
        cfg.validate()?;
        let json = serde_json::to_string(cfg).map_err(|e| PosError::Internal(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [SETTING_KEY, &json],
        )?;
        self.audit(Some(s.user_id), "printer.receipt", &json)
    }

    /// The name at the top of bills: the receipt's own, else the restaurant's.
    fn receipt_name(&self, cfg: &ReceiptSettings) -> Result<String> {
        let own = cfg.shop_name.as_deref().map(str::trim).filter(|n| !n.is_empty());
        Ok(match own {
            Some(n) => n.to_string(),
            None => self.setting("store_name")?.unwrap_or_else(|| "POS".into()),
        })
    }

    /// The bill (open order) or receipt (paid order), grouped by category.
    pub fn receipt(&self, s: &Session, order_id: i64) -> Result<Receipt> {
        let o = self.get_order(s, order_id)?;
        if o.status == "cancelled" {
            return invalid("a cancelled order has no receipt");
        }
        let cfg = self.receipt_settings()?;
        self.build_receipt(&o, &cfg)
    }

    fn build_receipt(&self, o: &Order, cfg: &ReceiptSettings) -> Result<Receipt> {
        let mut details = Vec::new();
        if let Some(f) = &o.floor_name {
            details.push(f.clone());
        }
        if let Some(n) = &o.customer_name {
            details.push(n.clone());
        }
        if o.order_type == OrderType::Delivery && o.channel_name.is_none() {
            details.extend(o.delivery_address.clone());
            details.extend(o.customer_phone.clone());
        }
        let staff: String = self
            .conn
            .prepare_cached("SELECT name FROM users WHERE id = ?1")?
            .query_row([o.paid_by.unwrap_or(o.opened_by)], |r| r.get(0))?;
        let mut payment = Vec::new();
        let mut staff_payment = Vec::new();
        let names = self.payment_names()?;
        // Each payment (several for a split bill), with its tip and change.
        // A bill printed after a part payment shows what is left to pay.
        for p in &o.payments {
            let label = match names.of(&p.method) {
                Some(n) => n.to_string(),
                None => o.channel_name.as_ref().map(|c| format!("Paid by {c}")).unwrap_or_else(|| "Other".into()),
            };
            // Cash: what was handed over (change follows); card: the bill part (tip follows).
            payment.push((label.clone(), if p.method == "cash" { p.tendered_cents } else { p.amount_cents }));
            if p.tip_cents > 0 {
                payment.push(("  Tip".into(), p.tip_cents));
            }
            if p.change_cents > 0 {
                payment.push(("  Change".into(), p.change_cents));
            }
            // The staff copy: what it paid towards the bill first.
            staff_payment.push((format!("Paid: {label}"), p.amount_cents));
            if p.tip_cents > 0 {
                staff_payment.push(("  Tip".into(), p.tip_cents));
            }
            if p.method == "cash" && p.change_cents > 0 {
                staff_payment.push(("  Tendered".into(), p.tendered_cents));
                staff_payment.push(("  Change".into(), p.change_cents));
            }
        }
        if o.status == "open" && o.paid_cents > 0 {
            let left = ("LEFT TO PAY".to_string(), (o.total_cents - o.paid_cents).max(0));
            payment.push(left.clone());
            staff_payment.push(left);
        }
        // Receipts print English, or English with Thai under it.
        let names = match cfg.printer.names {
            Some(PrintNames::Both | PrintNames::Thai) => PrintNames::Both,
            _ => PrintNames::English,
        };
        // A set shows as one line at its price; its dishes are for the kitchen.
        let billed: Vec<OrderItem> = o.items.iter().filter(|i| i.parent_id.is_none()).cloned().collect();
        let lines = self
            .by_category(&billed, names)?
            .into_iter()
            .map(|(_, i)| ReceiptLine {
                qty: i.qty,
                name: i.name.clone(),
                name_alt: names.pick(&i.name, i.name_alt.as_deref()).1.map(str::to_string),
                options: i.options.iter().map(|o| names.inline(&o.name, o.name_alt.as_deref())).collect(),
                amount_cents: i.unit_price_cents * i.qty,
                discount_cents: i.discount_cents,
            })
            .collect();
        let mut tax_classes: Vec<TaxClass> = Vec::new();
        for i in &billed {
            let gross = i.line_total_cents + if o.tax_incl { 0 } else { i.tax_cents };
            let at = match tax_classes.iter().position(|c| c.rate_bp == i.tax_rate_bp) {
                Some(at) => at,
                None => {
                    tax_classes.push(TaxClass { rate_bp: i.tax_rate_bp, net_cents: 0, vat_cents: 0, gross_cents: 0 });
                    tax_classes.len() - 1
                }
            };
            let c = &mut tax_classes[at];
            c.gross_cents += gross;
            c.vat_cents += i.tax_cents;
            c.net_cents += gross - i.tax_cents;
        }
        tax_classes.sort_by_key(|c| c.rate_bp);
        Ok(Receipt {
            store_name: self.receipt_name(cfg)?,
            header: lines_of(&cfg.header),
            footer: lines_of(&cfg.footer),
            currency: cfg.currency.clone(),
            tax_classes,
            title: match o.status.as_str() {
                "paid" => "RECEIPT",
                "voided" => "VOID",
                _ => "BILL",
            }
            .into(),
            order_id: o.id,
            time: local_time(o.paid_at.unwrap_or_else(|| chrono::Local::now().timestamp())),
            heading: self.heading(o)?,
            details,
            staff,
            lines,
            subtotal_cents: o.subtotal_cents,
            discount_cents: o.discount_cents,
            tax_cents: o.tax_cents,
            tax_incl: o.tax_incl,
            service: (o.service_cents != 0)
                .then(|| (format!("{} {}%", self.service_settings().map(|c| c.label).unwrap_or_default(), percent(o.service_bp)), o.service_cents)),
            total_cents: o.total_cents,
            payment,
            staff_payment,
            tax_breakdown: cfg.tax_breakdown,
            show_staff: cfg.show_staff,
        })
    }

    /// Manual "Print bill" / "Reprint receipt": one copy, no drawer.
    pub fn receipt_job(&self, s: &Session, order_id: i64) -> Result<PrintJob> {
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return invalid("no receipt printer is set up (Settings → Receipt printer)");
        }
        let r = self.receipt(s, order_id)?;
        // A bill handed over before payment: no second copy at payment.
        self.conn.execute("UPDATE orders SET bill_printed_at = unixepoch() WHERE id = ?1 AND status = 'open'", [order_id])?;
        Ok(PrintJob::new(cfg.printer.target.clone(), r.escpos_for(&cfg.printer)))
    }

    /// Reprint the staff copy of a paid order.
    pub fn staff_copy_job(&self, s: &Session, order_id: i64) -> Result<PrintJob> {
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return invalid("no receipt printer is set up (Settings → Receipt printer)");
        }
        let r = self.receipt(s, order_id)?;
        Ok(PrintJob::new(cfg.printer.target.clone(), r.staff_escpos_for(&cfg.printer)))
    }

    /// What to print right after a payment: the receipt (if automatic,
    /// with the configured number of copies) and the drawer pulse for cash.
    pub fn payment_print_job(&self, s: &Session, order: &Order) -> Result<Option<PrintJob>> {
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return Ok(None);
        }
        // The drawer opens for a cash payment, the last part of a split bill included.
        let drawer = cfg.drawer_on_cash && order.payments.last().is_some_and(|p| p.method == "cash");
        let lead: &[u8] = if drawer { &DRAWER_KICK } else { &[] };
        let p = &cfg.printer;
        // A part payment: only the drawer; the receipt waits for the last part.
        let r = (order.status == "paid" && (cfg.auto_print || cfg.staff_copy)).then(|| self.build_receipt(order, &cfg)).transpose()?;
        // The customer already has the bill (printed with "Bill"): not again.
        let bill_given: bool = self
            .conn
            .query_row("SELECT bill_printed_at IS NOT NULL FROM orders WHERE id = ?1", [order.id], |r| r.get(0))
            .optional()?
            .unwrap_or(false);
        let mut job = match &r {
            Some(r) if cfg.auto_print && !bill_given => Some(PrintJob::copies(p.target.clone(), lead, r.escpos_for(p), p.copies, p.copy_delay_secs)),
            _ => drawer.then(|| PrintJob::new(p.target.clone(), DRAWER_KICK.to_vec())),
        };
        // The staff copy follows the customer's, as one more part of the job.
        if let (Some(r), true) = (&r, cfg.staff_copy && order.status == "paid") {
            let staff = r.staff_escpos_for(p);
            match &mut job {
                Some(j) => j.parts.push(staff),
                None => job = Some(PrintJob::new(p.target.clone(), staff)),
            }
        }
        if drawer {
            self.audit(Some(s.user_id), "drawer.open", &format!("cash payment, order {}", order.id))?;
        }
        Ok(job)
    }

    /// "Open drawer" button (no sale). Logged, so it can be checked later.
    pub fn drawer_job(&self, s: &Session) -> Result<PrintJob> {
        s.require(Permission::Sell)?;
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return invalid("no receipt printer is set up, so the drawer cannot be opened");
        }
        self.audit(Some(s.user_id), "drawer.open", "no sale")?;
        Ok(PrintJob::new(cfg.printer.target, DRAWER_KICK.to_vec()))
    }

    /// Check the settings: a sample receipt (with the configured copies), or
    /// with `drawer_only` just the cash drawer pulse.
    pub fn test_receipt_job(&self, s: &Session, cfg: &ReceiptSettings, drawer_only: bool) -> Result<PrintJob> {
        s.require(Permission::ManageSettings)?;
        cfg.validate()?;
        if drawer_only {
            return Ok(PrintJob::new(cfg.printer.target.clone(), DRAWER_KICK.to_vec()));
        }
        let thai = matches!(cfg.printer.names, Some(PrintNames::Both | PrintNames::Thai));
        let alt = |t: &str| thai.then(|| t.to_string());
        let r = Receipt {
            store_name: self.receipt_name(cfg)?,
            header: lines_of(&cfg.header),
            footer: lines_of(&cfg.footer),
            currency: cfg.currency.clone(),
            title: "TEST".into(),
            order_id: 0,
            time: local_time(chrono::Local::now().timestamp()),
            heading: "DINE-IN T1".into(),
            details: vec![],
            staff: s.name.clone(),
            lines: vec![
                ReceiptLine { qty: 1, name: "Soup of the day".into(), name_alt: alt("ซุปประจำวัน"), options: vec![], amount_cents: 550, discount_cents: 0 },
                ReceiptLine { qty: 2, name: "Example burger".into(), name_alt: alt("เบอร์เกอร์"), options: vec!["Large".into()], amount_cents: 2300, discount_cents: 0 },
            ],
            subtotal_cents: 2850,
            discount_cents: 0,
            tax_cents: 285,
            tax_incl: false,
            tax_classes: vec![TaxClass { rate_bp: 1000, net_cents: 2850, vat_cents: 285, gross_cents: 3135 }],
            service: None,
            total_cents: 3135,
            payment: vec![("Cash".into(), 4000), ("  Change".into(), 865)],
            staff_payment: vec![("Paid: Cash".into(), 3135), ("  Tendered".into(), 4000), ("  Change".into(), 865)],
            tax_breakdown: cfg.tax_breakdown,
            show_staff: cfg.show_staff,
        };
        let p = &cfg.printer;
        Ok(PrintJob::copies(p.target.clone(), &[], r.escpos_for(p), p.copies, p.copy_delay_secs))
    }
}

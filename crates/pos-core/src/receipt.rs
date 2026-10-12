//! Customer receipts / bills and the cash drawer.
//!
//! The cash drawer plugs into the receipt printer (RJ11 "DK" port); the
//! printer opens it when it receives the ESC/POS drawer pulse, so "open the
//! drawer" is just a tiny print job.

use chrono::TimeZone;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::printing::{category_heading, heading, render_escpos, render_text, repeat_copies, wrap, Line, Size};
use crate::{Order, OrderType, Permission, Pos, PosError, PrintJob, PrinterConfig, PrinterTarget, Result, Session};

const SETTING_KEY: &str = "_receipt_printer";

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
    /// Lines under the restaurant name: address, phone, VAT number...
    #[serde(default)]
    pub header: Option<String>,
    /// Closing lines, e.g. "Thank you for your visit!".
    #[serde(default)]
    pub footer: Option<String>,
    #[serde(default = "pound")]
    pub currency: String,
}

impl Default for ReceiptSettings {
    fn default() -> Self {
        ReceiptSettings {
            printer: PrinterConfig::default(),
            auto_print: true,
            drawer_on_cash: true,
            header: None,
            footer: Some("Thank you!".into()),
            currency: pound(),
        }
    }
}

impl ReceiptSettings {
    fn validate(&self) -> Result<()> {
        self.printer.validate()?;
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
    pub category: Option<String>,
    pub qty: i64,
    pub name: String,
    pub options: Vec<String>,
    /// qty x unit price, before discount.
    pub amount_cents: i64,
    pub discount_cents: i64,
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
    pub total_cents: i64,
    /// ("Cash", 40.00), ("Change", 5.35)...
    pub payment: Vec<(String, i64)>,
}

impl Receipt {
    fn money(&self, cents: i64) -> String {
        let sign = if cents < 0 { "-" } else { "" };
        format!("{sign}{}{}.{:02}", self.currency, cents.abs() / 100, cents.abs() % 100)
    }

    fn layout(&self, width: usize) -> Vec<Line> {
        let mut v = Vec::new();
        let big = width / 2;
        // Left text and right-aligned amount on one line; long text wraps.
        let two_col = |v: &mut Vec<Line>, size: Size, bold: bool, left: &str, right: &str, w: usize, indent: &str| {
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
        };
        for t in wrap(&self.store_name, big, "", "") {
            v.push(Line::new(Size::Big, true, true, t));
        }
        for h in &self.header {
            for t in wrap(h, width, "", "") {
                v.push(Line::new(Size::Normal, false, true, t));
            }
        }
        v.push(Line::new(Size::Normal, false, false, ""));
        two_col(&mut v, Size::Normal, true, &format!("{} #{}", self.title, self.order_id), &self.time, width, "");
        v.push(Line::new(Size::Normal, true, false, self.heading.clone()));
        for d in &self.details {
            for t in wrap(d, width, "", "  ") {
                v.push(Line::new(Size::Normal, false, false, t));
            }
        }
        v.push(Line::new(Size::Normal, false, false, format!("Served by {}", self.staff)));
        v.push(Line::new(Size::Normal, false, false, "-".repeat(width)));

        let grouped = self.lines.iter().any(|l| l.category.is_some());
        let mut current: Option<Option<&str>> = None;
        for l in &self.lines {
            let cat = l.category.as_deref();
            if grouped && current != Some(cat) {
                if current.is_some() {
                    v.push(Line::new(Size::Normal, false, false, ""));
                }
                v.push(Line::new(Size::Normal, true, true, category_heading(cat)));
                current = Some(cat);
            }
            two_col(&mut v, Size::Normal, false, &format!("{} x {}", l.qty, l.name), &self.money(l.amount_cents), width, "    ");
            for o in &l.options {
                for t in wrap(o, width, "   + ", "     ") {
                    v.push(Line::new(Size::Normal, false, false, t));
                }
            }
            if l.discount_cents != 0 {
                two_col(&mut v, Size::Normal, false, "   discount", &self.money(-l.discount_cents), width, "");
            }
        }
        v.push(Line::new(Size::Normal, false, false, "-".repeat(width)));
        two_col(&mut v, Size::Normal, false, "Subtotal", &self.money(self.subtotal_cents), width, "");
        if self.discount_cents != 0 {
            two_col(&mut v, Size::Normal, false, "Discount", &self.money(-self.discount_cents), width, "");
        }
        two_col(&mut v, Size::Normal, false, "Tax", &self.money(self.tax_cents), width, "");
        two_col(&mut v, Size::Big, true, "TOTAL", &self.money(self.total_cents), big, "");
        if !self.payment.is_empty() {
            v.push(Line::new(Size::Normal, false, false, ""));
            for (label, amount) in &self.payment {
                two_col(&mut v, Size::Normal, false, label, &self.money(*amount), width, "");
            }
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

    pub fn to_text(&self, width: usize) -> String {
        render_text(&self.layout(width), width)
    }

    pub fn to_escpos(&self, width: usize) -> Vec<u8> {
        render_escpos(&self.layout(width))
    }
}

fn lines_of(s: &Option<String>) -> Vec<String> {
    s.as_deref()
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

fn local_time(ts: i64) -> String {
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
        if o.status == "paid" {
            let label = match o.payment_method.as_deref() {
                Some("cash") => "Cash".to_string(),
                Some("card") => "Card".to_string(),
                _ => o.channel_name.as_ref().map(|c| format!("Paid by {c}")).unwrap_or_else(|| "Other".into()),
            };
            payment.push((label, o.tendered_cents.unwrap_or(o.total_cents)));
            if let Some(change) = o.change_cents.filter(|c| *c > 0) {
                payment.push(("Change".into(), change));
            }
        }
        let lines = self
            .by_category(&o.items)?
            .into_iter()
            .map(|(category, i)| ReceiptLine {
                category,
                qty: i.qty,
                name: i.name.clone(),
                options: i.options.iter().map(|o| o.name.clone()).collect(),
                amount_cents: i.unit_price_cents * i.qty,
                discount_cents: i.discount_cents,
            })
            .collect();
        Ok(Receipt {
            store_name: self.setting("store_name")?.unwrap_or_else(|| "POS".into()),
            header: lines_of(&cfg.header),
            footer: lines_of(&cfg.footer),
            currency: cfg.currency.clone(),
            title: match o.status.as_str() {
                "paid" => "RECEIPT",
                "voided" => "VOID",
                _ => "BILL",
            }
            .into(),
            order_id: o.id,
            time: local_time(o.paid_at.unwrap_or_else(|| chrono::Local::now().timestamp())),
            heading: heading(o),
            details,
            staff,
            lines,
            subtotal_cents: o.subtotal_cents,
            discount_cents: o.discount_cents,
            tax_cents: o.tax_cents,
            total_cents: o.total_cents,
            payment,
        })
    }

    /// Manual "Print bill" / "Reprint receipt": one copy, no drawer.
    pub fn receipt_job(&self, s: &Session, order_id: i64) -> Result<PrintJob> {
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return invalid("no receipt printer is set up (Settings → Receipt printer)");
        }
        let r = self.receipt(s, order_id)?;
        Ok(PrintJob { target: cfg.printer.target.clone(), bytes: r.to_escpos(cfg.printer.width) })
    }

    /// What to print right after a payment: the receipt (if automatic,
    /// with the configured number of copies) and the drawer pulse for cash.
    pub fn payment_print_job(&self, s: &Session, order: &Order) -> Result<Option<PrintJob>> {
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return Ok(None);
        }
        let drawer = cfg.drawer_on_cash && order.payment_method.as_deref() == Some("cash");
        let mut bytes = Vec::new();
        if drawer {
            bytes.extend_from_slice(&DRAWER_KICK);
        }
        if cfg.auto_print {
            let r = self.build_receipt(order, &cfg)?;
            bytes.extend(repeat_copies(r.to_escpos(cfg.printer.width), cfg.printer.copies));
        }
        if drawer {
            self.audit(Some(s.user_id), "drawer.open", &format!("cash payment, order {}", order.id))?;
        }
        Ok((!bytes.is_empty()).then_some(PrintJob { target: cfg.printer.target, bytes }))
    }

    /// "Open drawer" button (no sale). Logged, so it can be checked later.
    pub fn drawer_job(&self, s: &Session) -> Result<PrintJob> {
        s.require(Permission::Sell)?;
        let cfg = self.receipt_settings()?;
        if cfg.printer.target == PrinterTarget::None {
            return invalid("no receipt printer is set up, so the drawer cannot be opened");
        }
        self.audit(Some(s.user_id), "drawer.open", "no sale")?;
        Ok(PrintJob { target: cfg.printer.target, bytes: DRAWER_KICK.to_vec() })
    }

    /// Check the settings: a sample receipt (with the configured copies), or
    /// with `drawer_only` just the cash drawer pulse.
    pub fn test_receipt_job(&self, s: &Session, cfg: &ReceiptSettings, drawer_only: bool) -> Result<PrintJob> {
        s.require(Permission::ManageSettings)?;
        cfg.validate()?;
        if drawer_only {
            return Ok(PrintJob { target: cfg.printer.target.clone(), bytes: DRAWER_KICK.to_vec() });
        }
        let r = Receipt {
            store_name: self.setting("store_name")?.unwrap_or_else(|| "POS".into()),
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
                ReceiptLine { category: Some("Starters".into()), qty: 1, name: "Soup of the day".into(), options: vec![], amount_cents: 550, discount_cents: 0 },
                ReceiptLine { category: Some("Mains".into()), qty: 2, name: "Example burger".into(), options: vec!["Large".into()], amount_cents: 2300, discount_cents: 0 },
            ],
            subtotal_cents: 2850,
            discount_cents: 0,
            tax_cents: 285,
            total_cents: 3135,
            payment: vec![("Cash".into(), 4000), ("Change".into(), 865)],
        };
        Ok(PrintJob { target: cfg.printer.target.clone(), bytes: repeat_copies(r.to_escpos(cfg.printer.width), cfg.printer.copies) })
    }
}

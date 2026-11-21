//! Restaurant orders: dine-in (on a table), takeaway, collection, delivery.
//!
//! Life cycle: `open_order` -> `add_item`/`update_item` -> `send_to_kitchen`
//! (any number of rounds) -> `pay_order`, or `cancel_order` if abandoned.
//! A paid order is the sales record; an admin can `void_order` it later.

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};

use crate::error::{invalid, map_unique};
use crate::options::{resolve_options, ItemOption};
use crate::service::{check_rate, service_of};
use crate::variants::resolve_variant;
use crate::{Permission, Pos, PosError, Result, Session};

const MAX_QTY: i64 = 1_000;
/// The most one line may be priced at, each (100000.00).
const MAX_LINE_PRICE: i64 = 10_000_000;
/// The hidden menu entry behind dishes typed on an order ("open items").
pub const OPEN_ITEM_SKU: &str = "__open_item__";
pub(crate) const MAX_ITEMS: i64 = 300;


/// SQL condition (on `orders`, no alias) for an order with nothing on it:
/// no dishes, payments or customer details.
pub(crate) const EMPTY_ORDER: &str = "NOT EXISTS (SELECT 1 FROM order_items WHERE order_id = orders.id)
    AND NOT EXISTS (SELECT 1 FROM payments WHERE order_id = orders.id)
    AND COALESCE(customer_name, '') = '' AND COALESCE(customer_phone, '') = ''
    AND COALESCE(delivery_address, '') = '' AND COALESCE(notes, '') = ''
    AND COALESCE(external_ref, '') = '' AND customer_id IS NULL";
/// Open orders with nothing on them (see [`EMPTY_ORDER`]) untouched for
/// longer than this (opened, or last changed: see [`ACTIVITY_TRIGGERS`])
/// are forgotten ones (a table tapped, a till reloaded) and get cancelled;
/// a till that opened one, or took its last dish off, just now is never
/// caught out.
pub const EMPTY_ORDER_AGE: i64 = 30 * 60;
/// The answer for anything done to an order the sweep cancelled for having
/// nothing on it ([`Pos::sweep_empty_orders`]), in place of "this order is
/// already closed", so the till can say so plainly. One a till cancelled as
/// empty (`cancel_if_empty`, day close) still answers "already closed".
pub const EMPTY_ORDER_SWEPT: &str = "this empty order was closed automatically";
/// `orders.touched_at`: when an open order last changed. Any change to its
/// dishes (added, changed, moved in or out, removed), its payments, or its
/// own details sets it, however it was made (a save that changes none of
/// its details does not count). TEMP triggers, created on every
/// connection like `receipt::BILL_STALE_TRIGGERS`, so they can change
/// without a migration. The orders trigger lists the columns it watches,
/// so its own update of `touched_at` never sets it off again.
pub(crate) const ACTIVITY_TRIGGERS: &str = "
    CREATE TEMP TRIGGER IF NOT EXISTS order_touched_item_added AFTER INSERT ON main.order_items
    BEGIN
        UPDATE orders SET touched_at = unixepoch() WHERE id = NEW.order_id AND status = 'open';
    END;
    CREATE TEMP TRIGGER IF NOT EXISTS order_touched_item_removed AFTER DELETE ON main.order_items
    BEGIN
        UPDATE orders SET touched_at = unixepoch() WHERE id = OLD.order_id AND status = 'open';
    END;
    CREATE TEMP TRIGGER IF NOT EXISTS order_touched_item_changed AFTER UPDATE ON main.order_items
    BEGIN
        UPDATE orders SET touched_at = unixepoch() WHERE id IN (OLD.order_id, NEW.order_id) AND status = 'open';
    END;
    CREATE TEMP TRIGGER IF NOT EXISTS order_touched_payment_added AFTER INSERT ON main.payments
    BEGIN
        UPDATE orders SET touched_at = unixepoch() WHERE id = NEW.order_id AND status = 'open';
    END;
    CREATE TEMP TRIGGER IF NOT EXISTS order_touched_payment_removed AFTER DELETE ON main.payments
    BEGIN
        UPDATE orders SET touched_at = unixepoch() WHERE id = OLD.order_id AND status = 'open';
    END;
    CREATE TEMP TRIGGER IF NOT EXISTS order_touched_details
    AFTER UPDATE OF order_type, table_id, customer_name, customer_phone, delivery_address, notes, channel_id,
        external_ref, customer_id, service_bp, tax_bp, tax_incl ON main.orders
    WHEN NEW.status = 'open' AND (OLD.order_type IS NOT NEW.order_type OR OLD.table_id IS NOT NEW.table_id
        OR OLD.customer_name IS NOT NEW.customer_name OR OLD.customer_phone IS NOT NEW.customer_phone
        OR OLD.delivery_address IS NOT NEW.delivery_address OR OLD.notes IS NOT NEW.notes
        OR OLD.channel_id IS NOT NEW.channel_id OR OLD.external_ref IS NOT NEW.external_ref
        OR OLD.customer_id IS NOT NEW.customer_id OR OLD.service_bp IS NOT NEW.service_bp
        OR OLD.tax_bp IS NOT NEW.tax_bp OR OLD.tax_incl IS NOT NEW.tax_incl)
    BEGIN
        UPDATE orders SET touched_at = unixepoch() WHERE id = NEW.id;
    END;";
/// [`Pos::sweep_empty_orders_if_due`] looks at most this often.
const EMPTY_SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    DineIn,
    Takeaway,
    Collection,
    Delivery,
}

impl OrderType {
    pub fn as_str(self) -> &'static str {
        match self {
            OrderType::DineIn => "dine_in",
            OrderType::Takeaway => "takeaway",
            OrderType::Collection => "collection",
            OrderType::Delivery => "delivery",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentMethod {
    Cash,
    Card,
    Other,
}

impl PaymentMethod {
    fn as_str(self) -> &'static str {
        match self {
            PaymentMethod::Cash => "cash",
            PaymentMethod::Card => "card",
            PaymentMethod::Other => "other",
        }
    }
}

/// Customer details. Required fields depend on the order type:
/// delivery needs name, phone and address; collection needs a name.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OrderDetails {
    #[serde(default)]
    pub customer_name: Option<String>,
    #[serde(default)]
    pub customer_phone: Option<String>,
    #[serde(default)]
    pub delivery_address: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// The delivery platform's order number, e.g. "#A1B2C".
    #[serde(default)]
    pub external_ref: Option<String>,
    /// Saved customer this order is for (details above are a copy).
    #[serde(default)]
    pub customer_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewOrder {
    pub order_type: OrderType,
    /// Required for dine-in, must be empty otherwise.
    #[serde(default)]
    pub table_id: Option<i64>,
    /// Delivery platform (Uber Eats...). The platform has the customer's
    /// details, so none are required.
    #[serde(default)]
    pub channel_id: Option<i64>,
    #[serde(flatten)]
    pub details: OrderDetails,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemUpdate {
    /// 0 removes the line.
    pub qty: i64,
    #[serde(default)]
    pub discount_cents: i64,
    #[serde(default)]
    pub note: Option<String>,
}

/// What a till knew about a line when it changed it ([`Pos::resolve_item_update`]),
/// so a change made on a stale screen never undoes one made on another till.
/// Every field is optional: left out, the update is taken as it comes.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemCheck {
    /// The quantity the till showed: refused with [`ITEM_CHANGED`] when the
    /// line now has another.
    #[serde(default)]
    pub expected_qty: Option<i64>,
    /// The discount the till showed, likewise.
    #[serde(default)]
    pub expected_discount_cents: Option<i64>,
    /// One more (+1) or one less (-1) than the line has now (the order
    /// screen's + and −): the update's own quantity, discount and note are
    /// ignored; the line's note stays and its discount keeps its share when
    /// it gets fewer. Taps made faster than the till hears back all count.
    /// 0: a new note alone (the update's note), on the line's quantity and
    /// discount as they are now: a note never conflicts with another till's
    /// change of quantity. Its `discount_cents` must be the line's discount
    /// now (else refused: a discount is sent on its own, with its checks).
    #[serde(default)]
    pub qty_delta: Option<i64>,
}

/// The answer when a line is not as the till showed it ([`ItemCheck`]).
pub const ITEM_CHANGED: &str = "This dish changed on another till";
/// The answer when a payment's `expected_left_cents` is not what is left to
/// pay now ([`Pos::add_payment_checked`]).
pub const BILL_CHANGED: &str = "The bill changed on another till";
/// The answer for a change that would make the bill less than its payments.
pub const BELOW_PAID: &str = "That would make the bill less than what is already paid. Take a payment back first";

#[derive(Debug, Clone, Serialize)]
pub struct OrderItem {
    pub id: i64,
    pub product_id: i64,
    pub name: String,
    /// Item price plus its options' prices.
    pub unit_price_cents: i64,
    pub tax_rate_bp: i64,
    pub qty: i64,
    pub discount_cents: i64,
    pub note: Option<String>,
    /// Thai name snapshot, e.g. "ผัดไทย (ไก่)"; None when the menu has none.
    pub name_alt: Option<String>,
    /// The item choice (e.g. "Beef"), when the item has choices.
    pub variant_id: Option<i64>,
    pub options: Vec<ItemOption>,
    /// None until sent to the kitchen.
    pub sent_at: Option<i64>,
    /// A starter: goes to the kitchen before the rest of a dine-in order.
    pub starter: bool,
    /// Prepare ahead: gets a PREPARE NOW ticket while it waits for mains.
    pub prep: bool,
    /// When that PREPARE NOW ticket printed.
    pub prep_sent_at: Option<i64>,
    /// Held back behind the starters. Unsent and held = waiting for
    /// "Call mains"; sent and held = it went out in the "MAINS AWAY" round.
    pub held: bool,
    /// unit price x qty - discount, before tax.
    pub line_total_cents: i64,
    pub tax_cents: i64,
    /// A set menu's line: priced, on the bill, never on kitchen tickets.
    pub is_set: bool,
    /// A dish inside a set: the set's line. Free, on kitchen tickets only.
    pub parent_id: Option<i64>,
    /// The set section it was chosen from, e.g. "Main" / "จานหลัก".
    pub section_name: Option<String>,
    pub section_alt: Option<String>,
    /// What a set's dish comes with (rice, noodles...), when chosen.
    pub with_name: Option<String>,
    pub with_alt: Option<String>,
    /// How many of this line were already paid for with "pay for these dishes".
    pub paid_qty: i64,
    /// The kitchen team this dish was moved to on this order ("Which team
    /// makes this?"); None = its menu item's team. Tickets, other kitchen
    /// printers and the kitchen screen go by it.
    pub team_id: Option<i64>,
}

/// One payment towards an order (an order can have several: split bill).
#[derive(Debug, Clone, Serialize)]
pub struct Payment {
    pub id: i64,
    pub method: String,
    /// Towards the bill.
    pub amount_cents: i64,
    /// On top of the bill; not sales, not taxed.
    pub tip_cents: i64,
    pub tendered_cents: i64,
    pub change_cents: i64,
    /// The dishes it paid for: (order item id, qty); empty for an amount.
    pub items: Vec<(i64, i64)>,
    pub paid_by_name: String,
    pub paid_at: i64,
}

/// A payment to take: the whole rest (no amount, no items), an amount
/// (split evenly or typed), or chosen dishes (their share of the bill).
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentInput {
    pub method: PaymentMethod,
    #[serde(default)]
    pub amount_cents: Option<i64>,
    /// Cash handed over (cash only); ignored for card.
    #[serde(default)]
    pub tendered_cents: i64,
    #[serde(default)]
    pub tip_cents: i64,
    #[serde(default)]
    pub items: Option<Vec<(i64, i64)>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Order {
    pub id: i64,
    pub order_type: OrderType,
    pub status: String,
    pub table_id: Option<i64>,
    pub table_name: Option<String>,
    pub floor_name: Option<String>,
    pub customer_name: Option<String>,
    pub customer_phone: Option<String>,
    pub delivery_address: Option<String>,
    pub notes: Option<String>,
    pub channel_id: Option<i64>,
    pub channel_name: Option<String>,
    pub external_ref: Option<String>,
    pub customer_id: Option<i64>,
    pub opened_by: i64,
    pub opened_by_name: String,
    pub opened_at: i64,
    pub subtotal_cents: i64,
    pub discount_cents: i64,
    pub tax_cents: i64,
    /// Service charge rate on this order (1250 = 12.5%), 0 for none.
    pub service_bp: i64,
    /// Service charge amount: rate x (subtotal - discount), not taxed.
    pub service_cents: i64,
    /// Menu prices on this order include the tax (so `tax_cents` is the part
    /// of the price that is tax and is not added again).
    pub tax_incl: bool,
    /// Tax rate set on this order for every line; None = menu rates.
    pub tax_bp: Option<i64>,
    /// subtotal - discount + tax + service.
    pub total_cents: i64,
    pub payment_method: Option<String>,
    pub tendered_cents: Option<i64>,
    pub change_cents: Option<i64>,
    pub paid_by: Option<i64>,
    pub paid_at: Option<i64>,
    pub closed_at: Option<i64>,
    pub close_reason: Option<String>,
    /// Who cancelled or voided it at the till (History), and who took the payment.
    pub closed_by_name: Option<String>,
    pub paid_by_name: Option<String>,
    /// Whose PIN approved cancelling or voiding it, when not the closer's own.
    pub close_approver_name: Option<String>,
    /// That person's role ("manager", "admin").
    pub close_approver_role: Option<String>,
    pub items: Vec<OrderItem>,
    /// Payments so far (one for a normal order, several for a split bill).
    pub payments: Vec<Payment>,
    /// Sum of the payments' amounts (towards the bill, tips not counted).
    pub paid_cents: i64,
    /// Sum of the tips.
    pub tip_cents: i64,
}

/// One row in the open-orders board or order history.
#[derive(Debug, Clone, Serialize)]
pub struct OrderSummary {
    pub id: i64,
    pub order_type: OrderType,
    pub status: String,
    pub table_name: Option<String>,
    pub floor_name: Option<String>,
    pub customer_name: Option<String>,
    pub customer_phone: Option<String>,
    pub channel_name: Option<String>,
    pub external_ref: Option<String>,
    pub opened_by_name: String,
    pub opened_at: i64,
    pub paid_at: Option<i64>,
    pub total_cents: i64,
    pub item_count: i64,
    /// Lines not yet sent, excluding mains waiting for "Call mains".
    pub unsent_count: i64,
    /// Lines waiting for "Call mains".
    pub held_count: i64,
}

/// Tax on a pre-tax amount, rounded half up (tax added on top of the price).
fn tax_of(amount: i64, rate_bp: i64) -> i64 {
    (amount * rate_bp + 5_000) / 10_000
}

/// The tax inside a price that already includes it, rounded half up:
/// 10.00 at 20% holds 1.67 of tax (10 x 20 / 120) and 8.33 of food.
pub(crate) fn tax_in(amount: i64, rate_bp: i64) -> i64 {
    (amount * rate_bp + (10_000 + rate_bp) / 2) / (10_000 + rate_bp)
}

/// A line's tax: added on top, or taken out of the price.
fn line_tax(amount: i64, rate_bp: i64, included: bool) -> i64 {
    if included {
        tax_in(amount, rate_bp)
    } else {
        tax_of(amount, rate_bp)
    }
}

fn parse_type(s: &str) -> OrderType {
    match s {
        "dine_in" => OrderType::DineIn,
        "collection" => OrderType::Collection,
        "delivery" => OrderType::Delivery,
        _ => OrderType::Takeaway,
    }
}

pub(crate) fn opt_text(v: &Option<String>, max: usize, what: &str) -> Result<Option<String>> {
    let v = v.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(t) = v {
        if t.chars().count() > max {
            return invalid(format!("{what} is too long (max {max} characters)"));
        }
    }
    Ok(v.map(str::to_string))
}

struct CleanDetails {
    name: Option<String>,
    phone: Option<String>,
    address: Option<String>,
    notes: Option<String>,
    external_ref: Option<String>,
}

fn clean_details(t: OrderType, via_platform: bool, d: &OrderDetails) -> Result<CleanDetails> {
    let c = CleanDetails {
        name: opt_text(&d.customer_name, 60, "customer name")?,
        phone: opt_text(&d.customer_phone, 30, "phone number")?,
        address: opt_text(&d.delivery_address, 300, "address")?,
        notes: opt_text(&d.notes, 200, "notes")?,
        external_ref: opt_text(&d.external_ref, 30, "platform order number")?,
    };
    if via_platform {
        return Ok(c);
    }
    match t {
        OrderType::Delivery if c.name.is_none() || c.phone.is_none() || c.address.is_none() => {
            invalid("delivery orders need a customer name, phone number and address")
        }
        OrderType::Collection if c.name.is_none() => invalid("collection orders need a customer name"),
        _ => Ok(c),
    }
}

/// Status of an order, or NotFound.
fn status_of(conn: &Connection, id: i64) -> Result<String> {
    conn.prepare_cached("SELECT status FROM orders WHERE id = ?1")?
        .query_row([id], |r| r.get(0))
        .optional()?
        .ok_or(PosError::NotFound)
}

pub(crate) fn require_open(conn: &Connection, id: i64) -> Result<()> {
    // Swept: cancelled as empty by no one (see `sweep_empty_orders`).
    let (status, swept): (String, bool) = conn
        .prepare_cached("SELECT status, close_reason IS 'empty' AND closed_by IS NULL FROM orders WHERE id = ?1")?
        .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or(PosError::NotFound)?;
    match status.as_str() {
        "open" => Ok(()),
        "cancelled" if swept => invalid(EMPTY_ORDER_SWEPT),
        _ => invalid("this order is already closed"),
    }
}

fn check_table(conn: &Connection, table_id: i64) -> Result<()> {
    let is_table: Option<bool> = conn
        .prepare_cached("SELECT is_table FROM dining_tables WHERE id = ?1 AND active = 1")?
        .query_row([table_id], |r| r.get(0))
        .optional()?;
    match is_table {
        None => return invalid("that table does not exist"),
        Some(false) => return invalid("that is not a table: it is only on the plan to show where something is"),
        Some(true) => {}
    }
    Ok(())
}

const TABLE_BUSY: &str = "that table already has an open order";

const ITEM_COLS: &str =
    "id, product_id, name, unit_price_cents, tax_rate_bp, qty, discount_cents, note, sent_at, variant_id, starter, held, name_alt, prep, prep_sent_at, is_set, parent_id, section_name, section_alt, with_name, with_alt, team_id";

fn item_from_row(r: &Row) -> rusqlite::Result<OrderItem> {
    let (price, rate, qty, disc): (i64, i64, i64, i64) = (r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?);
    let net = price * qty - disc;
    Ok(OrderItem {
        id: r.get(0)?,
        product_id: r.get(1)?,
        name: r.get(2)?,
        unit_price_cents: price,
        tax_rate_bp: rate,
        qty,
        discount_cents: disc,
        note: r.get(7)?,
        variant_id: r.get(9)?,
        options: Vec::new(),
        sent_at: r.get(8)?,
        starter: r.get(10)?,
        held: r.get(11)?,
        name_alt: r.get(12)?,
        prep: r.get(13)?,
        prep_sent_at: r.get(14)?,
        line_total_cents: net,
        tax_cents: tax_of(net, rate),
        is_set: r.get(15)?,
        parent_id: r.get(16)?,
        section_name: r.get(17)?,
        section_alt: r.get(18)?,
        with_name: r.get(19)?,
        with_alt: r.get(20)?,
        paid_qty: 0,
        team_id: r.get(21)?,
    })
}

fn load_items(conn: &Connection, order_id: i64) -> Result<Vec<OrderItem>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {ITEM_COLS} FROM order_items WHERE order_id = ?1 ORDER BY id"))?;
    let mut items: Vec<OrderItem> = stmt.query_map([order_id], item_from_row)?.collect::<rusqlite::Result<_>>()?;
    let included: bool =
        conn.query_row("SELECT tax_incl FROM orders WHERE id = ?1", [order_id], |r| r.get(0)).optional()?.unwrap_or(false);
    for it in &mut items {
        it.tax_cents = line_tax(it.line_total_cents, it.tax_rate_bp, included);
    }
    let mut opts = conn.prepare_cached(
        "SELECT o.item_id, o.option_id, o.group_name, o.name, o.price_cents, o.name_alt
         FROM order_item_options o JOIN order_items i ON i.id = o.item_id
         WHERE i.order_id = ?1 ORDER BY o.id",
    )?;
    let rows = opts.query_map([order_id], |r| {
        Ok((r.get::<_, i64>(0)?, ItemOption { option_id: r.get(1)?, group_name: r.get(2)?, name: r.get(3)?, price_cents: r.get(4)?, name_alt: r.get(5)? }))
    })?;
    for row in rows {
        let (item_id, o) = row?;
        if let Some(it) = items.iter_mut().find(|i| i.id == item_id) {
            it.options.push(o);
        }
    }
    Ok(items)
}

fn load_payments(conn: &Connection, order_id: i64) -> Result<Vec<Payment>> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.id, p.method, p.amount_cents, p.tip_cents, p.tendered_cents, p.change_cents, p.items, u.name, p.paid_at
         FROM payments p JOIN users u ON u.id = p.paid_by WHERE p.order_id = ?1 ORDER BY p.id",
    )?;
    let rows = stmt.query_map([order_id], |r| {
        let items: Option<String> = r.get(6)?;
        Ok(Payment {
            id: r.get(0)?,
            method: r.get(1)?,
            amount_cents: r.get(2)?,
            tip_cents: r.get(3)?,
            tendered_cents: r.get(4)?,
            change_cents: r.get(5)?,
            items: items.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
            paid_by_name: r.get(7)?,
            paid_at: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// What a customer pays for a line, without the service charge.
fn line_due(it: &OrderItem, tax_incl: bool) -> i64 {
    it.line_total_cents + if tax_incl { 0 } else { it.tax_cents }
}

/// Option ids on an order line, sorted, for comparing lines.
fn line_option_ids(conn: &Connection, item_id: i64) -> Result<Vec<i64>> {
    let mut ids: Vec<i64> = conn
        .prepare_cached("SELECT option_id FROM order_item_options WHERE item_id = ?1")?
        .query_map([item_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    ids.sort_unstable();
    Ok(ids)
}

struct Totals {
    subtotal: i64,
    discount: i64,
    tax: i64,
    service: i64,
    total: i64,
}

fn totals(items: &[OrderItem], service_bp: i64, tax_incl: bool) -> Totals {
    let subtotal: i64 = items.iter().map(|i| i.unit_price_cents * i.qty).sum();
    let discount: i64 = items.iter().map(|i| i.discount_cents).sum();
    let tax = items.iter().map(|i| i.tax_cents).sum();
    let service = service_of(subtotal - discount, service_bp);
    // Included tax is already in the prices; otherwise it is added on top.
    Totals { subtotal, discount, tax, service, total: subtotal - discount + if tax_incl { 0 } else { tax } + service }
}

pub(crate) const SUMMARY_SQL: &str = "
    SELECT o.id, o.order_type, o.status, t.name, f.name, o.customer_name, o.customer_phone, u.name,
           o.opened_at, o.paid_at, ch.name, o.external_ref,
           COALESCE(o.total_cents, (
               SELECT SUM(i.unit_price_cents * i.qty - i.discount_cents
                          + CASE WHEN o.tax_incl = 1 THEN 0
                                 ELSE ((i.unit_price_cents * i.qty - i.discount_cents) * i.tax_rate_bp + 5000) / 10000 END)
                      + (SUM(i.unit_price_cents * i.qty - i.discount_cents) * o.service_bp + 5000) / 10000
               FROM order_items i WHERE i.order_id = o.id), 0),
           (SELECT COALESCE(SUM(qty), 0) FROM order_items i WHERE i.order_id = o.id),
           (SELECT COUNT(*) FROM order_items i WHERE i.order_id = o.id AND i.sent_at IS NULL AND i.held = 0 AND i.is_set = 0),
           (SELECT COUNT(*) FROM order_items i WHERE i.order_id = o.id AND i.sent_at IS NULL AND i.held = 1)
    FROM orders o
    JOIN users u ON u.id = o.opened_by
    LEFT JOIN dining_tables t ON t.id = o.table_id
    LEFT JOIN floors f ON f.id = t.floor_id
    LEFT JOIN delivery_channels ch ON ch.id = o.channel_id";

pub(crate) fn summary_from_row(r: &Row) -> rusqlite::Result<OrderSummary> {
    Ok(OrderSummary {
        id: r.get(0)?,
        order_type: parse_type(&r.get::<_, String>(1)?),
        status: r.get(2)?,
        table_name: r.get(3)?,
        floor_name: r.get(4)?,
        customer_name: r.get(5)?,
        customer_phone: r.get(6)?,
        opened_by_name: r.get(7)?,
        opened_at: r.get(8)?,
        paid_at: r.get(9)?,
        channel_name: r.get(10)?,
        external_ref: r.get(11)?,
        total_cents: r.get(12)?,
        item_count: r.get(13)?,
        unsent_count: r.get(14)?,
        held_count: r.get(15)?,
    })
}

/// The order on `table_id` that dishes from `order_id` move to (opened if
/// the table is free, with the same service charge and tax as the source).
fn move_target(tx: &Transaction, s: &Session, order_id: i64, table_id: i64) -> Result<i64> {
    require_open(tx, order_id)?;
    let paid: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM payments WHERE order_id = ?1)", [order_id], |r| r.get(0))?;
    if paid {
        return invalid("this order has part payments: finish paying it first");
    }
    let here: Option<i64> = tx.query_row("SELECT table_id FROM orders WHERE id = ?1", [order_id], |r| r.get(0))?;
    if here == Some(table_id) {
        return invalid("that is the same table");
    }
    let open: Option<i64> =
        tx.query_row("SELECT id FROM orders WHERE table_id = ?1 AND status = 'open'", [table_id], |r| r.get(0)).optional()?;
    if let Some(id) = open {
        return Ok(id);
    }
    check_table(tx, table_id)?;
    tx.execute(
        "INSERT INTO orders (order_type, table_id, opened_by, service_bp, tax_incl, tax_bp)
         SELECT 'dine_in', ?2, ?3, service_bp, tax_incl, tax_bp FROM orders WHERE id = ?1",
        params![order_id, table_id, s.user_id],
    )
    .map_err(|e| map_unique(e, TABLE_BUSY))?;
    Ok(tx.last_insert_rowid())
}

/// A copy of an order line (with its options) on `order_id`, `qty` of it.
fn copy_line(tx: &Transaction, item: i64, order_id: i64, qty: i64, discount: i64, parent: Option<i64>) -> Result<i64> {
    tx.execute(
        "INSERT INTO order_items (order_id, product_id, name, unit_price_cents, tax_rate_bp, qty, discount_cents, note, sent_at,
                                  added_by, added_at, name_alt, starter, held, variant_id, prep, prep_sent_at, is_set,
                                  parent_id, section_name, section_alt, with_name, with_alt, team_id, menu_price_cents)
         SELECT ?2, product_id, name, unit_price_cents, tax_rate_bp, ?3, ?4, note, sent_at, added_by, added_at, name_alt, starter,
                held, variant_id, prep, prep_sent_at, is_set, COALESCE(?5, parent_id), section_name, section_alt, with_name, with_alt, team_id,
                menu_price_cents
         FROM order_items WHERE id = ?1",
        params![item, order_id, qty, discount, parent],
    )?;
    let new = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO order_item_options (item_id, option_id, group_name, name, price_cents, name_alt)
         SELECT ?2, option_id, group_name, name, price_cents, name_alt FROM order_item_options WHERE item_id = ?1",
        params![item, new],
    )?;
    Ok(new)
}

/// Who answers for dishes moving from order `from` onto the bill `to`
/// when it charges less for them: a lower service charge, or a lower tax
/// on any priced dish that goes (`tax_lowered`, see [`tax_lowered`]). A
/// manager's or admin's say-so, as lowering either on the bill itself is
/// (see [`approved_by`]); None when the move costs nothing.
fn move_approval<'a>(tx: &Transaction, s: &'a Session, approver: Option<&'a Session>, from: i64, to: i64, tax_lowered: bool) -> Result<Option<&'a Session>> {
    let service = |id: i64| tx.query_row("SELECT COALESCE(service_bp, 0) FROM orders WHERE id = ?1", [id], |r| r.get::<_, i64>(0));
    let what = if service(to)? < service(from)? {
        MOVE_SERVICE_NEEDS_PIN
    } else if tax_lowered {
        MOVE_TAX_NEEDS_PIN
    } else {
        return Ok(None);
    };
    approved_by(s, approver, MONEY_APPROVAL, what).map(Some)
}

/// The tax rates of the priced lines among `ids` (None for the others), to
/// compare before and after [`take_order_tax`].
fn priced_rates(tx: &Transaction, ids: &[i64]) -> Result<Vec<Option<i64>>> {
    let mut q = tx.prepare_cached("SELECT tax_rate_bp FROM order_items WHERE id = ?1 AND unit_price_cents > 0")?;
    ids.iter().map(|id| Ok(q.query_row([id], |r| r.get(0)).optional()?)).collect()
}

/// True when a priced line pays less tax `after` a move than `before`
/// (both from [`priced_rates`]): onto a bill with a lower rate of its own,
/// or from a bill with a rate of its own onto one at the menu's rates.
fn tax_lowered(before: &[Option<i64>], after: &[Option<i64>]) -> bool {
    before.iter().zip(after).any(|(b, a)| a.zip(*b).is_some_and(|(a, b)| a < b))
}

/// For the audit log: " (PIN of Quinn #3, manager)" after a move onto a
/// bill that charges less ([`move_approval`]); nothing otherwise.
fn move_note(s: &Session, by: Option<&Session>) -> String {
    by.map(|by| format!(" {}", approval_note(s, by))).unwrap_or_default()
}

/// Lines `moved` from order `from` onto order `to` take its tax: its own
/// rate for the whole order (as dishes added to it do), or the menu's
/// rates when it has none and `from` had one.
fn take_order_tax(tx: &Transaction, from: i64, to: i64, moved: &[i64]) -> Result<()> {
    let tax = |id: i64| tx.query_row("SELECT tax_bp FROM orders WHERE id = ?1", [id], |r| r.get::<_, Option<i64>>(0));
    let (was, now) = (tax(from)?, tax(to)?);
    if was == now {
        return Ok(());
    }
    let mut set = tx.prepare_cached(
        "UPDATE order_items SET tax_rate_bp = COALESCE(?2, (SELECT p.tax_rate_bp FROM products p WHERE p.id = order_items.product_id), tax_rate_bp)
         WHERE id = ?1",
    )?;
    for id in moved {
        set.execute(params![id, now])?;
    }
    Ok(())
}

/// Remove an open order with no dishes left (after moving them away).
fn drop_if_empty(tx: &Transaction, order_id: i64) -> Result<()> {
    let left: i64 = tx.query_row("SELECT COUNT(*) FROM order_items WHERE order_id = ?1", [order_id], |r| r.get(0))?;
    if left == 0 {
        tx.execute("DELETE FROM orders WHERE id = ?1 AND status = 'open'", [order_id])?;
    }
    Ok(())
}

/// After a change in `tx`: refused ([`BELOW_PAID`]) when the order's total
/// is now less than its payments (dishes removed or discounted, service or
/// tax cut after a part payment), so it never closes overpaid.
fn check_covers_paid(tx: &Transaction, order_id: i64) -> Result<()> {
    let paid: i64 = tx.query_row("SELECT COALESCE(SUM(amount_cents), 0) FROM payments WHERE order_id = ?1", [order_id], |r| r.get(0))?;
    if paid == 0 {
        return Ok(());
    }
    let (service_bp, tax_incl): (i64, bool) =
        tx.query_row("SELECT service_bp, tax_incl FROM orders WHERE id = ?1", [order_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if totals(&load_items(tx, order_id)?, service_bp, tax_incl).total < paid {
        return invalid(BELOW_PAID);
    }
    Ok(())
}

/// Close an order its payments cover: totals, the payment summary (one
/// method, or NULL when mixed) and stock for tracked items.
fn close_paid(tx: &Transaction, s: &Session, order_id: i64, t: &Totals) -> Result<()> {
    let (methods, method, tendered, change, tips): (i64, Option<String>, i64, i64, i64) = tx.query_row(
        "SELECT COUNT(DISTINCT method), MIN(method), COALESCE(SUM(tendered_cents), 0), COALESCE(SUM(change_cents), 0), COALESCE(SUM(tip_cents), 0)
         FROM payments WHERE order_id = ?1",
        [order_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let method = if methods == 1 { method } else { None };
    tx.prepare_cached(
        "UPDATE orders SET status = 'paid', subtotal_cents = ?2, discount_cents = ?3, tax_cents = ?4,
             total_cents = ?5, payment_method = ?6, tendered_cents = ?7, change_cents = ?8,
             paid_by = ?9, paid_at = unixepoch(), service_cents = ?10, tip_cents = ?11
         WHERE id = ?1",
    )?
    .execute(params![order_id, t.subtotal, t.discount, t.tax, t.total, method, tendered, change, s.user_id, t.service, tips])?;
    // Stock may go negative: service is never blocked by a miscount.
    tx.prepare_cached(
        "INSERT INTO stock_movements (product_id, delta, reason, order_id, user_id)
         SELECT i.product_id, -SUM(i.qty), 'sale', ?1, ?2
         FROM order_items i JOIN products p ON p.id = i.product_id
         WHERE i.order_id = ?1 AND p.track_stock = 1 GROUP BY i.product_id",
    )?
    .execute(params![order_id, s.user_id])?;
    tx.prepare_cached(
        "UPDATE products SET stock_qty = stock_qty + (
             SELECT m.delta FROM stock_movements m
             WHERE m.order_id = ?1 AND m.reason = 'sale' AND m.product_id = products.id)
         WHERE id IN (SELECT product_id FROM stock_movements WHERE order_id = ?1 AND reason = 'sale')",
    )?
    .execute([order_id])?;
    Ok(())
}

pub(crate) fn audit_tx(tx: &Transaction, user_id: i64, action: &str, detail: &str) -> Result<()> {
    crate::activity::insert(tx, Some(user_id), action, detail, None)
}

/// Who answers for a change to the money on an order that needs `p` (a
/// manager's or an admin's say-so): `s`, logged in on the till, when their
/// role has it; else `approver`, the owner of a PIN typed on the till for
/// it. Refused as "`what` needs a manager PIN" when no PIN came, and as
/// "`what` needs a manager's or admin's PIN" when the PIN's owner may not
/// either (the till asks for a PIN on "needs a manager").
pub(crate) fn approved_by<'a>(s: &'a Session, approver: Option<&'a Session>, p: Permission, what: &str) -> Result<&'a Session> {
    if s.role.can(p) {
        return Ok(s);
    }
    match approver {
        None => invalid(format!("{what} needs a manager PIN")),
        Some(a) if !a.role.can(p) => invalid(format!("{what} needs a manager's or admin's PIN")),
        Some(a) => Ok(a),
    }
}

/// How `by` (from [`approved_by`]) approved what `s` did, for the audit
/// log: "(no PIN, manager)", or "(PIN of Quinn #3, manager)".
pub(crate) fn approval_note(s: &Session, by: &Session) -> String {
    if by.user_id == s.user_id {
        format!("(no PIN, {})", s.role.as_str())
    } else {
        format!("(PIN of {} #{}, {})", by.name, by.user_id, by.role.as_str())
    }
}

/// Removing or lowering the service charge, or setting a tax rate for the
/// whole order: a manager's or admin's say-so, like a discount over the
/// staff limit.
const MONEY_APPROVAL: Permission = Permission::AnyDiscount;
const SERVICE_NEEDS_PIN: &str = "removing or lowering the service charge";
const TAX_NEEDS_PIN: &str = "changing the tax on an order";
const PAYMENT_NEEDS_PIN: &str = "taking a payment back";
/// Fewer of a dish already sent to the kitchen (or removing it): staff need
/// a manager's or admin's PIN ([`Permission::CancelWithoutPin`]), or a
/// cashier could take the dishes off, then cancel the empty order freely.
const VOID_NEEDS_PIN: &str = "removing a dish already sent to the kitchen";
/// A dish not on the menu at a typed price: the same say-so as a changed
/// price ([`Pos::set_item_price`]).
const OPEN_ITEM_NEEDS_PIN: &str = "a dish not on the menu";
/// A payment by "Other" on an order not from a delivery app: nothing is
/// taken at the till, so it is a manager's or admin's say-so.
const OTHER_PAY_NEEDS_PIN: &str = "a payment by Other (not a delivery app)";
/// Dishes moved (or a bill joined) onto a bill with a lower service charge
/// or tax: they would take its rate, the same say-so as lowering it there.
const MOVE_SERVICE_NEEDS_PIN: &str = "moving dishes to a bill with a lower service charge";
const MOVE_TAX_NEEDS_PIN: &str = "moving dishes to a bill with a lower tax";

impl Pos {
    pub fn open_order(&self, s: &Session, new: &NewOrder) -> Result<Order> {
        s.require(Permission::Sell)?;
        let d = clean_details(new.order_type, new.channel_id.is_some(), &new.details)?;
        match (new.order_type, new.table_id) {
            (OrderType::DineIn, Some(t)) => check_table(&self.conn, t)?,
            (OrderType::DineIn, None) => return invalid("choose a table for a dine-in order"),
            (_, Some(_)) => return invalid("only dine-in orders have a table"),
            (_, None) => {}
        }
        if let Some(ch) = new.channel_id {
            if new.order_type == OrderType::DineIn {
                return invalid("dine-in orders cannot come from a delivery platform");
            }
            self.check_channel(ch)?;
        }
        let service_bp = self.service_rate_for(new.order_type)?;
        let tax_incl = self.prices_include_tax()?;
        let tx = self.conn.unchecked_transaction()?;
        if let Some(c) = new.details.customer_id {
            self.touch_customer(c)?;
        }
        tx.prepare_cached(
            "INSERT INTO orders (order_type, table_id, customer_name, customer_phone, delivery_address, notes,
                                 channel_id, external_ref, customer_id, opened_by, service_bp, tax_incl)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?
        .execute(params![
            new.order_type.as_str(),
            new.table_id,
            d.name,
            d.phone,
            d.address,
            d.notes,
            new.channel_id,
            d.external_ref,
            new.details.customer_id,
            s.user_id,
            service_bp,
            tax_incl
        ])
        .map_err(|e| map_unique(e, TABLE_BUSY))?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.get_order(s, id)
    }

    /// The open order on a table, if any (used when a table is tapped).
    pub fn table_order(&self, s: &Session, table_id: i64) -> Result<Option<Order>> {
        s.require(Permission::Sell)?;
        let id: Option<i64> = self
            .conn
            .prepare_cached("SELECT id FROM orders WHERE table_id = ?1 AND status = 'open'")?
            .query_row([table_id], |r| r.get(0))
            .optional()?;
        id.map(|id| self.get_order(s, id)).transpose()
    }

    pub fn get_order(&self, s: &Session, id: i64) -> Result<Order> {
        let mut o = self
            .conn
            .prepare_cached(
                "SELECT o.id, o.order_type, o.status, o.table_id, t.name, f.name, o.customer_name, o.customer_phone,
                        o.delivery_address, o.notes, o.opened_by, u.name, o.opened_at, o.payment_method,
                        o.tendered_cents, o.change_cents, o.paid_by, o.paid_at, o.closed_at, o.close_reason,
                        o.channel_id, ch.name, o.external_ref, o.customer_id, o.service_bp, o.tax_bp, o.tax_incl,
                        cu.name, pu.name
                 FROM orders o JOIN users u ON u.id = o.opened_by
                 LEFT JOIN users cu ON cu.id = o.closed_by
                 LEFT JOIN users pu ON pu.id = o.paid_by
                 LEFT JOIN dining_tables t ON t.id = o.table_id
                 LEFT JOIN floors f ON f.id = t.floor_id
                 LEFT JOIN delivery_channels ch ON ch.id = o.channel_id
                 WHERE o.id = ?1",
            )?
            .query_row([id], |r| {
                Ok(Order {
                    id: r.get(0)?,
                    order_type: parse_type(&r.get::<_, String>(1)?),
                    status: r.get(2)?,
                    table_id: r.get(3)?,
                    table_name: r.get(4)?,
                    floor_name: r.get(5)?,
                    customer_name: r.get(6)?,
                    customer_phone: r.get(7)?,
                    delivery_address: r.get(8)?,
                    notes: r.get(9)?,
                    opened_by: r.get(10)?,
                    opened_by_name: r.get(11)?,
                    opened_at: r.get(12)?,
                    payment_method: r.get(13)?,
                    tendered_cents: r.get(14)?,
                    change_cents: r.get(15)?,
                    paid_by: r.get(16)?,
                    paid_at: r.get(17)?,
                    closed_at: r.get(18)?,
                    close_reason: r.get(19)?,
                    channel_id: r.get(20)?,
                    channel_name: r.get(21)?,
                    external_ref: r.get(22)?,
                    customer_id: r.get(23)?,
                    service_bp: r.get(24)?,
                    tax_bp: r.get(25)?,
                    tax_incl: r.get(26)?,
                    closed_by_name: r.get(27)?,
                    paid_by_name: r.get(28)?,
                    close_approver_name: None,
                    close_approver_role: None,
                    subtotal_cents: 0,
                    discount_cents: 0,
                    tax_cents: 0,
                    service_cents: 0,
                    total_cents: 0,
                    items: Vec::new(),
                    payments: Vec::new(),
                    paid_cents: 0,
                    tip_cents: 0,
                })
            })
            .optional()?
            .ok_or(PosError::NotFound)?;
        // Open orders are shared by the whole floor team; closed ones are
        // visible to admins, or to staff who opened or took payment for them.
        if o.status == "open" {
            s.require(Permission::Sell)?;
        } else if !s.role.can(Permission::ViewAllOrders) {
            s.require(Permission::ViewOwnOrders)?;
            if o.opened_by != s.user_id && o.paid_by != Some(s.user_id) {
                return Err(PosError::Forbidden);
            }
        }
        if matches!(o.status.as_str(), "cancelled" | "voided") {
            // With someone else's PIN: the person at the till closed it, the PIN approved it.
            if let Some((by, approver, role)) = self.close_approver(id)? {
                o.closed_by_name = by.or(o.closed_by_name);
                o.close_approver_name = Some(approver);
                o.close_approver_role = Some(role);
            }
        }
        o.items = load_items(&self.conn, id)?;
        o.payments = load_payments(&self.conn, id)?;
        o.paid_cents = o.payments.iter().map(|p| p.amount_cents).sum();
        o.tip_cents = o.payments.iter().map(|p| p.tip_cents).sum();
        for p in &o.payments {
            for (item, qty) in &p.items {
                if let Some(it) = o.items.iter_mut().find(|i| i.id == *item) {
                    it.paid_qty += qty;
                }
            }
        }
        let t = totals(&o.items, o.service_bp, o.tax_incl);
        (o.subtotal_cents, o.discount_cents, o.tax_cents, o.service_cents, o.total_cents) =
            (t.subtotal, t.discount, t.tax, t.service, t.total);
        Ok(o)
    }

    /// [`Pos::update_order_details`] changing only `fields` (customer_name,
    /// customer_phone, delivery_address, notes, external_ref, customer_id):
    /// the others keep what they have now, so two tills editing different
    /// details never undo each other. None: every field, as before.
    pub fn update_order_details_fields(&self, s: &Session, id: i64, details: &OrderDetails, fields: Option<&[String]>) -> Result<Order> {
        let Some(fields) = fields else { return self.update_order_details(s, id, details) };
        s.require(Permission::Sell)?;
        require_open(&self.conn, id)?;
        let mut d: OrderDetails = self.conn.query_row(
            "SELECT customer_name, customer_phone, delivery_address, notes, external_ref, customer_id FROM orders WHERE id = ?1",
            [id],
            |r| Ok(OrderDetails { customer_name: r.get(0)?, customer_phone: r.get(1)?, delivery_address: r.get(2)?, notes: r.get(3)?, external_ref: r.get(4)?, customer_id: r.get(5)? }),
        )?;
        for f in fields {
            match f.as_str() {
                "customer_name" => d.customer_name = details.customer_name.clone(),
                "customer_phone" => d.customer_phone = details.customer_phone.clone(),
                "delivery_address" => d.delivery_address = details.delivery_address.clone(),
                "notes" => d.notes = details.notes.clone(),
                "external_ref" => d.external_ref = details.external_ref.clone(),
                "customer_id" => d.customer_id = details.customer_id,
                other => return invalid(format!("unknown order detail {other}")),
            }
        }
        self.update_order_details(s, id, &d)
    }

    pub fn update_order_details(&self, s: &Session, id: i64, details: &OrderDetails) -> Result<Order> {
        s.require(Permission::Sell)?;
        require_open(&self.conn, id)?;
        let (t, channel, customer): (String, Option<i64>, Option<i64>) = self.conn.query_row(
            "SELECT order_type, channel_id, customer_id FROM orders WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let d = clean_details(parse_type(&t), channel.is_some(), details)?;
        let tx = self.conn.unchecked_transaction()?;
        if let Some(c) = details.customer_id.filter(|c| Some(*c) != customer) {
            self.touch_customer(c)?;
        }
        tx.execute(
            "UPDATE orders SET customer_name = ?2, customer_phone = ?3, delivery_address = ?4, notes = ?5,
                 external_ref = ?6, customer_id = ?7
             WHERE id = ?1",
            params![id, d.name, d.phone, d.address, d.notes, d.external_ref, details.customer_id],
        )?;
        tx.commit()?;
        self.get_order(s, id)
    }

    /// Where an order that no longer exists went: the open bill its dishes
    /// were last joined or moved onto (the till says "Moved to Table 5"),
    /// or None.
    pub fn moved_to(&self, s: &Session, order_id: i64) -> Result<Option<Order>> {
        s.require(Permission::Sell)?;
        let gone: bool = self.conn.query_row("SELECT NOT EXISTS (SELECT 1 FROM orders WHERE id = ?1)", [order_id], |r| r.get(0))?;
        if !gone {
            return Ok(None);
        }
        let detail: Option<String> = self
            .conn
            .query_row(
                "SELECT detail FROM audit_log WHERE order_id = ?1 AND action IN ('order.merge', 'order.move_items') ORDER BY id DESC LIMIT 1",
                [order_id],
                |r| r.get(0),
            )
            .optional()?;
        let prefix = format!("{order_id} -> order ");
        let target = detail
            .as_deref()
            .and_then(|d| d.strip_prefix(&prefix))
            .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse::<i64>().ok());
        let Some(target) = target else { return Ok(None) };
        match self.get_order(s, target) {
            Ok(o) if o.status == "open" => Ok(Some(o)),
            Ok(_) | Err(PosError::NotFound) | Err(PosError::Forbidden) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Move a dine-in order to another (free) table, on any floor.
    pub fn move_order(&self, s: &Session, id: i64, table_id: i64) -> Result<Order> {
        s.require(Permission::Sell)?;
        require_open(&self.conn, id)?;
        check_table(&self.conn, table_id)?;
        let changed = self
            .conn
            .execute(
                "UPDATE orders SET table_id = ?2 WHERE id = ?1 AND order_type = 'dine_in'",
                params![id, table_id],
            )
            .map_err(|e| map_unique(e, TABLE_BUSY))?;
        if changed == 0 {
            return invalid("only dine-in orders can move tables");
        }
        self.audit(Some(s.user_id), "order.move", &format!("{id} -> table {table_id}"))?;
        self.get_order(s, id)
    }

    /// Move dishes to another table: `items` (order line, how many) go to
    /// that table's open order, or a new one if the table is free. A set's
    /// dishes go with it. Sent dishes stay sent (no new kitchen ticket). An
    /// order left empty is removed. Returns the order they went to. The
    /// dishes take that order's service charge and tax: onto a bill that
    /// charges less for them, staff need a manager's or admin's PIN (see
    /// [`Pos::move_items_approved`]).
    pub fn move_items(&mut self, s: &Session, order_id: i64, table_id: i64, items: &[(i64, i64)]) -> Result<Order> {
        self.move_items_approved(s, None, order_id, table_id, items)
    }

    /// [`Pos::move_items`], with `approver` (the owner of a manager's or
    /// admin's PIN typed on the till) allowing staff to move dishes onto a
    /// bill with a lower service charge or tax. The audit log says who
    /// approved it.
    pub fn move_items_approved(&mut self, s: &Session, approver: Option<&Session>, order_id: i64, table_id: i64, items: &[(i64, i64)]) -> Result<Order> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let target = move_target(&tx, s, order_id, table_id)?;
        let lines = load_items(&tx, order_id)?;
        let mut moved = 0;
        // The lines now on the target order.
        let mut ids = Vec::new();
        for &(item, q) in items {
            if q <= 0 {
                continue;
            }
            let it = lines
                .iter()
                .find(|i| i.id == item && i.parent_id.is_none())
                .ok_or_else(|| PosError::Validation("that dish is not on this order".into()))?;
            let q = q.min(it.qty);
            if q == it.qty {
                tx.execute("UPDATE order_items SET order_id = ?2 WHERE id = ?1 OR parent_id = ?1", params![item, target])?;
                ids.extend(lines.iter().filter(|c| c.id == item || c.parent_id == Some(item)).map(|c| c.id));
            } else {
                let new = copy_line(&tx, item, target, q, it.discount_cents * q / it.qty, None)?;
                ids.push(new);
                tx.execute(
                    "UPDATE order_items SET qty = qty - ?2, discount_cents = discount_cents - ?3 WHERE id = ?1",
                    params![item, q, it.discount_cents * q / it.qty],
                )?;
                // Each dish of a set keeps its count per set.
                for c in lines.iter().filter(|c| c.parent_id == Some(item)) {
                    let part = c.qty / it.qty * q;
                    ids.push(copy_line(&tx, c.id, target, part, 0, Some(new))?);
                    tx.execute("UPDATE order_items SET qty = qty - ?2 WHERE id = ?1", params![c.id, part])?;
                }
            }
            moved += q;
        }
        if moved == 0 {
            return invalid("choose the dishes to move");
        }
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM order_items WHERE order_id = ?1", [target], |r| r.get(0))?;
        if count > MAX_ITEMS {
            return invalid("that table's order would have too many lines");
        }
        let before = priced_rates(&tx, &ids)?;
        take_order_tax(&tx, order_id, target, &ids)?;
        let by = move_approval(&tx, s, approver, order_id, target, tax_lowered(&before, &priced_rates(&tx, &ids)?))?;
        drop_if_empty(&tx, order_id)?;
        audit_tx(&tx, s.user_id, "order.move_items", &format!("{order_id} -> order {target}: {}{}", crate::plural(moved, "dish", "dishes"), move_note(s, by)))?;
        tx.commit()?;
        self.get_order(s, target)
    }

    /// Put a whole order onto another table. If that table already has an
    /// order, the two become one bill (this order is removed), with that
    /// bill's service charge and tax: when it charges less, staff need a
    /// manager's or admin's PIN (see [`Pos::merge_into_table_approved`]).
    /// Otherwise the order just moves there.
    pub fn merge_into_table(&mut self, s: &Session, order_id: i64, table_id: i64) -> Result<Order> {
        self.merge_into_table_approved(s, None, order_id, table_id)
    }

    /// [`Pos::merge_into_table`], with `approver` (the owner of a manager's
    /// or admin's PIN typed on the till) allowing staff to join a bill with
    /// a lower service charge or tax. The audit log says who approved it.
    pub fn merge_into_table_approved(&mut self, s: &Session, approver: Option<&Session>, order_id: i64, table_id: i64) -> Result<Order> {
        s.require(Permission::Sell)?;
        let busy: Option<i64> = self
            .conn
            .query_row("SELECT id FROM orders WHERE table_id = ?1 AND status = 'open'", [table_id], |r| r.get(0))
            .optional()?;
        if busy.is_none() {
            return self.move_order(s, order_id, table_id);
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let target = move_target(&tx, s, order_id, table_id)?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM order_items WHERE order_id IN (?1, ?2)",
            params![order_id, target],
            |r| r.get(0),
        )?;
        if count > MAX_ITEMS {
            return invalid("the joined bill would have too many lines");
        }
        let ids: Vec<i64> = tx
            .prepare("SELECT id FROM order_items WHERE order_id = ?1")?
            .query_map([order_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let before = priced_rates(&tx, &ids)?;
        tx.execute("UPDATE order_items SET order_id = ?2 WHERE order_id = ?1", params![order_id, target])?;
        take_order_tax(&tx, order_id, target, &ids)?;
        let by = move_approval(&tx, s, approver, order_id, target, tax_lowered(&before, &priced_rates(&tx, &ids)?))?;
        drop_if_empty(&tx, order_id)?;
        audit_tx(&tx, s.user_id, "order.merge", &format!("{order_id} -> order {target}{}", move_note(s, by)))?;
        tx.commit()?;
        self.get_order(s, target)
    }

    /// Add a menu item at its current price. Items with choices (e.g. soup:
    /// chicken / beef) need `variant_id`, whose price replaces the item's;
    /// options (validated against the item's option groups) add to it.
    /// Repeated taps on the same item, choice and options (no note, not yet
    /// sent) increase the quantity of one line.
    #[allow(clippy::too_many_arguments)]
    pub fn add_item(
        &mut self,
        s: &Session,
        order_id: i64,
        product_id: i64,
        qty: i64,
        note: Option<&str>,
        variant_id: Option<i64>,
        option_ids: &[i64],
    ) -> Result<Order> {
        self.add_item_as(s, order_id, product_id, qty, note, variant_id, option_ids, None)
    }

    /// [`Pos::add_item`] choosing the course: `starter` Some(false) orders a
    /// starter dish as a main (and Some(true) any dish as a starter); None
    /// keeps the menu's setting.
    #[allow(clippy::too_many_arguments)]
    pub fn add_item_as(
        &mut self,
        s: &Session,
        order_id: i64,
        product_id: i64,
        qty: i64,
        note: Option<&str>,
        variant_id: Option<i64>,
        option_ids: &[i64],
        starter: Option<bool>,
    ) -> Result<Order> {
        self.add_item_for(s, order_id, product_id, qty, note, variant_id, option_ids, starter, None)
    }

    /// [`Pos::add_item_as`] made by kitchen team `team_id` ("Which team makes
    /// this?"): it prints under that team and on its printer, and shows
    /// under it on the kitchen screen. None, or the item's own team = the
    /// menu's team, as always.
    #[allow(clippy::too_many_arguments)]
    pub fn add_item_for(
        &mut self,
        s: &Session,
        order_id: i64,
        product_id: i64,
        qty: i64,
        note: Option<&str>,
        variant_id: Option<i64>,
        option_ids: &[i64],
        starter: Option<bool>,
        team_id: Option<i64>,
    ) -> Result<Order> {
        s.require(Permission::Sell)?;
        if !(1..=MAX_QTY).contains(&qty) {
            return invalid(format!("quantity must be between 1 and {MAX_QTY}"));
        }
        let note = opt_text(&note.map(str::to_string), 100, "note")?;
        self.check_team(team_id)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        type ProductRow = (String, i64, i64, bool, Option<String>, bool, bool, Option<i64>);
        let (name, base, rate, starter_default, alt, prep, sold_out, menu_team): ProductRow = tx
            .prepare_cached("SELECT name, price_cents, tax_rate_bp, starter, name_alt, prep, sold_out, team_id FROM products WHERE id = ?1 AND active = 1 AND is_set = 0")?
            .query_row([product_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))
            .optional()?
            .ok_or_else(|| PosError::Validation("that item is not available (a set menu needs its dishes chosen)".into()))?;
        if sold_out {
            return invalid(format!("{name} is sold out today"));
        }
        let starter = starter.unwrap_or(starter_default);
        // Its own team is no move: the line keeps following the menu.
        let team = team_id.filter(|t| Some(*t) != menu_team);
        // A tax rate chosen for the whole order applies to new lines too.
        let order_tax: Option<i64> = tx.query_row("SELECT tax_bp FROM orders WHERE id = ?1", [order_id], |r| r.get(0))?;
        let rate = order_tax.unwrap_or(rate);
        let variant = resolve_variant(&tx, product_id, variant_id)?;
        let variant_id = variant.as_ref().map(|v| v.0);
        let (name, base, alt) = match variant {
            Some((_, vname, vprice, valt)) => {
                // Thai for whichever part has it, English for the other.
                let alt = (alt.is_some() || valt.is_some()).then(|| {
                    format!("{} ({})", alt.as_deref().unwrap_or(&name), valt.as_deref().unwrap_or(&vname))
                });
                (format!("{name} ({vname})"), vprice, alt)
            }
            None => (name, base, alt),
        };
        let options = resolve_options(&tx, product_id, option_ids)?;
        let price = base + options.iter().map(|o| o.price_cents).sum::<i64>();
        if price < 0 {
            return invalid("the options make this item's price negative");
        }

        let mut merged = false;
        if note.is_none() {
            let mut wanted: Vec<i64> = option_ids.to_vec();
            wanted.sort_unstable();
            let candidates: Vec<i64> = tx
                .prepare_cached(
                    "SELECT id FROM order_items
                     WHERE order_id = ?1 AND product_id = ?2 AND sent_at IS NULL AND note IS NULL
                       AND parent_id IS NULL AND is_set = 0 AND discount_cents = 0 AND unit_price_cents = ?3 AND qty + ?4 <= ?5
                       AND variant_id IS ?6 AND starter = ?7 AND team_id IS ?8 AND menu_price_cents IS NULL
                     ORDER BY id DESC",
                )?
                .query_map(params![order_id, product_id, price, qty, MAX_QTY, variant_id, starter, team], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in candidates {
                if line_option_ids(&tx, id)? == wanted {
                    tx.execute("UPDATE order_items SET qty = qty + ?2 WHERE id = ?1", params![id, qty])?;
                    merged = true;
                    break;
                }
            }
        }
        if !merged {
            let lines: i64 =
                tx.query_row("SELECT COUNT(*) FROM order_items WHERE order_id = ?1", [order_id], |r| r.get(0))?;
            if lines >= MAX_ITEMS {
                return invalid("this order has too many lines");
            }
            tx.prepare_cached(
                "INSERT INTO order_items (order_id, product_id, name, unit_price_cents, tax_rate_bp, qty, note, added_by, variant_id, starter, name_alt, prep, team_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?
            .execute(params![order_id, product_id, name, price, rate, qty, note, s.user_id, variant_id, starter, alt, prep, team])?;
            let item_id = tx.last_insert_rowid();
            let mut ins = tx.prepare_cached(
                "INSERT INTO order_item_options (item_id, option_id, group_name, name, price_cents, name_alt)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for o in &options {
                ins.execute(params![item_id, o.option_id, o.group_name, o.name, o.price_cents, o.name_alt])?;
            }
        }
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Make a line not yet sent a starter or a main course.
    pub fn set_item_course(&mut self, s: &Session, item_id: i64, starter: bool) -> Result<Order> {
        s.require(Permission::Sell)?;
        let order_id = self.order_of_item(item_id)?;
        require_open(&self.conn, order_id)?;
        let n = self.conn.execute(
            "UPDATE order_items SET starter = ?2, held = 0 WHERE id = ?1 AND sent_at IS NULL AND parent_id IS NULL AND is_set = 0",
            params![item_id, starter],
        )?;
        if n == 0 {
            return invalid("only a dish not yet sent to the kitchen can change course");
        }
        self.get_order(s, order_id)
    }

    /// Move a dish not yet sent to another kitchen team for this order
    /// (the ✎ editor's "Team"), or back to its menu team with None.
    pub fn set_item_team(&mut self, s: &Session, item_id: i64, team_id: Option<i64>) -> Result<Order> {
        s.require(Permission::Sell)?;
        let order_id = self.order_of_item(item_id)?;
        require_open(&self.conn, order_id)?;
        self.check_team(team_id)?;
        let n = self.conn.execute(
            "UPDATE order_items SET team_id = CASE WHEN ?2 IS (SELECT p.team_id FROM products p WHERE p.id = order_items.product_id) THEN NULL ELSE ?2 END
             WHERE id = ?1 AND sent_at IS NULL AND parent_id IS NULL AND is_set = 0",
            params![item_id, team_id],
        )?;
        if n == 0 {
            return invalid("only a dish not yet sent to the kitchen can change team");
        }
        self.get_order(s, order_id)
    }

    /// A dish that is not on the menu ("open item"): its name and price are
    /// typed on the order. It is taxed at the menu's main rate (or the
    /// order's rate) and prints on the general kitchen ticket.
    #[allow(clippy::too_many_arguments)]
    pub fn add_open_item(
        &mut self,
        s: &Session,
        order_id: i64,
        name: &str,
        price_cents: i64,
        qty: i64,
        note: Option<&str>,
        starter: bool,
    ) -> Result<Order> {
        self.add_open_item_approved(s, None, order_id, name, price_cents, qty, note, starter)
    }

    /// [`Pos::add_open_item`], with `approver` (the owner of a manager's or
    /// admin's PIN typed on the till) allowing staff to add it: a typed
    /// price is a changed price. The audit log says who approved it.
    #[allow(clippy::too_many_arguments)]
    pub fn add_open_item_approved(
        &mut self,
        s: &Session,
        approver: Option<&Session>,
        order_id: i64,
        name: &str,
        price_cents: i64,
        qty: i64,
        note: Option<&str>,
        starter: bool,
    ) -> Result<Order> {
        s.require(Permission::Sell)?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return invalid("type the dish's name (up to 60 characters)");
        }
        if !(0..=10_000_000).contains(&price_cents) {
            return invalid("enter the price");
        }
        if !(1..=MAX_QTY).contains(&qty) {
            return invalid(format!("quantity must be between 1 and {MAX_QTY}"));
        }
        let note = opt_text(&note.map(str::to_string), 100, "note")?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let by = approved_by(s, approver, MONEY_APPROVAL, OPEN_ITEM_NEEDS_PIN)?;
        let default_bp: i64 = tx
            .query_row("SELECT value FROM settings WHERE key = 'default_tax_bp'", [], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        // One hidden menu entry stands behind every open item.
        let product: i64 = match tx
            .query_row("SELECT id FROM products WHERE sku = ?1", [OPEN_ITEM_SKU], |r| r.get(0))
            .optional()?
        {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO products (sku, name, price_cents, tax_rate_bp, active, set_only) VALUES (?1, 'Open item', 0, ?2, 1, 1)",
                    params![OPEN_ITEM_SKU, default_bp],
                )?;
                tx.last_insert_rowid()
            }
        };
        let order_tax: Option<i64> = tx.query_row("SELECT tax_bp FROM orders WHERE id = ?1", [order_id], |r| r.get(0))?;
        let lines: i64 = tx.query_row("SELECT COUNT(*) FROM order_items WHERE order_id = ?1", [order_id], |r| r.get(0))?;
        if lines >= MAX_ITEMS {
            return invalid("this order has too many lines");
        }
        tx.execute(
            "INSERT INTO order_items (order_id, product_id, name, unit_price_cents, tax_rate_bp, qty, note, added_by, starter)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![order_id, product, name, price_cents, order_tax.unwrap_or(default_bp), qty, note, s.user_id, starter],
        )?;
        audit_tx(&tx, s.user_id, "order.open_item", &format!("order {order_id} {name} {price_cents} {}", approval_note(s, by)))?;
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// The order a line belongs to.
    /// Order `order_id` is open and the dish `item_id` is still on it, so a
    /// till showing an old copy never changes another table's bill. Closed:
    /// "already closed"; joined away (gone): not found; the dish moved to
    /// another bill: not found (the till reloads the order).
    pub fn check_item_on_order(&self, order_id: i64, item_id: i64) -> Result<()> {
        require_open(&self.conn, order_id)?;
        if self.order_of_item(item_id)? != order_id {
            return Err(PosError::NotFound);
        }
        Ok(())
    }

    pub fn order_of_item(&self, item_id: i64) -> Result<i64> {
        self.conn
            .query_row("SELECT order_id FROM order_items WHERE id = ?1", [item_id], |r| r.get(0))
            .optional()?
            .ok_or(PosError::NotFound)
    }

    /// The update to make to line `item_id` given what the till knew
    /// ([`ItemCheck`]): refused with [`ITEM_CHANGED`] (a conflict) when the
    /// line's quantity or discount is not what the till showed; with a
    /// `qty_delta`, the line as it is now with that many more or fewer.
    /// Call it under the same lock as the update itself (the commands do).
    pub fn resolve_item_update(&self, item_id: i64, upd: &ItemUpdate, check: &ItemCheck) -> Result<ItemUpdate> {
        let (qty, discount, note): (i64, i64, Option<String>) = self
            .conn
            .prepare_cached("SELECT qty, discount_cents, note FROM order_items WHERE id = ?1")?
            .query_row([item_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?
            .ok_or(PosError::NotFound)?;
        if check.expected_qty.is_some_and(|q| q != qty) || check.expected_discount_cents.is_some_and(|d| d != discount) {
            return Err(PosError::Conflict(ITEM_CHANGED.into()));
        }
        let Some(delta) = check.qty_delta else { return Ok(upd.clone()) };
        if !(-MAX_QTY..=MAX_QTY).contains(&delta) {
            return invalid("that is too many at once");
        }
        if delta == 0 {
            // A note alone: a discount sent with it is refused, never dropped
            // (a discount has its own checks: the staff limit, below paid).
            if upd.discount_cents != discount {
                return invalid("a note alone (qtyDelta 0) cannot change the discount: send the discount separately");
            }
            return Ok(ItemUpdate { qty, discount_cents: discount, note: upd.note.clone() });
        }
        let new = (qty + delta).max(0);
        // Fewer: the discount keeps its share of the line (as the till works it out).
        let discount = if new > 0 && new < qty { discount * new / qty } else { discount };
        Ok(ItemUpdate { qty: new, discount_cents: discount, note })
    }

    /// Change quantity, discount or note on a line; qty 0 removes it.
    /// Lines already sent to the kitchen (food that has been made) can only
    /// be reduced or removed by a manager or an admin, or by staff with one's
    /// PIN (see [`Pos::update_item_approved`]); with the setting
    /// `staff_cancel_sent` = "0", only by a manager or admin.
    pub fn update_item(&mut self, s: &Session, item_id: i64, upd: &ItemUpdate) -> Result<Order> {
        self.update_item_approved(s, None, item_id, upd)
    }

    /// [`Pos::update_item`], with `approver` (from a manager's PIN typed on
    /// this till) allowing a discount over the staff limit, or staff to
    /// reduce or remove a dish already sent (logged on `order.item_void`
    /// with who approved it).
    pub fn update_item_approved(&mut self, s: &Session, approver: Option<&Session>, item_id: i64, upd: &ItemUpdate) -> Result<Order> {
        s.require(Permission::Sell)?;
        let note = opt_text(&upd.note, 100, "note")?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let cur: OrderItem = tx
            .prepare_cached(&format!("SELECT {ITEM_COLS} FROM order_items WHERE id = ?1"))?
            .query_row([item_id], item_from_row)
            .optional()?
            .ok_or(PosError::NotFound)?;
        let order_id: i64 = tx.query_row("SELECT order_id FROM order_items WHERE id = ?1", [item_id], |r| r.get(0))?;
        require_open(&tx, order_id)?;
        if !(0..=MAX_QTY).contains(&upd.qty) {
            return invalid(format!("quantity must be between 0 and {MAX_QTY}"));
        }
        if cur.sent_at.is_some() {
            if upd.qty > cur.qty {
                return invalid("this item was already sent to the kitchen; add it again instead");
            }
            // Fixing a mistake after sending (the kitchen gets a CANCELLED
            // ticket): staff may, unless Settings keep it to admins.
            let staff_may = tx
                .query_row("SELECT value FROM settings WHERE key = 'staff_cancel_sent'", [], |r| r.get::<_, String>(0))
                .optional()?
                .as_deref()
                != Some("0");
            if upd.qty < cur.qty && !staff_may {
                s.require(Permission::VoidOrder)?;
            }
        }
        // A dish inside a set changes with its set; only its note is its own.
        if cur.parent_id.is_some() && (upd.qty != cur.qty || upd.discount_cents != cur.discount_cents) {
            return invalid("change or remove the set itself");
        }
        // Fewer of a sent dish: a manager's or admin's say-so (staff type a PIN).
        let void_by = if cur.sent_at.is_some() && upd.qty < cur.qty {
            Some(approved_by(s, approver, Permission::CancelWithoutPin, VOID_NEEDS_PIN)?)
        } else {
            None
        };
        if cur.is_set && upd.qty != cur.qty {
            if upd.qty == 0 {
                tx.execute("DELETE FROM order_items WHERE parent_id = ?1", [item_id])?;
            } else {
                // Each dish keeps its count per set.
                tx.execute(
                    "UPDATE order_items SET qty = qty / ?2 * ?3 WHERE parent_id = ?1",
                    params![item_id, cur.qty, upd.qty],
                )?;
            }
        }
        if upd.qty == 0 {
            tx.execute("DELETE FROM order_items WHERE id = ?1", [item_id])?;
        } else {
            if !(0..=cur.unit_price_cents * upd.qty).contains(&upd.discount_cents) {
                return invalid(format!("invalid discount on {}", cur.name));
            }
            // More of a dish whose price was made cheaper on this order: the
            // same say-so as the cheaper price (set_item_price), or one
            // approved line becomes many.
            if upd.qty > cur.qty && !s.role.can(Permission::AnyDiscount) {
                let menu: Option<i64> =
                    tx.query_row("SELECT menu_price_cents FROM order_items WHERE id = ?1", [item_id], |r| r.get(0))?;
                if menu.is_some_and(|m| cur.unit_price_cents < m) {
                    match approver {
                        None => return invalid("more of a dish at a changed price needs a manager PIN"),
                        Some(a) if !a.role.can(Permission::AnyDiscount) => {
                            return invalid("more of a dish at a changed price needs a manager's or admin's PIN")
                        }
                        Some(a) => audit_tx(
                            &tx,
                            a.user_id,
                            "order.price_qty_approved",
                            &format!("order {order_id} {} x{} -> x{} at {} for {}", cur.name, cur.qty, upd.qty, cur.unit_price_cents, s.name),
                        )?,
                    }
                }
            }
            // A bigger share off than before, over the staff limit: a manager's or admin's say-so.
            let more = upd.discount_cents * cur.qty > cur.discount_cents * upd.qty;
            if more && !s.role.can(Permission::AnyDiscount) {
                if let Some(max) = crate::settings::discount_limit(&tx)? {
                    // Rounded to the nearest penny, as the till works out "15%".
                    if upd.discount_cents * 100 > max * cur.unit_price_cents * upd.qty + 50 {
                        match approver {
                            None => return invalid(format!("a discount over {max}% needs a manager PIN")),
                            Some(a) if !a.role.can(Permission::AnyDiscount) => {
                                return invalid(format!("a discount over {max}% needs a manager's or admin's PIN"))
                            }
                            Some(a) => audit_tx(&tx, a.user_id, "order.discount_approved", &format!("order {order_id} {} {} for {}", cur.name, upd.discount_cents, s.name))?,
                        }
                    }
                }
            }
            tx.execute(
                "UPDATE order_items SET qty = ?2, discount_cents = ?3, note = ?4 WHERE id = ?1",
                params![item_id, upd.qty, upd.discount_cents, note],
            )?;
        }
        if let Some(by) = void_by {
            audit_tx(
                &tx,
                s.user_id,
                "order.item_void",
                &format!("order {order_id} {} x{} {}", cur.name, cur.qty - upd.qty, approval_note(s, by)),
            )?;
        }
        if upd.discount_cents != cur.discount_cents && upd.qty > 0 {
            audit_tx(&tx, s.user_id, "order.discount", &format!("order {order_id} {} {}", cur.name, upd.discount_cents))?;
        }
        check_covers_paid(&tx, order_id)?;
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Change what one line of an open order costs each (e.g. a dish made
    /// bigger, or a price agreed with the customer). Managers and admins may;
    /// staff need `approver`, a manager's or admin's PIN typed on the till.
    /// Not on a dish inside a set (its set has the price), and not once part
    /// of the bill is paid. A discount bigger than the new line is cut to it.
    pub fn set_item_price(&mut self, s: &Session, approver: Option<&Session>, item_id: i64, price_cents: i64) -> Result<Order> {
        s.require(Permission::Sell)?;
        if !(0..=MAX_LINE_PRICE).contains(&price_cents) {
            return invalid("enter a price from 0 to 100000");
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let cur: OrderItem = tx
            .prepare_cached(&format!("SELECT {ITEM_COLS} FROM order_items WHERE id = ?1"))?
            .query_row([item_id], item_from_row)
            .optional()?
            .ok_or(PosError::NotFound)?;
        let order_id: i64 = tx.query_row("SELECT order_id FROM order_items WHERE id = ?1", [item_id], |r| r.get(0))?;
        require_open(&tx, order_id)?;
        if cur.parent_id.is_some() {
            return invalid("change the price of the set itself");
        }
        let paid: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM payments WHERE order_id = ?1)", [order_id], |r| r.get(0))?;
        if paid {
            return invalid("part of this bill is paid: take the payment back first to change a price");
        }
        if price_cents == cur.unit_price_cents {
            tx.commit()?;
            return self.get_order(s, order_id);
        }
        let by = if s.role.can(Permission::AnyDiscount) {
            s
        } else {
            match approver {
                None => return invalid("changing a price needs a manager PIN"),
                Some(a) if !a.role.can(Permission::AnyDiscount) => return invalid("changing a price needs a manager's or admin's PIN"),
                Some(a) => a,
            }
        };
        let discount = cur.discount_cents.min(price_cents * cur.qty);
        // The first price stays as the line's menu price (see update_item_approved).
        tx.execute(
            "UPDATE order_items SET unit_price_cents = ?2, discount_cents = ?3, menu_price_cents = COALESCE(menu_price_cents, unit_price_cents) WHERE id = ?1",
            params![item_id, price_cents, discount],
        )?;
        audit_tx(
            &tx,
            by.user_id,
            "order.price",
            &format!("order {order_id} {} {} -> {} by {}", cur.name, cur.unit_price_cents, price_cents, s.name),
        )?;
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Send the next course to the kitchen and return the lines sent, i.e.
    /// the kitchen ticket for this round.
    ///
    /// Dine-in orders are sent course by course: while there are new
    /// starters to send, or mains are already waiting, only starters go and
    /// every other new line is held for [`Pos::call_mains`]. Without starters
    /// everything goes at once. Other order types always send everything.
    pub fn send_to_kitchen(&mut self, s: &Session, order_id: i64) -> Result<Vec<OrderItem>> {
        self.send_round(s, order_id, false)
    }

    /// Send everything not yet sent, including mains held behind the
    /// starters ("Call mains", or "send all courses together").
    pub fn call_mains(&mut self, s: &Session, order_id: i64) -> Result<Vec<OrderItem>> {
        self.send_round(s, order_id, true)
    }

    fn send_round(&mut self, s: &Session, order_id: i64, all: bool) -> Result<Vec<OrderItem>> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let dine_in: bool =
            tx.query_row("SELECT order_type = 'dine_in' FROM orders WHERE id = ?1", [order_id], |r| r.get(0))?;
        let by_course = dine_in && !all && {
            let (new_starters, waiting): (bool, bool) = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM order_items WHERE order_id = ?1 AND sent_at IS NULL AND held = 0 AND starter = 1),
                        EXISTS (SELECT 1 FROM order_items WHERE order_id = ?1 AND sent_at IS NULL AND held = 1)",
                [order_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            new_starters || waiting
        };
        if by_course {
            tx.execute(
                // Drinks go now too, like starters.
                "UPDATE order_items SET held = 1 WHERE order_id = ?1 AND sent_at IS NULL AND starter = 0 AND is_set = 0
                   AND product_id NOT IN (SELECT id FROM products WHERE drink = 1)",
                [order_id],
            )?;
        }
        let ids: Vec<i64> = tx
            .prepare_cached(
                "UPDATE order_items SET sent_at = unixepoch()
                 WHERE order_id = ?1 AND sent_at IS NULL AND (?2 OR held = 0) RETURNING id",
            )?
            .query_map(params![order_id, !by_course], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let sent: Vec<OrderItem> = if ids.is_empty() {
            Vec::new()
        } else {
            load_items(&tx, order_id)?.into_iter().filter(|i| ids.contains(&i.id)).collect()
        };
        tx.commit()?;
        Ok(sent)
    }

    /// Prepare-ahead dishes that are waiting for "Call mains" and have not
    /// had a PREPARE NOW ticket yet: marks them and returns them, so the
    /// kitchen can start them now. They are still served with the mains.
    pub fn send_prep(&mut self, s: &Session, order_id: i64) -> Result<Vec<OrderItem>> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let ids: Vec<i64> = tx
            .prepare_cached(
                "UPDATE order_items SET prep_sent_at = unixepoch()
                 WHERE order_id = ?1 AND prep = 1 AND held = 1 AND sent_at IS NULL AND prep_sent_at IS NULL
                 RETURNING id",
            )?
            .query_map([order_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let items = if ids.is_empty() {
            Vec::new()
        } else {
            load_items(&tx, order_id)?.into_iter().filter(|i| ids.contains(&i.id)).collect()
        };
        tx.commit()?;
        Ok(items)
    }

    /// Change or remove (0) the service charge on an open order, e.g. at the
    /// customer's request. Raising it is anyone's; removing or lowering it
    /// is a manager's or admin's (staff need `approver`, see
    /// [`Pos::set_service_charge_approved`]). Logged.
    pub fn set_service_charge(&mut self, s: &Session, order_id: i64, rate_bp: i64) -> Result<Order> {
        self.set_service_charge_approved(s, None, order_id, rate_bp)
    }

    /// [`Pos::set_service_charge`], with `approver` (the owner of a manager's
    /// or admin's PIN typed on the till) allowing staff to remove or lower
    /// it. The audit log says who approved it.
    pub fn set_service_charge_approved(&mut self, s: &Session, approver: Option<&Session>, order_id: i64, rate_bp: i64) -> Result<Order> {
        s.require(Permission::Sell)?;
        check_rate(rate_bp)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let was: i64 = tx.query_row("SELECT COALESCE(service_bp, 0) FROM orders WHERE id = ?1", [order_id], |r| r.get(0))?;
        if rate_bp != was {
            let by = if rate_bp < was { approved_by(s, approver, MONEY_APPROVAL, SERVICE_NEEDS_PIN)? } else { s };
            tx.execute("UPDATE orders SET service_bp = ?2 WHERE id = ?1", params![order_id, rate_bp])?;
            audit_tx(&tx, s.user_id, "order.service", &format!("{order_id} {rate_bp} (was {was}) {}", approval_note(s, by)))?;
            check_covers_paid(&tx, order_id)?;
        }
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Set one tax rate for every line of an open order (e.g. 0% for a
    /// tax-exempt customer), or None to go back to each item's menu rate.
    /// A rate of the order's own, or going back to the menu rates when that
    /// lowers the tax on a priced line (from an order rate above its menu
    /// rate), is a manager's or admin's (staff need `approver`, see
    /// [`Pos::set_order_tax_approved`]); back to menu rates that keep or
    /// raise the tax is anyone's. Logged.
    pub fn set_order_tax(&mut self, s: &Session, order_id: i64, rate_bp: Option<i64>) -> Result<Order> {
        self.set_order_tax_approved(s, None, order_id, rate_bp)
    }

    /// [`Pos::set_order_tax`], with `approver` (the owner of a manager's or
    /// admin's PIN typed on the till) allowing staff to set a rate for the
    /// whole order. The audit log says who approved it.
    pub fn set_order_tax_approved(&mut self, s: &Session, approver: Option<&Session>, order_id: i64, rate_bp: Option<i64>) -> Result<Order> {
        s.require(Permission::Sell)?;
        if rate_bp.is_some_and(|r| !(0..=10_000).contains(&r)) {
            return invalid("tax rate must be between 0% and 100%");
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let was: Option<i64> = tx.query_row("SELECT tax_bp FROM orders WHERE id = ?1", [order_id], |r| r.get(0))?;
        // Any rate of the order's own (0% for a tax-free customer, or one
        // that is higher or lower) is not staff's to choose. Back to the
        // menu's rates is theirs unless it lowers the tax on a priced line
        // (an order rate above that dish's menu rate), as a move onto a
        // bill at menu rates is (see `move_approval`).
        // Unchanged (e.g. "Menu rates" tapped again): nothing to write or log.
        if rate_bp != was {
            let lowers = || -> Result<bool> {
                Ok(tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM order_items i JOIN products p ON p.id = i.product_id
                                    WHERE i.order_id = ?1 AND i.unit_price_cents > 0 AND p.tax_rate_bp < i.tax_rate_bp)",
                    [order_id],
                    |r| r.get(0),
                )?)
            };
            let by = if rate_bp.is_some() || lowers()? { approved_by(s, approver, MONEY_APPROVAL, TAX_NEEDS_PIN)? } else { s };
            tx.execute("UPDATE orders SET tax_bp = ?2 WHERE id = ?1", params![order_id, rate_bp])?;
            match rate_bp {
                Some(r) => tx.execute("UPDATE order_items SET tax_rate_bp = ?2 WHERE order_id = ?1", params![order_id, r])?,
                None => tx.execute(
                    "UPDATE order_items SET tax_rate_bp = (SELECT p.tax_rate_bp FROM products p WHERE p.id = order_items.product_id)
                     WHERE order_id = ?1",
                    [order_id],
                )?,
            };
            audit_tx(&tx, s.user_id, "order.tax", &format!("{order_id} {rate_bp:?} (was {was:?}) {}", approval_note(s, by)))?;
            check_covers_paid(&tx, order_id)?;
        }
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Take payment for the rest of the order and close it. Stock is taken
    /// for tracked items.
    pub fn pay_order(&mut self, s: &Session, order_id: i64, method: PaymentMethod, tendered_cents: i64) -> Result<Order> {
        self.add_payment(s, order_id, &PaymentInput { method, amount_cents: None, tendered_cents, tip_cents: 0, items: None })
    }

    /// Take one payment: the rest of the bill, an amount (split evenly or
    /// typed) or chosen dishes (their share of the bill, service included).
    /// A tip can go on top. The order closes once the payments cover it.
    pub fn add_payment(&mut self, s: &Session, order_id: i64, p: &PaymentInput) -> Result<Order> {
        self.add_payment_approved(s, None, order_id, p)
    }

    /// [`Pos::add_payment`], with `approver` (the owner of a manager's or
    /// admin's PIN typed on the till) allowing staff to take a payment by
    /// Other on an order that is not from a delivery app (no money comes
    /// into the till for it). The audit log says who approved it.
    pub fn add_payment_approved(&mut self, s: &Session, approver: Option<&Session>, order_id: i64, p: &PaymentInput) -> Result<Order> {
        self.add_payment_checked(s, approver, order_id, p, None)
    }

    /// [`Pos::add_payment_approved`] from a pay dialog that showed
    /// `expected_left` as left to pay: refused with [`BILL_CHANGED`] (a
    /// conflict, nothing taken) when another till has changed the bill
    /// since, so the payment is never for more or less than the till showed.
    /// None: no check, as before.
    ///
    /// A bill with nothing to pay (every dish free or fully discounted, a
    /// total of 0) closes as paid with no payment, logged as
    /// `order.closed_nothing_to_pay`.
    pub fn add_payment_checked(&mut self, s: &Session, approver: Option<&Session>, order_id: i64, p: &PaymentInput, expected_left: Option<i64>) -> Result<Order> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let items = load_items(&tx, order_id)?;
        if items.is_empty() {
            return invalid("the order is empty");
        }
        let (service_bp, tax_incl): (i64, bool) =
            tx.query_row("SELECT service_bp, tax_incl FROM orders WHERE id = ?1", [order_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let t = totals(&items, service_bp, tax_incl);
        let earlier = load_payments(&tx, order_id)?;
        let paid: i64 = earlier.iter().map(|p| p.amount_cents).sum();
        let left = t.total - paid;
        if expected_left.is_some_and(|e| e != left.max(0)) {
            return Err(PosError::Conflict(BILL_CHANGED.into()));
        }
        // Already covered (dishes taken off after a part payment), or nothing
        // to pay at all (a total of 0): just close.
        if left <= 0 && (!earlier.is_empty() || t.total == 0) {
            // Closed with no payment: a tip would have nothing to go on, so it is
            // refused rather than dropped.
            if p.tip_cents != 0 {
                return invalid("Nothing to pay on this bill, so a tip cannot be taken with it");
            }
            close_paid(&tx, s, order_id, &t)?;
            if earlier.is_empty() {
                audit_tx(&tx, s.user_id, "order.closed_nothing_to_pay", &format!("order {order_id}: closed with nothing to pay"))?;
            }
            tx.commit()?;
            return self.get_order(s, order_id);
        }
        let other_by = if p.method == PaymentMethod::Other {
            let channel: Option<i64> = tx.query_row("SELECT channel_id FROM orders WHERE id = ?1", [order_id], |r| r.get(0))?;
            if channel.is_none() {
                Some(approved_by(s, approver, MONEY_APPROVAL, OTHER_PAY_NEEDS_PIN)?)
            } else {
                None
            }
        } else {
            None
        };
        let mut chosen: Option<String> = None;
        let amount = if let Some(sel) = &p.items {
            let mut paid_qty: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
            for (id, q) in earlier.iter().flat_map(|p| p.items.iter()) {
                *paid_qty.entry(*id).or_default() += q;
            }
            let mut picked: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
            for &(id, q) in sel {
                if q > 0 {
                    *picked.entry(id).or_default() += q;
                }
            }
            if picked.is_empty() {
                return invalid("choose the dishes to pay for");
            }
            let bill: Vec<&OrderItem> = items.iter().filter(|i| i.parent_id.is_none()).collect();
            let all: i64 = bill.iter().map(|i| line_due(i, tax_incl)).sum();
            let mut part = 0i64;
            for (&id, &q) in &picked {
                let it = bill.iter().find(|i| i.id == id).ok_or_else(|| PosError::Validation("that dish is not on this bill".into()))?;
                if q > it.qty - paid_qty.get(&id).copied().unwrap_or(0) {
                    return invalid(format!("{} is already paid for", it.name));
                }
                part += line_due(it, tax_incl) * q / it.qty;
            }
            let everything = bill.iter().all(|i| paid_qty.get(&i.id).copied().unwrap_or(0) + picked.get(&i.id).copied().unwrap_or(0) >= i.qty);
            let mut list: Vec<(i64, i64)> = picked.into_iter().collect();
            list.sort();
            chosen = Some(serde_json::to_string(&list).map_err(|e| PosError::Internal(e.to_string()))?);
            let share = if everything || all == 0 {
                left
            } else {
                ((t.total as i128 * part as i128 + all as i128 / 2) / all as i128) as i64
            }
            .min(left);
            // The share the pay dialog showed (sent by tills from 0.14.1 on): the
            // dishes' share now is another amount = the bill changed since.
            if p.amount_cents.is_some_and(|shown| shown != share) {
                return Err(PosError::Conflict(BILL_CHANGED.into()));
            }
            share
        } else {
            p.amount_cents.unwrap_or(left)
        };
        if amount <= 0 {
            return invalid("enter an amount to pay");
        }
        if amount > left {
            return invalid("that is more than is left to pay");
        }
        if !(0..=t.total.max(amount) * 10).contains(&p.tip_cents) {
            return invalid("invalid tip");
        }
        let due = amount + p.tip_cents;
        let (tendered, change) = match p.method {
            PaymentMethod::Cash if p.tendered_cents < due => return invalid("cash tendered is less than the amount to pay"),
            PaymentMethod::Cash => (p.tendered_cents, p.tendered_cents - due),
            _ => (due, 0),
        };
        let shift: Option<i64> = tx
            .query_row("SELECT id FROM shifts WHERE closed_at IS NULL ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
            .optional()
            .unwrap_or(None);
        tx.execute(
            "INSERT INTO payments (order_id, method, amount_cents, tip_cents, tendered_cents, change_cents, items, paid_by, shift_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![order_id, p.method.as_str(), amount, p.tip_cents, tendered, change, chosen, s.user_id, shift],
        )?;
        if let Some(by) = other_by {
            audit_tx(&tx, s.user_id, "order.payment_other", &format!("order {order_id} {amount} tip {} {}", p.tip_cents, approval_note(s, by)))?;
        }
        if paid + amount >= t.total {
            close_paid(&tx, s, order_id, &t)?;
        } else {
            audit_tx(&tx, s.user_id, "order.part_payment", &format!("order {order_id} {} {amount}", p.method.as_str()))?;
        }
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Take back a part payment taken by mistake (the order is still open).
    /// Admins and managers may ([`Permission::CancelWithoutPin`]); staff need
    /// `approver` (see [`Pos::remove_payment_approved`]), or a cashier
    /// could take the cash, take the payment back and keep it. Logged.
    pub fn remove_payment(&mut self, s: &Session, order_id: i64, payment_id: i64) -> Result<Order> {
        self.remove_payment_approved(s, None, order_id, payment_id)
    }

    /// [`Pos::remove_payment`], with `approver` (the owner of a manager's or
    /// admin's PIN typed on the till) allowing staff to take a payment
    /// back. The audit log has the payment and who approved it.
    pub fn remove_payment_approved(&mut self, s: &Session, approver: Option<&Session>, order_id: i64, payment_id: i64) -> Result<Order> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let (method, amount, tip, taken_by): (String, i64, i64, String) = tx
            .query_row(
                "SELECT p.method, p.amount_cents, p.tip_cents, u.name FROM payments p JOIN users u ON u.id = p.paid_by
                 WHERE p.id = ?1 AND p.order_id = ?2",
                params![payment_id, order_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
            .ok_or(PosError::NotFound)?;
        let by = approved_by(s, approver, Permission::CancelWithoutPin, PAYMENT_NEEDS_PIN)?;
        tx.execute("DELETE FROM payments WHERE id = ?1 AND order_id = ?2", params![payment_id, order_id])?;
        audit_tx(
            &tx,
            s.user_id,
            "order.payment_removed",
            &format!("order {order_id} payment {payment_id} {method} {amount} tip {tip} taken by {taken_by} {}", approval_note(s, by)),
        )?;
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Abandon an open order. If anything was already sent to the kitchen
    /// this needs [`Permission::VoidOrder`] (an admin or a manager).
    pub fn cancel_order(&mut self, s: &Session, order_id: i64, reason: &str) -> Result<()> {
        self.cancel_order_logged(s, s, "", order_id, reason)
    }

    /// [`Pos::cancel_order`] with `s`'s rights (`s` closes the order), logged
    /// in the audit log as done by `by` (the user logged in on the till, who
    /// may differ from `s` when someone else's PIN approved it), with `note`
    /// (how it was approved) after the reason.
    pub(crate) fn cancel_order_logged(&mut self, s: &Session, by: &Session, note: &str, order_id: i64, reason: &str) -> Result<()> {
        s.require(Permission::Sell)?;
        let reason = reason.trim();
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let sent: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM order_items WHERE order_id = ?1 AND sent_at IS NOT NULL)",
            [order_id],
            |r| r.get(0),
        )?;
        if sent {
            s.require(Permission::VoidOrder)?;
        }
        let paid: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM payments WHERE order_id = ?1)", [order_id], |r| r.get(0))?;
        if paid {
            return invalid("this order has part payments: take them back first (tap the payment) or finish paying");
        }
        tx.execute(
            "UPDATE orders SET status = 'cancelled', closed_by = ?2, closed_at = unixepoch(), close_reason = ?3
             WHERE id = ?1",
            params![order_id, s.user_id, (!reason.is_empty()).then_some(reason)],
        )?;
        let detail = [order_id.to_string(), reason.to_string(), note.to_string()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ");
        audit_tx(&tx, by.user_id, "order.cancel", &detail)?;
        tx.commit()?;
        Ok(())
    }

    /// Cancel an order only if it is still open with nothing on it (no lines,
    /// no payments): a free table tapped, then left. Checked and cancelled in
    /// one transaction, so a dish another till adds meanwhile is never lost.
    /// Returns whether it was cancelled.
    pub fn cancel_if_empty(&mut self, s: &Session, order_id: i64) -> Result<bool> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let n = tx.execute(
            &format!("UPDATE orders SET status = 'cancelled', closed_by = ?2, closed_at = unixepoch(), close_reason = 'empty'
                      WHERE id = ?1 AND status = 'open' AND {EMPTY_ORDER}"),
            params![order_id, s.user_id],
        )?;
        if n > 0 {
            audit_tx(&tx, s.user_id, "order.cancel", &format!("{order_id} empty"))?;
        }
        tx.commit()?;
        Ok(n > 0)
    }

    /// Cancel every open order with nothing on it (no dishes, payments or
    /// customer details) untouched for over [`EMPTY_ORDER_AGE`] (opened, or
    /// last changed, e.g. its last dish taken off, that long ago), as "empty"
    /// (kept out of history and reports, like [`Pos::cancel_if_empty`]).
    /// Checked and cancelled in one statement, so a dish added meanwhile
    /// keeps its order. Returns how many were cancelled.
    pub fn sweep_empty_orders(&mut self) -> Result<usize> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let n = tx.execute(
            &format!(
                "UPDATE orders SET status = 'cancelled', closed_by = NULL, closed_at = unixepoch(), close_reason = 'empty'
                 WHERE status = 'open' AND MAX(opened_at, COALESCE(touched_at, opened_at)) < unixepoch() - ?1 AND {EMPTY_ORDER}"
            ),
            [EMPTY_ORDER_AGE],
        )?;
        if n > 0 {
            tx.execute(
                "INSERT INTO audit_log (user_id, action, detail) VALUES (NULL, 'order.cancel', ?1)",
                [format!("{n} empty order(s) untouched for over {} minutes", EMPTY_ORDER_AGE / 60)],
            )?;
        }
        tx.commit()?;
        self.last_empty_sweep = Some(std::time::Instant::now());
        Ok(n)
    }

    /// [`Pos::sweep_empty_orders`], at most every ten minutes (it is called
    /// with the header's business day, which every till asks for often).
    pub fn sweep_empty_orders_if_due(&mut self) -> Result<usize> {
        if self.last_empty_sweep.is_some_and(|t| t.elapsed() < EMPTY_SWEEP_EVERY) {
            return Ok(0);
        }
        self.sweep_empty_orders()
    }

    /// Reverse a paid order (refund) and put its stock back.
    pub fn void_order(&mut self, s: &Session, order_id: i64, reason: &str) -> Result<Order> {
        s.require(Permission::VoidOrder)?;
        let reason = reason.trim();
        if reason.is_empty() {
            return invalid("a reason is required to void an order");
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if status_of(&tx, order_id)? != "paid" {
            return invalid("only paid orders can be voided");
        }
        tx.execute(
            "UPDATE orders SET status = 'voided', closed_by = ?2, closed_at = unixepoch(), close_reason = ?3
             WHERE id = ?1",
            params![order_id, s.user_id, reason],
        )?;
        tx.execute(
            "UPDATE products SET stock_qty = stock_qty - (
                 SELECT m.delta FROM stock_movements m
                 WHERE m.order_id = ?1 AND m.reason = 'sale' AND m.product_id = products.id)
             WHERE id IN (SELECT product_id FROM stock_movements WHERE order_id = ?1 AND reason = 'sale')",
            [order_id],
        )?;
        tx.execute(
            "INSERT INTO stock_movements (product_id, delta, reason, order_id, user_id)
             SELECT product_id, -delta, 'void', order_id, ?2 FROM stock_movements
             WHERE order_id = ?1 AND reason = 'sale'",
            params![order_id, s.user_id],
        )?;
        audit_tx(&tx, s.user_id, "order.void", &format!("{order_id} {reason}"))?;
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Delete an order completely (a mistake to be done again): it leaves
    /// history, reports and the customer's order count. Stock taken by a
    /// paid order is put back. Admin only; logged with the reason.
    pub fn delete_order(&mut self, s: &Session, order_id: i64, reason: &str) -> Result<()> {
        s.require(Permission::Delete)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (status, total, customer): (String, Option<i64>, Option<i64>) = tx
            .query_row("SELECT status, total_cents, customer_id FROM orders WHERE id = ?1", [order_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?
            .ok_or(PosError::NotFound)?;
        if status == "paid" {
            tx.execute(
                "UPDATE products SET stock_qty = stock_qty - (
                     SELECT m.delta FROM stock_movements m
                     WHERE m.order_id = ?1 AND m.reason = 'sale' AND m.product_id = products.id)
                 WHERE id IN (SELECT product_id FROM stock_movements WHERE order_id = ?1 AND reason = 'sale')",
                [order_id],
            )?;
        }
        tx.execute(
            "DELETE FROM order_item_options WHERE item_id IN (SELECT id FROM order_items WHERE order_id = ?1)",
            [order_id],
        )?;
        tx.execute("DELETE FROM order_items WHERE order_id = ?1", [order_id])?;
        tx.execute("DELETE FROM stock_movements WHERE order_id = ?1", [order_id])?;
        tx.execute("DELETE FROM payments WHERE order_id = ?1", [order_id])?;
        tx.execute("DELETE FROM orders WHERE id = ?1", [order_id])?;
        if let Some(c) = customer {
            tx.execute("UPDATE customers SET order_count = MAX(order_count - 1, 0) WHERE id = ?1", [c])?;
        }
        audit_tx(&tx, s.user_id, "order.delete", &format!("#{order_id} {status} total {} {}", total.unwrap_or(0), reason.trim()))?;
        tx.commit()?;
        Ok(())
    }

    /// Every open order (all types), oldest first: the live service board.
    pub fn list_open_orders(&self, s: &Session) -> Result<Vec<OrderSummary>> {
        s.require(Permission::Sell)?;
        let mut stmt =
            self.conn.prepare_cached(&format!("{SUMMARY_SQL} WHERE o.status = 'open' ORDER BY o.opened_at, o.id"))?;
        let rows = stmt.query_map([], summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Closed orders opened in `[from, to)` (unix seconds), newest first.
    /// Staff only see orders they opened or took payment for.
    pub fn list_orders(&self, s: &Session, from: i64, to: i64, limit: i64, offset: i64) -> Result<Vec<OrderSummary>> {
        let only_user = if s.role.can(Permission::ViewAllOrders) {
            None
        } else {
            s.require(Permission::ViewOwnOrders)?;
            Some(s.user_id)
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "{SUMMARY_SQL}
             WHERE o.opened_at >= ?1 AND o.opened_at < ?2 AND o.status <> 'open'
               AND NOT (o.status = 'cancelled' AND COALESCE(o.close_reason, '') = 'empty')
               AND (?3 IS NULL OR o.opened_by = ?3 OR o.paid_by = ?3)
             ORDER BY o.opened_at DESC, o.id DESC LIMIT ?4 OFFSET ?5"
        ))?;
        let rows = stmt.query_map(params![from, to, only_user, limit.clamp(1, 500), offset.max(0)], summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::{tax_of, EMPTY_SWEEP_EVERY};
    use crate::*;

    #[test]
    fn forgotten_empty_orders_are_cancelled_but_not_new_or_real_ones() {
        let mut pos = Pos::open_in_memory().unwrap();
        let s = pos.setup_first_admin("Owner", "1111").unwrap();
        let cat = pos.create_category(&s, "Food", None, 1).unwrap().id;
        let dish = pos
            .create_product(&s, NewProduct {
                category_id: Some(cat), sku: None, barcode: None, name: "Soup".into(), price_cents: 500, cost_cents: 0, tax_rate_bp: Some(0),
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap()
            .id;
        let floor = pos.create_floor(&s, "Ground").unwrap();
        let t = pos
            .save_floor_layout(&s, floor.id, &[TableLayout { is_table: Some(true), id: None, name: "1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
            .unwrap()[0]
            .id;
        let takeaway = |pos: &Pos, details: OrderDetails| {
            pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details }).unwrap().id
        };
        let old_empty = takeaway(&pos, Default::default());
        let old_table = pos.open_order(&s, &NewOrder { order_type: OrderType::DineIn, table_id: Some(t), channel_id: None, details: Default::default() }).unwrap().id;
        let old_dish = takeaway(&pos, Default::default());
        pos.add_item(&s, old_dish, dish, 1, None, None, &[]).unwrap();
        let old_named = takeaway(&pos, OrderDetails { customer_name: Some("Ann".into()), ..Default::default() });
        let just_now = takeaway(&pos, Default::default());
        let age = |pos: &Pos, id: i64, secs: i64| {
            pos.conn.execute("UPDATE orders SET opened_at = unixepoch() - ?2 WHERE id = ?1", [id, secs]).unwrap();
        };
        for id in [old_empty, old_table, old_dish, old_named] {
            age(&pos, id, EMPTY_ORDER_AGE + 60);
        }
        age(&pos, just_now, EMPTY_ORDER_AGE - 60);
        assert_eq!(pos.sweep_empty_orders_if_due().unwrap(), 2);
        let status = |pos: &Pos, id: i64| -> (String, Option<String>, Option<i64>) {
            pos.conn.query_row("SELECT status, close_reason, closed_by FROM orders WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
        };
        for id in [old_empty, old_table] {
            assert_eq!(status(&pos, id), ("cancelled".into(), Some("empty".into()), None));
        }
        for id in [old_dish, old_named, just_now] {
            assert_eq!(status(&pos, id).0, "open");
        }
        // The table is free again; the cancelled ones stay out of history.
        assert!(pos.floor_status(&s, floor.id).unwrap()[0].order_id.is_none());
        assert!(pos.list_orders(&s, 0, i64::MAX, 100, 0).unwrap().is_empty());
        // At most every ten minutes...
        age(&pos, just_now, EMPTY_ORDER_AGE + 60);
        assert_eq!(pos.sweep_empty_orders_if_due().unwrap(), 0);
        assert_eq!(status(&pos, just_now).0, "open");
        // ...then again.
        pos.last_empty_sweep = Some(std::time::Instant::now() - EMPTY_SWEEP_EVERY);
        assert_eq!(pos.sweep_empty_orders_if_due().unwrap(), 1);
        assert_eq!(status(&pos, just_now).0, "cancelled");
        assert_eq!(pos.sweep_empty_orders().unwrap(), 0);
    }

    #[test]
    fn an_empty_order_counts_from_its_last_change_and_says_it_was_closed_automatically() {
        let mut pos = Pos::open_in_memory().unwrap();
        let s = pos.setup_first_admin("Owner", "1111").unwrap();
        let dish = pos
            .create_product(&s, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Soup".into(), price_cents: 500, cost_cents: 0, tax_rate_bp: Some(0),
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap()
            .id;
        let takeaway = |pos: &Pos| pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap().id;
        let opened_long_ago = |pos: &Pos, id: i64| {
            pos.conn.execute("UPDATE orders SET opened_at = unixepoch() - ?2, touched_at = NULL WHERE id = ?1", [id, EMPTY_ORDER_AGE + 600]).unwrap();
        };
        let touched = |pos: &Pos, id: i64| -> Option<i64> { pos.conn.query_row("SELECT touched_at FROM orders WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
        let status = |pos: &Pos, id: i64| -> String { pos.conn.query_row("SELECT status FROM orders WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
        // Opened long ago; its last dish was taken off just now (a waiter is on it).
        let emptied = takeaway(&pos);
        let line = pos.add_item(&s, emptied, dish, 1, None, None, &[]).unwrap().items[0].id;
        opened_long_ago(&pos, emptied);
        pos.update_item(&s, line, &ItemUpdate { qty: 0, discount_cents: 0, note: None }).unwrap();
        assert!(touched(&pos, emptied).is_some());
        // Opened long ago; a note typed and cleared again just now.
        let edited = takeaway(&pos);
        opened_long_ago(&pos, edited);
        pos.update_order_details(&s, edited, &OrderDetails { notes: Some("window seat".into()), ..Default::default() }).unwrap();
        pos.update_order_details(&s, edited, &OrderDetails::default()).unwrap();
        // Opened long ago, never changed: swept as before.
        let forgotten = takeaway(&pos);
        opened_long_ago(&pos, forgotten);
        // Opened long ago, saved since without any change (and an update of
        // every order that changes none, like the customers reset): swept too.
        let saved_unchanged = takeaway(&pos);
        opened_long_ago(&pos, saved_unchanged);
        pos.update_order_details(&s, saved_unchanged, &OrderDetails::default()).unwrap();
        pos.set_service_charge(&s, saved_unchanged, 0).unwrap();
        pos.conn.execute_batch("UPDATE orders SET customer_id = NULL, notes = notes, table_id = table_id").unwrap();
        for id in [forgotten, saved_unchanged] {
            assert_eq!(touched(&pos, id), None);
        }
        assert_eq!(pos.sweep_empty_orders().unwrap(), 2);
        assert_eq!(status(&pos, forgotten), "cancelled");
        assert_eq!(status(&pos, saved_unchanged), "cancelled");
        for id in [emptied, edited] {
            assert_eq!(status(&pos, id), "open");
        }
        // Their last change long ago too: swept.
        pos.conn.execute("UPDATE orders SET touched_at = unixepoch() - ?1 WHERE status = 'open'", [EMPTY_ORDER_AGE + 60]).unwrap();
        assert_eq!(pos.sweep_empty_orders().unwrap(), 2);
        // A till still showing a swept order is told what happened...
        let err = pos.add_item(&s, emptied, dish, 1, None, None, &[]).unwrap_err();
        assert_eq!(err.to_string(), EMPTY_ORDER_SWEPT);
        assert_eq!(err.to_string(), "this empty order was closed automatically");
        assert_eq!(pos.update_order_details(&s, forgotten, &OrderDetails::default()).unwrap_err().to_string(), EMPTY_ORDER_SWEPT);
        // ...while one a till cancelled itself is just closed.
        let left = takeaway(&pos);
        assert!(pos.cancel_if_empty(&s, left).unwrap());
        assert_eq!(pos.add_item(&s, left, dish, 1, None, None, &[]).unwrap_err().to_string(), "this order is already closed");
        // Closed orders are never touched again (the record keeps its last change).
        let paid = takeaway(&pos);
        pos.add_item(&s, paid, dish, 1, None, None, &[]).unwrap();
        pos.pay_order(&s, paid, PaymentMethod::Cash, 500).unwrap();
        pos.conn.execute("UPDATE orders SET touched_at = 1 WHERE id = ?1", [paid]).unwrap();
        pos.conn.execute("UPDATE order_items SET kitchen_done_at = 2 WHERE order_id = ?1", [paid]).unwrap();
        assert_eq!(touched(&pos, paid), Some(1));
    }

    #[test]
    fn tax_rounding() {
        assert_eq!(tax_of(1000, 1500), 150);
        assert_eq!(tax_of(333, 1500), 50); // 49.95 -> 50
        assert_eq!(tax_of(1, 1000), 0); // 0.1 -> 0
        assert_eq!(tax_of(5, 1000), 1); // 0.5 -> 1
        assert_eq!(tax_of(999, 0), 0);
    }
}

//! Restaurant orders: dine-in (on a table), takeaway, collection, delivery.
//!
//! Life cycle: `open_order` -> `add_item`/`update_item` -> `send_to_kitchen`
//! (any number of rounds) -> `pay_order`, or `cancel_order` if abandoned.
//! A paid order is the sales record; an admin can `void_order` it later.

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};

use crate::error::{invalid, map_unique};
use crate::options::{resolve_options, ItemOption};
use crate::{Permission, Pos, PosError, Result, Session};

const MAX_QTY: i64 = 1_000;
const MAX_ITEMS: i64 = 300;

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
    pub options: Vec<ItemOption>,
    /// None until sent to the kitchen.
    pub sent_at: Option<i64>,
    /// unit price x qty - discount, before tax.
    pub line_total_cents: i64,
    pub tax_cents: i64,
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
    pub total_cents: i64,
    pub payment_method: Option<String>,
    pub tendered_cents: Option<i64>,
    pub change_cents: Option<i64>,
    pub paid_by: Option<i64>,
    pub paid_at: Option<i64>,
    pub closed_at: Option<i64>,
    pub close_reason: Option<String>,
    pub items: Vec<OrderItem>,
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
    pub unsent_count: i64,
}

/// Tax on a pre-tax amount, rounded half up. Prices are tax-exclusive.
fn tax_of(amount: i64, rate_bp: i64) -> i64 {
    (amount * rate_bp + 5_000) / 10_000
}

fn parse_type(s: &str) -> OrderType {
    match s {
        "dine_in" => OrderType::DineIn,
        "collection" => OrderType::Collection,
        "delivery" => OrderType::Delivery,
        _ => OrderType::Takeaway,
    }
}

fn opt_text(v: &Option<String>, max: usize, what: &str) -> Result<Option<String>> {
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

fn require_open(conn: &Connection, id: i64) -> Result<()> {
    if status_of(conn, id)? != "open" {
        return invalid("this order is already closed");
    }
    Ok(())
}

fn check_table(conn: &Connection, table_id: i64) -> Result<()> {
    let ok: bool = conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM dining_tables WHERE id = ?1 AND active = 1)")?
        .query_row([table_id], |r| r.get(0))?;
    if !ok {
        return invalid("that table does not exist");
    }
    Ok(())
}

const TABLE_BUSY: &str = "that table already has an open order";

const ITEM_COLS: &str = "id, product_id, name, unit_price_cents, tax_rate_bp, qty, discount_cents, note, sent_at";

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
        options: Vec::new(),
        sent_at: r.get(8)?,
        line_total_cents: net,
        tax_cents: tax_of(net, rate),
    })
}

fn load_items(conn: &Connection, order_id: i64) -> Result<Vec<OrderItem>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {ITEM_COLS} FROM order_items WHERE order_id = ?1 ORDER BY id"))?;
    let mut items: Vec<OrderItem> = stmt.query_map([order_id], item_from_row)?.collect::<rusqlite::Result<_>>()?;
    let mut opts = conn.prepare_cached(
        "SELECT o.item_id, o.option_id, o.group_name, o.name, o.price_cents
         FROM order_item_options o JOIN order_items i ON i.id = o.item_id
         WHERE i.order_id = ?1 ORDER BY o.id",
    )?;
    let rows = opts.query_map([order_id], |r| {
        Ok((r.get::<_, i64>(0)?, ItemOption { option_id: r.get(1)?, group_name: r.get(2)?, name: r.get(3)?, price_cents: r.get(4)? }))
    })?;
    for row in rows {
        let (item_id, o) = row?;
        if let Some(it) = items.iter_mut().find(|i| i.id == item_id) {
            it.options.push(o);
        }
    }
    Ok(items)
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
    total: i64,
}

fn totals(items: &[OrderItem]) -> Totals {
    let subtotal = items.iter().map(|i| i.unit_price_cents * i.qty).sum();
    let discount = items.iter().map(|i| i.discount_cents).sum();
    let tax = items.iter().map(|i| i.tax_cents).sum();
    Totals { subtotal, discount, tax, total: subtotal - discount + tax }
}

pub(crate) const SUMMARY_SQL: &str = "
    SELECT o.id, o.order_type, o.status, t.name, f.name, o.customer_name, o.customer_phone, u.name,
           o.opened_at, o.paid_at, ch.name, o.external_ref,
           COALESCE(o.total_cents, (
               SELECT SUM(i.unit_price_cents * i.qty - i.discount_cents
                          + ((i.unit_price_cents * i.qty - i.discount_cents) * i.tax_rate_bp + 5000) / 10000)
               FROM order_items i WHERE i.order_id = o.id), 0),
           (SELECT COALESCE(SUM(qty), 0) FROM order_items i WHERE i.order_id = o.id),
           (SELECT COUNT(*) FROM order_items i WHERE i.order_id = o.id AND i.sent_at IS NULL)
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
    })
}

fn audit_tx(tx: &Transaction, user_id: i64, action: &str, detail: &str) -> Result<()> {
    tx.prepare_cached("INSERT INTO audit_log (user_id, action, detail) VALUES (?1, ?2, ?3)")?
        .execute(params![user_id, action, detail])?;
    Ok(())
}

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
        let tx = self.conn.unchecked_transaction()?;
        if let Some(c) = new.details.customer_id {
            self.touch_customer(c)?;
        }
        tx.prepare_cached(
            "INSERT INTO orders (order_type, table_id, customer_name, customer_phone, delivery_address, notes,
                                 channel_id, external_ref, customer_id, opened_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            s.user_id
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
                        o.channel_id, ch.name, o.external_ref, o.customer_id
                 FROM orders o JOIN users u ON u.id = o.opened_by
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
                    subtotal_cents: 0,
                    discount_cents: 0,
                    tax_cents: 0,
                    total_cents: 0,
                    items: Vec::new(),
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
        o.items = load_items(&self.conn, id)?;
        let t = totals(&o.items);
        (o.subtotal_cents, o.discount_cents, o.tax_cents, o.total_cents) = (t.subtotal, t.discount, t.tax, t.total);
        Ok(o)
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

    /// Add a menu item at its current price, with the chosen options
    /// (validated against the item's option groups). Repeated taps on the
    /// same item with the same options (no note, not yet sent) increase the
    /// quantity of one line.
    pub fn add_item(
        &mut self,
        s: &Session,
        order_id: i64,
        product_id: i64,
        qty: i64,
        note: Option<&str>,
        option_ids: &[i64],
    ) -> Result<Order> {
        s.require(Permission::Sell)?;
        if !(1..=MAX_QTY).contains(&qty) {
            return invalid(format!("quantity must be between 1 and {MAX_QTY}"));
        }
        let note = opt_text(&note.map(str::to_string), 100, "note")?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let (name, base, rate): (String, i64, i64) = tx
            .prepare_cached("SELECT name, price_cents, tax_rate_bp FROM products WHERE id = ?1 AND active = 1")?
            .query_row([product_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?
            .ok_or_else(|| PosError::Validation("that item is not available".into()))?;
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
                       AND discount_cents = 0 AND unit_price_cents = ?3 AND qty + ?4 <= ?5
                     ORDER BY id DESC",
                )?
                .query_map(params![order_id, product_id, price, qty, MAX_QTY], |r| r.get(0))?
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
                "INSERT INTO order_items (order_id, product_id, name, unit_price_cents, tax_rate_bp, qty, note, added_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?
            .execute(params![order_id, product_id, name, price, rate, qty, note, s.user_id])?;
            let item_id = tx.last_insert_rowid();
            let mut ins = tx.prepare_cached(
                "INSERT INTO order_item_options (item_id, option_id, group_name, name, price_cents)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for o in &options {
                ins.execute(params![item_id, o.option_id, o.group_name, o.name, o.price_cents])?;
            }
        }
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Change quantity, discount or note on a line; qty 0 removes it.
    /// Lines already sent to the kitchen can only be reduced or removed by
    /// an admin (it is food that has been made), and their note is fixed.
    pub fn update_item(&mut self, s: &Session, item_id: i64, upd: &ItemUpdate) -> Result<Order> {
        s.require(Permission::Sell)?;
        let note = opt_text(&upd.note, 100, "note")?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (order_id, cur): (i64, OrderItem) = tx
            .prepare_cached(&format!("SELECT order_id, {ITEM_COLS} FROM order_items WHERE id = ?1"))?
            .query_row([item_id], |r| {
                let it = OrderItem {
                    id: r.get(1)?,
                    product_id: r.get(2)?,
                    name: r.get(3)?,
                    unit_price_cents: r.get(4)?,
                    tax_rate_bp: r.get(5)?,
                    qty: r.get(6)?,
                    discount_cents: r.get(7)?,
                    note: r.get(8)?,
                    options: Vec::new(),
                    sent_at: r.get(9)?,
                    line_total_cents: 0,
                    tax_cents: 0,
                };
                Ok((r.get(0)?, it))
            })
            .optional()?
            .ok_or(PosError::NotFound)?;
        require_open(&tx, order_id)?;
        if !(0..=MAX_QTY).contains(&upd.qty) {
            return invalid(format!("quantity must be between 0 and {MAX_QTY}"));
        }
        if cur.sent_at.is_some() {
            if upd.qty > cur.qty {
                return invalid("this item was already sent to the kitchen; add it again instead");
            }
            if upd.qty < cur.qty {
                s.require(Permission::VoidOrder)?;
            }
            if note != cur.note {
                return invalid("the note cannot change after the item was sent to the kitchen");
            }
        }
        if upd.qty == 0 {
            tx.execute("DELETE FROM order_items WHERE id = ?1", [item_id])?;
        } else {
            if !(0..=cur.unit_price_cents * upd.qty).contains(&upd.discount_cents) {
                return invalid(format!("invalid discount on {}", cur.name));
            }
            tx.execute(
                "UPDATE order_items SET qty = ?2, discount_cents = ?3, note = ?4 WHERE id = ?1",
                params![item_id, upd.qty, upd.discount_cents, note],
            )?;
        }
        if cur.sent_at.is_some() && upd.qty < cur.qty {
            audit_tx(&tx, s.user_id, "order.item_void", &format!("order {order_id} {} x{}", cur.name, cur.qty - upd.qty))?;
        }
        if upd.discount_cents != cur.discount_cents && upd.qty > 0 {
            audit_tx(&tx, s.user_id, "order.discount", &format!("order {order_id} {} {}", cur.name, upd.discount_cents))?;
        }
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Mark all unsent lines as sent. Returns just the newly sent lines,
    /// i.e. the kitchen ticket for this round.
    pub fn send_to_kitchen(&mut self, s: &Session, order_id: i64) -> Result<Vec<OrderItem>> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let ids: Vec<i64> = tx
            .prepare_cached(
                "UPDATE order_items SET sent_at = unixepoch() WHERE order_id = ?1 AND sent_at IS NULL RETURNING id",
            )?
            .query_map([order_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let sent: Vec<OrderItem> = if ids.is_empty() {
            Vec::new()
        } else {
            load_items(&tx, order_id)?.into_iter().filter(|i| ids.contains(&i.id)).collect()
        };
        tx.commit()?;
        Ok(sent)
    }

    /// Take payment and close the order. Stock is taken for tracked items.
    pub fn pay_order(&mut self, s: &Session, order_id: i64, method: PaymentMethod, tendered_cents: i64) -> Result<Order> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let items = load_items(&tx, order_id)?;
        if items.is_empty() {
            return invalid("the order is empty");
        }
        let t = totals(&items);
        let tendered = match method {
            PaymentMethod::Cash if tendered_cents < t.total => return invalid("cash tendered is less than the total"),
            PaymentMethod::Cash => tendered_cents,
            _ => t.total,
        };
        tx.prepare_cached(
            "UPDATE orders SET status = 'paid', subtotal_cents = ?2, discount_cents = ?3, tax_cents = ?4,
                 total_cents = ?5, payment_method = ?6, tendered_cents = ?7, change_cents = ?8,
                 paid_by = ?9, paid_at = unixepoch()
             WHERE id = ?1",
        )?
        .execute(params![order_id, t.subtotal, t.discount, t.tax, t.total, method.as_str(), tendered, tendered - t.total, s.user_id])?;
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
        tx.commit()?;
        self.get_order(s, order_id)
    }

    /// Abandon an open order. If anything was already sent to the kitchen
    /// this needs an admin and a reason.
    pub fn cancel_order(&mut self, s: &Session, order_id: i64, reason: &str) -> Result<()> {
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
            if reason.is_empty() {
                return invalid("a reason is required to cancel an order already sent to the kitchen");
            }
        }
        tx.execute(
            "UPDATE orders SET status = 'cancelled', closed_by = ?2, closed_at = unixepoch(), close_reason = ?3
             WHERE id = ?1",
            params![order_id, s.user_id, (!reason.is_empty()).then_some(reason)],
        )?;
        audit_tx(&tx, s.user_id, "order.cancel", &format!("{order_id} {reason}"))?;
        tx.commit()?;
        Ok(())
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
               AND (?3 IS NULL OR o.opened_by = ?3 OR o.paid_by = ?3)
             ORDER BY o.opened_at DESC, o.id DESC LIMIT ?4 OFFSET ?5"
        ))?;
        let rows = stmt.query_map(params![from, to, only_user, limit.clamp(1, 500), offset.max(0)], summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::tax_of;

    #[test]
    fn tax_rounding() {
        assert_eq!(tax_of(1000, 1500), 150);
        assert_eq!(tax_of(333, 1500), 50); // 49.95 -> 50
        assert_eq!(tax_of(1, 1000), 0); // 0.1 -> 0
        assert_eq!(tax_of(5, 1000), 1); // 0.5 -> 1
        assert_eq!(tax_of(999, 0), 0);
    }
}

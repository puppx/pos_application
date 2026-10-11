use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{Permission, Pos, PosError, Result, Session};

const MAX_LINES: usize = 500;
const MAX_QTY: i64 = 100_000;

#[derive(Debug, Clone, Deserialize)]
pub struct CartLine {
    pub product_id: i64,
    pub qty: i64,
    #[serde(default)]
    pub discount_cents: i64,
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

#[derive(Debug, Clone, Serialize)]
pub struct QuoteLine {
    pub product_id: i64,
    pub name: String,
    pub unit_price_cents: i64,
    pub qty: i64,
    pub discount_cents: i64,
    pub tax_rate_bp: i64,
    pub tax_cents: i64,
    /// unit price x qty - discount, before tax.
    pub line_total_cents: i64,
    track_stock: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Quote {
    pub lines: Vec<QuoteLine>,
    pub subtotal_cents: i64,
    pub discount_cents: i64,
    pub tax_cents: i64,
    pub total_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SaleItem {
    pub product_id: Option<i64>,
    pub name: String,
    pub unit_price_cents: i64,
    pub qty: i64,
    pub discount_cents: i64,
    pub tax_rate_bp: i64,
    pub tax_cents: i64,
    pub line_total_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Sale {
    pub id: i64,
    pub user_id: i64,
    pub user_name: String,
    pub status: String,
    pub subtotal_cents: i64,
    pub discount_cents: i64,
    pub tax_cents: i64,
    pub total_cents: i64,
    pub payment_method: String,
    pub tendered_cents: i64,
    pub change_cents: i64,
    pub created_at: i64,
    pub voided_at: Option<i64>,
    pub void_reason: Option<String>,
    pub items: Vec<SaleItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SaleListItem {
    pub id: i64,
    pub user_name: String,
    pub status: String,
    pub total_cents: i64,
    pub payment_method: String,
    pub created_at: i64,
}

/// Tax on a pre-tax amount, rounded half up. Prices are tax-exclusive.
fn tax_of(amount: i64, rate_bp: i64) -> i64 {
    (amount * rate_bp + 5_000) / 10_000
}

/// Price a cart from the database. Client-side prices are never trusted.
fn compute_quote(conn: &Connection, lines: &[CartLine]) -> Result<Quote> {
    if lines.is_empty() {
        return invalid("the cart is empty");
    }
    if lines.len() > MAX_LINES {
        return invalid("too many lines in one sale");
    }
    let mut stmt = conn.prepare_cached(
        "SELECT name, price_cents, tax_rate_bp, track_stock FROM products WHERE id = ?1 AND active = 1",
    )?;
    let mut q = Quote { lines: Vec::with_capacity(lines.len()), subtotal_cents: 0, discount_cents: 0, tax_cents: 0, total_cents: 0 };
    for l in lines {
        if !(1..=MAX_QTY).contains(&l.qty) {
            return invalid(format!("quantity must be between 1 and {MAX_QTY}"));
        }
        let (name, price, rate, track): (String, i64, i64, bool) = stmt
            .query_row([l.product_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?
            .ok_or_else(|| PosError::Validation(format!("product {} is not available", l.product_id)))?;
        let gross = price * l.qty;
        if !(0..=gross).contains(&l.discount_cents) {
            return invalid(format!("invalid discount on {name}"));
        }
        let net = gross - l.discount_cents;
        let tax = tax_of(net, rate);
        q.subtotal_cents += gross;
        q.discount_cents += l.discount_cents;
        q.tax_cents += tax;
        q.lines.push(QuoteLine {
            product_id: l.product_id,
            name,
            unit_price_cents: price,
            qty: l.qty,
            discount_cents: l.discount_cents,
            tax_rate_bp: rate,
            tax_cents: tax,
            line_total_cents: net,
            track_stock: track,
        });
    }
    q.total_cents = q.subtotal_cents - q.discount_cents + q.tax_cents;
    Ok(q)
}

impl Pos {
    /// Totals for the current cart, for live display. Writes nothing.
    pub fn quote_sale(&self, s: &Session, lines: &[CartLine]) -> Result<Quote> {
        s.require(Permission::Sell)?;
        compute_quote(&self.conn, lines)
    }

    /// Record a sale and decrement stock in a single transaction.
    /// Stock may go negative: the counter should never be blocked by a
    /// miscounted shelf; the ledger shows where corrections are needed.
    pub fn complete_sale(
        &mut self,
        s: &Session,
        lines: &[CartLine],
        method: PaymentMethod,
        tendered_cents: i64,
    ) -> Result<Sale> {
        s.require(Permission::Sell)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let q = compute_quote(&tx, lines)?;
        let tendered = match method {
            PaymentMethod::Cash if tendered_cents < q.total_cents => {
                return invalid("cash tendered is less than the total");
            }
            PaymentMethod::Cash => tendered_cents,
            _ => q.total_cents,
        };
        tx.prepare_cached(
            "INSERT INTO sales (user_id, subtotal_cents, discount_cents, tax_cents, total_cents,
                                payment_method, tendered_cents, change_cents)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?
        .execute(params![
            s.user_id,
            q.subtotal_cents,
            q.discount_cents,
            q.tax_cents,
            q.total_cents,
            method.as_str(),
            tendered,
            tendered - q.total_cents
        ])?;
        let sale_id = tx.last_insert_rowid();
        {
            let mut item = tx.prepare_cached(
                "INSERT INTO sale_items (sale_id, product_id, name, unit_price_cents, qty, discount_cents,
                                         tax_rate_bp, tax_cents, line_total_cents)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            let mut stock =
                tx.prepare_cached("UPDATE products SET stock_qty = stock_qty - ?2 WHERE id = ?1")?;
            let mut movement = tx.prepare_cached(
                "INSERT INTO stock_movements (product_id, delta, reason, sale_id, user_id)
                 VALUES (?1, ?2, 'sale', ?3, ?4)",
            )?;
            for l in &q.lines {
                item.execute(params![
                    sale_id,
                    l.product_id,
                    l.name,
                    l.unit_price_cents,
                    l.qty,
                    l.discount_cents,
                    l.tax_rate_bp,
                    l.tax_cents,
                    l.line_total_cents
                ])?;
                if l.track_stock {
                    stock.execute(params![l.product_id, l.qty])?;
                    movement.execute(params![l.product_id, -l.qty, sale_id, s.user_id])?;
                }
            }
        }
        tx.commit()?;
        self.get_sale(s, sale_id)
    }

    pub fn get_sale(&self, s: &Session, id: i64) -> Result<Sale> {
        let mut sale = self
            .conn
            .prepare_cached(
                "SELECT s.id, s.user_id, u.name, s.status, s.subtotal_cents, s.discount_cents, s.tax_cents,
                        s.total_cents, s.payment_method, s.tendered_cents, s.change_cents, s.created_at,
                        s.voided_at, s.void_reason
                 FROM sales s JOIN users u ON u.id = s.user_id WHERE s.id = ?1",
            )?
            .query_row([id], |r| {
                Ok(Sale {
                    id: r.get(0)?,
                    user_id: r.get(1)?,
                    user_name: r.get(2)?,
                    status: r.get(3)?,
                    subtotal_cents: r.get(4)?,
                    discount_cents: r.get(5)?,
                    tax_cents: r.get(6)?,
                    total_cents: r.get(7)?,
                    payment_method: r.get(8)?,
                    tendered_cents: r.get(9)?,
                    change_cents: r.get(10)?,
                    created_at: r.get(11)?,
                    voided_at: r.get(12)?,
                    void_reason: r.get(13)?,
                    items: Vec::new(),
                })
            })
            .optional()?
            .ok_or(PosError::NotFound)?;
        if !s.role.can(Permission::ViewAllSales) {
            s.require(Permission::ViewOwnSales)?;
            if sale.user_id != s.user_id {
                return Err(PosError::Forbidden);
            }
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT product_id, name, unit_price_cents, qty, discount_cents, tax_rate_bp, tax_cents, line_total_cents
             FROM sale_items WHERE sale_id = ?1 ORDER BY id",
        )?;
        let items = stmt.query_map([id], |r| {
            Ok(SaleItem {
                product_id: r.get(0)?,
                name: r.get(1)?,
                unit_price_cents: r.get(2)?,
                qty: r.get(3)?,
                discount_cents: r.get(4)?,
                tax_rate_bp: r.get(5)?,
                tax_cents: r.get(6)?,
                line_total_cents: r.get(7)?,
            })
        })?;
        sale.items = items.collect::<rusqlite::Result<_>>()?;
        Ok(sale)
    }

    /// Sales in `[from, to)` (unix seconds), newest first. Staff only see
    /// their own sales.
    pub fn list_sales(&self, s: &Session, from: i64, to: i64, limit: i64, offset: i64) -> Result<Vec<SaleListItem>> {
        let only_user = if s.role.can(Permission::ViewAllSales) {
            None
        } else {
            s.require(Permission::ViewOwnSales)?;
            Some(s.user_id)
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT s.id, u.name, s.status, s.total_cents, s.payment_method, s.created_at
             FROM sales s JOIN users u ON u.id = s.user_id
             WHERE s.created_at >= ?1 AND s.created_at < ?2 AND (?3 IS NULL OR s.user_id = ?3)
             ORDER BY s.created_at DESC, s.id DESC LIMIT ?4 OFFSET ?5",
        )?;
        let rows = stmt.query_map(params![from, to, only_user, limit.clamp(1, 500), offset.max(0)], |r| {
            Ok(SaleListItem {
                id: r.get(0)?,
                user_name: r.get(1)?,
                status: r.get(2)?,
                total_cents: r.get(3)?,
                payment_method: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Cancel a completed sale and put its stock back.
    pub fn void_sale(&mut self, s: &Session, id: i64, reason: &str) -> Result<Sale> {
        s.require(Permission::VoidSale)?;
        let reason = reason.trim();
        if reason.is_empty() {
            return invalid("a reason is required to void a sale");
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE sales SET status = 'voided', voided_by = ?2, voided_at = unixepoch(), void_reason = ?3
             WHERE id = ?1 AND status = 'completed'",
            params![id, s.user_id, reason],
        )?;
        if changed == 0 {
            return invalid("sale not found or already voided");
        }
        // Reverse exactly what the sale's own stock movements took out.
        tx.execute(
            "UPDATE products SET stock_qty = stock_qty - (
                 SELECT SUM(m.delta) FROM stock_movements m
                 WHERE m.sale_id = ?1 AND m.reason = 'sale' AND m.product_id = products.id)
             WHERE id IN (SELECT product_id FROM stock_movements WHERE sale_id = ?1 AND reason = 'sale')",
            [id],
        )?;
        tx.execute(
            "INSERT INTO stock_movements (product_id, delta, reason, sale_id, user_id)
             SELECT product_id, -SUM(delta), 'void', sale_id, ?2 FROM stock_movements
             WHERE sale_id = ?1 AND reason = 'sale' GROUP BY product_id",
            params![id, s.user_id],
        )?;
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'sale.void', ?2)",
            params![s.user_id, format!("{id} {reason}")],
        )?;
        tx.commit()?;
        self.get_sale(s, id)
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

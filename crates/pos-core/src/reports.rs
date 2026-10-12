use rusqlite::params;
use serde::Serialize;

use crate::{Permission, Pos, Result, Session};

#[derive(Debug, Clone, Serialize)]
pub struct GroupTotal {
    /// Payment method ("cash", "card"...) or order type ("dine_in", "delivery"...).
    pub key: String,
    pub count: i64,
    pub total_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SalesSummary {
    pub from: i64,
    pub to: i64,
    pub order_count: i64,
    pub subtotal_cents: i64,
    pub discount_cents: i64,
    pub tax_cents: i64,
    pub total_cents: i64,
    pub voided_count: i64,
    pub voided_total_cents: i64,
    pub cancelled_count: i64,
    pub by_payment: Vec<GroupTotal>,
    pub by_type: Vec<GroupTotal>,
    /// Delivery platforms; orders taken directly are under "Direct".
    pub by_channel: Vec<GroupTotal>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopProduct {
    pub product_id: i64,
    pub name: String,
    pub qty: i64,
    pub revenue_cents: i64,
}

impl Pos {
    /// Totals for orders paid in `[from, to)` (unix seconds). The UI passes
    /// local-midnight boundaries for a "today" / end-of-day view.
    pub fn sales_summary(&self, s: &Session, from: i64, to: i64) -> Result<SalesSummary> {
        s.require(Permission::ViewReports)?;
        let mut sum = self
            .conn
            .prepare_cached(
                "SELECT
                    COUNT(*) FILTER (WHERE status = 'paid'),
                    COALESCE(SUM(subtotal_cents) FILTER (WHERE status = 'paid'), 0),
                    COALESCE(SUM(discount_cents) FILTER (WHERE status = 'paid'), 0),
                    COALESCE(SUM(tax_cents) FILTER (WHERE status = 'paid'), 0),
                    COALESCE(SUM(total_cents) FILTER (WHERE status = 'paid'), 0),
                    COUNT(*) FILTER (WHERE status = 'voided'),
                    COALESCE(SUM(total_cents) FILTER (WHERE status = 'voided'), 0)
                 FROM orders WHERE paid_at >= ?1 AND paid_at < ?2",
            )?
            .query_row(params![from, to], |r| {
                Ok(SalesSummary {
                    from,
                    to,
                    order_count: r.get(0)?,
                    subtotal_cents: r.get(1)?,
                    discount_cents: r.get(2)?,
                    tax_cents: r.get(3)?,
                    total_cents: r.get(4)?,
                    voided_count: r.get(5)?,
                    voided_total_cents: r.get(6)?,
                    cancelled_count: 0,
                    by_payment: Vec::new(),
                    by_type: Vec::new(),
                    by_channel: Vec::new(),
                })
            })?;
        sum.cancelled_count = self
            .conn
            .prepare_cached(
                "SELECT COUNT(*) FROM orders WHERE opened_at >= ?1 AND opened_at < ?2 AND status = 'cancelled'",
            )?
            .query_row(params![from, to], |r| r.get(0))?;
        for (col, out) in [("payment_method", &mut sum.by_payment), ("order_type", &mut sum.by_type)] {
            let mut stmt = self.conn.prepare_cached(&format!(
                "SELECT {col}, COUNT(*), SUM(total_cents) FROM orders
                 WHERE paid_at >= ?1 AND paid_at < ?2 AND status = 'paid'
                 GROUP BY {col} ORDER BY 3 DESC"
            ))?;
            let rows = stmt.query_map(params![from, to], |r| {
                Ok(GroupTotal { key: r.get(0)?, count: r.get(1)?, total_cents: r.get(2)? })
            })?;
            *out = rows.collect::<rusqlite::Result<_>>()?;
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT COALESCE(ch.name, 'Direct'), COUNT(*), SUM(o.total_cents)
             FROM orders o LEFT JOIN delivery_channels ch ON ch.id = o.channel_id
             WHERE o.paid_at >= ?1 AND o.paid_at < ?2 AND o.status = 'paid'
             GROUP BY o.channel_id ORDER BY 3 DESC",
        )?;
        let rows = stmt.query_map(params![from, to], |r| {
            Ok(GroupTotal { key: r.get(0)?, count: r.get(1)?, total_cents: r.get(2)? })
        })?;
        sum.by_channel = rows.collect::<rusqlite::Result<_>>()?;
        Ok(sum)
    }

    /// Best sellers by revenue in `[from, to)`.
    pub fn top_products(&self, s: &Session, from: i64, to: i64, limit: i64) -> Result<Vec<TopProduct>> {
        s.require(Permission::ViewReports)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT i.product_id, MAX(i.name), SUM(i.qty), SUM(i.unit_price_cents * i.qty - i.discount_cents)
             FROM orders o JOIN order_items i ON i.order_id = o.id
             WHERE o.paid_at >= ?1 AND o.paid_at < ?2 AND o.status = 'paid'
             GROUP BY i.product_id, i.variant_id ORDER BY 4 DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![from, to, limit.clamp(1, 100)], |r| {
            Ok(TopProduct { product_id: r.get(0)?, name: r.get(1)?, qty: r.get(2)?, revenue_cents: r.get(3)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

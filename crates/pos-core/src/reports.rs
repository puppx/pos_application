use rusqlite::params;
use serde::Serialize;

use crate::{Permission, Pos, Result, Session};

#[derive(Debug, Clone, Serialize)]
pub struct PaymentTotal {
    pub method: String,
    pub count: i64,
    pub total_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SalesSummary {
    pub from: i64,
    pub to: i64,
    pub sale_count: i64,
    pub subtotal_cents: i64,
    pub discount_cents: i64,
    pub tax_cents: i64,
    pub total_cents: i64,
    pub voided_count: i64,
    pub voided_total_cents: i64,
    pub by_payment: Vec<PaymentTotal>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopProduct {
    pub product_id: Option<i64>,
    pub name: String,
    pub qty: i64,
    pub revenue_cents: i64,
}

impl Pos {
    /// Totals for completed sales in `[from, to)` (unix seconds). The UI
    /// passes local-midnight boundaries for a "today" / "Z report" view.
    pub fn sales_summary(&self, s: &Session, from: i64, to: i64) -> Result<SalesSummary> {
        s.require(Permission::ViewReports)?;
        let mut sum = self
            .conn
            .prepare_cached(
                "SELECT
                    COUNT(*) FILTER (WHERE status = 'completed'),
                    COALESCE(SUM(subtotal_cents) FILTER (WHERE status = 'completed'), 0),
                    COALESCE(SUM(discount_cents) FILTER (WHERE status = 'completed'), 0),
                    COALESCE(SUM(tax_cents) FILTER (WHERE status = 'completed'), 0),
                    COALESCE(SUM(total_cents) FILTER (WHERE status = 'completed'), 0),
                    COUNT(*) FILTER (WHERE status = 'voided'),
                    COALESCE(SUM(total_cents) FILTER (WHERE status = 'voided'), 0)
                 FROM sales WHERE created_at >= ?1 AND created_at < ?2",
            )?
            .query_row(params![from, to], |r| {
                Ok(SalesSummary {
                    from,
                    to,
                    sale_count: r.get(0)?,
                    subtotal_cents: r.get(1)?,
                    discount_cents: r.get(2)?,
                    tax_cents: r.get(3)?,
                    total_cents: r.get(4)?,
                    voided_count: r.get(5)?,
                    voided_total_cents: r.get(6)?,
                    by_payment: Vec::new(),
                })
            })?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT payment_method, COUNT(*), SUM(total_cents) FROM sales
             WHERE created_at >= ?1 AND created_at < ?2 AND status = 'completed'
             GROUP BY payment_method ORDER BY payment_method",
        )?;
        let rows = stmt.query_map(params![from, to], |r| {
            Ok(PaymentTotal { method: r.get(0)?, count: r.get(1)?, total_cents: r.get(2)? })
        })?;
        sum.by_payment = rows.collect::<rusqlite::Result<_>>()?;
        Ok(sum)
    }

    /// Best sellers by revenue in `[from, to)`.
    pub fn top_products(&self, s: &Session, from: i64, to: i64, limit: i64) -> Result<Vec<TopProduct>> {
        s.require(Permission::ViewReports)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT i.product_id, MAX(i.name), SUM(i.qty), SUM(i.line_total_cents)
             FROM sales s JOIN sale_items i ON i.sale_id = s.id
             WHERE s.created_at >= ?1 AND s.created_at < ?2 AND s.status = 'completed'
             GROUP BY i.product_id ORDER BY 4 DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![from, to, limit.clamp(1, 100)], |r| {
            Ok(TopProduct { product_id: r.get(0)?, name: r.get(1)?, qty: r.get(2)?, revenue_cents: r.get(3)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

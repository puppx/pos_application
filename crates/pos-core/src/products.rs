use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{invalid, map_unique};
use crate::{Permission, Pos, PosError, Result, Session};

#[derive(Debug, Clone, Serialize)]
pub struct Product {
    pub id: i64,
    pub category_id: Option<i64>,
    pub sku: Option<String>,
    pub barcode: Option<String>,
    pub name: String,
    pub price_cents: i64,
    pub cost_cents: i64,
    pub tax_rate_bp: i64,
    pub stock_qty: i64,
    pub track_stock: bool,
    pub active: bool,
    /// Sent to the kitchen first on dine-in orders (see `send_to_kitchen`).
    pub starter: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewProduct {
    #[serde(default)]
    pub category_id: Option<i64>,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub barcode: Option<String>,
    pub name: String,
    pub price_cents: i64,
    #[serde(default)]
    pub cost_cents: i64,
    #[serde(default)]
    pub tax_rate_bp: i64,
    #[serde(default)]
    pub stock_qty: i64,
    /// Off by default: most dishes are not counted; bottled drinks may be.
    #[serde(default)]
    pub track_stock: bool,
    #[serde(default)]
    pub starter: bool,
}

/// Stock is not editable here: it only changes through sales and
/// `adjust_stock`, so the movement ledger always adds up.
#[derive(Debug, Clone, Deserialize)]
pub struct ProductUpdate {
    #[serde(default)]
    pub category_id: Option<i64>,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub barcode: Option<String>,
    pub name: String,
    pub price_cents: i64,
    #[serde(default)]
    pub cost_cents: i64,
    #[serde(default)]
    pub tax_rate_bp: i64,
    #[serde(default)]
    pub track_stock: bool,
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub starter: bool,
}

fn yes() -> bool {
    true
}

const COLS: &str =
    "p.id, p.category_id, p.sku, p.barcode, p.name, p.price_cents, p.cost_cents, p.tax_rate_bp, p.stock_qty, p.track_stock, p.active, p.starter";
const CODE_TAKEN: &str = "SKU or barcode is already used by another product";
const MAX_LIMIT: i64 = 500;

pub(crate) fn product_from_row(r: &Row) -> rusqlite::Result<Product> {
    Ok(Product {
        id: r.get(0)?,
        category_id: r.get(1)?,
        sku: r.get(2)?,
        barcode: r.get(3)?,
        name: r.get(4)?,
        price_cents: r.get(5)?,
        cost_cents: r.get(6)?,
        tax_rate_bp: r.get(7)?,
        stock_qty: r.get(8)?,
        track_stock: r.get(9)?,
        active: r.get(10)?,
        starter: r.get(11)?,
    })
}

fn clean_code(code: Option<String>) -> Option<String> {
    code.map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
}

fn validate(name: &str, price: i64, cost: i64, tax_bp: i64) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return invalid("product name must be 1 to 120 characters");
    }
    if price < 0 || cost < 0 {
        return invalid("prices cannot be negative");
    }
    if !(0..=10_000).contains(&tax_bp) {
        return invalid("tax rate must be between 0% and 100%");
    }
    Ok(name.to_string())
}

impl Pos {
    /// Search active products by name/SKU/barcode for the sell screen.
    /// 1-2 characters: indexed name-prefix match. 3+: trigram full-text
    /// substring match. Both stay fast with 100k+ products.
    pub fn search_products(&self, s: &Session, query: &str, limit: i64) -> Result<Vec<Product>> {
        s.require(Permission::Sell)?;
        let limit = limit.clamp(1, MAX_LIMIT);
        let q = query.trim();
        let mut stmt;
        let rows = if q.chars().count() >= 3 {
            stmt = self.conn.prepare_cached(&format!(
                "SELECT {COLS} FROM products_fts f JOIN products p ON p.id = f.rowid
                 WHERE products_fts MATCH ?1 AND p.active = 1
                 ORDER BY rank LIMIT ?2"
            ))?;
            let phrase = format!("\"{}\"", q.replace('"', "\"\""));
            stmt.query_map(params![phrase, limit], product_from_row)?
        } else {
            stmt = self.conn.prepare_cached(&format!(
                "SELECT {COLS} FROM products p
                 WHERE p.active = 1 AND p.name LIKE ?1 ESCAPE '\\'
                 ORDER BY p.name LIMIT ?2"
            ))?;
            let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            stmt.query_map(params![format!("{escaped}%"), limit], product_from_row)?
        };
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Exact barcode or SKU lookup, for scanners. Single index probe.
    pub fn find_by_code(&self, s: &Session, code: &str) -> Result<Option<Product>> {
        s.require(Permission::Sell)?;
        let code = code.trim();
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM products p WHERE p.barcode = ?1 AND p.active = 1
             UNION ALL
             SELECT {COLS} FROM products p WHERE p.sku = ?1 AND p.active = 1
             LIMIT 1"
        ))?;
        Ok(stmt.query_row([code], product_from_row).optional()?)
    }

    pub fn get_product(&self, s: &Session, id: i64) -> Result<Product> {
        s.require(Permission::Sell)?;
        self.conn
            .prepare_cached(&format!("SELECT {COLS} FROM products p WHERE p.id = ?1"))?
            .query_row([id], product_from_row)
            .optional()?
            .ok_or(PosError::NotFound)
    }

    pub fn list_products(
        &self,
        s: &Session,
        include_inactive: bool,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Product>> {
        s.require(Permission::ManageMenu)?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM products p WHERE (?1 OR p.active = 1)
             ORDER BY p.name LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(
            params![include_inactive, limit.clamp(1, MAX_LIMIT), offset.max(0)],
            product_from_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_product(&mut self, s: &Session, p: NewProduct) -> Result<Product> {
        s.require(Permission::ManageMenu)?;
        let name = validate(&p.name, p.price_cents, p.cost_cents, p.tax_rate_bp)?;
        self.check_category(p.category_id)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO products (sku, barcode, name, price_cents, cost_cents, tax_rate_bp, stock_qty, track_stock, category_id, starter)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                clean_code(p.sku),
                clean_code(p.barcode),
                name,
                p.price_cents,
                p.cost_cents,
                p.tax_rate_bp,
                p.stock_qty,
                p.track_stock,
                p.category_id,
                p.starter
            ],
        )
        .map_err(|e| map_unique(e, CODE_TAKEN))?;
        let id = tx.last_insert_rowid();
        if p.stock_qty != 0 {
            tx.execute(
                "INSERT INTO stock_movements (product_id, delta, reason, user_id) VALUES (?1, ?2, 'initial', ?3)",
                params![id, p.stock_qty, s.user_id],
            )?;
        }
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'product.create', ?2)",
            params![s.user_id, format!("{id} {name}")],
        )?;
        tx.commit()?;
        self.get_product(s, id)
    }

    pub fn update_product(&self, s: &Session, id: i64, p: ProductUpdate) -> Result<Product> {
        s.require(Permission::ManageMenu)?;
        let name = validate(&p.name, p.price_cents, p.cost_cents, p.tax_rate_bp)?;
        self.check_category(p.category_id)?;
        let changed = self
            .conn
            .execute(
                "UPDATE products SET sku = ?2, barcode = ?3, name = ?4, price_cents = ?5, cost_cents = ?6,
                     tax_rate_bp = ?7, track_stock = ?8, active = ?9, category_id = ?10, starter = ?11, updated_at = unixepoch()
                 WHERE id = ?1",
                params![
                    id,
                    clean_code(p.sku),
                    clean_code(p.barcode),
                    name,
                    p.price_cents,
                    p.cost_cents,
                    p.tax_rate_bp,
                    p.track_stock,
                    p.active,
                    p.category_id,
                    p.starter
                ],
            )
            .map_err(|e| map_unique(e, CODE_TAKEN))?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "product.update", &format!("{id} {name}"))?;
        self.get_product(s, id)
    }

    /// Quick "Starter" toggle from the menu list.
    pub fn set_starter(&self, s: &Session, id: i64, starter: bool) -> Result<()> {
        s.require(Permission::ManageMenu)?;
        let changed = self
            .conn
            .execute("UPDATE products SET starter = ?2, updated_at = unixepoch() WHERE id = ?1", params![id, starter])?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "product.starter", &format!("{id} {starter}"))
    }

    /// Manual stock change (delivery, count correction, damage...).
    pub fn adjust_stock(&mut self, s: &Session, id: i64, delta: i64, reason: &str) -> Result<Product> {
        s.require(Permission::AdjustStock)?;
        let reason = reason.trim();
        if delta == 0 || reason.is_empty() {
            return invalid("a non-zero quantity and a reason are required");
        }
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE products SET stock_qty = stock_qty + ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, delta],
        )?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        tx.execute(
            "INSERT INTO stock_movements (product_id, delta, reason, user_id) VALUES (?1, ?2, ?3, ?4)",
            params![id, delta, reason, s.user_id],
        )?;
        tx.commit()?;
        self.get_product(s, id)
    }
}

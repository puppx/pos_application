use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

use crate::error::{invalid, map_unique};
use crate::products::product_from_row;
use crate::{Permission, Pos, PosError, Product, Result, Session};

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub color: Option<String>,
    pub active: bool,
}

const NAME_TAKEN: &str = "a category with that name already exists";

fn category_from_row(r: &Row) -> rusqlite::Result<Category> {
    Ok(Category {
        id: r.get(0)?,
        name: r.get(1)?,
        sort_order: r.get(2)?,
        color: r.get(3)?,
        active: r.get(4)?,
    })
}

fn clean(name: &str, color: Option<&str>) -> Result<(String, Option<String>)> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return invalid("category name must be 1 to 40 characters");
    }
    let color = color.map(str::trim).filter(|c| !c.is_empty());
    if let Some(c) = color {
        let hex = c.strip_prefix('#').unwrap_or("");
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return invalid("colour must look like #1a2b3c");
        }
    }
    Ok((name.to_string(), color.map(str::to_string)))
}

impl Pos {
    pub fn list_categories(&self, s: &Session, include_inactive: bool) -> Result<Vec<Category>> {
        s.require(Permission::Sell)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, sort_order, color, active FROM categories
             WHERE ?1 OR active = 1 ORDER BY sort_order, name",
        )?;
        let rows = stmt.query_map([include_inactive], category_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_category(&self, s: &Session, name: &str, color: Option<&str>, sort_order: i64) -> Result<Category> {
        s.require(Permission::ManageMenu)?;
        let (name, color) = clean(name, color)?;
        self.conn
            .execute(
                "INSERT INTO categories (name, color, sort_order) VALUES (?1, ?2, ?3)",
                params![name, color, sort_order],
            )
            .map_err(|e| map_unique(e, NAME_TAKEN))?;
        let id = self.conn.last_insert_rowid();
        self.audit(Some(s.user_id), "category.create", &format!("{id} {name}"))?;
        self.get_category(id)
    }

    pub fn update_category(
        &self,
        s: &Session,
        id: i64,
        name: &str,
        color: Option<&str>,
        sort_order: i64,
        active: bool,
    ) -> Result<Category> {
        s.require(Permission::ManageMenu)?;
        let (name, color) = clean(name, color)?;
        let changed = self
            .conn
            .execute(
                "UPDATE categories SET name = ?2, color = ?3, sort_order = ?4, active = ?5 WHERE id = ?1",
                params![id, name, color, sort_order, active],
            )
            .map_err(|e| map_unique(e, NAME_TAKEN))?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "category.update", &format!("{id} {name} active={active}"))?;
        self.get_category(id)
    }

    fn get_category(&self, id: i64) -> Result<Category> {
        self.conn
            .prepare_cached("SELECT id, name, sort_order, color, active FROM categories WHERE id = ?1")?
            .query_row([id], category_from_row)
            .optional()?
            .ok_or(PosError::NotFound)
    }

    pub(crate) fn check_category(&self, id: Option<i64>) -> Result<()> {
        if let Some(id) = id {
            let ok: bool = self
                .conn
                .prepare_cached("SELECT EXISTS (SELECT 1 FROM categories WHERE id = ?1 AND active = 1)")?
                .query_row([id], |r| r.get(0))?;
            if !ok {
                return invalid("that category does not exist");
            }
        }
        Ok(())
    }

    /// Every sellable item, ordered for the menu-button grid. The UI loads
    /// this once per screen and filters locally, so tapping is instant.
    pub fn menu(&self, s: &Session) -> Result<Vec<Product>> {
        s.require(Permission::Sell)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.category_id, p.sku, p.barcode, p.name, p.price_cents, p.cost_cents,
                    p.tax_rate_bp, p.stock_qty, p.track_stock, p.active
             FROM products p LEFT JOIN categories c ON c.id = p.category_id AND c.active = 1
             WHERE p.active = 1
             ORDER BY c.sort_order IS NULL, c.sort_order, c.name, p.name",
        )?;
        let rows = stmt.query_map([], product_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

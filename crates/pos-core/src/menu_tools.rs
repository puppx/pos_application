//! Tools for building the menu quickly: category defaults, ordering by drag,
//! duplicating an item, and importing many items at once (pasted from a
//! spreadsheet).

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{invalid, map_unique};
use crate::{Category, Permission, Pos, PosError, Product, Result, Session};

/// Setting (Settings > Menu): tax rate for new items, in basis points.
pub(crate) const DEFAULT_TAX_KEY: &str = "default_tax_bp";
/// Setting: "1" (or unset) = menu prices include tax, "0" = tax is added on top.
pub(crate) const INCLUDE_TAX_KEY: &str = "prices_include_tax";
const MAX_IMPORT: usize = 2_000;
/// The tax an item would get today (`?1` = the main default).
const RATE: &str = "COALESCE((SELECT c.tax_rate_bp FROM categories c WHERE c.id = p.category_id), ?1)";
/// Live items at 0% that would get a rate above 0.
const UNTAXED: &str = "p.deleted = 0 AND p.tax_rate_bp = 0
    AND COALESCE((SELECT c.tax_rate_bp FROM categories c WHERE c.id = p.category_id), ?1) > 0";

/// One line of a pasted menu.
#[derive(Debug, Clone, Deserialize)]
pub struct ImportRow {
    pub name: String,
    pub price_cents: i64,
    /// Category name; created when it does not exist yet.
    #[serde(default)]
    pub category: Option<String>,
    /// None = the category's default, else the main default.
    #[serde(default)]
    pub tax_rate_bp: Option<i64>,
    /// Thai name.
    #[serde(default)]
    pub name_alt: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub items: usize,
    /// Names of the categories that were created.
    pub new_categories: Vec<String>,
}

impl Pos {
    /// Menu prices already include the tax (Settings > Menu defaults). On by
    /// default; read when an order is opened, so changing it only affects
    /// new orders.
    pub fn prices_include_tax(&self) -> Result<bool> {
        Ok(self.setting(INCLUDE_TAX_KEY)?.is_none_or(|v| v.trim() != "0"))
    }

    /// The main default tax rate for new items (Settings > Menu).
    pub fn default_tax_bp(&self) -> Result<i64> {
        Ok(self.setting(DEFAULT_TAX_KEY)?.and_then(|v| v.trim().parse().ok()).filter(|v| (0..=10_000).contains(v)).unwrap_or(0))
    }

    /// Menu items still at 0% tax whose category (or, without one, the main
    /// default) has a rate: what [`Pos::apply_default_tax`] would change.
    pub fn untaxed_items(&self, s: &Session) -> Result<i64> {
        s.require(Permission::ManageMenu)?;
        let default = self.default_tax_bp()?;
        Ok(self.conn.query_row(&format!("SELECT COUNT(*) FROM products p WHERE {UNTAXED}"), [default], |r| r.get(0))?)
    }

    /// Give items left at 0% tax (never set up) the tax they would get today:
    /// their category's rate, else the main default. Categories set to 0%
    /// keep their items at 0%. Open bills keep the tax they were rung up at.
    pub fn apply_default_tax(&self, s: &Session) -> Result<usize> {
        s.require(Permission::ManageMenu)?;
        let default = self.default_tax_bp()?;
        let changed = self.conn.execute(
            &format!("UPDATE products AS p SET tax_rate_bp = {RATE}, updated_at = unixepoch() WHERE {UNTAXED}"),
            [default],
        )?;
        self.audit(Some(s.user_id), "menu.apply_default_tax", &format!("{changed} items, default {default}"))?;
        Ok(changed)
    }

    /// Defaults for new items in a category, and its Thai name.
    pub fn set_category_defaults(
        &self,
        s: &Session,
        id: i64,
        tax_rate_bp: Option<i64>,
        starter: bool,
        prep: bool,
        name_alt: Option<&str>,
    ) -> Result<Category> {
        s.require(Permission::ManageMenu)?;
        if tax_rate_bp.is_some_and(|t| !(0..=10_000).contains(&t)) {
            return invalid("tax rate must be between 0% and 100%");
        }
        let name_alt = crate::products::clean_alt(name_alt, 40)?;
        let changed = self.conn.execute(
            "UPDATE categories SET tax_rate_bp = ?2, starter = ?3, name_alt = ?4, prep = ?5 WHERE id = ?1",
            params![id, tax_rate_bp, starter, name_alt, prep],
        )?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "category.defaults", &format!("{id} tax={tax_rate_bp:?} starter={starter} prep={prep}"))?;
        self.list_categories(s, true)?.into_iter().find(|c| c.id == id).ok_or(PosError::NotFound)
    }

    /// Put categories in this order (the order of `ids`).
    pub fn reorder_categories(&mut self, s: &Session, ids: &[i64]) -> Result<()> {
        s.require(Permission::ManageMenu)?;
        let tx = self.conn.transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute("UPDATE categories SET sort_order = ?2 WHERE id = ?1", params![id, i as i64 + 1])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Put items in this order (the order of `ids`, normally one category).
    pub fn reorder_products(&mut self, s: &Session, ids: &[i64]) -> Result<()> {
        s.require(Permission::ManageMenu)?;
        let tx = self.conn.transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute("UPDATE products SET sort_order = ?2 WHERE id = ?1", params![id, i as i64 + 1])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Copy an item with its choices and options, e.g. Chicken Burger ->
    /// Beef Burger. The copy is named "<name> (copy)", has no SKU or
    /// barcode and no stock, and sits right after the original.
    pub fn duplicate_product(&mut self, s: &Session, id: i64) -> Result<Product> {
        s.require(Permission::ManageMenu)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let found: Option<(String, i64)> = tx
            .query_row("SELECT name, sort_order FROM products WHERE id = ?1 AND deleted = 0", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let (name, sort) = found.ok_or(PosError::NotFound)?;
        let copy: String = format!("{name} (copy)").chars().take(120).collect();
        tx.execute(
            "UPDATE products SET sort_order = sort_order + 1
             WHERE category_id IS (SELECT category_id FROM products WHERE id = ?1) AND sort_order > ?2",
            params![id, sort],
        )?;
        tx.execute(
            "INSERT INTO products (category_id, name, price_cents, cost_cents, tax_rate_bp, track_stock, starter, sort_order, name_alt, prep, team_id)
             SELECT category_id, ?2, price_cents, cost_cents, tax_rate_bp, track_stock, starter, sort_order + 1, name_alt, prep, team_id
             FROM products WHERE id = ?1",
            params![id, copy],
        )?;
        let new_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO product_variants (product_id, name, price_cents, sort_order, name_alt)
             SELECT ?2, name, price_cents, sort_order, name_alt FROM product_variants WHERE product_id = ?1 AND active = 1",
            params![id, new_id],
        )?;
        tx.execute(
            "INSERT INTO product_option_groups (product_id, group_id, sort_order)
             SELECT ?2, group_id, sort_order FROM product_option_groups WHERE product_id = ?1",
            params![id, new_id],
        )?;
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'product.duplicate', ?2)",
            params![s.user_id, format!("{id} -> {new_id}")],
        )?;
        tx.commit()?;
        self.get_product(s, new_id)
    }

    /// Add many items at once. Categories are matched by name (any case)
    /// and created when missing; each item gets its category's defaults.
    /// All or nothing: one bad line rejects the whole import.
    pub fn import_items(&mut self, s: &Session, rows: &[ImportRow]) -> Result<ImportReport> {
        s.require(Permission::ManageMenu)?;
        if rows.is_empty() {
            return invalid("there is nothing to import");
        }
        if rows.len() > MAX_IMPORT {
            return invalid(format!("import at most {MAX_IMPORT} items at a time"));
        }
        let default_tax = self.default_tax_bp()?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // name (lower case) -> (id, tax, starter, prep, team)
        let mut cats: HashMap<String, CatRow> = tx
            .prepare("SELECT id, name, tax_rate_bp, starter, prep, team_id FROM categories WHERE active = 1")?
            .query_map([], |r| Ok((r.get::<_, String>(1)?.to_lowercase(), (r.get(0)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))))?
            .collect::<rusqlite::Result<_>>()?;
        let mut report = ImportReport { items: 0, new_categories: vec![] };
        for (line, row) in rows.iter().enumerate() {
            let at = |msg: String| PosError::Validation(format!("line {}: {msg}", line + 1));
            let cat = match row.category.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                None => None,
                Some(c) => Some(match cats.get(&c.to_lowercase()) {
                    Some(found) => *found,
                    None => {
                        let (name, _) = crate::menu::clean(c, None).map_err(|e| at(e.to_string()))?;
                        tx.execute(
                            "INSERT INTO categories (name, sort_order) VALUES (?1, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM categories))",
                            [&name],
                        )
                        .map_err(|e| map_unique(e, "duplicate category"))?;
                        let entry = (tx.last_insert_rowid(), None, false, false, None);
                        cats.insert(name.to_lowercase(), entry);
                        report.new_categories.push(name);
                        entry
                    }
                }),
            };
            let tax = row.tax_rate_bp.or(cat.and_then(|c| c.1)).unwrap_or(default_tax);
            let name = crate::products::validate(&row.name, row.price_cents, 0, tax).map_err(|e| at(e.to_string()))?;
            let category_id = cat.map(|c| c.0);
            let alt = crate::products::clean_alt(row.name_alt.as_deref(), 120).map_err(|e| at(e.to_string()))?;
            tx.execute(
                "INSERT INTO products (category_id, name, price_cents, tax_rate_bp, starter, sort_order, name_alt, prep, team_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM products WHERE category_id IS ?1), ?6, ?7, ?8)",
                params![category_id, name, row.price_cents, tax, cat.is_some_and(|c| c.2), alt, cat.is_some_and(|c| c.3), cat.and_then(|c| c.4)],
            )?;
            report.items += 1;
        }
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'menu.import', ?2)",
            params![s.user_id, format!("{} items, {} new categories", report.items, report.new_categories.len())],
        )?;
        tx.commit()?;
        Ok(report)
    }
}

/// A category found while importing: id, tax, starter, prep, team.
type CatRow = (i64, Option<i64>, bool, bool, Option<i64>);

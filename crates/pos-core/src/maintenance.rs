//! Menu-only backups and "Reset data".
//!
//! A menu backup is a small JSON file (categories, items with their choices
//! and options, option groups) that can be restored on this till or copied
//! to another one. Restoring replaces the current menu.
//!
//! Reset deletes the kinds of data the admin chose and keeps the rest. Rows
//! that kept data still points to (an old order's items, tables or staff)
//! are retired instead of deleted, so history stays intact; nothing is
//! deleted that a kept row needs.

use std::collections::HashMap;

use rusqlite::{params, Transaction};
use serde::{Deserialize, Serialize};

use crate::dayclose::{local_midnight, parse_date};
use crate::error::invalid;
use crate::{Permission, Pos, PosError, Result, Session};

const MENU_FORMAT: &str = "pos-menu";
const MAX_ITEMS: usize = 5_000;

// ------------------------------------------------------------ menu backup

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuBackup {
    /// Always "pos-menu", so other JSON files are refused.
    pub format: String,
    pub version: u32,
    /// Unix seconds.
    pub exported_at: i64,
    /// Kitchen teams (v0.8.2+; absent in older backups).
    #[serde(default)]
    pub teams: Vec<MenuTeam>,
    pub categories: Vec<MenuCategory>,
    pub option_groups: Vec<MenuOptionGroup>,
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuTeam {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_alt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuCategory {
    /// Reference used by items in this file only.
    pub key: i64,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub sort_order: i64,
    /// Default tax for new items in this category (None = main default).
    #[serde(default)]
    pub tax_rate_bp: Option<i64>,
    /// New items in this category are starters.
    #[serde(default)]
    pub starter: bool,
    /// Thai name (v0.7.4+; absent in older backups).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_alt: Option<String>,
    /// Prepare ahead (v0.7.9+).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prep: bool,
    /// Kitchen team, by name (restoring creates any that are missing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuOptionGroup {
    pub key: i64,
    pub name: String,
    pub min_select: i64,
    pub max_select: i64,
    #[serde(default)]
    pub options: Vec<MenuPrice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuPrice {
    pub name: String,
    pub price_cents: i64,
    /// Thai name (v0.7.4+; absent in older backups).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_alt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuItem {
    pub name: String,
    #[serde(default)]
    pub category: Option<i64>,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub barcode: Option<String>,
    pub price_cents: i64,
    #[serde(default)]
    pub cost_cents: i64,
    #[serde(default)]
    pub tax_rate_bp: i64,
    #[serde(default)]
    pub track_stock: bool,
    #[serde(default)]
    pub stock_qty: i64,
    #[serde(default)]
    pub starter: bool,
    /// Choices with their own price (soup: chicken / beef).
    #[serde(default)]
    pub choices: Vec<MenuPrice>,
    /// Option group keys, in display order.
    #[serde(default)]
    pub option_groups: Vec<i64>,
    /// Thai name (v0.7.4+; absent in older backups).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_alt: Option<String>,
    /// Prepare ahead (v0.7.9+).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prep: bool,
    /// Kitchen team, by name (restoring creates any that are missing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
}

/// What a menu restore created.
#[derive(Debug, Clone, Serialize)]
pub struct MenuCounts {
    pub categories: usize,
    pub items: usize,
    pub option_groups: usize,
}

// ------------------------------------------------------------------ reset

/// What to delete; everything left false / None is kept.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResetOptions {
    /// Orders, payments, kitchen history and day closes.
    #[serde(default)]
    pub sales: Option<SalesReset>,
    #[serde(default)]
    pub customers: bool,
    /// Categories, items, choices, option groups, stock counts.
    #[serde(default)]
    pub menu: bool,
    /// Floors and tables.
    #[serde(default)]
    pub floor_plan: bool,
    /// Every user except the admin doing the reset.
    #[serde(default)]
    pub staff: bool,
    /// Store name, printers, receipt text, delivery platforms, display and
    /// local server settings.
    #[serde(default)]
    pub settings: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "when", rename_all = "snake_case")]
pub enum SalesReset {
    /// Every order, including open ones; the business day restarts today.
    All,
    /// Closed orders opened before this date ("YYYY-MM-DD"), and day closes
    /// before it. Open orders are kept.
    Before { date: String },
}

/// Rows removed by a reset, for the confirmation message.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ResetReport {
    pub orders: usize,
    pub customers: usize,
    pub items: usize,
    pub tables: usize,
    pub staff: usize,
}

fn count(tx: &Transaction, sql: &str) -> Result<usize> {
    Ok(tx.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
}

/// Retire the whole menu: delete what nothing points to, deactivate the rest
/// (freeing its names, SKUs and barcodes for new items).
fn clear_menu(tx: &Transaction) -> Result<usize> {
    let items = count(tx, "SELECT COUNT(*) FROM products WHERE active = 1")?;
    tx.execute_batch(
        "DELETE FROM product_option_groups;
         DELETE FROM stock_movements WHERE order_id IS NULL;
         DELETE FROM product_variants WHERE id NOT IN (SELECT variant_id FROM order_items WHERE variant_id IS NOT NULL);
         UPDATE product_variants SET active = 0;
         DELETE FROM products WHERE id NOT IN (SELECT product_id FROM order_items)
             AND id NOT IN (SELECT product_id FROM stock_movements)
             AND id NOT IN (SELECT product_id FROM product_variants);
         UPDATE products SET active = 0, deleted = 1, sku = NULL, barcode = NULL, category_id = NULL, updated_at = unixepoch();
         DELETE FROM categories;
         DELETE FROM options WHERE id NOT IN (SELECT option_id FROM order_item_options);
         UPDATE options SET active = 0;
         DELETE FROM option_groups WHERE id NOT IN (SELECT group_id FROM options);
         UPDATE option_groups SET active = 0, deleted = 1;",
    )?;
    Ok(items)
}

impl Pos {
    /// The current menu as a backup file.
    pub fn export_menu(&self, s: &Session) -> Result<MenuBackup> {
        s.require(Permission::ManageMenu)?;
        let categories = self
            .conn
            .prepare("SELECT c.id, c.name, c.color, c.sort_order, c.tax_rate_bp, c.starter, c.name_alt, c.prep, t.name FROM categories c LEFT JOIN teams t ON t.id = c.team_id AND t.active = 1 WHERE c.active = 1 ORDER BY c.sort_order, c.name")?
            .query_map([], |r| {
                Ok(MenuCategory {
                    key: r.get(0)?,
                    name: r.get(1)?,
                    color: r.get(2)?,
                    sort_order: r.get(3)?,
                    tax_rate_bp: r.get(4)?,
                    starter: r.get(5)?,
                    name_alt: r.get(6)?,
                    prep: r.get(7)?,
                    team: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut option_groups = self
            .conn
            .prepare("SELECT id, name, min_select, max_select FROM option_groups WHERE active = 1 AND deleted = 0 ORDER BY sort_order, name")?
            .query_map([], |r| {
                Ok(MenuOptionGroup { key: r.get(0)?, name: r.get(1)?, min_select: r.get(2)?, max_select: r.get(3)?, options: vec![] })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for g in &mut option_groups {
            g.options = self
                .conn
                .prepare_cached("SELECT name, price_cents, name_alt FROM options WHERE group_id = ?1 AND active = 1 ORDER BY sort_order, id")?
                .query_map([g.key], |r| Ok(MenuPrice { name: r.get(0)?, price_cents: r.get(1)?, name_alt: r.get(2)? }))?
                .collect::<rusqlite::Result<_>>()?;
        }
        let active_cats: Vec<i64> = categories.iter().map(|c| c.key).collect();
        let active_groups: Vec<i64> = option_groups.iter().map(|g| g.key).collect();
        let mut items: Vec<(i64, MenuItem)> = self
            .conn
            .prepare(
                "SELECT p.id, p.name, p.category_id, p.sku, p.barcode, p.price_cents, p.cost_cents, p.tax_rate_bp, p.track_stock, p.stock_qty, p.starter, p.name_alt, p.prep, t.name
                 FROM products p LEFT JOIN teams t ON t.id = p.team_id AND t.active = 1
                 WHERE p.active = 1 ORDER BY p.category_id IS NULL, p.category_id, p.sort_order, p.name",
            )?
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    MenuItem {
                        name: r.get(1)?,
                        category: r.get(2)?,
                        sku: r.get(3)?,
                        barcode: r.get(4)?,
                        price_cents: r.get(5)?,
                        cost_cents: r.get(6)?,
                        tax_rate_bp: r.get(7)?,
                        track_stock: r.get(8)?,
                        stock_qty: r.get(9)?,
                        starter: r.get(10)?,
                        choices: vec![],
                        option_groups: vec![],
                        name_alt: r.get(11)?,
                        prep: r.get(12)?,
                        team: r.get(13)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (id, it) in &mut items {
            it.category = it.category.filter(|c| active_cats.contains(c));
            it.choices = self
                .conn
                .prepare_cached(
                    "SELECT name, price_cents, name_alt FROM product_variants WHERE product_id = ?1 AND active = 1 ORDER BY sort_order, id",
                )?
                .query_map([*id], |r| Ok(MenuPrice { name: r.get(0)?, price_cents: r.get(1)?, name_alt: r.get(2)? }))?
                .collect::<rusqlite::Result<_>>()?;
            let groups: Vec<i64> = self
                .conn
                .prepare_cached("SELECT group_id FROM product_option_groups WHERE product_id = ?1 ORDER BY sort_order")?
                .query_map([*id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            it.option_groups = groups.into_iter().filter(|g| active_groups.contains(g)).collect();
        }
        Ok(MenuBackup {
            format: MENU_FORMAT.into(),
            version: 1,
            exported_at: chrono::Utc::now().timestamp(),
            teams: crate::teams::load_teams(&self.conn)?
                .into_iter()
                .map(|t| MenuTeam { name: t.name, name_alt: t.name_alt })
                .collect(),
            categories,
            option_groups,
            items: items.into_iter().map(|(_, it)| it).collect(),
        })
    }

    /// Replace the current menu with `backup`. Open bills and past orders
    /// keep their own copies of names and prices, so they are unaffected.
    pub fn import_menu(&mut self, s: &Session, backup: &MenuBackup) -> Result<MenuCounts> {
        s.require(Permission::ManageMenu)?;
        if backup.format != MENU_FORMAT || backup.version != 1 {
            return invalid("this file is not a POS menu backup");
        }
        if backup.items.len() > MAX_ITEMS {
            return invalid("the menu backup has too many items");
        }
        let alt = |n: &Option<String>, max| crate::products::clean_alt(n.as_deref(), max);
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        clear_menu(&tx)?;
        // Teams by name: keep the ones this till has, create the missing ones.
        for t in &backup.teams {
            let name = t.name.trim();
            if name.is_empty() || name.chars().count() > 30 {
                return invalid("a team name in the backup is not valid");
            }
            let exists: bool =
                tx.query_row("SELECT EXISTS (SELECT 1 FROM teams WHERE active = 1 AND name = ?1 COLLATE NOCASE)", [name], |r| r.get(0))?;
            if !exists {
                tx.execute(
                    "INSERT INTO teams (name, name_alt, sort_order) VALUES (?1, ?2, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM teams))",
                    params![name, alt(&t.name_alt, 30)?],
                )?;
            }
        }
        let team_ids: HashMap<String, i64> = tx
            .prepare("SELECT lower(name), id FROM teams WHERE active = 1")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let team_of = |n: &Option<String>| n.as_deref().and_then(|n| team_ids.get(&n.trim().to_lowercase()).copied());
        let mut cats = HashMap::new();
        for c in &backup.categories {
            let (name, color) = crate::menu::clean(&c.name, c.color.as_deref())?;
            if c.tax_rate_bp.is_some_and(|t| !(0..=10_000).contains(&t)) {
                return invalid(format!("category \"{name}\" has an invalid tax rate"));
            }
            tx.execute(
                "INSERT INTO categories (name, color, sort_order, tax_rate_bp, starter, name_alt, prep, team_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![name, color, c.sort_order, c.tax_rate_bp, c.starter, alt(&c.name_alt, 40)?, c.prep, team_of(&c.team)],
            )
            .map_err(|e| crate::error::map_unique(e, &format!("the category \"{name}\" appears twice")))?;
            cats.insert(c.key, tx.last_insert_rowid());
        }
        let mut groups = HashMap::new();
        for (i, g) in backup.option_groups.iter().enumerate() {
            let name = g.name.trim();
            if name.is_empty() || !(1..=20).contains(&g.max_select) || !(0..=g.max_select).contains(&g.min_select) {
                return invalid(format!("option group \"{name}\" is not valid"));
            }
            tx.execute(
                "INSERT INTO option_groups (name, min_select, max_select, sort_order) VALUES (?1, ?2, ?3, ?4)",
                params![name, g.min_select, g.max_select, i as i64],
            )
            .map_err(|e| crate::error::map_unique(e, &format!("the option group \"{name}\" appears twice")))?;
            let gid = tx.last_insert_rowid();
            for (j, o) in g.options.iter().enumerate() {
                if o.name.trim().is_empty() || o.price_cents.abs() > 100_000 {
                    return invalid(format!("an option in \"{name}\" is not valid"));
                }
                tx.execute(
                    "INSERT INTO options (group_id, name, price_cents, sort_order, name_alt) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![gid, o.name.trim(), o.price_cents, j as i64, alt(&o.name_alt, 40)?],
                )?;
            }
            groups.insert(g.key, gid);
        }
        for (pos, it) in backup.items.iter().enumerate() {
            let name = crate::products::validate(&it.name, it.price_cents, it.cost_cents, it.tax_rate_bp)?;
            let code = |c: &Option<String>| c.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
            tx.execute(
                "INSERT INTO products (category_id, sku, barcode, name, price_cents, cost_cents, tax_rate_bp, stock_qty, track_stock, starter, sort_order, name_alt, prep, team_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    it.category.and_then(|k| cats.get(&k)),
                    code(&it.sku),
                    code(&it.barcode),
                    name,
                    it.price_cents,
                    it.cost_cents,
                    it.tax_rate_bp,
                    it.stock_qty,
                    it.track_stock,
                    it.starter,
                    pos as i64 + 1,
                    alt(&it.name_alt, 120)?,
                    it.prep,
                    team_of(&it.team)
                ],
            )
            .map_err(|e| crate::error::map_unique(e, &format!("a SKU or barcode on \"{name}\" is used twice")))?;
            let pid = tx.last_insert_rowid();
            if it.stock_qty != 0 {
                tx.execute(
                    "INSERT INTO stock_movements (product_id, delta, reason, user_id) VALUES (?1, ?2, 'menu restore', ?3)",
                    params![pid, it.stock_qty, s.user_id],
                )?;
            }
            for (j, v) in it.choices.iter().enumerate() {
                if v.name.trim().is_empty() || !(0..=10_000_000).contains(&v.price_cents) {
                    return invalid(format!("a choice on \"{name}\" is not valid"));
                }
                tx.execute(
                    "INSERT INTO product_variants (product_id, name, price_cents, sort_order, name_alt) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![pid, v.name.trim(), v.price_cents, j as i64, alt(&v.name_alt, 40)?],
                )?;
            }
            for (j, key) in it.option_groups.iter().enumerate() {
                if let Some(gid) = groups.get(key) {
                    tx.execute(
                        "INSERT OR IGNORE INTO product_option_groups (product_id, group_id, sort_order) VALUES (?1, ?2, ?3)",
                        params![pid, gid, j as i64],
                    )?;
                }
            }
        }
        let counts = MenuCounts { categories: cats.len(), items: backup.items.len(), option_groups: groups.len() };
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'menu.restore', ?2)",
            params![s.user_id, format!("{} items", counts.items)],
        )?;
        tx.commit()?;
        Ok(counts)
    }

    /// Check the PIN of the logged-in user (asked again before a reset).
    pub fn verify_pin(&self, s: &Session, pin: &str) -> Result<()> {
        let hash = self.hash_pin(pin).map_err(|_| PosError::InvalidPin)?;
        let ok: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM users WHERE id = ?1 AND pin_hash = ?2 AND active = 1)",
            params![s.user_id, hash],
            |r| r.get(0),
        )?;
        if ok {
            Ok(())
        } else {
            self.audit(Some(s.user_id), "reset.bad_pin", "")?;
            Err(PosError::InvalidPin)
        }
    }

    /// Delete the chosen kinds of data. The caller makes a full backup first.
    pub fn reset_data(&mut self, s: &Session, opts: &ResetOptions) -> Result<ResetReport> {
        s.require(Permission::ManageSettings)?;
        s.require(Permission::Backup)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut report = ResetReport::default();

        if let Some(sales) = &opts.sales {
            let (cond, closes) = match sales {
                SalesReset::All => ("1".to_string(), "1".to_string()),
                SalesReset::Before { date } => {
                    let d = parse_date(date).ok_or_else(|| PosError::Validation("choose a date".into()))?;
                    (
                        format!("status <> 'open' AND opened_at < {}", local_midnight(d)),
                        format!("business_date < '{}'", d.format("%Y-%m-%d")),
                    )
                }
            };
            tx.execute_batch(&format!(
                "CREATE TEMP TABLE reset_orders AS SELECT id FROM orders WHERE {cond};
                 DELETE FROM order_item_options WHERE item_id IN
                     (SELECT id FROM order_items WHERE order_id IN (SELECT id FROM reset_orders));
                 DELETE FROM order_items WHERE order_id IN (SELECT id FROM reset_orders);
                 DELETE FROM stock_movements WHERE order_id IN (SELECT id FROM reset_orders);
                 DELETE FROM day_closes WHERE {closes};"
            ))?;
            report.orders = count(&tx, "SELECT COUNT(*) FROM reset_orders")?;
            tx.execute_batch(
                "DELETE FROM orders WHERE id IN (SELECT id FROM reset_orders);
                 DROP TABLE reset_orders;",
            )?;
            if matches!(sales, SalesReset::All) {
                tx.execute("DELETE FROM settings WHERE key = '_business_date'", [])?;
                tx.execute("UPDATE customers SET order_count = 0, last_order_at = NULL", [])?;
            }
        }

        if opts.customers {
            report.customers = count(&tx, "SELECT COUNT(*) FROM customers WHERE active = 1")?;
            tx.execute_batch("UPDATE orders SET customer_id = NULL; DELETE FROM customers;")?;
        }

        if opts.menu {
            report.items = clear_menu(&tx)?;
        }

        if opts.floor_plan {
            let busy = count(&tx, "SELECT COUNT(*) FROM orders WHERE status = 'open' AND table_id IS NOT NULL")?;
            if busy > 0 {
                return invalid("tables still have open orders: pay or cancel them first (or also delete all sales)");
            }
            report.tables = count(&tx, "SELECT COUNT(*) FROM dining_tables WHERE active = 1")?;
            tx.execute_batch(
                "DELETE FROM dining_tables WHERE id NOT IN (SELECT table_id FROM orders WHERE table_id IS NOT NULL);
                 UPDATE dining_tables SET active = 0;
                 DELETE FROM floors WHERE id NOT IN (SELECT floor_id FROM dining_tables);
                 UPDATE floors SET active = 0;",
            )?;
        }

        if opts.staff {
            report.staff = count(&tx, &format!("SELECT COUNT(*) FROM users WHERE active = 1 AND id <> {}", s.user_id))?;
            // Nullable references are cleared; staff still named on kept
            // orders are disabled with an unusable PIN instead of deleted.
            tx.execute_batch(&format!(
                "UPDATE audit_log SET user_id = NULL WHERE user_id <> {me};
                 UPDATE stock_movements SET user_id = NULL WHERE user_id <> {me};
                 UPDATE orders SET paid_by = {me} WHERE paid_by <> {me};
                 UPDATE orders SET closed_by = {me} WHERE closed_by <> {me};
                 DELETE FROM users WHERE id <> {me}
                     AND id NOT IN (SELECT opened_by FROM orders)
                     AND id NOT IN (SELECT added_by FROM order_items)
                     AND id NOT IN (SELECT closed_by FROM day_closes);
                 UPDATE users SET active = 0, pin_hash = randomblob(32) WHERE id <> {me};",
                me = s.user_id
            ))?;
        }

        if opts.settings {
            tx.execute_batch(
                "DELETE FROM settings WHERE key NOT IN ('_pin_salt', '_business_date');
                 DELETE FROM delivery_channels WHERE id NOT IN (SELECT channel_id FROM orders WHERE channel_id IS NOT NULL);
                 UPDATE delivery_channels SET active = 0;
                 INSERT INTO delivery_channels (name, color, sort_order) VALUES
                     ('Uber Eats', '#06c167', 1), ('Just Eat', '#ff8000', 2), ('Deliveroo', '#00ccbc', 3);",
            )?;
        }

        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'data.reset', ?2)",
            params![s.user_id, format!("{opts:?}")],
        )?;
        tx.commit()?;
        Ok(report)
    }
}

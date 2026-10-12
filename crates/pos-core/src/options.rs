//! Item options ("modifiers"): sizes, extras, cooking preferences...
//!
//! An option group (e.g. "Burger size", choose exactly 1) holds options with
//! a price each, and is attached to any number of menu items. When an item is
//! added to an order the chosen options are validated against the item's
//! groups and snapshotted onto the order line.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{invalid, map_unique};
use crate::{Permission, Pos, PosError, Result, Session};

const MAX_PER_GROUP: i64 = 20;
const MAX_OPTION_PRICE: i64 = 100_000; // 1,000.00 either way

#[derive(Debug, Clone, Serialize)]
pub struct OptionChoice {
    pub id: i64,
    pub name: String,
    pub price_cents: i64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptionGroup {
    pub id: i64,
    pub name: String,
    /// 0 = optional; 1+ = the customer must choose at least this many.
    pub min_select: i64,
    pub max_select: i64,
    pub sort_order: i64,
    pub active: bool,
    pub options: Vec<OptionChoice>,
}

/// One option as edited in the admin screen; `id` is None for new ones.
#[derive(Debug, Clone, Deserialize)]
pub struct OptionInput {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub price_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProductGroupLink {
    pub product_id: i64,
    pub group_id: i64,
}

/// Everything the order screen needs to show option pickers, loaded once.
#[derive(Debug, Clone, Serialize)]
pub struct MenuOptions {
    pub groups: Vec<OptionGroup>,
    /// Which groups each item uses, in display order.
    pub links: Vec<ProductGroupLink>,
}

/// An option as chosen on an order line.
#[derive(Debug, Clone, Serialize)]
pub struct ItemOption {
    pub option_id: i64,
    pub group_name: String,
    pub name: String,
    pub price_cents: i64,
}

const GROUP_TAKEN: &str = "an option group with that name already exists";

fn clean_group(name: &str, min: i64, max: i64) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return invalid("group name must be 1 to 40 characters");
    }
    if !(1..=MAX_PER_GROUP).contains(&max) || !(0..=max).contains(&min) {
        return invalid(format!("choose between 0 and {MAX_PER_GROUP}, with minimum not above maximum"));
    }
    Ok(name.to_string())
}

fn load_groups(conn: &Connection, include_inactive: bool) -> Result<Vec<OptionGroup>> {
    let mut groups: Vec<OptionGroup> = conn
        .prepare_cached(
            "SELECT id, name, min_select, max_select, sort_order, active FROM option_groups
             WHERE deleted = 0 AND (?1 OR active = 1) ORDER BY sort_order, name",
        )?
        .query_map([include_inactive], |r| {
            Ok(OptionGroup {
                id: r.get(0)?,
                name: r.get(1)?,
                min_select: r.get(2)?,
                max_select: r.get(3)?,
                sort_order: r.get(4)?,
                active: r.get(5)?,
                options: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let index: HashMap<i64, usize> = groups.iter().enumerate().map(|(i, g)| (g.id, i)).collect();
    let mut stmt = conn.prepare_cached(
        "SELECT group_id, id, name, price_cents, active FROM options
         WHERE ?1 OR active = 1 ORDER BY group_id, sort_order, id",
    )?;
    let rows = stmt.query_map([include_inactive], |r| {
        Ok((r.get::<_, i64>(0)?, OptionChoice { id: r.get(1)?, name: r.get(2)?, price_cents: r.get(3)?, active: r.get(4)? }))
    })?;
    for row in rows {
        let (gid, o) = row?;
        if let Some(&i) = index.get(&gid) {
            groups[i].options.push(o);
        }
    }
    Ok(groups)
}

/// Validate `option_ids` for `product_id` and return them as snapshots, in
/// menu order. Enforces each group's minimum and maximum.
pub(crate) fn resolve_options(conn: &Connection, product_id: i64, option_ids: &[i64]) -> Result<Vec<ItemOption>> {
    let unique: HashSet<i64> = option_ids.iter().copied().collect();
    if unique.len() != option_ids.len() {
        return invalid("the same option was chosen twice");
    }
    let groups: Vec<(i64, String, i64, i64)> = conn
        .prepare_cached(
            "SELECT g.id, g.name, g.min_select, g.max_select
             FROM product_option_groups pg JOIN option_groups g ON g.id = pg.group_id AND g.active = 1
             WHERE pg.product_id = ?1 ORDER BY pg.sort_order, g.sort_order",
        )?
        .query_map([product_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare_cached(
        "SELECT group_id, name, price_cents, sort_order FROM options WHERE id = ?1 AND active = 1",
    )?;
    let mut chosen = Vec::with_capacity(option_ids.len());
    for &id in option_ids {
        let (gid, name, price, sort): (i64, String, i64, i64) = stmt
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?
            .ok_or_else(|| PosError::Validation("that option is no longer available".into()))?;
        let pos = groups
            .iter()
            .position(|g| g.0 == gid)
            .ok_or_else(|| PosError::Validation(format!("{name} is not an option for this item")))?;
        chosen.push((pos, sort, id, ItemOption { option_id: id, group_name: groups[pos].1.clone(), name, price_cents: price }));
    }
    for (pos, (_, name, min, max)) in groups.iter().enumerate() {
        let n = chosen.iter().filter(|c| c.0 == pos).count() as i64;
        if n < *min {
            return invalid(if *min == 1 { format!("choose a {name}") } else { format!("choose at least {min} for {name}") });
        }
        if n > *max {
            return invalid(if *max == 1 { format!("choose only one {name}") } else { format!("choose at most {max} for {name}") });
        }
    }
    chosen.sort_by_key(|c| (c.0, c.1, c.2));
    Ok(chosen.into_iter().map(|c| c.3).collect())
}

impl Pos {
    pub fn list_option_groups(&self, s: &Session, include_inactive: bool) -> Result<Vec<OptionGroup>> {
        s.require(if include_inactive { Permission::ManageMenu } else { Permission::Sell })?;
        load_groups(&self.conn, include_inactive)
    }

    pub fn menu_options(&self, s: &Session) -> Result<MenuOptions> {
        s.require(Permission::Sell)?;
        let groups = load_groups(&self.conn, false)?;
        let links = self
            .conn
            .prepare_cached(
                "SELECT pg.product_id, pg.group_id FROM product_option_groups pg
                 JOIN option_groups g ON g.id = pg.group_id AND g.active = 1
                 ORDER BY pg.product_id, pg.sort_order, g.sort_order",
            )?
            .query_map([], |r| Ok(ProductGroupLink { product_id: r.get(0)?, group_id: r.get(1)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(MenuOptions { groups, links })
    }

    pub fn create_option_group(&self, s: &Session, name: &str, min_select: i64, max_select: i64) -> Result<OptionGroup> {
        s.require(Permission::ManageMenu)?;
        let name = clean_group(name, min_select, max_select)?;
        self.conn
            .execute(
                "INSERT INTO option_groups (name, min_select, max_select, sort_order)
                 VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM option_groups))",
                params![name, min_select, max_select],
            )
            .map_err(|e| map_unique(e, GROUP_TAKEN))?;
        let id = self.conn.last_insert_rowid();
        self.audit(Some(s.user_id), "options.group_create", &format!("{id} {name}"))?;
        self.option_group(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_option_group(
        &self,
        s: &Session,
        id: i64,
        name: &str,
        min_select: i64,
        max_select: i64,
        sort_order: i64,
        active: bool,
    ) -> Result<OptionGroup> {
        s.require(Permission::ManageMenu)?;
        let name = clean_group(name, min_select, max_select)?;
        let changed = self
            .conn
            .execute(
                "UPDATE option_groups SET name = ?2, min_select = ?3, max_select = ?4, sort_order = ?5, active = ?6
                 WHERE id = ?1 AND deleted = 0",
                params![id, name, min_select, max_select, sort_order, active],
            )
            .map_err(|e| map_unique(e, GROUP_TAKEN))?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "options.group_update", &format!("{id} {name} {min_select}-{max_select} active={active}"))?;
        self.option_group(id)
    }

    /// Delete a group: it disappears from the menu editor and from every
    /// item that used it. Past orders keep their own copy of the options.
    pub fn delete_option_group(&mut self, s: &Session, id: i64) -> Result<()> {
        s.require(Permission::ManageMenu)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let name: String = tx
            .query_row("SELECT name FROM option_groups WHERE id = ?1 AND deleted = 0", [id], |r| r.get(0))
            .optional()?
            .ok_or(PosError::NotFound)?;
        tx.execute("DELETE FROM product_option_groups WHERE group_id = ?1", [id])?;
        tx.execute("UPDATE options SET active = 0 WHERE group_id = ?1", [id])?;
        tx.execute("UPDATE option_groups SET active = 0, deleted = 1 WHERE id = ?1", [id])?;
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'options.group_delete', ?2)",
            params![s.user_id, format!("{id} {name}")],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Replace a group's options with the editor's list (in display order):
    /// existing ids are updated, new ones added, missing ones retired.
    /// Past orders keep their own snapshot, so retiring is always safe.
    pub fn save_group_options(&mut self, s: &Session, group_id: i64, options: &[OptionInput]) -> Result<OptionGroup> {
        s.require(Permission::ManageMenu)?;
        if options.len() > 50 {
            return invalid("a group can have at most 50 options");
        }
        let mut cleaned = Vec::with_capacity(options.len());
        for o in options {
            let name = o.name.trim();
            if name.is_empty() || name.chars().count() > 40 {
                return invalid("option names must be 1 to 40 characters");
            }
            if !(-MAX_OPTION_PRICE..=MAX_OPTION_PRICE).contains(&o.price_cents) {
                return invalid(format!("price for {name} is out of range"));
            }
            cleaned.push((o.id, name.to_string(), o.price_cents));
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: HashSet<i64> = tx
            .prepare("SELECT id FROM options WHERE group_id = ?1 AND active = 1")?
            .query_map([group_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let exists: bool =
            tx.query_row("SELECT EXISTS (SELECT 1 FROM option_groups WHERE id = ?1 AND deleted = 0)", [group_id], |r| r.get(0))?;
        if !exists {
            return Err(PosError::NotFound);
        }
        let keep: HashSet<i64> = cleaned.iter().filter_map(|c| c.0).collect();
        if let Some(bad) = keep.iter().find(|id| !existing.contains(id)) {
            return invalid(format!("option {bad} is not in this group"));
        }
        for id in existing.difference(&keep) {
            tx.execute("UPDATE options SET active = 0 WHERE id = ?1", [id])?;
        }
        for (i, (id, name, price)) in cleaned.iter().enumerate() {
            match id {
                Some(id) => tx.execute(
                    "UPDATE options SET name = ?2, price_cents = ?3, sort_order = ?4 WHERE id = ?1",
                    params![id, name, price, i as i64],
                )?,
                None => tx.execute(
                    "INSERT INTO options (group_id, name, price_cents, sort_order) VALUES (?1, ?2, ?3, ?4)",
                    params![group_id, name, price, i as i64],
                )?,
            };
        }
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'options.save', ?2)",
            params![s.user_id, format!("group {group_id}: {} options", cleaned.len())],
        )?;
        tx.commit()?;
        self.option_group(group_id)
    }

    /// Set which option groups a menu item uses, in display order.
    pub fn set_product_option_groups(&mut self, s: &Session, product_id: i64, group_ids: &[i64]) -> Result<()> {
        s.require(Permission::ManageMenu)?;
        if group_ids.iter().collect::<HashSet<_>>().len() != group_ids.len() {
            return invalid("the same group was chosen twice");
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let product_ok: bool =
            tx.query_row("SELECT EXISTS (SELECT 1 FROM products WHERE id = ?1)", [product_id], |r| r.get(0))?;
        if !product_ok {
            return Err(PosError::NotFound);
        }
        tx.execute("DELETE FROM product_option_groups WHERE product_id = ?1", [product_id])?;
        for (i, gid) in group_ids.iter().enumerate() {
            let ok: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM option_groups WHERE id = ?1 AND active = 1)",
                [gid],
                |r| r.get(0),
            )?;
            if !ok {
                return invalid("that option group does not exist");
            }
            tx.execute(
                "INSERT INTO product_option_groups (product_id, group_id, sort_order) VALUES (?1, ?2, ?3)",
                params![product_id, gid, i as i64],
            )?;
        }
        tx.commit()?;
        self.audit(Some(s.user_id), "options.product_groups", &format!("{product_id} {group_ids:?}"))
    }

    fn option_group(&self, id: i64) -> Result<OptionGroup> {
        load_groups(&self.conn, true)?.into_iter().find(|g| g.id == id).ok_or(PosError::NotFound)
    }
}

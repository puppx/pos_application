//! Set menus: a product made of sections (Starter, Main, Side...), each
//! listing the menu items the customer chooses from, at the set's own price.
//!
//! On an order a set is one priced line (the bill shows only that) with a
//! free line under it for each chosen dish. Those dishes go to the kitchen
//! like any other: their section decides whether they go with the starters,
//! and their own team / drink settings decide which ticket they print on.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::orders::{opt_text, require_open, MAX_ITEMS};
use crate::products::clean_alt;
use crate::{Order, Permission, Pos, PosError, Result, Session};

const MAX_SECTIONS: usize = 10;
const MAX_CHOICES: usize = 60;
const MAX_PICK: i64 = 10;

/// One section of a set, with the dishes to choose from.
#[derive(Debug, Clone, Serialize)]
pub struct SetSection {
    pub id: i64,
    pub name: String,
    pub name_alt: Option<String>,
    /// How many dishes to choose (the same dish may be chosen more than once).
    pub pick: i64,
    /// Goes to the kitchen with the starters on dine-in orders.
    pub starter: bool,
    pub items: Vec<SetChoice>,
    /// What each dish here comes with (one to choose per dish), e.g. rice or
    /// noodles; empty when the section has nothing to pair.
    pub with_items: Vec<SetChoice>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetChoice {
    pub product_id: i64,
    pub name: String,
    pub name_alt: Option<String>,
}

/// A section as edited in the set editor.
#[derive(Debug, Clone, Deserialize)]
pub struct SetSectionInput {
    pub name: String,
    #[serde(default)]
    pub name_alt: Option<String>,
    #[serde(default = "one")]
    pub pick: i64,
    #[serde(default)]
    pub starter: bool,
    pub product_ids: Vec<i64>,
    /// "Comes with" choices for this section's dishes (may be empty).
    #[serde(default)]
    pub with_ids: Vec<i64>,
}

/// A chosen dish: product, what it comes with, how many.
type Chosen<'a> = (i64, Option<&'a SetChoice>, i64);

fn one() -> i64 {
    1
}

/// A dish chosen for one section when ordering a set.
#[derive(Debug, Clone, Deserialize)]
pub struct SetPick {
    pub section_id: i64,
    pub product_id: i64,
    /// What it comes with, when the section has "comes with" choices.
    #[serde(default)]
    pub with_id: Option<i64>,
}

impl Pos {
    /// The sections of a set, in order, with the dishes still on the menu.
    pub fn get_set(&self, s: &Session, set_id: i64) -> Result<Vec<SetSection>> {
        s.require(Permission::Sell)?;
        let mut sections: Vec<SetSection> = self
            .conn
            .prepare_cached("SELECT id, name, name_alt, pick, starter FROM set_sections WHERE set_id = ?1 ORDER BY sort_order, id")?
            .query_map([set_id], |r| {
                Ok(SetSection { id: r.get(0)?, name: r.get(1)?, name_alt: r.get(2)?, pick: r.get(3)?, starter: r.get(4)?, items: vec![], with_items: vec![] })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.name, p.name_alt FROM set_section_items i JOIN products p ON p.id = i.product_id
             WHERE i.section_id = ?1 AND p.active = 1 AND p.deleted = 0 ORDER BY i.sort_order, p.name",
        )?;
        let mut with = self.conn.prepare_cached(
            "SELECT p.id, p.name, p.name_alt FROM set_section_with i JOIN products p ON p.id = i.product_id
             WHERE i.section_id = ?1 AND p.active = 1 AND p.deleted = 0 ORDER BY i.sort_order, p.name",
        )?;
        let choice = |r: &rusqlite::Row| Ok(SetChoice { product_id: r.get(0)?, name: r.get(1)?, name_alt: r.get(2)? });
        for sec in &mut sections {
            sec.items = stmt.query_map([sec.id], choice)?.collect::<rusqlite::Result<_>>()?;
            sec.with_items = with.query_map([sec.id], choice)?.collect::<rusqlite::Result<_>>()?;
        }
        Ok(sections)
    }

    /// Replace a set's sections (and make the product a set). Dishes with
    /// choices or required options cannot go in a set: there is no room to
    /// pick those inside the set.
    pub fn save_set(&mut self, s: &Session, set_id: i64, sections: &[SetSectionInput]) -> Result<Vec<SetSection>> {
        s.require(Permission::ManageMenu)?;
        if sections.is_empty() || sections.len() > MAX_SECTIONS {
            return invalid(format!("a set needs 1 to {MAX_SECTIONS} sections"));
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.query_row("SELECT id FROM products WHERE id = ?1 AND deleted = 0", [set_id], |r| r.get::<_, i64>(0))
            .optional()?
            .ok_or(PosError::NotFound)?;
        let mut cleaned = Vec::new();
        for sec in sections {
            let name = sec.name.trim();
            if name.is_empty() || name.chars().count() > 40 {
                return invalid("give every section a name (up to 40 characters)");
            }
            if !(1..=MAX_PICK).contains(&sec.pick) {
                return invalid(format!("{name}: choose 1 to {MAX_PICK} dishes"));
            }
            if sec.product_ids.is_empty() || sec.product_ids.len() > MAX_CHOICES {
                return invalid(format!("{name}: add 1 to {MAX_CHOICES} dishes"));
            }
            for &p in &sec.product_ids {
                let (pname, is_set, variants, required): (String, bool, bool, bool) = tx
                    .query_row(
                        "SELECT name, is_set,
                                EXISTS (SELECT 1 FROM product_variants v WHERE v.product_id = p.id AND v.active = 1),
                                EXISTS (SELECT 1 FROM product_option_groups l JOIN option_groups g ON g.id = l.group_id
                                        WHERE l.product_id = p.id AND g.active = 1 AND g.min_select > 0)
                         FROM products p WHERE id = ?1 AND deleted = 0",
                        [p],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?
                    .ok_or_else(|| PosError::Validation(format!("{name}: a dish is no longer on the menu")))?;
                if is_set || p == set_id {
                    return invalid(format!("{pname} is a set menu itself"));
                }
                if variants || required {
                    return invalid(format!("{pname} has choices or options to pick, so it cannot go in a set; make a plain item for the set"));
                }
            }
            if sec.with_ids.len() > MAX_CHOICES {
                return invalid(format!("{name}: up to {MAX_CHOICES} things to come with"));
            }
            for &p in &sec.with_ids {
                let is_set: bool = tx
                    .query_row("SELECT is_set FROM products WHERE id = ?1 AND deleted = 0", [p], |r| r.get(0))
                    .optional()?
                    .ok_or_else(|| PosError::Validation(format!("{name}: something it comes with is no longer on the menu")))?;
                if is_set {
                    return invalid(format!("{name}: a set menu cannot come with a dish"));
                }
            }
            cleaned.push((name.to_string(), clean_alt(sec.name_alt.as_deref(), 40)?));
        }
        tx.execute(
            "DELETE FROM set_section_items WHERE section_id IN (SELECT id FROM set_sections WHERE set_id = ?1)",
            [set_id],
        )?;
        tx.execute(
            "DELETE FROM set_section_with WHERE section_id IN (SELECT id FROM set_sections WHERE set_id = ?1)",
            [set_id],
        )?;
        tx.execute("DELETE FROM set_sections WHERE set_id = ?1", [set_id])?;
        for (i, (sec, (name, alt))) in sections.iter().zip(cleaned).enumerate() {
            tx.execute(
                "INSERT INTO set_sections (set_id, name, name_alt, sort_order, pick, starter) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![set_id, name, alt, i, sec.pick, sec.starter],
            )?;
            let section = tx.last_insert_rowid();
            for (j, p) in sec.product_ids.iter().enumerate() {
                tx.execute(
                    "INSERT OR IGNORE INTO set_section_items (section_id, product_id, sort_order) VALUES (?1, ?2, ?3)",
                    params![section, p, j],
                )?;
            }
            for (j, p) in sec.with_ids.iter().enumerate() {
                tx.execute(
                    "INSERT OR IGNORE INTO set_section_with (section_id, product_id, sort_order) VALUES (?1, ?2, ?3)",
                    params![section, p, j],
                )?;
            }
        }
        // A set is sold as itself: no stock of its own, no starter/prep flags.
        tx.execute(
            "UPDATE products SET is_set = 1, set_only = 0, track_stock = 0, starter = 0, prep = 0, drink = 0, updated_at = unixepoch() WHERE id = ?1",
            [set_id],
        )?;
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'menu.set', ?2)",
            params![s.user_id, format!("{set_id} sections={}", sections.len())],
        )?;
        tx.commit()?;
        self.get_set(s, set_id)
    }

    /// Mark an item as made for sets only (hidden from the order screen).
    pub fn set_set_only(&self, s: &Session, id: i64, set_only: bool) -> Result<()> {
        s.require(Permission::ManageMenu)?;
        let changed = self.conn.execute(
            "UPDATE products SET set_only = ?2, updated_at = unixepoch() WHERE id = ?1 AND is_set = 0",
            params![id, set_only],
        )?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "product.set_only", &format!("{id} {set_only}"))
    }

    /// Add one set to an order with the dishes chosen for each section.
    pub fn add_set(&mut self, s: &Session, order_id: i64, set_id: i64, picks: &[SetPick], note: Option<&str>) -> Result<Order> {
        s.require(Permission::Sell)?;
        let note = opt_text(&note.map(str::to_string), 100, "note")?;
        let sections = self.get_set(s, set_id)?;
        if sections.is_empty() {
            return invalid("this set menu has no sections yet");
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_open(&tx, order_id)?;
        let (name, price, rate, alt): (String, i64, i64, Option<String>) = tx
            .query_row(
                "SELECT name, price_cents, tax_rate_bp, name_alt FROM products WHERE id = ?1 AND active = 1 AND is_set = 1",
                [set_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
            .ok_or_else(|| PosError::Validation("that set menu is not available".into()))?;
        let order_tax: Option<i64> = tx.query_row("SELECT tax_bp FROM orders WHERE id = ?1", [order_id], |r| r.get(0))?;
        let rate = order_tax.unwrap_or(rate);

        // Every section gets exactly its number of dishes, from its own list.
        // (dish, what it comes with, how many)
        let mut chosen: Vec<(&SetSection, Vec<Chosen>)> = Vec::new();
        for sec in &sections {
            let mine: Vec<&SetPick> = picks.iter().filter(|p| p.section_id == sec.id).collect();
            if mine.len() as i64 != sec.pick {
                return invalid(format!("{}: choose {}", sec.name, sec.pick));
            }
            let mut counts: Vec<Chosen> = Vec::new();
            for p in mine {
                if !sec.items.iter().any(|c| c.product_id == p.product_id) {
                    return invalid(format!("{}: that dish is not in this section", sec.name));
                }
                // A section with "comes with" choices needs one for every dish.
                let with = match p.with_id {
                    _ if sec.with_items.is_empty() => None,
                    None => return invalid(format!("{}: choose what it comes with", sec.name)),
                    Some(w) => Some(
                        sec.with_items
                            .iter()
                            .find(|c| c.product_id == w)
                            .ok_or_else(|| PosError::Validation(format!("{}: it cannot come with that", sec.name)))?,
                    ),
                };
                let key = with.map(|c| c.product_id);
                match counts.iter_mut().find(|c| c.0 == p.product_id && c.1.map(|w| w.product_id) == key) {
                    Some(c) => c.2 += 1,
                    None => counts.push((p.product_id, with, 1)),
                }
            }
            chosen.push((sec, counts));
        }
        if picks.iter().any(|p| !sections.iter().any(|s| s.id == p.section_id)) {
            return invalid("a dish was chosen for a section this set does not have");
        }
        let lines: i64 = tx.query_row("SELECT COUNT(*) FROM order_items WHERE order_id = ?1", [order_id], |r| r.get(0))?;
        let dishes: i64 = chosen.iter().map(|c| c.1.len() as i64).sum();
        if lines + 1 + dishes > MAX_ITEMS {
            return invalid("this order has too many lines");
        }
        tx.execute(
            "INSERT INTO order_items (order_id, product_id, name, unit_price_cents, tax_rate_bp, qty, note, added_by, starter, name_alt, prep, is_set)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, 0, ?8, 0, 1)",
            params![order_id, set_id, name, price, rate, note, s.user_id, alt],
        )?;
        let parent = tx.last_insert_rowid();
        let mut info = tx.prepare_cached("SELECT name, name_alt, prep FROM products WHERE id = ?1")?;
        let mut ins = tx.prepare_cached(
            "INSERT INTO order_items (order_id, product_id, name, unit_price_cents, tax_rate_bp, qty, added_by, starter, name_alt, prep,
                                      parent_id, section_name, section_alt, with_name, with_alt)
             VALUES (?1, ?2, ?3, 0, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?;
        let mut names: HashMap<i64, (String, Option<String>, bool)> = HashMap::new();
        for (sec, counts) in &chosen {
            for &(p, with, qty) in counts {
                let (pname, palt, prep) = match names.get(&p) {
                    Some(v) => v.clone(),
                    None => {
                        let v: (String, Option<String>, bool) = info.query_row([p], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                        names.insert(p, v.clone());
                        v
                    }
                };
                ins.execute(params![
                    order_id,
                    p,
                    pname,
                    qty,
                    s.user_id,
                    sec.starter,
                    palt,
                    prep,
                    parent,
                    sec.name,
                    sec.name_alt,
                    with.map(|c| c.name.clone()),
                    with.and_then(|c| c.name_alt.clone())
                ])?;
            }
        }
        drop(ins);
        drop(info);
        tx.commit()?;
        self.get_order(s, order_id)
    }
}

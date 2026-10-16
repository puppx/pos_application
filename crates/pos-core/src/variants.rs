//! Item choices ("variants"): one menu item sold in several versions, each
//! with its own full price, e.g. Tom Yum Soup: Chicken 6.50 / Beef 7.50 /
//! Prawn 8.50. Set up on the item itself; one must be picked when ordering.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{Permission, Pos, PosError, Result, Session};

const MAX_VARIANTS: usize = 30;
const MAX_PRICE: i64 = 10_000_000;

#[derive(Debug, Clone, Serialize)]
pub struct Variant {
    pub id: i64,
    pub product_id: i64,
    pub name: String,
    pub price_cents: i64,
    pub name_alt: Option<String>,
}

/// One choice as edited on the item; `id` is None for new ones.
#[derive(Debug, Clone, Deserialize)]
pub struct VariantInput {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    pub price_cents: i64,
    #[serde(default)]
    pub name_alt: Option<String>,
}

pub(crate) fn load_variants(conn: &Connection, product_id: Option<i64>) -> Result<Vec<Variant>> {
    let mut stmt = conn.prepare_cached(
        "SELECT v.id, v.product_id, v.name, v.price_cents, v.name_alt FROM product_variants v
         JOIN products p ON p.id = v.product_id AND p.active = 1
         WHERE v.active = 1 AND (?1 IS NULL OR v.product_id = ?1)
         ORDER BY v.product_id, v.sort_order, v.id",
    )?;
    let rows = stmt.query_map([product_id], |r| {
        Ok(Variant { id: r.get(0)?, product_id: r.get(1)?, name: r.get(2)?, price_cents: r.get(3)?, name_alt: r.get(4)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The chosen choice: (id, name, price, Thai name).
pub(crate) type ChosenVariant = (i64, String, i64, Option<String>);

/// Check the chosen variant for an item. Returns it when the item has
/// choices; requires one to be picked in that case.
pub(crate) fn resolve_variant(conn: &Connection, product_id: i64, variant_id: Option<i64>) -> Result<Option<ChosenVariant>> {
    let has_variants: bool = conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM product_variants WHERE product_id = ?1 AND active = 1)")?
        .query_row([product_id], |r| r.get(0))?;
    match (has_variants, variant_id) {
        (false, None) => Ok(None),
        (false, Some(_)) => invalid("this item has no choices"),
        (true, None) => invalid("choose which one (e.g. chicken or beef) for this item"),
        (true, Some(v)) => conn
            .prepare_cached(
                "SELECT id, name, price_cents, name_alt FROM product_variants WHERE id = ?1 AND product_id = ?2 AND active = 1",
            )?
            .query_row([v, product_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?
            .map(Some)
            .ok_or_else(|| PosError::Validation("that choice is no longer available".into())),
    }
}

impl Pos {
    /// Choices for one item, or for every item when `product_id` is None
    /// (the order screen loads them all once with the menu).
    pub fn list_variants(&self, s: &Session, product_id: Option<i64>) -> Result<Vec<Variant>> {
        s.require(Permission::Sell)?;
        load_variants(&self.conn, product_id)
    }

    /// Replace an item's choices with the editor's list (in display order).
    /// An empty list turns choices off, so the item's own price is used.
    /// Past orders keep their own copy of the name and price.
    pub fn set_variants(&mut self, s: &Session, product_id: i64, list: &[VariantInput]) -> Result<Vec<Variant>> {
        s.require(Permission::ManageMenu)?;
        if list.len() > MAX_VARIANTS {
            return invalid(format!("an item can have at most {MAX_VARIANTS} choices"));
        }
        let mut names = HashSet::new();
        let mut cleaned = Vec::with_capacity(list.len());
        for v in list {
            let name = v.name.trim();
            if name.is_empty() || name.chars().count() > 40 {
                return invalid("choice names must be 1 to 40 characters");
            }
            if !names.insert(name.to_lowercase()) {
                return invalid(format!("{name} is listed twice"));
            }
            if !(0..=MAX_PRICE).contains(&v.price_cents) {
                return invalid(format!("price for {name} is out of range"));
            }
            cleaned.push((v.id, name.to_string(), v.price_cents, crate::products::clean_alt(v.name_alt.as_deref(), 40)?));
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let exists: bool =
            tx.query_row("SELECT EXISTS (SELECT 1 FROM products WHERE id = ?1)", [product_id], |r| r.get(0))?;
        if !exists {
            return Err(PosError::NotFound);
        }
        let existing: HashSet<i64> = tx
            .prepare("SELECT id FROM product_variants WHERE product_id = ?1 AND active = 1")?
            .query_map([product_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let keep: HashSet<i64> = cleaned.iter().filter_map(|c| c.0).collect();
        if keep.iter().any(|id| !existing.contains(id)) {
            return invalid("that choice belongs to another item");
        }
        for id in existing.difference(&keep) {
            tx.execute("UPDATE product_variants SET active = 0 WHERE id = ?1", [id])?;
        }
        for (i, (id, name, price, alt)) in cleaned.iter().enumerate() {
            match id {
                Some(id) => tx.execute(
                    "UPDATE product_variants SET name = ?2, price_cents = ?3, sort_order = ?4, name_alt = ?5 WHERE id = ?1",
                    params![id, name, price, i as i64, alt],
                )?,
                None => tx.execute(
                    "INSERT INTO product_variants (product_id, name, price_cents, sort_order, name_alt) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![product_id, name, price, i as i64, alt],
                )?,
            };
        }
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'menu.variants', ?2)",
            params![s.user_id, format!("product {product_id}: {} choices", cleaned.len())],
        )?;
        tx.commit()?;
        load_variants(&self.conn, Some(product_id))
    }
}

//! Saved customers for delivery and collection orders.
//!
//! Staff search by phone, name or postcode and pick a customer to fill the
//! order. Typing a postcode lists the addresses already saved there, so door
//! numbers in your delivery area come up without any paid lookup service.

use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{OrderSummary, Permission, Pos, PosError, Result, Session};

#[derive(Debug, Clone, Serialize)]
pub struct Customer {
    pub id: i64,
    pub name: String,
    pub phone: Option<String>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub town: Option<String>,
    pub postcode: Option<String>,
    pub notes: Option<String>,
    pub order_count: i64,
    pub last_order_at: Option<i64>,
}

impl Customer {
    /// Address on one line, as copied onto orders and tickets.
    pub fn address(&self) -> Option<String> {
        let parts: Vec<&str> = [&self.address_line1, &self.address_line2, &self.town, &self.postcode]
            .into_iter()
            .filter_map(|p| p.as_deref())
            .collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CustomerInput {
    pub name: String,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub address_line1: Option<String>,
    #[serde(default)]
    pub address_line2: Option<String>,
    #[serde(default)]
    pub town: Option<String>,
    #[serde(default)]
    pub postcode: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// "sw1a2aa" / "SW1A 2AA " -> "SW1A 2AA". None if it is not shaped like a
/// UK postcode (outward 2-4 characters, inward digit + two letters).
pub fn normalize_postcode(s: &str) -> Option<String> {
    let c: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    let n = c.len();
    if !(5..=7).contains(&n) || !c.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    let (out, inw) = c.split_at(n - 3);
    let ib = inw.as_bytes();
    let ob = out.as_bytes();
    let ok = ib[0].is_ascii_digit()
        && ib[1].is_ascii_alphabetic()
        && ib[2].is_ascii_alphabetic()
        && ob[0].is_ascii_alphabetic()
        && ob.iter().any(u8::is_ascii_digit);
    ok.then(|| format!("{out} {inw}"))
}

/// Digits only, with +44 / 0044 turned into a leading 0, so the same number
/// typed different ways matches.
pub fn phone_digits(s: &str) -> String {
    let d: String = s.chars().filter(char::is_ascii_digit).collect();
    if let Some(rest) = d.strip_prefix("0044") {
        format!("0{rest}")
    } else if d.len() == 12 && d.starts_with("44") {
        format!("0{}", &d[2..])
    } else {
        d
    }
}

const COLS: &str =
    "id, name, phone, address_line1, address_line2, town, postcode, notes, order_count, last_order_at";

fn customer_from_row(r: &Row) -> rusqlite::Result<Customer> {
    Ok(Customer {
        id: r.get(0)?,
        name: r.get(1)?,
        phone: r.get(2)?,
        address_line1: r.get(3)?,
        address_line2: r.get(4)?,
        town: r.get(5)?,
        postcode: r.get(6)?,
        notes: r.get(7)?,
        order_count: r.get(8)?,
        last_order_at: r.get(9)?,
    })
}

fn text(v: &Option<String>, max: usize, what: &str) -> Result<Option<String>> {
    let v = v.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(t) = v {
        if t.chars().count() > max {
            return invalid(format!("{what} is too long (max {max} characters)"));
        }
    }
    Ok(v.map(str::to_string))
}

struct Clean {
    name: String,
    phone: Option<String>,
    digits: Option<String>,
    line1: Option<String>,
    line2: Option<String>,
    town: Option<String>,
    postcode: Option<String>,
    notes: Option<String>,
}

fn clean(c: &CustomerInput) -> Result<Clean> {
    let name = c.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return invalid("customer name must be 1 to 60 characters");
    }
    let phone = text(&c.phone, 30, "phone number")?;
    let postcode = match text(&c.postcode, 10, "postcode")? {
        Some(p) => Some(normalize_postcode(&p).ok_or_else(|| PosError::Validation(format!("{p} is not a valid postcode")))?),
        None => None,
    };
    Ok(Clean {
        name: name.to_string(),
        digits: phone.as_deref().map(phone_digits).filter(|d| !d.is_empty()),
        phone,
        line1: text(&c.address_line1, 100, "address")?,
        line2: text(&c.address_line2, 100, "address")?,
        town: text(&c.town, 60, "town")?,
        postcode,
        notes: text(&c.notes, 200, "notes")?,
    })
}

impl Pos {
    /// Search by phone (any 3+ digits of it), name, or postcode, most
    /// recent customers first.
    pub fn search_customers(&self, s: &Session, query: &str, limit: i64) -> Result<Vec<Customer>> {
        s.require(Permission::Sell)?;
        let q = query.trim();
        let limit = limit.clamp(1, 100);
        let digits = phone_digits(q);
        let mut stmt;
        let rows = if q.is_empty() {
            stmt = self.conn.prepare_cached(&format!(
                "SELECT {COLS} FROM customers WHERE active = 1
                 ORDER BY last_order_at DESC NULLS LAST, id DESC LIMIT ?1"
            ))?;
            stmt.query_map(params![limit], customer_from_row)?
        } else {
            stmt = self.conn.prepare_cached(&format!(
                "SELECT {COLS} FROM customers
                 WHERE active = 1 AND (
                     (length(?2) >= 3 AND phone_digits LIKE '%' || ?2 || '%')
                     OR name LIKE '%' || ?1 || '%'
                     OR replace(postcode, ' ', '') LIKE replace(?1, ' ', '') || '%')
                 ORDER BY last_order_at DESC NULLS LAST, id DESC LIMIT ?3"
            ))?;
            stmt.query_map(params![q, digits, limit], customer_from_row)?
        };
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Saved addresses at a postcode, in door-number order.
    pub fn customers_at_postcode(&self, s: &Session, postcode: &str) -> Result<Vec<Customer>> {
        s.require(Permission::Sell)?;
        let Some(pc) = normalize_postcode(postcode) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM customers WHERE active = 1 AND postcode = ?1
             ORDER BY CAST(address_line1 AS INTEGER), address_line1, name"
        ))?;
        let rows = stmt.query_map([pc], customer_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get_customer(&self, s: &Session, id: i64) -> Result<Customer> {
        s.require(Permission::Sell)?;
        self.conn
            .prepare_cached(&format!("SELECT {COLS} FROM customers WHERE id = ?1 AND active = 1"))?
            .query_row([id], customer_from_row)
            .optional()?
            .ok_or(PosError::NotFound)
    }

    pub fn create_customer(&self, s: &Session, input: &CustomerInput) -> Result<Customer> {
        s.require(Permission::Sell)?;
        let c = clean(input)?;
        self.conn.execute(
            "INSERT INTO customers (name, phone, phone_digits, address_line1, address_line2, town, postcode, notes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![c.name, c.phone, c.digits, c.line1, c.line2, c.town, c.postcode, c.notes],
        )?;
        let id = self.conn.last_insert_rowid();
        self.audit(Some(s.user_id), "customer.create", &id.to_string())?;
        self.get_customer(s, id)
    }

    pub fn update_customer(&self, s: &Session, id: i64, input: &CustomerInput) -> Result<Customer> {
        s.require(Permission::Sell)?;
        let c = clean(input)?;
        let changed = self.conn.execute(
            "UPDATE customers SET name = ?2, phone = ?3, phone_digits = ?4, address_line1 = ?5, address_line2 = ?6,
                 town = ?7, postcode = ?8, notes = ?9, updated_at = unixepoch()
             WHERE id = ?1 AND active = 1",
            params![id, c.name, c.phone, c.digits, c.line1, c.line2, c.town, c.postcode, c.notes],
        )?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "customer.update", &id.to_string())?;
        self.get_customer(s, id)
    }

    /// Remove a customer from the list (admin). Past orders keep their copy
    /// of the name and address.
    pub fn delete_customer(&self, s: &Session, id: i64) -> Result<()> {
        s.require(Permission::ManageSettings)?;
        if self.conn.execute("UPDATE customers SET active = 0 WHERE id = ?1 AND active = 1", [id])? == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "customer.delete", &id.to_string())
    }

    /// A customer's recent orders, newest first (for "same as last time").
    pub fn customer_orders(&self, s: &Session, id: i64, limit: i64) -> Result<Vec<OrderSummary>> {
        s.require(Permission::Sell)?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "{} WHERE o.customer_id = ?1 ORDER BY o.opened_at DESC, o.id DESC LIMIT ?2",
            crate::orders::SUMMARY_SQL
        ))?;
        let rows = stmt.query_map(params![id, limit.clamp(1, 50)], crate::orders::summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn touch_customer(&self, id: i64) -> Result<()> {
        let changed = self
            .conn
            .prepare_cached(
                "UPDATE customers SET order_count = order_count + 1, last_order_at = unixepoch()
                 WHERE id = ?1 AND active = 1",
            )?
            .execute([id])?;
        if changed == 0 {
            return invalid("that customer does not exist");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postcodes() {
        assert_eq!(normalize_postcode("sw1a2aa").as_deref(), Some("SW1A 2AA"));
        assert_eq!(normalize_postcode(" M1 1AE ").as_deref(), Some("M1 1AE"));
        assert_eq!(normalize_postcode("b338th").as_deref(), Some("B33 8TH"));
        assert_eq!(normalize_postcode("CR2 6XH").as_deref(), Some("CR2 6XH"));
        assert_eq!(normalize_postcode("DN55 1PT").as_deref(), Some("DN55 1PT"));
        assert!(normalize_postcode("hello").is_none());
        assert!(normalize_postcode("12345").is_none());
        assert!(normalize_postcode("SW1A 2A").is_none());
    }

    #[test]
    fn phones() {
        assert_eq!(phone_digits("07700 900 123"), "07700900123");
        assert_eq!(phone_digits("+44 7700 900123"), "07700900123");
        assert_eq!(phone_digits("0044 7700 900123"), "07700900123");
        assert_eq!(phone_digits("(020) 7946-0018"), "02079460018");
    }
}

//! Delivery platforms ("channels") such as Uber Eats, Just Eat and Deliveroo.
//! Orders taken through one are tagged with it, so they get their own quick
//! button, ticket heading and line in reports. The list is editable.

use std::collections::HashSet;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{Permission, Pos, PosError, Result, Session};

#[derive(Debug, Clone, Serialize)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
}

/// One platform as edited in Settings; `id` is None for new ones.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelInput {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}

pub(crate) fn valid_color(c: Option<&str>) -> Result<Option<String>> {
    let c = c.map(str::trim).filter(|c| !c.is_empty());
    if let Some(c) = c {
        let hex = c.strip_prefix('#').unwrap_or("");
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return invalid("colour must look like #1a2b3c");
        }
    }
    Ok(c.map(str::to_string))
}

impl Pos {
    pub fn list_channels(&self, s: &Session) -> Result<Vec<Channel>> {
        s.require(Permission::Sell)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, color FROM delivery_channels WHERE active = 1 ORDER BY sort_order, id",
        )?;
        let rows = stmt.query_map([], |r| Ok(Channel { id: r.get(0)?, name: r.get(1)?, color: r.get(2)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replace the platform list with the editor's (in display order):
    /// existing ids are renamed, new ones added, missing ones retired.
    /// Past orders keep pointing at retired platforms, so reports still work.
    pub fn save_channels(&mut self, s: &Session, list: &[ChannelInput]) -> Result<Vec<Channel>> {
        s.require(Permission::ManageSettings)?;
        let mut names = HashSet::new();
        let mut cleaned = Vec::with_capacity(list.len());
        for c in list {
            let name = c.name.trim();
            if name.is_empty() || name.chars().count() > 30 {
                return invalid("platform names must be 1 to 30 characters");
            }
            if !names.insert(name.to_lowercase()) {
                return invalid(format!("{name} is listed twice"));
            }
            cleaned.push((c.id, name.to_string(), valid_color(c.color.as_deref())?));
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: HashSet<i64> = tx
            .prepare("SELECT id FROM delivery_channels WHERE active = 1")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let keep: HashSet<i64> = cleaned.iter().filter_map(|c| c.0).collect();
        if keep.iter().any(|id| !existing.contains(id)) {
            return Err(PosError::NotFound);
        }
        for id in existing.difference(&keep) {
            tx.execute("UPDATE delivery_channels SET active = 0 WHERE id = ?1", [id])?;
        }
        // Park names first so renames that swap names don't collide.
        tx.execute("UPDATE delivery_channels SET name = char(1) || id WHERE active = 1", [])?;
        for (i, (id, name, color)) in cleaned.iter().enumerate() {
            match id {
                Some(id) => tx.execute(
                    "UPDATE delivery_channels SET name = ?2, color = ?3, sort_order = ?4 WHERE id = ?1",
                    params![id, name, color, i as i64],
                )?,
                None => tx.execute(
                    "INSERT INTO delivery_channels (name, color, sort_order) VALUES (?1, ?2, ?3)",
                    params![name, color, i as i64],
                )?,
            };
        }
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'channels.save', ?2)",
            params![s.user_id, cleaned.iter().map(|c| c.1.as_str()).collect::<Vec<_>>().join(", ")],
        )?;
        tx.commit()?;
        self.list_channels(s)
    }

    pub(crate) fn check_channel(&self, id: i64) -> Result<()> {
        let ok: bool = self
            .conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM delivery_channels WHERE id = ?1 AND active = 1)")?
            .query_row([id], |r| r.get(0))?;
        if !ok {
            return invalid("that delivery platform does not exist");
        }
        Ok(())
    }
}

//! Kitchen teams (stations): Starter, Main, Curry, Rice... Each menu item
//! belongs to at most one team, and a kitchen ticket is printed for each team
//! that has something to cook, headed by the team's name. The list is editable
//! in Settings; a category can give its team to the items in it.

use std::collections::HashSet;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{Permission, Pos, PosError, Result, Session};

const MAX_TEAMS: usize = 20;

/// Setting: the teams a dish can be moved to when it is ordered ("Which team
/// makes this?"), as team ids in a comma list, e.g. "1,2". Empty or not set:
/// off, and "Starter or Main course?" (`starter_choice`) asks as before.
/// A kitchen setting: managers may change it.
pub const TEAM_CHOICE_KEY: &str = "team_choice";

#[derive(Debug, Clone, Serialize)]
pub struct Team {
    pub id: i64,
    pub name: String,
    /// Second name (Thai), printed under the name when tickets show both.
    pub name_alt: Option<String>,
    /// Menu items in this team.
    pub items: i64,
}

/// One team as edited in Settings; `id` is None for new ones.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamInput {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub name_alt: Option<String>,
}

pub(crate) fn load_teams(conn: &Connection) -> Result<Vec<Team>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.name, t.name_alt, (SELECT COUNT(*) FROM products p WHERE p.team_id = t.id AND p.deleted = 0)
         FROM teams t WHERE t.active = 1 ORDER BY t.sort_order, t.id",
    )?;
    let rows = stmt.query_map([], |r| Ok(Team { id: r.get(0)?, name: r.get(1)?, name_alt: r.get(2)?, items: r.get(3)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

impl Pos {
    /// The kitchen teams (names and how many menu items each has): anyone
    /// selling may read them, e.g. for the kitchen screen's team filter.
    pub fn list_teams(&self, s: &Session) -> Result<Vec<Team>> {
        s.require(Permission::Sell)?;
        load_teams(&self.conn)
    }

    /// Replace the team list with the editor's (in display order): existing
    /// ids are renamed, new ones added, missing ones removed. A removed team's
    /// items and categories go back to the general ticket.
    pub fn save_teams(&mut self, s: &Session, list: &[TeamInput]) -> Result<Vec<Team>> {
        s.require(Permission::KitchenSettings)?;
        if list.len() > MAX_TEAMS {
            return invalid(format!("at most {MAX_TEAMS} teams"));
        }
        let mut names = HashSet::new();
        let mut cleaned = Vec::with_capacity(list.len());
        for t in list {
            let name = t.name.trim();
            if name.is_empty() || name.chars().count() > 30 {
                return invalid("team names must be 1 to 30 characters");
            }
            if !names.insert(name.to_lowercase()) {
                return invalid(format!("{name} is listed twice"));
            }
            cleaned.push((t.id, name.to_string(), crate::products::clean_alt(t.name_alt.as_deref(), 30)?));
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: HashSet<i64> = tx
            .prepare("SELECT id FROM teams WHERE active = 1")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let keep: HashSet<i64> = cleaned.iter().filter_map(|c| c.0).collect();
        if keep.iter().any(|id| !existing.contains(id)) {
            return Err(PosError::NotFound);
        }
        for id in existing.difference(&keep) {
            tx.execute("UPDATE products SET team_id = NULL WHERE team_id = ?1", [id])?;
            tx.execute("UPDATE categories SET team_id = NULL WHERE team_id = ?1", [id])?;
            tx.execute("UPDATE order_items SET team_id = NULL WHERE team_id = ?1", [id])?;
            tx.execute("UPDATE teams SET active = 0 WHERE id = ?1", [id])?;
        }
        // Park names first so renames that swap names don't collide.
        tx.execute("UPDATE teams SET name = char(1) || id WHERE active = 1", [])?;
        for (i, (id, name, alt)) in cleaned.iter().enumerate() {
            match id {
                Some(id) => tx.execute(
                    "UPDATE teams SET name = ?2, name_alt = ?3, sort_order = ?4 WHERE id = ?1",
                    params![id, name, alt, i as i64],
                )?,
                None => tx.execute(
                    "INSERT INTO teams (name, name_alt, sort_order) VALUES (?1, ?2, ?3)",
                    params![name, alt, i as i64],
                )?,
            };
        }
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'teams.save', ?2)",
            params![s.user_id, cleaned.iter().map(|c| c.1.as_str()).collect::<Vec<_>>().join(", ")],
        )?;
        tx.commit()?;
        load_teams(&self.conn)
    }

    /// Give a category a team for its new items, and optionally move the
    /// items already in it. Returns how many items were moved.
    pub fn set_category_team(&self, s: &Session, id: i64, team_id: Option<i64>, move_items: bool) -> Result<usize> {
        s.require(Permission::ManageMenu)?;
        self.check_team(team_id)?;
        let changed = self.conn.execute("UPDATE categories SET team_id = ?2 WHERE id = ?1", params![id, team_id])?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        let moved = if move_items {
            self.conn.execute(
                "UPDATE products SET team_id = ?2, updated_at = unixepoch() WHERE category_id = ?1 AND deleted = 0",
                params![id, team_id],
            )?
        } else {
            0
        };
        self.audit(Some(s.user_id), "category.team", &format!("{id} team={team_id:?} moved={moved}"))?;
        Ok(moved)
    }

    /// Mark a category as desserts (or not); `move_items` does the same to the
    /// items already in it. Returns how many items changed.
    pub fn set_category_dessert(&self, s: &Session, id: i64, dessert: bool, move_items: bool) -> Result<usize> {
        s.require(Permission::ManageMenu)?;
        let changed = self.conn.execute("UPDATE categories SET dessert = ?2 WHERE id = ?1", params![id, dessert])?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        let moved = if move_items {
            self.conn.execute(
                "UPDATE products SET dessert = ?2, updated_at = unixepoch() WHERE category_id = ?1 AND deleted = 0 AND dessert <> ?2",
                params![id, dessert],
            )?
        } else {
            0
        };
        self.audit(Some(s.user_id), "category.dessert", &format!("{id} dessert={dessert} moved={moved}"))?;
        Ok(moved)
    }

    /// Mark a category as drinks (or not); `move_items` does the same to the
    /// items already in it. Returns how many items changed.
    pub fn set_category_drink(&self, s: &Session, id: i64, drink: bool, move_items: bool) -> Result<usize> {
        s.require(Permission::ManageMenu)?;
        let changed = self.conn.execute("UPDATE categories SET drink = ?2 WHERE id = ?1", params![id, drink])?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        let moved = if move_items {
            self.conn.execute(
                "UPDATE products SET drink = ?2, updated_at = unixepoch() WHERE category_id = ?1 AND deleted = 0 AND drink <> ?2",
                params![id, drink],
            )?
        } else {
            0
        };
        self.audit(Some(s.user_id), "category.drink", &format!("{id} drink={drink} moved={moved}"))?;
        Ok(moved)
    }

    /// The teams staff may move a dish to (setting [`TEAM_CHOICE_KEY`]), in
    /// team order. Removed teams are left out; empty = the picker is off
    /// (also while kitchen teams are switched off).
    pub fn team_choice(&self) -> Result<Vec<i64>> {
        if self.setting("teams_on")?.as_deref() == Some("0") {
            return Ok(Vec::new());
        }
        let raw = self.setting(TEAM_CHOICE_KEY)?.unwrap_or_default();
        let ids: Vec<i64> = raw.split([',', ' ']).filter_map(|p| p.trim().parse().ok()).collect();
        let picked: Vec<i64> = load_teams(&self.conn)?.into_iter().map(|t| t.id).filter(|id| ids.contains(id)).collect();
        // One team left is no choice.
        Ok(if picked.len() < 2 { Vec::new() } else { picked })
    }

    /// A [`TEAM_CHOICE_KEY`] value checked and tidied: "" (off), or two or
    /// more existing teams, saved as "1,2" in team order.
    pub(crate) fn clean_team_choice(&self, value: &str) -> Result<String> {
        let parts: Vec<&str> = value.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return Ok(String::new());
        }
        let mut ids = HashSet::new();
        for p in &parts {
            let id: i64 = p.parse().map_err(|_| PosError::Validation("teams to choose from: team numbers separated by commas".into()))?;
            self.check_team(Some(id))?;
            if !ids.insert(id) {
                return invalid("a team is listed twice");
            }
        }
        if ids.len() < 2 {
            return invalid("tick at least two teams to choose from (or none to switch it off)");
        }
        let order: Vec<String> = load_teams(&self.conn)?.into_iter().filter(|t| ids.contains(&t.id)).map(|t| t.id.to_string()).collect();
        Ok(order.join(","))
    }

    pub(crate) fn check_team(&self, id: Option<i64>) -> Result<()> {
        if let Some(id) = id {
            let ok: bool = self
                .conn
                .prepare_cached("SELECT EXISTS (SELECT 1 FROM teams WHERE id = ?1 AND active = 1)")?
                .query_row([id], |r| r.get(0))?;
            if !ok {
                return invalid("that team does not exist");
            }
        }
        Ok(())
    }
}

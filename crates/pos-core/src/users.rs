use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

use crate::error::{invalid, map_unique};
use crate::{Permission, Pos, PosError, Result, Role, Session};

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub role: Role,
    pub active: bool,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
}

const PIN_TAKEN: &str = "that PIN is already in use, choose another";
const COLS: &str = "id, name, role, active, created_at, last_login_at";

fn user_from_row(r: &Row) -> rusqlite::Result<User> {
    let role: String = r.get(2)?;
    Ok(User {
        id: r.get(0)?,
        name: r.get(1)?,
        role: Role::parse(&role).unwrap_or(Role::Staff),
        active: r.get(3)?,
        created_at: r.get(4)?,
        last_login_at: r.get(5)?,
    })
}

pub(crate) fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return invalid("name must be 1 to 60 characters");
    }
    Ok(name.to_string())
}

impl Pos {
    pub fn list_users(&self, s: &Session) -> Result<Vec<User>> {
        s.require(Permission::ManageUsers)?;
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {COLS} FROM users ORDER BY active DESC, name"))?;
        let rows = stmt.query_map([], user_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get_user(&self, s: &Session, id: i64) -> Result<User> {
        if s.user_id != id {
            s.require(Permission::ManageUsers)?;
        }
        self.conn
            .prepare_cached(&format!("SELECT {COLS} FROM users WHERE id = ?1"))?
            .query_row([id], user_from_row)
            .optional()?
            .ok_or(PosError::NotFound)
    }

    pub fn create_user(&self, s: &Session, name: &str, role: Role, pin: &str) -> Result<User> {
        s.require(Permission::ManageUsers)?;
        let name = clean_name(name)?;
        let hash = self.hash_pin(pin)?;
        self.conn
            .execute(
                "INSERT INTO users (name, role, pin_hash) VALUES (?1, ?2, ?3)",
                params![name, role.as_str(), hash],
            )
            .map_err(|e| map_unique(e, PIN_TAKEN))?;
        let id = self.conn.last_insert_rowid();
        self.audit(Some(s.user_id), "user.create", &format!("{id} {name} {}", role.as_str()))?;
        self.get_user(s, id)
    }

    /// Change name, role or active flag. Refuses to remove the last admin.
    pub fn update_user(
        &mut self,
        s: &Session,
        id: i64,
        name: &str,
        role: Role,
        active: bool,
    ) -> Result<User> {
        s.require(Permission::ManageUsers)?;
        let name = clean_name(name)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE users SET name = ?2, role = ?3, active = ?4 WHERE id = ?1",
            params![id, name, role.as_str(), active],
        )?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        let admins: i64 = tx.query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1",
            [],
            |r| r.get(0),
        )?;
        if admins == 0 {
            return invalid("there must be at least one active admin");
        }
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'user.update', ?2)",
            params![s.user_id, format!("{id} {name} {} active={active}", role.as_str())],
        )?;
        tx.commit()?;
        self.get_user(s, id)
    }

    /// Admin sets a new PIN for any user (including themselves).
    pub fn reset_pin(&self, s: &Session, id: i64, new_pin: &str) -> Result<()> {
        s.require(Permission::ManageUsers)?;
        let hash = self.hash_pin(new_pin)?;
        let changed = self
            .conn
            .execute("UPDATE users SET pin_hash = ?2 WHERE id = ?1", params![id, hash])
            .map_err(|e| map_unique(e, PIN_TAKEN))?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "user.reset_pin", &id.to_string())
    }
}

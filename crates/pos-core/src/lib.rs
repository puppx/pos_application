//! POS business logic over a local SQLite database.
//!
//! Everything the app can do goes through [`Pos`]. Methods that need
//! authorisation take a [`Session`], which only `Pos::login` can produce.

mod auth;
mod error;
mod products;
mod reports;
mod roles;
mod sales;
mod users;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};

pub use auth::LoginThrottle;
pub use error::{PosError, Result};
pub use products::{NewProduct, Product, ProductUpdate};
pub use reports::{PaymentTotal, SalesSummary, TopProduct};
pub use roles::{Permission, Role, Session};
pub use sales::{CartLine, PaymentMethod, Quote, QuoteLine, Sale, SaleItem, SaleListItem};
pub use users::User;

const SCHEMA_VERSION: i32 = 1;
const MIGRATIONS: &[&str] = &[include_str!("schema.sql")];

pub struct Pos {
    conn: Connection,
    pin_salt: [u8; 16],
    pin_params: argon2::Params,
    throttle: LoginThrottle,
}

impl Pos {
    /// Open (creating if needed) the database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Pos> {
        Self::from_conn(Connection::open(path)?, auth::default_params())
    }

    /// In-memory database, for tests.
    pub fn open_in_memory() -> Result<Pos> {
        Self::from_conn(Connection::open_in_memory()?, auth::test_params())
    }

    fn from_conn(conn: Connection, pin_params: argon2::Params) -> Result<Pos> {
        conn.busy_timeout(Duration::from_secs(5))?;
        // WAL: readers never block the writer. synchronous=FULL: a completed
        // sale survives power loss (costs one fsync per commit, which is
        // negligible at checkout rates). mmap + 16 MB page cache keep hot
        // pages in RAM without a large footprint on 4 GB machines.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -16000;
             PRAGMA mmap_size = 134217728;",
        )?;
        conn.set_prepared_statement_cache_capacity(64);
        migrate(&conn)?;
        let pin_salt = load_or_create_salt(&conn)?;
        Ok(Pos { conn, pin_salt, pin_params, throttle: LoginThrottle::default() })
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        if key.starts_with('_') {
            return Ok(None);
        }
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([key], |r| r.get::<_, String>(0))
            .optional()?)
    }

    pub fn set_setting(&self, s: &Session, key: &str, value: &str) -> Result<()> {
        s.require(Permission::ManageSettings)?;
        if key.starts_with('_') {
            return error::invalid("reserved setting key");
        }
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        self.audit(Some(s.user_id), "setting.update", key)
    }

    /// Consistent online copy of the database; safe while the app is in use.
    pub fn backup_to(&self, s: &Session, dest: impl AsRef<Path>) -> Result<()> {
        s.require(Permission::Backup)?;
        let dest = dest.as_ref();
        self.backup_unchecked(dest)?;
        self.audit(Some(s.user_id), "backup", &dest.display().to_string())
    }

    /// Backup without a session, for the app's automatic startup backups.
    pub fn backup_unchecked(&self, dest: impl AsRef<Path>) -> Result<()> {
        self.conn.backup(rusqlite::DatabaseName::Main, dest, None)?;
        Ok(())
    }

    /// Run on shutdown: lets SQLite refresh planner statistics cheaply.
    pub fn optimize(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA optimize;")?;
        Ok(())
    }

    fn audit(&self, user_id: Option<i64>, action: &str, detail: &str) -> Result<()> {
        self.conn
            .prepare_cached("INSERT INTO audit_log (user_id, action, detail) VALUES (?1, ?2, ?3)")?
            .execute(rusqlite::params![user_id, action, detail])?;
        Ok(())
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(PosError::Internal(format!(
            "database schema v{version} is newer than this app (v{SCHEMA_VERSION}); update the app"
        )));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

fn load_or_create_salt(conn: &Connection) -> Result<[u8; 16]> {
    let existing: Option<Vec<u8>> = conn
        .query_row("SELECT value FROM settings WHERE key = '_pin_salt'", [], |r| r.get(0))
        .optional()?;
    if let Some(v) = existing {
        return v.try_into().map_err(|_| PosError::Internal("corrupt PIN salt".into()));
    }
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|e| PosError::Internal(e.to_string()))?;
    conn.execute("INSERT INTO settings (key, value) VALUES ('_pin_salt', ?1)", [&salt[..]])?;
    Ok(salt)
}

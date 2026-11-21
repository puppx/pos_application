//! Restoring a full backup (a whole database file), e.g. to move the POS to
//! a new PC.
//!
//! The backup is never opened in place. It is copied to a private work file,
//! checked there (a SQLite database, `integrity_check` ok, the POS tables, a
//! schema no newer than this app's) and migrated to this app's schema. Only
//! then is it copied into the live connection with SQLite's online backup
//! API, which writes every page in one write transaction: it either
//! completes or rolls back, so the current data is never half replaced. The
//! live database keeps its file, path, WAL and connection, so the app keeps
//! running and the next start opens the restored data.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;

use crate::error::invalid;
use crate::{migrate, receipt, Permission, Pos, PosError, Result, Session, OPEN_ITEM_SKU, SCHEMA_VERSION};

/// Tables every POS database has had since the first version.
const REQUIRED_TABLES: &[&str] = &["settings", "users", "orders", "order_items", "products", "categories", "audit_log"];

/// What a backup holds, shown before restoring it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BackupSummary {
    /// Schema version of the file before it was brought up to date.
    pub schema_version: i32,
    /// This app's schema version.
    pub app_schema_version: i32,
    pub store_name: Option<String>,
    pub orders: i64,
    pub open_orders: i64,
    /// Menu items (not deleted or hidden).
    pub menu_items: i64,
    pub customers: i64,
    /// Active staff (everyone who can log in).
    pub staff: i64,
    /// When the newest order was opened (unix seconds).
    pub last_order_at: Option<i64>,
    /// Last thing recorded in the backup (a login, a sale…): about when it was taken.
    pub last_used_at: Option<i64>,
}

/// A backup checked and migrated into a private work file, ready to be
/// copied into the live database. Dropping it deletes the work file.
pub struct PreparedRestore {
    work: PathBuf,
    salt: [u8; 16],
    pub summary: BackupSummary,
}

impl Drop for PreparedRestore {
    fn drop(&mut self) {
        remove_with_sidecars(&self.work);
    }
}

/// Put `path`'s modified time back to `made` if reading it changed it (some
/// systems count a copy, e.g. through a backup or antivirus filter): the
/// backup folder is ordered by these times, so a restored or checked file
/// must never pass for the newest daily backup. Best effort.
fn keep_modified(path: &Path, made: Option<std::time::SystemTime>) {
    let Some(made) = made else { return };
    if std::fs::metadata(path).and_then(|m| m.modified()).is_ok_and(|now| now != made) {
        let reset = std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(made));
        if let Err(e) = reset {
            eprintln!("backup check: cannot keep the file's time: {e}");
        }
    }
}

/// Delete a database file and any journal / WAL files next to it.
fn remove_with_sidecars(path: &Path) {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}

fn not_a_backup<T>() -> Result<T> {
    invalid(NOT_A_BACKUP)
}

const NOT_A_BACKUP: &str = "This file is not a POS backup. Choose a .db file made with \"Back up everything now\".";
const DAMAGED: &str = "This backup file is damaged and cannot be restored. Try another backup.";
/// Shown when the disk fills up while copying or restoring.
pub const DISK_FULL: &str = "There is not enough free disk space on this PC. Free some space (for example delete old backups) and try again.";

fn damaged<T>() -> Result<T> {
    invalid(DAMAGED)
}

/// True for SQLite saying the database file is damaged ("database disk
/// image is malformed") or is not a database at all.
/// The `backup.restore` audit row's text, the same for every way of restoring:
/// "714 orders, 13 menu items, 4 staff, by Owner (…), from Demo Bistro".
pub fn restore_audit_detail(summary: &BackupSummary, by: &str) -> String {
    format!(
        "{}, {}, {} staff, by {}{}",
        crate::plural(summary.orders, "order", "orders"),
        crate::plural(summary.menu_items, "menu item", "menu items"),
        summary.staff,
        by,
        summary.store_name.as_deref().map(|n| format!(", from {n}")).unwrap_or_default(),
    )
}

pub fn is_corrupt(e: &PosError) -> bool {
    matches!(e, PosError::Db(d) if matches!(d.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)))
}

/// True for "disk full" from the OS (Linux ENOSPC, Windows ERROR_DISK_FULL
/// and ERROR_HANDLE_DISK_FULL) or SQLite.
pub fn is_disk_full(e: &PosError) -> bool {
    match e {
        PosError::Db(e) => e.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull),
        PosError::Validation(m) => m == DISK_FULL,
        _ => false,
    }
}

/// An I/O error as a message for the user (disk full gets its own).
pub fn io_error(e: &std::io::Error, what: &str) -> PosError {
    let full = e.kind() == std::io::ErrorKind::StorageFull
        || matches!(e.raw_os_error(), Some(28) if cfg!(unix))
        || matches!(e.raw_os_error(), Some(39 | 112) if cfg!(windows));
    if full {
        PosError::Validation(DISK_FULL.into())
    } else {
        PosError::Internal(format!("{what}: {e}"))
    }
}

/// Anything that went wrong while checking a file, as one of the plain
/// messages (the detail goes to the log): never "no such table" and the like.
fn plain(e: PosError) -> PosError {
    match &e {
        // Our own messages (newer version, no admin…) pass through.
        PosError::Validation(_) | PosError::NotFound => return e,
        _ if is_disk_full(&e) => return PosError::Validation(DISK_FULL.into()),
        _ => {}
    }
    eprintln!("backup check: {e}");
    let corrupt = matches!(&e, PosError::Db(d) if d.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseCorrupt))
        || matches!(&e, PosError::Internal(_));
    PosError::Validation(if corrupt { DAMAGED } else { NOT_A_BACKUP }.into())
}

/// ((type, name), sql) of every schema object except SQLite's own, in creation order.
type SchemaObjects = Vec<((String, String), Option<String>)>;

fn schema_objects(conn: &Connection) -> Result<SchemaObjects> {
    let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY rowid")?;
    let rows = stmt.query_map([], |r| Ok(((r.get::<_, String>(0)?, r.get::<_, String>(1)?), r.get::<_, Option<String>>(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<(String, String, bool, i64)>> {
    let mut stmt = conn.prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_xinfo(?1) ORDER BY cid")?;
    let rows = stmt.query_map([table], |r| Ok((r.get(0)?, r.get::<_, String>(1)?.to_ascii_lowercase(), r.get(2)?, r.get(3)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Make the (migrated) file's schema exactly this app's: a backup could
/// carry its own triggers or views, which would then run inside the live
/// database (e.g. a trigger that deletes payments on every login).
/// - Any object this app does not create (a table, view, trigger or index
///   by type and name) is refused.
/// - Every table must have this app's columns.
/// - Triggers, views and indexes are always replaced by this app's own
///   definitions, so one with an expected name but other contents cannot
///   slip through.
fn match_app_schema(conn: &Connection) -> Result<()> {
    let fresh = Connection::open_in_memory()?;
    fresh.execute_batch("PRAGMA foreign_keys = ON;")?;
    migrate(&fresh)?;
    let want = schema_objects(&fresh)?;
    let have = schema_objects(conn)?;
    let wanted: std::collections::HashSet<_> = want.iter().map(|(k, _)| k).collect();
    if let Some(((kind, name), _)) = have.iter().find(|(k, _)| !wanted.contains(k)) {
        eprintln!("backup check: unexpected {kind} {name}");
        return not_a_backup();
    }
    // Each table exactly as this app makes it: same columns, and the same
    // definition text, so no CHECK, DEFAULT or foreign key slips in (a
    // crafted CHECK on audit_log could stop anyone logging in). Migrations
    // never change, so a real database from any version matches.
    let norm = |s: &Option<String>| s.as_deref().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "));
    let have_sql: std::collections::HashMap<_, _> = have.iter().map(|(k, s)| (k, norm(s))).collect();
    for (key @ (kind, name), sql) in &want {
        if kind != "table" {
            continue;
        }
        let differs = match have_sql.get(key) {
            None => true,
            Some(theirs) => *theirs != norm(sql) || columns(conn, name)? != columns(&fresh, name)?,
        };
        if differs {
            eprintln!("backup check: table {name} differs");
            return not_a_backup();
        }
    }
    let code = |k: &str| matches!(k, "trigger" | "view" | "index");
    let tx = conn.unchecked_transaction()?;
    for ((kind, name), _) in have.iter().filter(|((k, _), _)| code(k)) {
        tx.execute_batch(&format!("DROP {} IF EXISTS \"{}\";", kind.to_ascii_uppercase(), name.replace('"', "\"\"")))?;
    }
    // In this app's own order (indexes and views before the triggers that use them).
    for ((kind, _), sql) in want.iter().filter(|((k, _), _)| code(k)) {
        if let Some(sql) = sql {
            tx.execute_batch(sql).map_err(|e| {
                eprintln!("backup check: cannot recreate {kind}: {e}");
                PosError::Validation(DAMAGED.into())
            })?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Copy `src` to `work`, check it, and bring it up to this app's schema.
/// `page_size`: the live database's, so SQLite can copy it in (a WAL
/// database cannot change its page size). `src` itself is never written.
fn prepare(src: &Path, work: &Path, page_size: Option<i64>) -> Result<PreparedRestore> {
    let mut head = [0u8; 16];
    let read = std::fs::File::open(src)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => PosError::NotFound,
            std::io::ErrorKind::UnexpectedEof => PosError::Validation(NOT_A_BACKUP.into()),
            _ => io_error(&e, "cannot read the backup"),
        });
    match read {
        Err(e) => return Err(e),
        Ok(()) if &head != b"SQLite format 3\0" => return not_a_backup(),
        Ok(()) => {}
    }
    // Never pick up a journal or WAL left over from an earlier attempt.
    remove_with_sidecars(work);
    let made = std::fs::metadata(src).and_then(|m| m.modified()).ok();
    let copied = std::fs::copy(src, work);
    keep_modified(src, made);
    if let Err(e) = copied {
        remove_with_sidecars(work);
        return Err(io_error(&e, "cannot copy the backup"));
    }
    // From here on, dropping `prepared` deletes the work file.
    let mut prepared = PreparedRestore { work: work.to_path_buf(), salt: [0; 16], summary: empty_summary() };
    let conn = Connection::open_with_flags(work, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| plain(e.into()))?;
    let (salt, summary) = check_and_migrate(&conn, page_size).map_err(plain)?;
    prepared.salt = salt;
    prepared.summary = summary;
    conn.close().map_err(|(_, e)| plain(PosError::Db(e)))?;
    Ok(prepared)
}

fn check_and_migrate(conn: &Connection, page_size: Option<i64>) -> Result<([u8; 16], BackupSummary)> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let ok: String = conn.query_row("PRAGMA integrity_check(1)", [], |r| r.get(0))?;
    if ok != "ok" {
        eprintln!("backup check: integrity: {ok}");
        return damaged();
    }
    for t in REQUIRED_TABLES {
        let found: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)", [t], |r| r.get(0))?;
        if !found {
            return not_a_backup();
        }
    }
    if version < 1 {
        return not_a_backup();
    }
    if version > SCHEMA_VERSION {
        return invalid("This backup is from a newer version of the app: update this PC first, then restore it.");
    }
    // A plain rollback journal: the work file needs no WAL, and only then can
    // its page size change.
    let mode: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(PosError::Internal(format!("cannot prepare the backup (journal mode {mode})")));
    }
    let own: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    if let Some(size) = page_size.filter(|&p| p != own) {
        conn.execute_batch(&format!("PRAGMA page_size = {size}; VACUUM;"))?;
    }
    // Same settings as `Pos::from_conn`, so migrations run exactly as on startup.
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    migrate(conn)?;
    match_app_schema(conn)?;
    let salt = crate::load_or_create_salt(conn)?;
    // A new database id: every terminal sees the data changed and reloads.
    crate::new_database_id(conn)?;
    let summary = summarize(conn, version)?;
    if !has_admin(conn)? {
        return invalid("This backup has no admin who can log in, so it cannot be restored.");
    }
    Ok((salt, summary))
}

fn empty_summary() -> BackupSummary {
    BackupSummary {
        schema_version: 0, app_schema_version: SCHEMA_VERSION, store_name: None, orders: 0, open_orders: 0, menu_items: 0, customers: 0,
        staff: 0, last_order_at: None, last_used_at: None,
    }
}

fn has_admin(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS (SELECT 1 FROM users WHERE role = 'admin' AND active = 1)", [], |r| r.get(0))?)
}

/// Counts from an up-to-date database.
fn summarize(conn: &Connection, schema_version: i32) -> Result<BackupSummary> {
    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    Ok(BackupSummary {
        schema_version,
        app_schema_version: SCHEMA_VERSION,
        store_name: conn
            .query_row("SELECT CAST(value AS TEXT) FROM settings WHERE key = 'store_name'", [], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten()
            .filter(|s| !s.trim().is_empty()),
        orders: count("SELECT COUNT(*) FROM orders")?,
        open_orders: count("SELECT COUNT(*) FROM orders WHERE status = 'open'")?,
        menu_items: conn.query_row(
            "SELECT COUNT(*) FROM products WHERE active = 1 AND deleted = 0 AND sku IS NOT ?1",
            [OPEN_ITEM_SKU],
            |r| r.get(0),
        )?,
        customers: count("SELECT COUNT(*) FROM customers")?,
        staff: count("SELECT COUNT(*) FROM users WHERE active = 1")?,
        last_order_at: count("SELECT COALESCE(MAX(opened_at), -1) FROM orders").map(|t| (t >= 0).then_some(t))?,
        last_used_at: count("SELECT COALESCE(MAX(created_at), -1) FROM audit_log").map(|t| (t >= 0).then_some(t))?,
    })
}

impl PreparedRestore {
    /// The name of the backup's active admin whose PIN this is, if any. For
    /// restoring when the live data cannot be read (its staff list may be
    /// the damaged part): the admin proves they are an admin of the data
    /// being put back, which is who runs the till afterwards.
    pub fn admin_with_pin(&self, pin: &str) -> Result<Option<String>> {
        let Ok(hash) = crate::auth::argon_hash(pin, &self.salt, &crate::auth::default_params()) else { return Ok(None) };
        let conn = Connection::open_with_flags(&self.work, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        Ok(conn
            .query_row("SELECT name FROM users WHERE role = 'admin' AND active = 1 AND pin_hash = ?1 ORDER BY id LIMIT 1", [hash], |r| r.get(0))
            .optional()?)
    }

    /// Put the backup in place of the database file `db` when it cannot be
    /// opened at all (no connection to it is open): its WAL and shared
    /// memory files go first (never replayed onto the backup), then the
    /// checked copy is written beside it and renamed over it.
    pub fn install_as(&self, db: &Path) -> Result<()> {
        let side = |suffix: &str| {
            let mut p = db.as_os_str().to_owned();
            p.push(suffix);
            PathBuf::from(p)
        };
        for suffix in ["-wal", "-shm", "-journal"] {
            match std::fs::remove_file(side(suffix)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io_error(&e, "cannot remove the damaged data")),
                _ => {}
            }
        }
        let tmp = side(".restore-tmp");
        let copied = std::fs::copy(&self.work, &tmp).and_then(|_| std::fs::File::open(&tmp)?.sync_all()).and_then(|()| std::fs::rename(&tmp, db));
        if let Err(e) = copied {
            let _ = std::fs::remove_file(&tmp);
            return Err(io_error(&e, "cannot put the backup in place"));
        }
        Ok(())
    }
}

impl Pos {
    /// `PRAGMA quick_check` of the database file at `path` through a
    /// connection of its own (no lock on the till's): None when sound, else
    /// what is wrong. A few hundred ms on a big database.
    pub fn quick_check_file(path: impl AsRef<Path>) -> Result<Option<String>> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let ok: String = conn.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
        Ok((ok != "ok").then_some(ok))
    }

    /// The name of the live database's active admin whose PIN this is.
    pub fn admin_with_pin(&self, pin: &str) -> Result<Option<String>> {
        let Ok(hash) = self.hash_pin(pin) else { return Ok(None) };
        Ok(self
            .conn
            .query_row("SELECT name FROM users WHERE role = 'admin' AND active = 1 AND pin_hash = ?1 ORDER BY id LIMIT 1", [hash], |r| r.get(0))
            .optional()?)
    }

    /// A backup file just written is a whole, readable POS database
    /// (`PRAGMA quick_check`), before it counts as a backup or is copied on.
    pub fn verify_backup_file(path: impl AsRef<Path>) -> Result<()> {
        // Read-write (nothing is written): a read-only connection to a WAL
        // file would leave its -wal and -shm files behind.
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        let ok: String = conn.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        conn.close().map_err(|(_, e)| PosError::Db(e))?;
        if ok != "ok" || version < 1 {
            return damaged();
        }
        Ok(())
    }

    /// Check a full backup without changing anything (the work file is
    /// deleted again). Errors are plain messages for the user.
    pub fn check_backup(src: impl AsRef<Path>, work: impl AsRef<Path>) -> Result<BackupSummary> {
        Ok(prepare(src.as_ref(), work.as_ref(), None)?.summary.clone())
    }

    /// [`Pos::prepare_restore`] when the live database cannot be opened
    /// (see [`PreparedRestore::install_as`]): the backup keeps its page size.
    pub fn prepare_restore_file(src: impl AsRef<Path>, work: impl AsRef<Path>) -> Result<PreparedRestore> {
        prepare(src.as_ref(), work.as_ref(), None)
    }

    /// The `backup.restore` row of a restore by [`PreparedRestore::install_as`]
    /// (`detail` from [`restore_audit_detail`], as every restore writes it).
    pub fn audit_restored(&self, detail: &str) -> Result<()> {
        self.audit(None, "backup.restore", detail)
    }

    /// Check `src` and get it ready to restore into this database; nothing
    /// live is touched. `work` is a scratch file the caller picks.
    pub fn prepare_restore(&self, src: impl AsRef<Path>, work: impl AsRef<Path>) -> Result<PreparedRestore> {
        let page_size: i64 = self.conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        prepare(src.as_ref(), work.as_ref(), Some(page_size))
    }

    /// Replace ALL data (orders, menu, customers, staff and PINs, settings)
    /// with a prepared backup. Admins only; the caller checks the PIN and
    /// makes a safety copy first.
    ///
    /// `Err` means nothing changed: the copy never happened (or rolled
    /// back). `Ok` means the data IS replaced, so the caller must log every
    /// terminal out (users and sessions now come from the backup). The
    /// tidying after the copy (audit entry, checkpoint…) is best effort:
    /// a failure there is logged, never reported as a failed restore.
    pub fn restore(&mut self, s: &Session, prepared: PreparedRestore) -> Result<BackupSummary> {
        s.require(Permission::ManageSettings)?;
        s.require(Permission::Backup)?;
        self.restore_by(&s.name, prepared)
    }

    /// [`Pos::restore`] by `by` (an admin's name) without a session: the
    /// recovery on the main till, when the live data is too damaged to log
    /// in. The caller has checked an admin PIN.
    pub fn restore_by(&mut self, by: &str, prepared: PreparedRestore) -> Result<BackupSummary> {
        let src = Connection::open_with_flags(&prepared.work, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        // Another program holding the database: give up soon, not after 5 s.
        self.conn.busy_timeout(std::time::Duration::from_secs(1))?;
        let copied = (|| -> Result<()> {
            // One step copies every page in a single write transaction on the
            // live database; dropping the backup unfinished rolls it back.
            let backup = rusqlite::backup::Backup::new(&src, &mut self.conn)?;
            match backup.step(-1)? {
                rusqlite::backup::StepResult::Done => Ok(()),
                _ => Err(PosError::Conflict("the database is busy; try again".into())),
            }
        })();
        let _ = self.conn.busy_timeout(std::time::Duration::from_secs(5));
        drop(src);
        copied.map_err(|e| match e {
            PosError::Db(ref d) if matches!(d.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)) => {
                PosError::Conflict("the database is busy; try again".into())
            }
            e if is_disk_full(&e) => PosError::Validation(DISK_FULL.into()),
            e => e,
        })?;
        // The data is replaced: from here on, use the backup's PIN salt and
        // forget anything cached about the old schema.
        self.pin_salt = prepared.salt;
        self.conn.flush_prepared_statement_cache();
        self.throttle = Default::default();
        self.log_counts = Default::default();
        let detail = restore_audit_detail(&prepared.summary, by);
        let steps: [(&str, &AfterCopy); 7] = [
            // Keep WAL for a file database (in memory it stays "memory").
            ("journal mode", &|p| Ok(p.conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?)),
            ("migrate", &|p| migrate(&p.conn)),
            ("triggers", &|p| Ok(p.conn.execute_batch(receipt::BILL_STALE_TRIGGERS)?)),
            ("activity triggers", &|p| Ok(p.conn.execute_batch(crate::orders::ACTIVITY_TRIGGERS)?)),
            ("log triggers", &|p| Ok(p.conn.execute_batch(crate::activity::LOG_CHANGE_TRIGGERS)?)),
            ("audit", &|p| p.audit(None, "backup.restore", &detail)),
            // Fold the copied pages back into the main file now.
            ("checkpoint", &|p| Ok(p.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?)),
        ];
        for (what, step) in steps {
            let r = if fail_after_copy(what) { Err(PosError::Internal("test".into())) } else { step(self) };
            if let Err(e) = r {
                eprintln!("restore: {what} after the copy failed: {e}");
            }
        }
        Ok(prepared.summary.clone())
    }
}

/// One tidying step after the copy.
type AfterCopy<'a> = dyn Fn(&mut Pos) -> Result<()> + 'a;

// Tests make a step after the copy fail.
#[cfg(test)]
thread_local! {
    static FAIL_AFTER_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn fail_after_copy(what: &str) -> bool {
    what != "journal mode" && FAIL_AFTER_COPY.with(|f| f.get())
}

#[cfg(not(test))]
fn fail_after_copy(_: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth, NewOrder, NewProduct, OrderDetails, OrderType, Role, MIGRATIONS};

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn product(name: &str) -> NewProduct {
        NewProduct {
            category_id: None, sku: None, barcode: None, name: name.into(), price_cents: 900, cost_cents: 0, tax_rate_bp: Some(0), stock_qty: 0,
            track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        }
    }

    /// The "other PC": its own staff, PINs (and salt), menu and an order.
    fn other_pc(path: &Path) {
        let mut pos = Pos::from_conn(Connection::open(path).unwrap(), auth::test_params()).unwrap();
        let admin = pos.setup_first_admin("Nok", "4321").unwrap();
        pos.create_user(&admin, "Ben", Role::Staff, "5555").unwrap();
        pos.set_setting(&admin, "store_name", "Old Shop").unwrap();
        let curry = pos.create_product(&admin, product("Massaman")).unwrap();
        let o = pos.open_order(&admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: OrderDetails::default() }).unwrap();
        pos.add_item(&admin, o.id, curry.id, 2, None, None, &[]).unwrap();
    }

    fn live() -> (Pos, Session) {
        let mut pos = Pos::open_in_memory().unwrap();
        let s = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_product(&s, product("Pad thai")).unwrap();
        (pos, s)
    }

    #[test]
    fn a_backup_replaces_everything_and_its_pins_work() {
        let dir = tmp();
        let src = dir.path().join("old.db");
        other_pc(&src);
        let (mut pos, s) = live();
        // An old file: checking and restoring it never makes it look new.
        let made = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86_400);
        std::fs::File::options().write(true).open(&src).unwrap().set_modified(made).unwrap();
        let modified = || std::fs::metadata(&src).unwrap().modified().unwrap();
        let before = std::fs::read(&src).unwrap();
        Pos::check_backup(&src, dir.path().join("check.db")).unwrap();
        assert_eq!(modified(), made);
        let prepared = pos.prepare_restore(&src, dir.path().join("work.db")).unwrap();
        assert_eq!(prepared.summary.store_name.as_deref(), Some("Old Shop"));
        assert_eq!((prepared.summary.orders, prepared.summary.open_orders, prepared.summary.menu_items, prepared.summary.staff), (1, 1, 1, 2));
        let summary = pos.restore(&s, prepared).unwrap();
        assert_eq!(summary.schema_version, SCHEMA_VERSION);
        // The backup file itself is untouched (its time too) and the work file is gone.
        assert_eq!(std::fs::read(&src).unwrap(), before);
        assert_eq!(modified(), made);
        assert!(!dir.path().join("work.db").exists());
        // The old PC's PINs work; this till's do not.
        let mut t = Default::default();
        assert!(matches!(pos.login_as(None, "1111", &mut t), Err(PosError::InvalidPin)));
        let nok = pos.login_as(None, "4321", &mut t).unwrap();
        assert_eq!(nok.name, "Nok");
        assert_eq!(pos.login_as(None, "5555", &mut t).unwrap().name, "Ben");
        assert_eq!(pos.setting("store_name").unwrap().as_deref(), Some("Old Shop"));
        let menu: Vec<String> = pos.menu(&nok).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(menu, ["Massaman"]);
        let open = pos.list_open_orders(&nok).unwrap();
        assert_eq!(open.len(), 1);
        // Still a working database: new orders, and the bill and activity triggers are back.
        let o = pos.get_order(&nok, open[0].id).unwrap();
        pos.conn.execute("UPDATE orders SET bill_printed_at = 1, touched_at = NULL WHERE id = ?1", [o.id]).unwrap();
        pos.add_item(&nok, o.id, menu_id(&pos, &nok), 1, None, None, &[]).unwrap();
        let (stale, touched): (Option<i64>, Option<i64>) =
            pos.conn.query_row("SELECT bill_printed_at, touched_at FROM orders WHERE id = ?1", [o.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(stale, None);
        assert!(touched.is_some());
        let audit: String =
            pos.conn.query_row("SELECT detail FROM audit_log WHERE action = 'backup.restore'", [], |r| r.get(0)).unwrap();
        assert!(audit.contains("by Owner") && audit.contains("Old Shop"), "{audit}");
    }

    fn menu_id(pos: &Pos, s: &Session) -> i64 {
        pos.menu(s).unwrap()[0].id
    }

    #[test]
    fn restoring_into_a_wal_file_keeps_the_path_and_reopens() {
        let dir = tmp();
        let src = dir.path().join("old.db");
        other_pc(&src);
        let live_path = dir.path().join("pos.db");
        let mut pos = Pos::from_conn(Connection::open(&live_path).unwrap(), auth::test_params()).unwrap();
        let s = pos.setup_first_admin("Owner", "1111").unwrap();
        let prepared = pos.prepare_restore(&src, dir.path().join("work.db")).unwrap();
        pos.restore(&s, prepared).unwrap();
        let mode: String = pos.conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        drop(pos);
        // The next start opens the restored data from the same file.
        let mut pos = Pos::from_conn(Connection::open(&live_path).unwrap(), auth::test_params()).unwrap();
        assert_eq!(pos.login_as(None, "4321", &mut Default::default()).unwrap().name, "Nok");
    }

    #[test]
    fn checking_a_backup_leaves_no_files_beside_it() {
        let dir = tmp();
        let live = dir.path().join("live.db");
        let mut pos = Pos::from_conn(Connection::open(&live).unwrap(), auth::test_params()).unwrap();
        pos.setup_first_admin("Owner", "1111").unwrap();
        let backups = dir.path().join("backups");
        std::fs::create_dir(&backups).unwrap();
        let file = backups.join("pos-1.db");
        pos.backup_unchecked(&file).unwrap();
        Pos::backup_database_file(&live, backups.join("pos-2.db")).unwrap();
        for name in ["pos-1.db", "pos-2.db"] {
            let f = backups.join(name);
            Pos::verify_backup_file(&f).unwrap();
            assert_eq!(Pos::quick_check_file(&f).unwrap(), None);
            Pos::check_backup(&f, backups.join("restore-work-1.tmp")).unwrap();
            pos.prepare_restore(&f, backups.join("restore-work-2.tmp")).unwrap();
        }
        let mut left: Vec<String> = std::fs::read_dir(&backups).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["pos-1.db", "pos-2.db"]);
    }

    #[test]
    fn an_older_backup_is_brought_up_to_date() {
        let dir = tmp();
        let src = dir.path().join("old.db");
        // A database from an older app (schema v12: before managers and shared PINs).
        let conn = Connection::open(&src).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (i, sql) in MIGRATIONS.iter().enumerate().take(12) {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", i as i32 + 1).unwrap();
        }
        let salt = [7u8; 16];
        conn.execute("INSERT INTO settings (key, value) VALUES ('_pin_salt', ?1)", [&salt[..]]).unwrap();
        let mut hash = vec![0u8; 32];
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, auth::test_params())
            .hash_password_into(b"2468", &salt, &mut hash)
            .unwrap();
        conn.execute("INSERT INTO users (name, role, pin_hash) VALUES ('Old admin', 'admin', ?1)", [&hash]).unwrap();
        conn.execute("INSERT INTO products (name, price_cents) VALUES ('Tom yum', 750)", []).unwrap();
        drop(conn);
        let (mut pos, s) = live();
        let prepared = pos.prepare_restore(&src, dir.path().join("work.db")).unwrap();
        assert_eq!(prepared.summary.schema_version, 12);
        assert_eq!(prepared.summary.menu_items, 1);
        pos.restore(&s, prepared).unwrap();
        let v: i32 = pos.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let admin = pos.login_as(None, "2468", &mut Default::default()).unwrap();
        assert_eq!((admin.name.as_str(), admin.role), ("Old admin", Role::Admin));
        assert_eq!(pos.menu(&admin).unwrap()[0].name, "Tom yum");
        // The backup file stays at its old version (only the work copy was migrated).
        let old: i32 = Connection::open(&src).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(old, 12);
    }

    #[test]
    fn a_0_13_backup_restores_with_the_activity_columns_filled_in() {
        let dir = tmp();
        let src = dir.path().join("v31.db");
        // A database from v0.13.9 (schema v31): no tickets, no audit_log.order_id.
        let conn = Connection::open(&src).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (i, sql) in MIGRATIONS.iter().enumerate().take(31) {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", i as i32 + 1).unwrap();
        }
        let salt = [7u8; 16];
        conn.execute("INSERT INTO settings (key, value) VALUES ('_pin_salt', ?1)", [&salt[..]]).unwrap();
        let mut hash = vec![0u8; 32];
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, auth::test_params())
            .hash_password_into(b"2468", &salt, &mut hash)
            .unwrap();
        conn.execute("INSERT INTO users (name, role, pin_hash) VALUES ('Old admin', 'admin', ?1)", [&hash]).unwrap();
        conn.execute_batch(
            "INSERT INTO users (name, role, pin_hash) VALUES ('Mia', 'manager', x'01'), ('Sam', 'staff', x'02');
             INSERT INTO audit_log (user_id, action, detail) VALUES
                 (3, 'order.item_void', 'order 12 Tom yum x1 (PIN of Mia #2, manager)'),
                 (2, 'order.price', 'order 14 Pad thai 900 -> 800 by Sam'),
                 (1, 'order.delete', '#9 paid total 1200 oops'),
                 (3, 'order.cancel', '15 (PIN of Mia #2, manager)'),
                 (1, 'drawer.open', 'cash payment, order 16'),
                 (1, 'order.cancel', '2 empty order(s) at day close'),
                 (1, 'product.update', '7 Rice');",
        )
        .unwrap();
        drop(conn);
        let (mut pos, s) = live();
        let prepared = pos.prepare_restore(&src, dir.path().join("work.db")).unwrap();
        assert_eq!(prepared.summary.schema_version, 31);
        pos.restore(&s, prepared).unwrap();
        let v: i32 = pos.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let refs: Vec<(String, Option<i64>, Option<i64>)> = pos
            .conn
            .prepare("SELECT action, order_id, approver_id FROM audit_log WHERE action <> 'backup.restore' ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let want = |a: &str, o: Option<i64>, p: Option<i64>| (a.to_string(), o, p);
        assert_eq!(
            refs,
            [
                want("order.item_void", Some(12), Some(2)),
                want("order.price", Some(14), Some(2)),
                want("order.delete", Some(9), None),
                want("order.cancel", Some(15), Some(2)),
                want("drawer.open", Some(16), None),
                want("order.cancel", None, None),
                want("product.update", None, None),
            ]
        );
        // The new tables are there and work after the restore.
        let admin = pos.login_as(None, "2468", &mut Default::default()).unwrap();
        let t = pos
            .create_ticket(&admin, &crate::NewTicket { category: "till".into(), urgency: None, body: "after restore".into(), order_id: None, screen: None, screen_label: None, device: None })
            .unwrap();
        assert_eq!(t.messages.len(), 1);
        let terminal: Option<String> = pos.conn.query_row("SELECT terminal FROM audit_log WHERE action = 'ticket.create'", [], |r| r.get(0)).unwrap();
        assert_eq!(terminal, None);
    }

    #[test]
    fn a_backup_from_before_unique_payment_ids_restores_with_its_payments() {
        let dir = tmp();
        let src = dir.path().join("v30.db");
        // A database from v0.13.5 (schema v30): payment ids could be reused.
        let conn = Connection::open(&src).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (i, sql) in MIGRATIONS.iter().enumerate().take(30) {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", i as i32 + 1).unwrap();
        }
        let salt = [7u8; 16];
        conn.execute("INSERT INTO settings (key, value) VALUES ('_pin_salt', ?1)", [&salt[..]]).unwrap();
        let mut hash = vec![0u8; 32];
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, auth::test_params())
            .hash_password_into(b"2468", &salt, &mut hash)
            .unwrap();
        conn.execute("INSERT INTO users (name, role, pin_hash) VALUES ('Old admin', 'admin', ?1)", [&hash]).unwrap();
        conn.execute_batch(
            "INSERT INTO orders (id, order_type, opened_by) VALUES (1, 'takeaway', 1);
             INSERT INTO payments (id, order_id, method, amount_cents, tip_cents, tendered_cents, change_cents, items, paid_by, paid_at, shift_id)
                 VALUES (3, 1, 'cash', 500, 50, 1000, 450, NULL, 1, 1700000000, NULL),
                        (7, 1, 'card', 200, 0, 0, 0, '[[1,1]]', 1, 1700000100, NULL);
             -- Payment 9 was taken back (the newest): its id was free again.
             INSERT INTO audit_log (user_id, action, detail)
                 VALUES (1, 'order.payment_removed', 'order 1 payment 9 cash 100 tip 0 taken by Old admin (no PIN, admin)');",
        )
        .unwrap();
        drop(conn);
        let rows = |c: &Connection| -> Vec<(i64, String, i64, i64, Option<String>, i64)> {
            c.prepare("SELECT id, method, amount_cents, tip_cents, items, paid_at FROM payments ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let before = rows(&Connection::open(&src).unwrap());
        let (mut pos, s) = live();
        let prepared = pos.prepare_restore(&src, dir.path().join("work.db")).unwrap();
        assert_eq!(prepared.summary.schema_version, 30);
        pos.restore(&s, prepared).unwrap();
        let v: i32 = pos.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // The payments as they were, ids included; the indexes and foreign keys too.
        assert_eq!(rows(&pos.conn), before);
        let names = |sql: &str| -> Vec<String> {
            pos.conn.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
        };
        assert_eq!(names("SELECT name FROM sqlite_schema WHERE type = 'index' AND tbl_name = 'payments' ORDER BY name"), ["payments_order", "payments_paid_at"]);
        assert_eq!(names("SELECT \"table\" FROM pragma_foreign_key_list('payments') ORDER BY 1"), ["orders", "users"]);
        let sql: String = pos.conn.query_row("SELECT sql FROM sqlite_schema WHERE name = 'payments'", [], |r| r.get(0)).unwrap();
        assert!(sql.contains("AUTOINCREMENT"), "{sql}");
        // A new payment after them all, and after the one taken back.
        let admin = pos.login_as(None, "2468", &mut Default::default()).unwrap();
        let o = pos.open_order(&admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: OrderDetails::default() }).unwrap();
        let tea = pos.create_product(&admin, product("Tea")).unwrap();
        let p = pos.add_item(&admin, o.id, tea.id, 1, None, None, &[]).unwrap();
        let paid = pos
            .add_payment(&admin, p.id, &crate::PaymentInput { method: crate::PaymentMethod::Card, amount_cents: None, tendered_cents: 0, tip_cents: 0, items: None })
            .unwrap();
        assert_eq!(paid.payments[0].id, 10);
    }

    #[test]
    fn newer_damaged_and_foreign_files_are_refused_and_nothing_changes() {
        let dir = tmp();
        let work = dir.path().join("work.db");
        let (pos, _) = live();
        let err = |p: &Path| pos.prepare_restore(p, &work).err().map(|e| e.to_string()).unwrap();
        // From a newer app.
        let newer = dir.path().join("newer.db");
        other_pc(&newer);
        Connection::open(&newer).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();
        assert!(err(&newer).contains("newer version of the app"), "{}", err(&newer));
        // Not a database at all, or empty.
        let text = dir.path().join("notes.db");
        std::fs::write(&text, "hello, this is not a database at all").unwrap();
        assert!(err(&text).contains("not a POS backup"));
        std::fs::write(&text, "").unwrap();
        assert!(err(&text).contains("not a POS backup"));
        // Some other SQLite database.
        let other = dir.path().join("other.db");
        Connection::open(&other).unwrap().execute_batch("CREATE TABLE notes (t TEXT); PRAGMA user_version = 3;").unwrap();
        assert!(err(&other).contains("not a POS backup"));
        // A POS database cut short (e.g. a copy that did not finish).
        let full = dir.path().join("full.db");
        other_pc(&full);
        let bytes = std::fs::read(&full).unwrap();
        let cut = dir.path().join("cut.db");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        let e = err(&cut);
        assert!(e.contains("damaged") || e.contains("not a POS backup"), "{e}");
        // Garbage in the middle of the pages.
        let mut bad = bytes.clone();
        for b in &mut bad[4096..bytes.len() - 4096] {
            *b = 0x5a;
        }
        let bad_path = dir.path().join("bad.db");
        std::fs::write(&bad_path, &bad).unwrap();
        let e = err(&bad_path);
        assert!(e.contains("damaged") || e.contains("not a POS backup"), "{e}");
        // No admin: nobody could manage the till afterwards.
        let no_admin = dir.path().join("noadmin.db");
        other_pc(&no_admin);
        Connection::open(&no_admin).unwrap().execute("UPDATE users SET role = 'staff'", []).unwrap();
        assert!(err(&no_admin).contains("no admin"));
        // The live data is untouched and no work files are left.
        let s = pos.refresh_session(&Session { user_id: 1, name: String::new(), role: Role::Admin }).unwrap();
        assert_eq!(s.name, "Owner");
        assert_eq!(pos.menu(&s).unwrap()[0].name, "Pad thai");
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(!left.iter().any(|n| n.starts_with("work")), "{left:?}");
    }

    #[test]
    fn only_admins_restore_and_a_different_page_size_is_fine() {
        let dir = tmp();
        let src = dir.path().join("old.db");
        other_pc(&src);
        // Same data with 8 KB pages: the work copy is converted to the live page size.
        let big = dir.path().join("big.db");
        Connection::open(&src).unwrap().execute_batch(&format!("PRAGMA journal_mode = DELETE; VACUUM INTO '{}';", big.display())).unwrap();
        Connection::open(&big).unwrap().execute_batch("PRAGMA page_size = 8192; VACUUM;").unwrap();
        let ps: i64 = Connection::open(&big).unwrap().query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        assert_eq!(ps, 8192);
        let (mut pos, admin) = live();
        let sam = pos.create_user(&admin, "Sam", Role::Manager, "2222").unwrap();
        let sam = pos.refresh_session(&Session { user_id: sam.id, name: String::new(), role: Role::Staff }).unwrap();
        let prepared = pos.prepare_restore(&big, dir.path().join("work.db")).unwrap();
        assert!(matches!(pos.restore(&sam, prepared), Err(PosError::Forbidden)));
        assert_eq!(pos.menu(&admin).unwrap()[0].name, "Pad thai");
        let prepared = pos.prepare_restore(&big, dir.path().join("work.db")).unwrap();
        pos.restore(&admin, prepared).unwrap();
        assert_eq!(pos.login_as(None, "4321", &mut Default::default()).unwrap().name, "Nok");
    }

    /// The old PC's backup with `sql` run on it (as a tampered file would be).
    fn tampered(dir: &Path, name: &str, sql: &str) -> PathBuf {
        let path = dir.join(name);
        other_pc(&path);
        Connection::open(&path).unwrap().execute_batch(sql).unwrap();
        path
    }

    #[test]
    fn a_backup_with_its_own_triggers_views_or_tables_is_refused() {
        let dir = tmp();
        let (pos, s) = live();
        let work = dir.path().join("work.db");
        for (name, sql) in [
            ("trigger.db", "CREATE TRIGGER evil_t AFTER INSERT ON audit_log BEGIN DELETE FROM payments; END;"),
            ("abort.db", "CREATE TRIGGER no_login BEFORE INSERT ON audit_log BEGIN SELECT RAISE(ABORT, 'no'); END;"),
            ("view.db", "CREATE VIEW sneaky AS SELECT * FROM users;"),
            ("table.db", "CREATE TABLE extra (x TEXT);"),
            ("index.db", "CREATE UNIQUE INDEX one_order ON orders (order_type);"),
            ("column.db", "ALTER TABLE users ADD COLUMN note TEXT;"),
            ("missing.db", "DROP TABLE customers;"),
        ] {
            let path = tampered(dir.path(), name, sql);
            let err = pos.prepare_restore(&path, &work).err().map(|e| e.to_string());
            assert_eq!(err.as_deref(), Some(NOT_A_BACKUP), "{name}");
        }
        // A table with the right columns but its own CHECK or DEFAULT (a CHECK
        // on audit_log could stop anyone logging in after the restore).
        let fresh = Connection::open_in_memory().unwrap();
        migrate(&fresh).unwrap();
        let audit: String = fresh.query_row("SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'audit_log'", [], |r| r.get(0)).unwrap();
        let end = audit.rfind(')').unwrap();
        let checked = format!("{}, CHECK (action NOT IN ('login', 'logout')))", &audit[..end]);
        let rebuilt = |new_sql: &str| {
            format!("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON; ALTER TABLE audit_log RENAME TO audit_old; {new_sql}; INSERT INTO audit_log SELECT * FROM audit_old; DROP TABLE audit_old;")
        };
        for (name, sql) in [("check.db", rebuilt(&checked)), ("default.db", rebuilt(&audit.replacen("TEXT", "TEXT DEFAULT 'x'", 1)))] {
            assert_ne!(sql, rebuilt(&audit), "{name}: the test must change the table");
            let path = tampered(dir.path(), name, &sql);
            let err = pos.prepare_restore(&path, &work).err().map(|e| e.to_string());
            assert_eq!(err.as_deref(), Some(NOT_A_BACKUP), "{name}");
        }
        // Nothing changed here.
        assert_eq!(pos.menu(&s).unwrap()[0].name, "Pad thai");
    }

    #[test]
    fn a_trigger_with_an_expected_name_but_other_contents_is_replaced() {
        let dir = tmp();
        let src = tampered(
            dir.path(),
            "old.db",
            "DROP TRIGGER products_ai; CREATE TRIGGER products_ai AFTER INSERT ON products BEGIN DELETE FROM users; END;
             DROP INDEX orders_opened_at; CREATE UNIQUE INDEX orders_opened_at ON orders (order_type);",
        );
        let (mut pos, s) = live();
        let prepared = pos.prepare_restore(&src, dir.path().join("work.db")).unwrap();
        pos.restore(&s, prepared).unwrap();
        let fresh = Connection::open_in_memory().unwrap();
        migrate(&fresh).unwrap();
        let sql = |c: &Connection, n: &str| -> String { c.query_row("SELECT sql FROM sqlite_schema WHERE name = ?1", [n], |r| r.get(0)).unwrap() };
        for n in ["products_ai", "orders_opened_at"] {
            assert_eq!(sql(&pos.conn, n), sql(&fresh, n), "{n}");
        }
        let nok = pos.login_as(None, "4321", &mut Default::default()).unwrap();
        pos.create_product(&nok, product("Larb")).unwrap();
        assert_eq!(pos.login_as(None, "5555", &mut Default::default()).unwrap().name, "Ben");
    }

    #[test]
    fn errors_while_checking_are_plain_messages() {
        let dir = tmp();
        let (pos, _) = live();
        let work = dir.path().join("work.db");
        let salt = tampered(dir.path(), "salt.db", "UPDATE settings SET value = x'0102' WHERE key = '_pin_salt';");
        assert_eq!(pos.prepare_restore(&salt, &work).err().unwrap().to_string(), DAMAGED);
        let no_users = tampered(dir.path(), "users.db", "PRAGMA foreign_keys = OFF; DROP TABLE users;");
        assert_eq!(pos.prepare_restore(&no_users, &work).err().unwrap().to_string(), NOT_A_BACKUP);
    }

    #[test]
    fn a_failure_after_the_copy_still_counts_as_restored() {
        let dir = tmp();
        let src = dir.path().join("old.db");
        other_pc(&src);
        let (mut pos, s) = live();
        let id = pos.database_id().unwrap();
        assert_eq!(id.len(), 16);
        let prepared = pos.prepare_restore(&src, dir.path().join("work.db")).unwrap();
        FAIL_AFTER_COPY.with(|f| f.set(true));
        let r = pos.restore(&s, prepared);
        FAIL_AFTER_COPY.with(|f| f.set(false));
        assert_eq!(r.unwrap().orders, 1);
        // The data is the backup's (with a new id); only the audit entry is missing.
        assert_eq!(pos.login_as(None, "4321", &mut Default::default()).unwrap().name, "Nok");
        let n: i64 = pos.conn.query_row("SELECT COUNT(*) FROM audit_log WHERE action = 'backup.restore'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        let new_id = pos.database_id().unwrap();
        assert!(new_id.len() == 16 && new_id != id);
        // Restoring the same file again gives yet another id.
        let prepared = pos.prepare_restore(&src, dir.path().join("work.db")).unwrap();
        let nok = pos.login_as(None, "4321", &mut Default::default()).unwrap();
        pos.restore(&nok, prepared).unwrap();
        assert_ne!(pos.database_id().unwrap(), new_id);
    }
}

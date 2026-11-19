//! Every POS command, in one place, for both front ends: the desktop window
//! (Tauri IPC) and browsers on the local network (see `server.rs`).
//!
//! Each command takes its arguments as JSON (camelCase keys, as the UI sends
//! them) and runs as the user logged in on the calling terminal. The desktop
//! window is the terminal [`LOCAL`]; each browser is its own terminal with
//! its own PIN login, identified by a random id it generates.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pos_core::*;
use serde::Serialize;
use serde_json::Value;

/// The terminal id of the desktop window.
pub const LOCAL: &str = "local";
/// Browsers left idle this long are logged out.
const IDLE_LOGOUT: Duration = Duration::from_secs(12 * 60 * 60);
/// Logged-in browsers kept at once (the longest idle is logged out first).
const MAX_TERMINALS: usize = 64;
/// Menu-only backups kept (each is a small JSON file).
const MENU_BACKUPS_KEPT: usize = 30;
/// Largest full backup that can be loaded from a file.
const MAX_UPLOAD: u64 = 2 << 30;
/// Largest piece of an upload per call (base64 of it stays well under the
/// local server's 1 MB request limit).
const UPLOAD_CHUNK: usize = 512 * 1024;
/// What the admin types to confirm a restore.
const RESTORE_WORD: &str = "RESTORE";

struct Login {
    session: Session,
    last_used: Instant,
}

/// One database connection, shared by every terminal. Sessions live here,
/// never in the UI, so they cannot be forged.
pub struct Inner {
    pub pos: Pos,
    logins: HashMap<String, Login>,
    /// Wrong-PIN lockout shared by every browser: rotating terminal ids does
    /// not get more guesses, and it never locks out the main till.
    network_throttle: LoginThrottle,
    local_throttle: LoginThrottle,
}

pub struct App {
    inner: Mutex<Inner>,
    pub backup_dir: PathBuf,
}

impl App {
    pub fn new(pos: Pos, backup_dir: PathBuf) -> App {
        App { inner: Mutex::new(Inner { pos, logins: HashMap::new(), network_throttle: LoginThrottle::default(), local_throttle: LoginThrottle::default() }), backup_dir }
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` as the user logged in on terminal `t`, re-validated against
    /// the database so a deactivated or demoted user loses access at once.
    pub fn with_session<T>(&self, t: &str, f: impl FnOnce(&mut Pos, &Session) -> Result<T>) -> Result<T> {
        self.with_session_inner(t, |inner, s| f(&mut inner.pos, s))
    }

    /// [`App::with_session`] with all of [`Inner`], e.g. to log every
    /// terminal out in the same lock as the change that requires it.
    fn with_session_inner<T>(&self, t: &str, f: impl FnOnce(&mut Inner, &Session) -> Result<T>) -> Result<T> {
        let mut g = self.lock();
        let inner = &mut *g;
        let current = match inner.logins.get(t) {
            Some(l) if t == LOCAL || l.last_used.elapsed() < IDLE_LOGOUT => l.session.clone(),
            Some(_) => {
                inner.logins.remove(t);
                return Err(PosError::NotLoggedIn);
            }
            None => return Err(PosError::NotLoggedIn),
        };
        match inner.pos.refresh_session(&current) {
            Ok(s) => {
                inner.logins.insert(t.to_string(), Login { session: s.clone(), last_used: Instant::now() });
                f(inner, &s)
            }
            Err(_) => {
                inner.logins.remove(t);
                Err(PosError::NotLoggedIn)
            }
        }
    }
}

impl Inner {
    /// Everyone on every terminal (till, browsers, kitchen screens) must log in again.
    fn log_everyone_out(&mut self) {
        self.logins.clear();
        self.network_throttle = LoginThrottle::default();
        self.local_throttle = LoginThrottle::default();
    }

    fn sign_in(&mut self, t: &str, s: &Session) {
        if self.logins.len() >= MAX_TERMINALS && !self.logins.contains_key(t) {
            let oldest = self
                .logins
                .iter()
                .filter(|(k, _)| k.as_str() != LOCAL)
                .min_by_key(|(_, l)| l.last_used)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.logins.remove(&k);
            }
        }
        self.logins.insert(t.to_string(), Login { session: s.clone(), last_used: Instant::now() });
    }
}

#[derive(Serialize)]
pub struct AppStatus {
    version: &'static str,
    needs_setup: bool,
    session: Option<Session>,
    store_name: Option<String>,
    plan_width: i64,
    plan_height: i64,
    /// True in a browser (no updates, server controls or local backups UI).
    remote: bool,
    /// Log in as soon as the picked user's PIN is complete (setting on).
    auto_pin_on: bool,
    /// Log in as soon as this many digits are typed (no OK needed).
    auto_pin: Option<usize>,
    /// Changes when a backup is restored: the UI reloads to drop what it
    /// remembers from the old data.
    db_id: String,
}

/// Result of a print attempt. Printing problems never undo the action
/// (items stay "sent"); the UI shows the error and offers a reprint.
#[derive(Serialize, Default)]
pub struct PrintOutcome {
    /// False when no printer is configured.
    attempted: bool,
    error: Option<String>,
}

/// Send a print job after the database lock has been released, so a slow or
/// offline printer never blocks other commands. Extra copies follow in the
/// background, spaced by the configured delay.
fn print(job: Option<PrintJob>) -> PrintOutcome {
    match job {
        None => PrintOutcome { attempted: false, error: None },
        Some(job) => PrintOutcome { attempted: true, error: job.send_in_background().err().map(|e| e.to_string()) },
    }
}

#[derive(Serialize)]
pub struct SendResult {
    items: Vec<OrderItem>,
    print: PrintOutcome,
    /// Prepare-ahead dishes put on a PREPARE NOW ticket this time.
    prep: Vec<OrderItem>,
    prep_print: PrintOutcome,
}

#[derive(Serialize)]
pub struct PayResult {
    order: Order,
    /// Receipt and/or cash drawer, per Settings > Receipt printer.
    print: PrintOutcome,
}

#[derive(Serialize)]
pub struct DayCloseView {
    close: DayClose,
    /// The report laid out like the printout.
    text: String,
}

/// A shift with its report as text (as printed), and the print result.
#[derive(Serialize)]
pub struct ShiftResult {
    shift: Shift,
    text: String,
    print: PrintOutcome,
}

#[derive(Serialize)]
pub struct CloseResult {
    view: DayCloseView,
    print: PrintOutcome,
    /// Nothing happened that day, so nothing was printed.
    empty: bool,
}

#[derive(Serialize)]
pub struct MenuBackupFile {
    name: String,
    /// Unix seconds.
    saved_at: u64,
    items: usize,
}

#[derive(Serialize)]
pub struct MenuBackupSaved {
    file: MenuBackupFile,
    path: String,
    /// The file's contents, so a browser can also download it.
    menu: MenuBackup,
}

#[derive(Serialize)]
pub struct ResetResult {
    report: ResetReport,
    /// Full backup taken just before the reset.
    backup: String,
}

/// Result of deleting several things at once.
#[derive(Serialize)]
pub struct BulkResult {
    deleted: usize,
    /// Menu items still named on past orders: hidden instead of deleted.
    hidden: usize,
    /// One message per thing that could not be deleted.
    errors: Vec<String>,
}

#[derive(Serialize)]
pub struct RestoreResult {
    /// What is in the till now.
    summary: BackupSummary,
    /// The backup that was restored.
    restored: String,
    /// Safety copy of the data from before the restore, in the backup folder.
    safety_copy: String,
}

/// A file loaded with "Restore from a file…", checked and saved in the backup folder.
#[derive(Serialize)]
pub struct UploadedBackup {
    file: BackupFile,
    summary: BackupSummary,
}

/// A full backup file in the backup folder.
#[derive(Serialize)]
pub struct BackupFile {
    name: String,
    size: u64,
    /// Unix seconds.
    modified: i64,
}

/// "pos-....db" in the backup folder only: never a path. Daily and manual
/// backups ("pos-<time>.db"), the safety copies made before an update, a
/// reset or a restore, and files loaded from elsewhere ("pos-imported-…").
fn is_full_backup_name(name: &str) -> bool {
    name.starts_with("pos-") && name.ends_with(".db") && !name.contains(['/', '\\']) && !name.contains("..")
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A fresh scratch file for checking a backup (see `Pos::prepare_restore`);
/// never the same twice, so two checks at once cannot share one.
fn work_path(dir: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!("restore-work-{}-{}-{n}.tmp", unix_now(), std::process::id()))
}

/// An upload in progress: "upload-<id>.part" (the id is the browser's own random hex).
fn upload_path(dir: &Path, upload: &str) -> Result<PathBuf> {
    if !(16..=64).contains(&upload.len()) || !upload.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PosError::Validation("bad upload id".into()));
    }
    Ok(dir.join(format!("upload-{upload}.part")))
}

/// Uploads nobody finished (closed page, lost network) and scratch files
/// left by a crash: untouched for an hour (a live upload writes every few
/// seconds, so another admin's upload in progress is left alone).
fn remove_stale_uploads(dir: &Path) {
    let day = Duration::from_secs(60 * 60);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
        if old && ((name.starts_with("upload-") && name.ends_with(".part")) || (name.starts_with("restore-work-") && name.ends_with(".tmp"))) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A backup file name "<prefix><time>.db" not used yet (two in the same
/// second never overwrite each other). Returns (path, name).
pub fn unique_backup_path(dir: &Path, prefix: &str) -> (PathBuf, String) {
    let mut ts = unix_now();
    loop {
        let name = format!("{prefix}{ts}.db");
        let path = dir.join(&name);
        if !path.exists() {
            return (path, name);
        }
        ts += 1;
    }
}

/// "<kind><digits>.db", e.g. kind "pos-" for the daily and manual backups.
fn is_kind(name: &str, kind: &str) -> bool {
    name.strip_prefix(kind)
        .and_then(|r| r.strip_suffix(".db"))
        .is_some_and(|ts| !ts.is_empty() && ts.bytes().all(|b| b.is_ascii_digit()))
}

/// Disk full as the plain message, whatever reported it.
fn disk_full(e: PosError) -> PosError {
    if is_disk_full(&e) { PosError::Validation(DISK_FULL.into()) } else { e }
}

/// Who may load and restore full backups: admins (settings and backups).
fn require_restore(s: &Session) -> Result<()> {
    s.require(Permission::ManageSettings)?;
    s.require(Permission::Backup)
}

fn backup_file(path: &Path) -> Option<BackupFile> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(BackupFile { name, size: meta.len(), modified })
}

/// A full backup's path, for downloading it in a browser (admins only).
pub fn backup_download(app: &App, t: &str, name: &str) -> Result<PathBuf> {
    app.with_session(t, |_, s| require_restore(s))?;
    if !is_full_backup_name(name) {
        return Err(PosError::Validation("that is not a backup".into()));
    }
    let path = app.backup_dir.join(name);
    if !path.is_file() {
        return Err(PosError::NotFound);
    }
    Ok(path)
}

/// Menu backups in `dir`, newest first.
fn menu_backups(dir: &Path) -> Vec<(MenuBackupFile, PathBuf)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !is_menu_backup_name(&name) {
                return None;
            }
            let saved_at = name[5..name.len() - 5].parse().ok()?;
            let items = std::fs::read(e.path())
                .ok()
                .and_then(|b| serde_json::from_slice::<MenuBackup>(&b).ok())
                .map_or(0, |m| m.items.len());
            Some((MenuBackupFile { name, saved_at, items }, e.path()))
        })
        .collect();
    v.sort_by_key(|(f, _)| std::cmp::Reverse(f.saved_at));
    v
}

/// "menu-<unix seconds>.json" only: never a path.
fn is_menu_backup_name(name: &str) -> bool {
    name.strip_prefix("menu-")
        .and_then(|r| r.strip_suffix(".json"))
        .is_some_and(|ts| !ts.is_empty() && ts.bytes().all(|b| b.is_ascii_digit()))
}

pub fn backup_path(dir: &Path) -> PathBuf {
    unique_backup_path(dir, "pos-").0
}

/// The newest daily or manual backup ("pos-<time>.db"), for the automatic
/// daily backup: safety copies and loaded files do not count.
pub fn newest_regular_backup(dir: &Path) -> Option<SystemTime> {
    list_backups(dir)
        .into_iter()
        .find(|(_, p)| p.file_name().is_some_and(|n| is_kind(&n.to_string_lossy(), "pos-")))
        .map(|(t, _)| t)
}

pub fn list_backups(dir: &Path) -> Vec<(SystemTime, PathBuf)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("pos-"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.0));
    v
}

/// Safety copies made before an update, a reset or a restore, and files
/// loaded from elsewhere: the newest few of each kind always stay,
/// whatever `backups_kept` says, and so does any younger than
/// [`SAFETY_COPIES_MIN_AGE`].
pub const SAFETY_COPIES_KEPT: usize = 3;
pub const SAFETY_COPIES_MIN_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Keep the newest `keep` regular backups ("pos-<time>.db", Settings →
/// Backup & data, `backups_kept`) and the newest [`SAFETY_COPIES_KEPT`]
/// before-update, before-reset, before-restore and imported copies (plus
/// safety copies under a week old). Nothing else is ever deleted.
pub fn prune_backups(dir: &Path, keep: usize) {
    let all = list_backups(dir);
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let of_kind = |kind: &'static str| all.iter().filter(move |(_, p)| is_kind(&name(p), kind));
    let old_enough = |(t, _): &&(SystemTime, PathBuf)| t.elapsed().is_ok_and(|age| age > SAFETY_COPIES_MIN_AGE);
    let safety = ["pos-before-update-", "pos-before-reset-", "pos-before-restore-"]
        .into_iter()
        .flat_map(|kind| of_kind(kind).skip(SAFETY_COPIES_KEPT).filter(old_enough));
    // Files loaded from elsewhere are copies of data kept somewhere else:
    // just the newest few, so repeated uploads cannot fill the disk.
    let imported = of_kind("pos-imported-").skip(SAFETY_COPIES_KEPT);
    for (_, old) in of_kind("pos-").skip(keep.max(1)).chain(safety).chain(imported) {
        let _ = std::fs::remove_file(old);
    }
}

/// Declares the commands and generates [`dispatch`]. Each command lists its
/// arguments (read from a JSON object with camelCase keys; `Option` ones may
/// be left out) and a body returning `Result<T>`; `app` is the [`App`] and
/// `t` the calling terminal's id.
macro_rules! commands {
    ($( $(#[doc = $doc:literal])* fn $name:ident($app:ident, $t:ident $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty $body:block )*) => {
        /// Run command `cmd` with JSON `args` for terminal `t`.
        pub fn dispatch(app: &App, t: &str, cmd: &str, args: Value) -> Result<Value> {
            let args = if args.is_null() { Value::Object(Default::default()) } else { args };
            match cmd {
                $( stringify!($name) => {
                    #[derive(serde::Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct Args { $( $arg: $ty, )* }
                    let Args { $( $arg, )* } = serde_json::from_value(args).map_err(|e| {
                        PosError::Validation(format!("{}: {e}", stringify!($name)))
                    })?;
                    #[allow(unused_variables)]
                    let ($app, $t) = (app, t);
                    let out: Result<$ret> = $body;
                    serde_json::to_value(out?).map_err(|e| PosError::Internal(e.to_string()))
                } )*
                _ => Err(PosError::Validation(format!("unknown command {cmd}"))),
            }
        }
    };
}

commands! {
    fn app_status(app, t) -> AppStatus {
        let g = app.lock();
        let session = g.logins.get(t).map(|l| l.session.clone());
        Ok(AppStatus {
            version: env!("CARGO_PKG_VERSION"),
            needs_setup: g.pos.needs_setup()?,
            session,
            store_name: g.pos.setting("store_name")?,
            plan_width: PLAN_WIDTH,
            plan_height: PLAN_HEIGHT,
            remote: t != LOCAL,
            auto_pin: g.pos.auto_login_length()?,
            auto_pin_on: g.pos.pin_auto_login()?,
            db_id: g.pos.database_id()?,
        })
    }

    /// Who can log in, for the login screen (pick a name, then the PIN).
    fn login_users(app, t) -> Vec<LoginUser> {
        let _ = t;
        app.lock().pos.login_users()
    }

    fn setup_admin(app, t, name: String, pin: String) -> Session {
        let mut g = app.lock();
        let s = g.pos.setup_first_admin(&name, &pin)?;
        g.sign_in(t, &s);
        Ok(s)
    }

    /// `user_id`: the name picked on the login screen (PINs may be shared).
    fn login(app, t, pin: String, user_id: Option<i64>) -> Session {
        let mut g = app.lock();
        let inner = &mut *g;
        let s = if t == LOCAL {
            let mut throttle = std::mem::take(&mut inner.local_throttle);
            let r = inner.pos.login_as(user_id, &pin, &mut throttle);
            inner.local_throttle = throttle;
            r?
        } else {
            inner.pos.login_as(user_id, &pin, &mut inner.network_throttle)?
        };
        g.sign_in(t, &s);
        Ok(s)
    }

    fn logout(app, t) -> () {
        let mut g = app.lock();
        if let Some(l) = g.logins.remove(t) {
            g.pos.logout(&l.session)?;
        }
        Ok(())
    }

    /// `all_courses`: also send mains held behind the starters ("Call mains",
    /// or "send all courses together").
    fn send_to_kitchen(app, t, order_id: i64, all_courses: Option<bool>) -> SendResult {
        let (items, job, prep, prep_job) = app.with_session(t, |pos, s| {
            let items =
                if all_courses.unwrap_or(false) { pos.call_mains(s, order_id)? } else { pos.send_to_kitchen(s, order_id)? };
            let job = pos.kitchen_job(s, order_id, &items, false)?;
            let prep = pos.send_prep(s, order_id)?;
            let prep_job = pos.prep_job(s, order_id, &prep)?;
            Ok((items, job, prep, prep_job))
        })?;
        Ok(SendResult { items, print: print(job), prep, prep_print: print(prep_job) })
    }

    fn search_products(app, t, query: String, limit: Option<i64>) -> Vec<Product> {
        app.with_session(t, |pos, s| pos.search_products(s, &query, limit.unwrap_or(50)))
    }

    fn find_by_code(app, t, code: String) -> Option<Product> {
        app.with_session(t, |pos, s| pos.find_by_code(s, &code))
    }

    fn list_products(app, t, include_inactive: bool, limit: i64, offset: i64) -> Vec<Product> {
        app.with_session(t, |pos, s| pos.list_products(s, include_inactive, limit, offset))
    }

    fn create_product(app, t, product: NewProduct) -> Product {
        app.with_session(t, |pos, s| pos.create_product(s, product))
    }

    fn update_product(app, t, id: i64, product: ProductUpdate) -> Product {
        app.with_session(t, |pos, s| pos.update_product(s, id, product))
    }

    fn set_starter(app, t, id: i64, starter: bool) -> () {
        app.with_session(t, |pos, s| pos.set_starter(s, id, starter))
    }

    fn set_prep(app, t, id: i64, prep: bool) -> () {
        app.with_session(t, |pos, s| pos.set_prep(s, id, prep))
    }

    fn adjust_stock(app, t, id: i64, delta: i64, reason: String) -> Product {
        app.with_session(t, |pos, s| pos.adjust_stock(s, id, delta, &reason))
    }

    fn menu(app, t) -> Vec<Product> {
        app.with_session(t, |pos, s| pos.menu(s))
    }

    fn list_categories(app, t, include_inactive: bool) -> Vec<Category> {
        app.with_session(t, |pos, s| pos.list_categories(s, include_inactive))
    }

    fn create_category(app, t, name: String, color: Option<String>, sort_order: i64) -> Category {
        app.with_session(t, |pos, s| pos.create_category(s, &name, color.as_deref(), sort_order))
    }

    fn update_category(app, t, id: i64, name: String, color: Option<String>, sort_order: i64, active: bool) -> Category {
        app.with_session(t, |pos, s| pos.update_category(s, id, &name, color.as_deref(), sort_order, active))
    }

    fn list_channels(app, t) -> Vec<Channel> {
        app.with_session(t, |pos, s| pos.list_channels(s))
    }

    fn save_channels(app, t, channels: Vec<ChannelInput>) -> Vec<Channel> {
        app.with_session(t, |pos, s| pos.save_channels(s, &channels))
    }

    fn search_customers(app, t, query: String, limit: Option<i64>) -> Vec<Customer> {
        app.with_session(t, |pos, s| pos.search_customers(s, &query, limit.unwrap_or(20)))
    }

    fn customers_at_postcode(app, t, postcode: String) -> Vec<Customer> {
        app.with_session(t, |pos, s| pos.customers_at_postcode(s, &postcode))
    }

    fn create_customer(app, t, customer: CustomerInput) -> Customer {
        app.with_session(t, |pos, s| pos.create_customer(s, &customer))
    }

    fn update_customer(app, t, id: i64, customer: CustomerInput) -> Customer {
        app.with_session(t, |pos, s| pos.update_customer(s, id, &customer))
    }

    fn delete_customer(app, t, id: i64) -> () {
        app.with_session(t, |pos, s| pos.delete_customer(s, id))
    }

    fn customer_orders(app, t, id: i64) -> Vec<OrderSummary> {
        app.with_session(t, |pos, s| pos.customer_orders(s, id, 10))
    }

    fn list_variants(app, t, product_id: Option<i64>) -> Vec<Variant> {
        app.with_session(t, |pos, s| pos.list_variants(s, product_id))
    }

    fn set_variants(app, t, product_id: i64, variants: Vec<VariantInput>) -> Vec<Variant> {
        app.with_session(t, |pos, s| pos.set_variants(s, product_id, &variants))
    }

    fn menu_options(app, t) -> MenuOptions {
        app.with_session(t, |pos, s| pos.menu_options(s))
    }

    fn list_option_groups(app, t, include_inactive: bool) -> Vec<OptionGroup> {
        app.with_session(t, |pos, s| pos.list_option_groups(s, include_inactive))
    }

    fn create_option_group(app, t, name: String, min_select: i64, max_select: i64) -> OptionGroup {
        app.with_session(t, |pos, s| pos.create_option_group(s, &name, min_select, max_select))
    }

    fn delete_option_group(app, t, id: i64) -> () {
        app.with_session(t, |pos, s| pos.delete_option_group(s, id))
    }

    fn update_option_group(app, t, id: i64, name: String, min_select: i64, max_select: i64, sort_order: i64, active: bool) -> OptionGroup {
        app.with_session(t, |pos, s| pos.update_option_group(s, id, &name, min_select, max_select, sort_order, active))
    }

    fn save_group_options(app, t, group_id: i64, options: Vec<OptionInput>) -> OptionGroup {
        app.with_session(t, |pos, s| pos.save_group_options(s, group_id, &options))
    }

    fn set_product_option_groups(app, t, product_id: i64, group_ids: Vec<i64>) -> () {
        app.with_session(t, |pos, s| pos.set_product_option_groups(s, product_id, &group_ids))
    }

    fn list_floors(app, t) -> Vec<Floor> {
        app.with_session(t, |pos, s| pos.list_floors(s))
    }

    fn create_floor(app, t, name: String) -> Floor {
        app.with_session(t, |pos, s| pos.create_floor(s, &name))
    }

    fn update_floor(app, t, id: i64, name: String, sort_order: i64) -> Floor {
        app.with_session(t, |pos, s| pos.update_floor(s, id, &name, sort_order))
    }

    fn delete_floor(app, t, id: i64) -> () {
        app.with_session(t, |pos, s| pos.delete_floor(s, id))
    }

    fn list_tables(app, t, floor_id: i64) -> Vec<DiningTable> {
        app.with_session(t, |pos, s| pos.list_tables(s, floor_id))
    }

    fn floor_status(app, t, floor_id: i64) -> Vec<TableStatus> {
        app.with_session(t, |pos, s| pos.floor_status(s, floor_id))
    }

    fn save_floor_layout(app, t, floor_id: i64, tables: Vec<TableLayout>) -> Vec<DiningTable> {
        app.with_session(t, |pos, s| pos.save_floor_layout(s, floor_id, &tables))
    }

    fn open_order(app, t, order: NewOrder) -> Order {
        app.with_session(t, |pos, s| pos.open_order(s, &order))
    }

    fn table_order(app, t, table_id: i64) -> Option<Order> {
        app.with_session(t, |pos, s| pos.table_order(s, table_id))
    }

    fn get_order(app, t, id: i64) -> Order {
        app.with_session(t, |pos, s| pos.get_order(s, id))
    }

    fn update_order_details(app, t, id: i64, details: OrderDetails) -> Order {
        app.with_session(t, |pos, s| pos.update_order_details(s, id, &details))
    }

    fn move_order(app, t, id: i64, table_id: i64) -> Order {
        app.with_session(t, |pos, s| pos.move_order(s, id, table_id))
    }

    /// `starter`: order a starter as a main (false) or any dish as a starter
    /// (true); left out = the menu's setting.
    fn add_item(app, t, order_id: i64, product_id: i64, qty: i64, note: Option<String>, variant_id: Option<i64>, option_ids: Option<Vec<i64>>, starter: Option<bool>) -> Order {
        let option_ids = option_ids.unwrap_or_default();
        app.with_session(t, |pos, s| pos.add_item_as(s, order_id, product_id, qty, note.as_deref(), variant_id, &option_ids, starter))
    }

    fn set_item_course(app, t, item_id: i64, starter: bool) -> Order {
        app.with_session(t, |pos, s| pos.set_item_course(s, item_id, starter))
    }

    /// A dish not on the menu: name and price typed on the order.
    fn add_open_item(app, t, order_id: i64, name: String, price_cents: i64, qty: Option<i64>, note: Option<String>, starter: Option<bool>) -> Order {
        app.with_session(t, |pos, s| pos.add_open_item(s, order_id, &name, price_cents, qty.unwrap_or(1), note.as_deref(), starter.unwrap_or(false)))
    }

    /// A change to a dish already sent tells the kitchen: fewer or removed
    /// prints CANCELLED (with the set's dishes for a set), a new note CHANGED.
    /// `pin`: a manager's or admin's PIN approving a discount over the staff
    /// limit (it only approves; nobody is logged in or out).
    fn update_item(app, t, item_id: i64, update: ItemUpdate, pin: Option<String>, order_id: Option<i64>) -> Order {
        let (order, job) = app.with_session(t, |pos, s| {
            // The till says which order it shows: the dish must still be on it.
            if let Some(o) = order_id {
                pos.check_item_on_order(o, item_id)?;
            }
            let approver = pin.as_deref().map(|p| pos.pin_owner_for(p, Some(s.user_id))).transpose()?;
            pos.update_item_for_kitchen_approved(s, approver.as_ref(), item_id, &update)
        })?;
        // The change is saved either way; a printer problem must not undo it.
        let _ = print(job);
        Ok(order)
    }

    fn reprint_kitchen(app, t, order_id: i64) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.kitchen_reprint_job(s, order_id))?;
        Ok(print(job))
    }

    /// Print chosen dishes (already sent) again, e.g. after fixing one.
    /// A set prints with its dishes.
    fn reprint_kitchen_items(app, t, order_id: i64, item_ids: Vec<i64>) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| {
            let o = pos.get_order(s, order_id)?;
            let items: Vec<OrderItem> = o
                .items
                .into_iter()
                .filter(|i| i.sent_at.is_some() && (item_ids.contains(&i.id) || i.parent_id.is_some_and(|p| item_ids.contains(&p))))
                .collect();
            if items.is_empty() {
                return Err(PosError::Validation("nothing to print again".into()));
            }
            pos.kitchen_job(s, order_id, &items, true)
        })?;
        Ok(print(job))
    }

    /// "Call mains" again when the mains are already in the kitchen.
    fn call_main_course(app, t, order_id: i64) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.main_course_job(s, order_id))?;
        Ok(print(job))
    }

    /// Teams: the list (menu editor and Settings) and saving it.
    fn list_teams(app, t) -> Vec<Team> {
        app.with_session(t, |pos, s| pos.list_teams(s))
    }

    fn save_teams(app, t, teams: Vec<TeamInput>) -> Vec<Team> {
        app.with_session(t, |pos, s| pos.save_teams(s, &teams))
    }

    /// Set menus: the sections, saving them, items only sold in sets, and
    /// adding a set (with the chosen dishes) to an order.
    fn get_set(app, t, set_id: i64) -> Vec<SetSection> {
        app.with_session(t, |pos, s| pos.get_set(s, set_id))
    }

    fn save_set(app, t, set_id: i64, sections: Vec<SetSectionInput>) -> Vec<SetSection> {
        app.with_session(t, |pos, s| pos.save_set(s, set_id, &sections))
    }

    fn set_set_only(app, t, id: i64, set_only: bool) -> () {
        app.with_session(t, |pos, s| pos.set_set_only(s, id, set_only))
    }

    fn add_set(app, t, order_id: i64, set_id: i64, picks: Vec<SetPick>, note: Option<String>) -> Order {
        app.with_session(t, |pos, s| pos.add_set(s, order_id, set_id, &picks, note.as_deref()))
    }

    fn set_dessert(app, t, id: i64, dessert: bool) -> () {
        app.with_session(t, |pos, s| pos.set_dessert(s, id, dessert))
    }

    fn set_category_dessert(app, t, id: i64, dessert: bool, move_items: bool) -> usize {
        app.with_session(t, |pos, s| pos.set_category_dessert(s, id, dessert, move_items))
    }

    fn set_drink(app, t, id: i64, drink: bool) -> () {
        app.with_session(t, |pos, s| pos.set_drink(s, id, drink))
    }

    fn set_category_drink(app, t, id: i64, drink: bool, move_items: bool) -> usize {
        app.with_session(t, |pos, s| pos.set_category_drink(s, id, drink, move_items))
    }

    fn set_category_team(app, t, id: i64, team_id: Option<i64>, move_items: bool) -> usize {
        app.with_session(t, |pos, s| pos.set_category_team(s, id, team_id, move_items))
    }

    /// A sample ticket for every team on the kitchen printer.
    fn test_teams_print(app, t) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.test_teams_job(s))?;
        Ok(print(job))
    }

    fn print_receipt(app, t, order_id: i64) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.receipt_job(s, order_id))?;
        Ok(print(Some(job)))
    }

    fn print_staff_copy(app, t, order_id: i64) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.staff_copy_job(s, order_id))?;
        Ok(print(Some(job)))
    }

    fn set_sold_out(app, t, id: i64, sold_out: bool) -> () {
        app.with_session(t, |pos, s| pos.set_sold_out(s, id, sold_out))
    }

    /// Move chosen dishes to another table (a new order there if it is free).
    fn move_items(app, t, order_id: i64, table_id: i64, items: Vec<(i64, i64)>) -> Order {
        app.with_session(t, |pos, s| pos.move_items(s, order_id, table_id, &items))
    }

    /// Join this order onto another table's bill (or just move there).
    fn merge_into_table(app, t, order_id: i64, table_id: i64) -> Order {
        app.with_session(t, |pos, s| pos.merge_into_table(s, order_id, table_id))
    }

    /// Kitchen screen: what is still to do, and marking dishes done.
    fn kitchen_board(app, t, filter: String) -> Vec<BoardOrder> {
        app.with_session(t, |pos, s| pos.kitchen_board(s, &filter))
    }

    fn kitchen_done(app, t, item_ids: Vec<i64>, done: bool) -> () {
        app.with_session(t, |pos, s| pos.kitchen_done(s, &item_ids, done))
    }

    /// Shifts and cash-up.
    fn current_shift(app, t) -> Option<Shift> {
        app.with_session(t, |pos, s| pos.current_shift(s))
    }

    fn open_shift(app, t, float_cents: i64) -> Shift {
        app.with_session(t, |pos, s| pos.open_shift(s, float_cents))
    }

    /// Money in or out: the drawer opens.
    fn cash_move(app, t, kind: String, amount_cents: i64, reason: String) -> ShiftResult {
        let (shift, text, job) = app.with_session(t, |pos, s| {
            let shift = pos.cash_move(s, &kind, amount_cents, &reason)?;
            let text = pos.shift_text(&shift)?;
            let job = pos.drawer_job(s).ok();
            Ok((shift, text, job))
        })?;
        Ok(ShiftResult { shift, text, print: print(job) })
    }

    /// Close the shift with the cash counted; the report prints.
    fn close_shift(app, t, counted_cents: i64, note: String) -> ShiftResult {
        let (shift, text, job) = app.with_session(t, |pos, s| {
            let shift = pos.close_shift(s, counted_cents, &note)?;
            let text = pos.shift_text(&shift)?;
            let job = pos.shift_job(s, &shift)?;
            Ok((shift, text, job))
        })?;
        Ok(ShiftResult { shift, text, print: print(job) })
    }

    fn list_shifts(app, t, limit: i64) -> Vec<ShiftResult> {
        app.with_session(t, |pos, s| {
            pos.list_shifts(s, limit)?
                .into_iter()
                .map(|shift| Ok(ShiftResult { text: pos.shift_text(&shift)?, shift, print: PrintOutcome::default() }))
                .collect()
        })
    }

    fn print_shift(app, t, id: i64) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| {
            let shift = pos.list_shifts(s, 500)?.into_iter().find(|x| x.id == id).ok_or(PosError::NotFound)?;
            pos.shift_job(s, &shift)
        })?;
        Ok(print(job))
    }

    fn open_drawer(app, t) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.drawer_job(s))?;
        Ok(print(Some(job)))
    }

    fn get_receipt_settings(app, t) -> ReceiptSettings {
        app.with_session(t, |pos, _| pos.receipt_settings())
    }

    fn set_receipt_settings(app, t, settings: ReceiptSettings) -> () {
        app.with_session(t, |pos, s| pos.set_receipt_settings(s, &settings))
    }

    fn test_receipt_printer(app, t, settings: ReceiptSettings, drawer_only: bool) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.test_receipt_job(s, &settings, drawer_only))?;
        Ok(print(Some(job)))
    }

    fn get_kitchen_printer(app, t) -> PrinterConfig {
        app.with_session(t, |pos, _| pos.kitchen_printer())
    }

    fn set_kitchen_printer(app, t, config: PrinterConfig) -> () {
        app.with_session(t, |pos, s| pos.set_kitchen_printer(s, &config))
    }

    fn test_kitchen_printer(app, t, config: PrinterConfig) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.test_print_job(s, &config))?;
        Ok(print(Some(job)))
    }

    /// Extra kitchen printers (bar, desserts...) and what each prints.
    fn get_kitchen_routes(app, t) -> Vec<KitchenRoute> {
        app.with_session(t, |pos, _| pos.kitchen_routes())
    }

    fn set_kitchen_routes(app, t, routes: Vec<KitchenRoute>) -> () {
        app.with_session(t, |pos, s| pos.set_kitchen_routes(s, &routes))
    }

    fn test_route_printer(app, t, route: KitchenRoute) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| {
            let cfg = pos.route_config(&route)?;
            pos.test_print_job(s, &cfg)
        })?;
        Ok(print(Some(job)))
    }

    fn list_system_printers(app, t) -> Vec<String> {
        app.with_session(t, |_, s| s.require(Permission::ManageSettings))?;
        Ok(system_printers())
    }

    fn pay_order(app, t, order_id: i64, method: PaymentMethod, tendered_cents: i64) -> PayResult {
        let (order, job) = app.with_session(t, |pos, s| {
            let order = pos.pay_order(s, order_id, method, tendered_cents)?;
            let job = pos.payment_print_job(s, &order)?;
            Ok((order, job))
        })?;
        Ok(PayResult { order, print: print(job) })
    }

    /// One payment: the rest, an amount, or chosen dishes, with a tip.
    /// The receipt prints when the last part is paid; cash opens the drawer.
    fn add_payment(app, t, order_id: i64, payment: PaymentInput) -> PayResult {
        let (order, job) = app.with_session(t, |pos, s| {
            let order = pos.add_payment(s, order_id, &payment)?;
            let job = pos.payment_print_job(s, &order)?;
            Ok((order, job))
        })?;
        Ok(PayResult { order, print: print(job) })
    }

    fn remove_payment(app, t, order_id: i64, payment_id: i64) -> Order {
        app.with_session(t, |pos, s| pos.remove_payment(s, order_id, payment_id))
    }

    /// Cancel an open order. Admins and managers confirm with no PIN
    /// (`pin` left out or empty); anyone else needs a PIN, whose owner
    /// cancels it (an order already in the kitchen needs a manager's or
    /// admin's PIN). The user logged in here counts as the PIN's owner when
    /// it is theirs, even if they share it. Dishes still cooking get a
    /// CANCELLED ticket. See `Pos::cancel_order_confirmed`.
    fn cancel_order(app, t, order_id: i64, reason: String, pin: Option<String>) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.cancel_order_confirmed(s, order_id, &reason, pin.as_deref()))?;
        Ok(print(job))
    }

    /// Leaving an order with nothing on it: cancel it if it is still empty.
    fn cancel_empty_order(app, t, order_id: i64) -> bool {
        app.with_session(t, |pos, s| pos.cancel_if_empty(s, order_id))
    }

    fn void_order(app, t, order_id: i64, reason: String) -> Order {
        app.with_session(t, |pos, s| pos.void_order(s, order_id, &reason))
    }

    fn list_open_orders(app, t) -> Vec<OrderSummary> {
        app.with_session(t, |pos, s| pos.list_open_orders(s))
    }

    fn list_orders(app, t, from: i64, to: i64, limit: i64, offset: i64) -> Vec<OrderSummary> {
        app.with_session(t, |pos, s| pos.list_orders(s, from, to, limit, offset))
    }

    fn business_day(app, t) -> BusinessDay {
        app.with_session(t, |pos, s| pos.business_day(s))
    }

    fn close_day(app, t) -> CloseResult {
        let (view, job) = app.with_session(t, |pos, s| {
            let close = pos.close_day(s)?;
            let text = pos.day_close_text(&close)?;
            // A day with no sales, voids or cancellations prints nothing.
            let job = if close.summary.is_empty() { None } else { pos.day_close_job(s, &close)? };
            Ok((DayCloseView { close, text }, job))
        })?;
        let empty = view.close.summary.is_empty();
        Ok(CloseResult { view, print: print(job), empty })
    }

    fn day_closes(app, t, limit: i64) -> Vec<DayCloseView> {
        app.with_session(t, |pos, s| {
            pos.day_closes(s, limit)?
                .into_iter()
                .map(|close| Ok(DayCloseView { text: pos.day_close_text(&close)?, close }))
                .collect()
        })
    }

    fn reprint_day_close(app, t, id: i64) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| {
            let close = pos.day_close(s, id)?;
            pos.day_close_job(s, &close)
        })?;
        if job.is_none() {
            return Err(PosError::Validation("no receipt printer is set up (Settings → Receipt printer)".into()));
    }
        Ok(print(job))
    }

    fn list_users(app, t) -> Vec<User> {
        app.with_session(t, |pos, s| pos.list_users(s))
    }

    fn create_user(app, t, name: String, role: Role, pin: String) -> User {
        app.with_session(t, |pos, s| pos.create_user(s, &name, role, &pin))
    }

    fn update_user(app, t, id: i64, name: String, role: Role, active: bool) -> User {
        app.with_session(t, |pos, s| pos.update_user(s, id, &name, role, active))
    }

    fn reset_pin(app, t, id: i64, pin: String) -> () {
        app.with_session(t, |pos, s| pos.reset_pin(s, id, &pin))
    }

    /// Names of the other active users with this PIN (admins only), so the
    /// staff editor can warn before two people share a PIN.
    fn pin_shared(app, t, pin: String, except_user_id: Option<i64>) -> Vec<String> {
        app.with_session(t, |pos, s| pos.pin_shared(s, &pin, except_user_id))
    }

    fn sales_summary(app, t, from: i64, to: i64) -> SalesSummary {
        app.with_session(t, |pos, s| pos.sales_summary(s, from, to))
    }

    fn top_products(app, t, from: i64, to: i64, limit: i64) -> Vec<TopProduct> {
        app.with_session(t, |pos, s| pos.top_products(s, from, to, limit))
    }

    fn backup_menu(app, t) -> MenuBackupSaved {
        let menu = app.with_session(t, |pos, s| {
            s.require(Permission::Backup)?;
            pos.export_menu(s)
        })?;
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let name = format!("menu-{ts}.json");
        let path = app.backup_dir.join(&name);
        let json = serde_json::to_vec_pretty(&menu).map_err(|e| PosError::Internal(e.to_string()))?;
        std::fs::write(&path, json).map_err(|e| PosError::Internal(format!("cannot save the menu backup: {e}")))?;
        for (_, old) in menu_backups(&app.backup_dir).into_iter().skip(MENU_BACKUPS_KEPT) {
            let _ = std::fs::remove_file(old);
        }
        Ok(MenuBackupSaved { file: MenuBackupFile { name, saved_at: ts, items: menu.items.len() }, path: path.display().to_string(), menu })
    }

    fn list_menu_backups(app, t) -> Vec<MenuBackupFile> {
        app.with_session(t, |_, s| s.require(Permission::Backup))?;
        Ok(menu_backups(&app.backup_dir).into_iter().map(|(f, _)| f).collect())
    }

    fn restore_menu_backup(app, t, name: String) -> MenuCounts {
        if !is_menu_backup_name(&name) {
            return Err(PosError::Validation("that is not a menu backup".into()));
        }
        let bytes = std::fs::read(app.backup_dir.join(&name)).map_err(|_| PosError::NotFound)?;
        let menu: MenuBackup =
            serde_json::from_slice(&bytes).map_err(|_| PosError::Validation("this file is not a POS menu backup".into()))?;
        app.with_session(t, |pos, s| {
            s.require(Permission::Backup)?;
            pos.import_menu(s, &menu)
        })
    }

    /// Restore from a file the user picked (e.g. a menu from another till).
    fn import_menu(app, t, menu: MenuBackup) -> MenuCounts {
        app.with_session(t, |pos, s| {
            s.require(Permission::Backup)?;
            pos.import_menu(s, &menu)
        })
    }

    /// Delete the chosen data after re-checking the admin's PIN and taking
    /// a full backup (restorable like any other backup).
    fn reset_data(app, t, options: ResetOptions, pin: String, confirm: String) -> ResetResult {
        if confirm.trim() != "RESET" {
            return Err(PosError::Validation("type RESET to confirm".into()));
        }
        app.with_session(t, |pos, s| {
            s.require(Permission::ManageSettings)?;
            pos.verify_pin(s, &pin)?;
            let (backup, _) = unique_backup_path(&app.backup_dir, "pos-before-reset-");
            if let Err(e) = pos.backup_unchecked(&backup) {
                let _ = std::fs::remove_file(&backup);
                return Err(disk_full(e));
            }
            let report = pos.reset_data(s, &options)?;
            Ok(ResetResult { report, backup: backup.display().to_string() })
        })
    }

    fn get_service_settings(app, t) -> ServiceSettings {
        app.with_session(t, |pos, _| pos.service_settings())
    }

    fn set_service_settings(app, t, settings: ServiceSettings) -> () {
        app.with_session(t, |pos, s| pos.set_service_settings(s, &settings))
    }

    fn set_service_charge(app, t, order_id: i64, rate_bp: i64) -> Order {
        app.with_session(t, |pos, s| pos.set_service_charge(s, order_id, rate_bp))
    }

    /// One tax rate for the whole order; null = each item's menu rate.
    fn set_order_tax(app, t, order_id: i64, rate_bp: Option<i64>) -> Order {
        app.with_session(t, |pos, s| pos.set_order_tax(s, order_id, rate_bp))
    }

    fn delete_order(app, t, order_id: i64, reason: Option<String>) -> () {
        app.with_session(t, |pos, s| pos.delete_order(s, order_id, reason.as_deref().unwrap_or("")))
    }

    /// True when the item was removed; false when past orders name it, so
    /// it was hidden instead.
    fn delete_product(app, t, id: i64) -> bool {
        app.with_session(t, |pos, s| pos.delete_product(s, id))
    }

    /// Make a category a sub-category of `parent_id` (or a main one: null).
    fn set_category_parent(app, t, id: i64, parent_id: Option<i64>) -> Category {
        app.with_session(t, |pos, s| pos.set_category_parent(s, id, parent_id))
    }

    fn set_category_flags_off(app, t, id: i64, flags_off: i64) -> Category {
        app.with_session(t, |pos, s| pos.set_category_flags_off(s, id, flags_off))
    }

    fn delete_category(app, t, id: i64) -> () {
        app.with_session(t, |pos, s| pos.delete_category(s, id))
    }

    fn set_category_defaults(app, t, id: i64, tax_rate_bp: Option<i64>, starter: bool, prep: Option<bool>, name_alt: Option<String>) -> Category {
        app.with_session(t, |pos, s| pos.set_category_defaults(s, id, tax_rate_bp, starter, prep.unwrap_or(false), name_alt.as_deref()))
    }

    fn untaxed_items(app, t) -> i64 {
        app.with_session(t, |pos, s| pos.untaxed_items(s))
    }

    /// Items at 0% get their category's rate, else the main default.
    fn apply_default_tax(app, t) -> usize {
        app.with_session(t, |pos, s| pos.apply_default_tax(s))
    }

    fn reorder_categories(app, t, ids: Vec<i64>) -> () {
        app.with_session(t, |pos, s| pos.reorder_categories(s, &ids))
    }

    fn reorder_products(app, t, ids: Vec<i64>) -> () {
        app.with_session(t, |pos, s| pos.reorder_products(s, &ids))
    }

    fn duplicate_product(app, t, id: i64) -> Product {
        app.with_session(t, |pos, s| pos.duplicate_product(s, id))
    }

    fn import_items(app, t, rows: Vec<ImportRow>) -> ImportReport {
        app.with_session(t, |pos, s| pos.import_items(s, &rows))
    }

    fn get_setting(app, t, key: String) -> Option<String> {
        app.with_session(t, |pos, _| pos.setting(&key))
    }

    /// Several settings in one go (e.g. everything Finish & Pay needs).
    /// Reserved keys (starting with '_') are never returned.
    fn get_settings(app, t, keys: Vec<String>) -> HashMap<String, Option<String>> {
        if keys.len() > 100 {
            return Err(PosError::Validation("too many settings at once".into()));
        }
        app.with_session(t, |pos, _| {
            keys.into_iter().filter(|k| !k.starts_with('_')).map(|k| Ok((k.clone(), pos.setting(&k)?))).collect()
        })
    }

    fn set_setting(app, t, key: String, value: String) -> () {
        app.with_session(t, |pos, s| pos.set_setting(s, &key, &value))
    }

    /// Delete several things at once (admins only). `kind`: order, product,
    /// category, customer, floor or option_group. Each is checked like a
    /// single delete; the ones that cannot go are listed in `errors`.
    fn delete_many(app, t, kind: String, ids: Vec<i64>, reason: Option<String>) -> BulkResult {
        app.with_session(t, |pos, s| {
            s.require(Permission::Delete)?;
            if ids.is_empty() || ids.len() > 1000 {
                return Err(PosError::Validation("choose what to delete".into()));
            }
            let mut r = BulkResult { deleted: 0, hidden: 0, errors: Vec::new() };
            for id in ids {
                let res = match kind.as_str() {
                    "order" => pos.delete_order(s, id, reason.as_deref().unwrap_or("")).map(|_| true),
                    "product" => pos.delete_product(s, id),
                    "category" => pos.delete_category(s, id).map(|_| true),
                    "customer" => pos.delete_customer(s, id).map(|_| true),
                    "floor" => pos.delete_floor(s, id).map(|_| true),
                    "option_group" => pos.delete_option_group(s, id).map(|_| true),
                    _ => return Err(PosError::Validation("nothing of that kind can be deleted".into())),
                };
                match res {
                    Ok(true) => r.deleted += 1,
                    Ok(false) => r.hidden += 1,
                    Err(e) => r.errors.push(e.to_string()),
                }
            }
            Ok(r)
        })
    }

    /// Full backups in the backup folder, newest first.
    fn list_full_backups(app, t) -> Vec<BackupFile> {
        app.with_session(t, |_, s| s.require(Permission::Backup))?;
        Ok(list_backups(&app.backup_dir).into_iter().filter_map(|(_, p)| backup_file(&p)).filter(|b| is_full_backup_name(&b.name)).collect())
    }

    /// Delete full backups by name (admins only).
    fn delete_backups(app, t, names: Vec<String>) -> usize {
        app.with_session(t, |pos, s| {
            s.require(Permission::Backup)?;
            s.require(Permission::Delete)?;
            let mut n = 0;
            for name in &names {
                if !is_full_backup_name(name) {
                    return Err(PosError::Validation("that is not a backup".into()));
                }
                if std::fs::remove_file(app.backup_dir.join(name)).is_ok() {
                    n += 1;
                }
            }
            pos.audit_event(s, "backup.delete", &names.join(" "))?;
            Ok(n)
        })
    }

    /// Delete menu backups by name (admins only).
    fn delete_menu_backups(app, t, names: Vec<String>) -> usize {
        app.with_session(t, |pos, s| {
            s.require(Permission::Delete)?;
            let mut n = 0;
            for name in &names {
                if !is_menu_backup_name(name) {
                    return Err(PosError::Validation("that is not a menu backup".into()));
                }
                if std::fs::remove_file(app.backup_dir.join(name)).is_ok() {
                    n += 1;
                }
            }
            pos.audit_event(s, "backup.menu_delete", &names.join(" "))?;
            Ok(n)
        })
    }

    fn backup_now(app, t) -> String {
        let path = backup_path(&app.backup_dir);
        let keep = app.with_session(t, |pos, s| {
            pos.backup_to(s, &path)?;
            pos.backups_kept()
        })?;
        prune_backups(&app.backup_dir, keep);
        Ok(path.display().to_string())
    }

    /// Where the full backups are, so they can be copied to a USB stick.
    fn backup_folder(app, t) -> String {
        app.with_session(t, |_, s| s.require(Permission::Backup))?;
        Ok(app.backup_dir.display().to_string())
    }

    /// What a full backup in the folder holds, checked as for a restore
    /// (nothing is changed). Admins only.
    fn inspect_backup(app, t, name: String) -> BackupSummary {
        app.with_session(t, |_, s| require_restore(s))?;
        if !is_full_backup_name(&name) {
            return Err(PosError::Validation("that is not a backup".into()));
        }
        Pos::check_backup(app.backup_dir.join(&name), work_path(&app.backup_dir))
    }

    /// "Restore everything from a file…": the file arrives in pieces of at most
    /// [`UPLOAD_CHUNK`] bytes (base64), in order, under a random `upload`
    /// id. Returns the bytes received so far. Admins only.
    fn upload_backup_chunk(app, t, upload: String, offset: u64, data: String) -> u64 {
        use base64::Engine as _;
        app.with_session(t, |_, s| require_restore(s))?;
        let path = upload_path(&app.backup_dir, &upload)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|_| PosError::Validation("the file did not arrive intact; try again".into()))?;
        if bytes.len() > UPLOAD_CHUNK {
            return Err(PosError::Validation("upload piece too large".into()));
        }
        let end = offset.checked_add(bytes.len() as u64).filter(|&n| n <= MAX_UPLOAD);
        let Some(end) = end else {
            let _ = std::fs::remove_file(&path);
            return Err(PosError::Validation("This file is too large to be a POS backup.".into()));
        };
        let io = |e: std::io::Error| io_error(&e, "cannot save the file");
        let mut f = if offset == 0 {
            remove_stale_uploads(&app.backup_dir);
            std::fs::File::create(&path).map_err(io)?
        } else {
            let f = std::fs::OpenOptions::new().append(true).open(&path).map_err(|_| PosError::Validation("the upload was interrupted; try again".into()))?;
            if f.metadata().map_err(io)?.len() != offset {
                return Err(PosError::Validation("the upload was interrupted; try again".into()));
            }
            f
        };
        let written = std::io::Write::write_all(&mut f, &bytes).and_then(|_| f.sync_all());
        if let Err(e) = written {
            // Do not leave half a file filling the disk.
            drop(f);
            let _ = std::fs::remove_file(&path);
            return Err(io(e));
        }
        Ok(end)
    }

    /// Check an uploaded file and keep it in the backup folder as
    /// "pos-imported-<time>.db", ready to restore. A file that is not a
    /// good POS backup is deleted again and the error says why.
    fn finish_backup_upload(app, t, upload: String) -> UploadedBackup {
        app.with_session(t, |_, s| require_restore(s))?;
        let part = upload_path(&app.backup_dir, &upload)?;
        if !part.is_file() {
            return Err(PosError::Validation("the upload was interrupted; try again".into()));
        }
        let summary = match Pos::check_backup(&part, work_path(&app.backup_dir)) {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        };
        let (dest, _) = unique_backup_path(&app.backup_dir, "pos-imported-");
        std::fs::rename(&part, &dest).map_err(|e| io_error(&e, "cannot save the file"))?;
        let file = backup_file(&dest).ok_or(PosError::NotFound)?;
        let keep = app.with_session(t, |pos, s| {
            pos.audit_event(s, "backup.import", &file.name)?;
            pos.backups_kept()
        })?;
        // Older loaded files beyond the newest few go (never this one: it is the newest).
        prune_backups(&app.backup_dir, keep);
        Ok(UploadedBackup { file, summary })
    }

    /// Replace ALL data with a full backup from the backup folder: orders,
    /// menu, customers, staff and PINs, settings. Admins only, with their
    /// PIN and RESTORE typed. The backup is checked first; then the current
    /// data is saved as "pos-before-restore-<time>.db" and replaced. Every
    /// terminal is logged out: the staff and PINs now come from the backup.
    fn restore_backup(app, t, name: String, pin: String, confirm: String) -> RestoreResult {
        if confirm.trim() != RESTORE_WORD {
            return Err(PosError::Validation(format!("type {RESTORE_WORD} to confirm")));
        }
        if !is_full_backup_name(&name) {
            return Err(PosError::Validation("that is not a backup".into()));
        }
        let src = app.backup_dir.join(&name);
        let (restored, keep) = app.with_session_inner(t, |inner, s| {
            require_restore(s)?;
            inner.pos.verify_pin(s, &pin)?;
            if !src.is_file() {
                return Err(PosError::NotFound);
            }
            // Check and migrate a copy first: a bad file never touches the live data.
            let prepared = inner.pos.prepare_restore(&src, work_path(&app.backup_dir))?;
            let (safety_path, safety_copy) = unique_backup_path(&app.backup_dir, "pos-before-restore-");
            if let Err(e) = inner.pos.backup_unchecked(&safety_path) {
                let _ = std::fs::remove_file(&safety_path);
                return Err(disk_full(e));
            }
            let summary = match inner.pos.restore(s, prepared) {
                // The copy did not happen: nothing changed, everyone stays
                // logged in, and this attempt's safety copy goes (so failed
                // attempts never push older safety copies out).
                Err(e) => {
                    let _ = std::fs::remove_file(&safety_path);
                    return Err(e);
                }
                Ok(summary) => summary,
            };
            // The data is replaced: sessions from the old staff list are void.
            inner.log_everyone_out();
            let keep = inner.pos.backups_kept().unwrap_or(BACKUPS_KEPT_DEFAULT);
            Ok((RestoreResult { summary, restored: name, safety_copy }, keep))
        })?;
        prune_backups(&app.backup_dir, keep);
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    fn app() -> App {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        App::new(pos, std::env::temp_dir())
    }

    #[test]
    fn pruning_keeps_safety_copies_and_other_files() {
        let dir = std::env::temp_dir().join(format!("pos-prune-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut names: Vec<String> = (0..6).map(|i| format!("pos-{}.db", 1_700_000_000 + i)).collect();
        names.extend((0..5).map(|i| format!("pos-before-update-{}.db", 1_600_000_000 + i)));
        names.extend((0..2).map(|i| format!("pos-before-reset-{}.db", 1_500_000_000 + i)));
        names.extend((0..4).map(|i| format!("pos-before-restore-{}.db", 1_400_000_000 + i)));
        names.extend((0..4).map(|i| format!("pos-imported-{}.db", 1_300_000_000 + i)));
        names.extend(["pos-notes.txt".to_string(), "pos-mine.db".to_string()]);
        // A month old: only the newest few safety copies of each kind stay.
        let t0 = SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 60 * 60);
        for (i, n) in names.iter().enumerate() {
            let f = std::fs::File::create(dir.join(n)).unwrap();
            // Newer timestamp in the name = newer file.
            let ts: u64 = n.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0);
            f.set_modified(t0 + std::time::Duration::from_secs(ts % 10_000 + i as u64)).unwrap();
        }
        // Four safety copies from this week: all stay, more than the newest 3.
        for i in 0..4 {
            let f = std::fs::File::create(dir.join(format!("pos-before-reset-{}.db", 1_500_000_100 + i))).unwrap();
            f.set_modified(SystemTime::now() - std::time::Duration::from_secs(86_400 - i * 60)).unwrap();
        }
        prune_backups(&dir, 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, [
            "pos-1700000003.db", "pos-1700000004.db", "pos-1700000005.db",
            "pos-before-reset-1500000100.db", "pos-before-reset-1500000101.db", "pos-before-reset-1500000102.db", "pos-before-reset-1500000103.db",
            "pos-before-restore-1400000001.db", "pos-before-restore-1400000002.db", "pos-before-restore-1400000003.db",
            "pos-before-update-1600000002.db", "pos-before-update-1600000003.db", "pos-before-update-1600000004.db",
            "pos-imported-1300000001.db", "pos-imported-1300000002.db", "pos-imported-1300000003.db",
            "pos-mine.db", "pos-notes.txt",
        ]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn get_settings_returns_many_but_never_reserved_keys() {
        let app = app();
        let keys = json!({ "keys": ["tip_presets", "store_name", "_pin_salt", "_lan_server"] });
        assert!(matches!(dispatch(&app, LOCAL, "get_settings", keys.clone()), Err(PosError::NotLoggedIn)));
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        dispatch(&app, LOCAL, "set_setting", json!({ "key": "tip_presets", "value": "5,10,20" })).unwrap();
        // Staff may read them too (Finish & Pay needs them).
        dispatch(&app, LOCAL, "logout", json!({})).unwrap();
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        let v = dispatch(&app, LOCAL, "get_settings", keys).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m["tip_presets"], "5,10,20");
        assert!(m["store_name"].is_null() || m["store_name"].is_string());
        assert!(!m.keys().any(|k| k.starts_with('_')));
    }

    #[test]
    fn each_terminal_has_its_own_login() {
        let app = app();
        let tablet = "web-0123456789abcdef0123456789abcdef";
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        assert!(matches!(dispatch(&app, tablet, "list_open_orders", json!({})), Err(PosError::NotLoggedIn)));
        dispatch(&app, tablet, "login", json!({ "pin": "2222" })).unwrap();
        let status = |t| dispatch(&app, t, "app_status", Value::Null).unwrap();
        assert_eq!(status(LOCAL)["session"]["name"], "Owner");
        assert_eq!((status(tablet)["session"]["name"].clone(), status(tablet)["remote"].clone()), (json!("Sam"), json!(true)));
        // Staff on the tablet cannot do admin things; the desktop admin still can.
        assert!(matches!(dispatch(&app, tablet, "list_users", json!({})), Err(PosError::Forbidden)));
        assert!(dispatch(&app, LOCAL, "list_users", json!({})).is_ok());
        // Logging out on the tablet leaves the desktop logged in.
        dispatch(&app, tablet, "logout", json!({})).unwrap();
        assert!(status(tablet)["session"].is_null());
        assert_eq!(status(LOCAL)["session"]["name"], "Owner");
    }

    #[test]
    fn wrong_pins_on_the_network_never_lock_out_the_till() {
        let app = app();
        for i in 0..6 {
            let t = format!("web-{i:032x}"); // a new terminal id each time does not help
            let _ = dispatch(&app, &t, "login", json!({ "pin": "9999" }));
        }
        let err = dispatch(&app, "web-ffffffffffffffffffffffffffffffff", "login", json!({ "pin": "2222" })).unwrap_err();
        assert!(matches!(err, PosError::LockedOut(_)), "{err}");
        assert!(dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).is_ok());
    }

    #[test]
    fn arguments_are_camel_case_json() {
        let app = app();
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
        let id = o["id"].as_i64().unwrap();
        // Optional arguments may be left out.
        let r = dispatch(&app, LOCAL, "send_to_kitchen", json!({ "orderId": id })).unwrap();
        assert_eq!(r["items"], json!([]));
        let err = dispatch(&app, LOCAL, "get_order", json!({ "order_id": id })).unwrap_err();
        assert!(err.to_string().starts_with("get_order: missing field `id`"), "{err}");
        let err = dispatch(&app, LOCAL, "install_update", json!({})).unwrap_err();
        assert_eq!(err.to_string(), "unknown command install_update");
    }

    #[test]
    fn cancelling_with_a_shared_pin_on_your_own_till_and_the_kitchen_is_told() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        let mia = pos.create_user(&admin, "Mia", Role::Manager, "2222").unwrap();
        let curry = pos
            .create_product(&admin, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 900, cost_cents: 0, tax_rate_bp: 0,
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap();
        let path = std::env::temp_dir().join(format!("pos-cancel-test-{}.bin", std::process::id()));
        pos.set_kitchen_printer(&admin, &PrinterConfig {
            target: PrinterTarget::File { path: path.display().to_string() }, copy_delay_secs: 0, app_font: false, ..Default::default()
        })
        .unwrap();
        let app = App::new(pos, std::env::temp_dir());
        // The admin's editor can see the PIN is shared.
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let names = dispatch(&app, LOCAL, "pin_shared", json!({ "pin": "2222", "exceptUserId": mia.id })).unwrap();
        assert_eq!(names, json!(["Sam"]));
        // Mia, logged in, sends a dish, then cancels the order with her own PIN.
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222", "userId": mia.id })).unwrap();
        // Staff and managers may read the kitchen teams (kitchen screen).
        assert!(dispatch(&app, LOCAL, "list_teams", json!({})).unwrap().is_array());
        let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
        let id = o["id"].as_i64().unwrap();
        dispatch(&app, LOCAL, "add_item", json!({ "orderId": id, "productId": curry.id, "qty": 1 })).unwrap();
        dispatch(&app, LOCAL, "send_to_kitchen", json!({ "orderId": id })).unwrap();
        let r = dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": id, "reason": "mistake", "pin": "2222" })).unwrap();
        assert_eq!(r, json!({ "attempted": true, "error": null }));
        let printed = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).to_string();
        let _ = std::fs::remove_file(&path);
        assert!(printed.contains("CANCELLED") && printed.contains("Green curry"), "{printed}");
        assert_eq!(dispatch(&app, LOCAL, "get_order", json!({ "id": id })).unwrap()["status"], "cancelled");
        // Nothing in the kitchen: nothing printed.
        let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
        let r = dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": o["id"], "reason": "" })).unwrap();
        assert_eq!(r, json!({ "attempted": false, "error": null }));
    }

    #[test]
    fn the_cancel_command_needs_a_pin_from_staff_only() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        let curry = pos
            .create_product(&admin, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 900, cost_cents: 0, tax_rate_bp: 0,
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap();
        let path = std::env::temp_dir().join(format!("pos-cancel-no-pin-test-{}.bin", std::process::id()));
        pos.set_kitchen_printer(&admin, &PrinterConfig {
            target: PrinterTarget::File { path: path.display().to_string() }, copy_delay_secs: 0, app_font: false, ..Default::default()
        })
        .unwrap();
        let app = App::new(pos, std::env::temp_dir());
        let sent_order = || {
            let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
            dispatch(&app, LOCAL, "add_item", json!({ "orderId": o["id"], "productId": curry.id, "qty": 1 })).unwrap();
            dispatch(&app, LOCAL, "send_to_kitchen", json!({ "orderId": o["id"] })).unwrap();
            o["id"].clone()
        };
        let status = |id: &Value| dispatch(&app, LOCAL, "get_order", json!({ "id": id })).unwrap()["status"].clone();
        // Staff: no PIN (left out or empty) is refused, sent or not.
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        let unsent = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap()["id"].clone();
        let sent = sent_order();
        for id in [&unsent, &sent] {
            for args in [json!({ "orderId": id, "reason": "" }), json!({ "orderId": id, "reason": "", "pin": "" }), json!({ "orderId": id, "reason": "", "pin": null })] {
                assert!(matches!(dispatch(&app, LOCAL, "cancel_order", args), Err(PosError::Forbidden)));
            }
            assert_eq!(status(id), "open");
        }
        // Their own PIN for the unsent one; a manager's for the sent one.
        assert!(matches!(dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": sent, "reason": "", "pin": "2222" })), Err(PosError::Forbidden)));
        dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": unsent, "reason": "", "pin": "2222" })).unwrap();
        dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": sent, "reason": "", "pin": "4646" })).unwrap();
        assert_eq!((status(&unsent), status(&sent)), (json!("cancelled"), json!("cancelled")));
        // A manager and an admin: no PIN, even for an order in the kitchen.
        for pin in ["4646", "1111"] {
            dispatch(&app, LOCAL, "login", json!({ "pin": pin })).unwrap();
            let sent = sent_order();
            let r = dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": sent, "reason": "" })).unwrap();
            assert_eq!(r, json!({ "attempted": true, "error": null }), "the kitchen is told");
            assert_eq!(status(&sent), "cancelled");
            let sent = sent_order();
            dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": sent, "reason": "", "pin": "" })).unwrap();
            assert_eq!(status(&sent), "cancelled");
        }
        let printed = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).to_string();
        let _ = std::fs::remove_file(&path);
        assert!(printed.contains("CANCELLED") && printed.contains("Green curry"), "{printed}");
    }

    #[test]
    fn a_discount_over_the_staff_limit_is_approved_with_a_manager_pin() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.create_user(&admin, "Mia", Role::Manager, "3333").unwrap();
        pos.set_setting(&admin, "discount_staff_max_pct", "10").unwrap();
        let curry = pos
            .create_product(&admin, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: 0,
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap();
        let app = App::new(pos, std::env::temp_dir());
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
        let o = dispatch(&app, LOCAL, "add_item", json!({ "orderId": o["id"], "productId": curry.id, "qty": 1 })).unwrap();
        let item = o["items"][0]["id"].clone();
        let upd = json!({ "qty": 1, "discount_cents": 300, "note": null });
        let err = dispatch(&app, LOCAL, "update_item", json!({ "itemId": item, "update": upd })).unwrap_err();
        assert_eq!(err.to_string(), "a discount over 10% needs a manager PIN");
        let err = dispatch(&app, LOCAL, "update_item", json!({ "itemId": item, "update": upd, "pin": "9999" })).unwrap_err();
        assert!(matches!(err, PosError::InvalidPin), "{err}");
        let o = dispatch(&app, LOCAL, "update_item", json!({ "itemId": item, "update": upd, "pin": "3333" })).unwrap();
        assert_eq!(o["discount_cents"], 300);
        // Sam is still the one logged in.
        assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["session"]["name"], "Sam");
    }

    fn curry(name: &str) -> NewProduct {
        NewProduct {
            category_id: None, sku: None, barcode: None, name: name.into(), price_cents: 900, cost_cents: 0, tax_rate_bp: 0,
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        }
    }

    /// A full backup from "the other PC" (its own staff, PINs, menu, an order), saved in `dir` as `name`.
    fn other_pc_backup(dir: &Path, name: &str) {
        let mut pos = Pos::open_in_memory().unwrap();
        let nok = pos.setup_first_admin("Nok", "4321").unwrap();
        pos.create_user(&nok, "Ben", Role::Staff, "5555").unwrap();
        pos.set_setting(&nok, "store_name", "Old Shop").unwrap();
        let p = pos.create_product(&nok, curry("Massaman")).unwrap();
        let o = pos.open_order(&nok, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
        pos.add_item(&nok, o.id, p.id, 2, None, None, &[]).unwrap();
        pos.backup_unchecked(dir.join(name)).unwrap();
    }

    /// This till: Owner (admin, 1111) and Sam (staff, 2222), a Pad thai, backups in `dir`.
    fn till(dir: &Path) -> App {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.create_product(&admin, curry("Pad thai")).unwrap();
        App::new(pos, dir.to_path_buf())
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    fn menu_names(app: &App, t: &str) -> Vec<String> {
        let v = dispatch(app, t, "menu", json!({})).unwrap();
        v.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn restoring_a_full_backup_replaces_everything_and_logs_everyone_out() {
        let dir = tempfile::tempdir().unwrap();
        other_pc_backup(dir.path(), "pos-1700000000.db");
        let app = till(dir.path());
        let tablet = "web-0123456789abcdef0123456789abcdef";
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        dispatch(&app, tablet, "login", json!({ "pin": "2222" })).unwrap();
        let restore = |t: &str, pin: &str, confirm: &str| {
            dispatch(&app, t, "restore_backup", json!({ "name": "pos-1700000000.db", "pin": pin, "confirm": confirm }))
        };
        // Staff can neither look inside nor restore.
        assert!(matches!(dispatch(&app, tablet, "inspect_backup", json!({ "name": "pos-1700000000.db" })), Err(PosError::Forbidden)));
        assert!(matches!(restore(tablet, "2222", "RESTORE"), Err(PosError::Forbidden)));
        // The admin sees what is in it.
        let info = dispatch(&app, LOCAL, "inspect_backup", json!({ "name": "pos-1700000000.db" })).unwrap();
        assert_eq!((info["store_name"].clone(), info["orders"].clone(), info["menu_items"].clone(), info["staff"].clone()), (json!("Old Shop"), json!(1), json!(1), json!(2)));
        // RESTORE must be typed and the admin's own PIN given.
        assert_eq!(restore(LOCAL, "1111", "restore please").unwrap_err().to_string(), "type RESTORE to confirm");
        assert!(matches!(restore(LOCAL, "", "RESTORE"), Err(PosError::InvalidPin)));
        assert!(matches!(restore(LOCAL, "9999", "RESTORE"), Err(PosError::InvalidPin)));
        assert!(matches!(restore(LOCAL, "2222", "RESTORE"), Err(PosError::InvalidPin)));
        let err = dispatch(&app, LOCAL, "restore_backup", json!({ "name": "../pos.db", "pin": "1111", "confirm": "RESTORE" })).unwrap_err();
        assert_eq!(err.to_string(), "that is not a backup");
        // None of that changed anything or logged anyone out.
        assert_eq!(menu_names(&app, LOCAL), ["Pad thai"]);
        assert_eq!(dispatch(&app, tablet, "app_status", Value::Null).unwrap()["session"]["name"], "Sam");
        assert_eq!(files(dir.path()), ["pos-1700000000.db"]);

        let r = restore(LOCAL, "1111", " RESTORE ").unwrap();
        assert_eq!(r["summary"]["orders"], 1);
        assert_eq!(r["restored"], "pos-1700000000.db");
        let safety = r["safety_copy"].as_str().unwrap().to_string();
        assert!(safety.starts_with("pos-before-restore-") && safety.ends_with(".db"), "{safety}");
        assert_eq!(files(dir.path()), ["pos-1700000000.db".to_string(), safety.clone()]);
        // Everyone is logged out, everywhere.
        for t in [LOCAL, tablet] {
            assert!(dispatch(&app, t, "app_status", Value::Null).unwrap()["session"].is_null());
            assert!(matches!(dispatch(&app, t, "list_open_orders", json!({})), Err(PosError::NotLoggedIn)));
        }
        // The staff and PINs are the other PC's now.
        assert!(matches!(dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })), Err(PosError::InvalidPin)));
        assert_eq!(dispatch(&app, tablet, "login", json!({ "pin": "5555" })).unwrap()["name"], "Ben");
        assert_eq!(dispatch(&app, LOCAL, "login", json!({ "pin": "4321" })).unwrap()["name"], "Nok");
        assert_eq!(menu_names(&app, LOCAL), ["Massaman"]);
        assert_eq!(dispatch(&app, LOCAL, "list_open_orders", json!({})).unwrap().as_array().unwrap().len(), 1);
        assert_eq!(dispatch(&app, LOCAL, "get_setting", json!({ "key": "store_name" })).unwrap(), "Old Shop");
        // The safety copy undoes it.
        dispatch(&app, LOCAL, "restore_backup", json!({ "name": safety, "pin": "4321", "confirm": "RESTORE" })).unwrap();
        assert_eq!(dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap()["name"], "Owner");
        assert_eq!(menu_names(&app, LOCAL), ["Pad thai"]);
        // A second safety copy, even within the same second (never overwritten).
        assert_eq!(files(dir.path()).iter().filter(|n| n.starts_with("pos-before-restore-")).count(), 2);
    }

    #[test]
    fn a_bad_file_is_refused_and_the_live_data_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pos-1.db"), b"not a database, just some text that is long enough").unwrap();
        other_pc_backup(dir.path(), "pos-2.db");
        Connection::open(dir.path().join("pos-2.db")).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();
        let app = till(dir.path());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let restore = |name: &str| dispatch(&app, LOCAL, "restore_backup", json!({ "name": name, "pin": "1111", "confirm": "RESTORE" }));
        assert!(restore("pos-1.db").unwrap_err().to_string().contains("not a POS backup"));
        assert!(restore("pos-2.db").unwrap_err().to_string().contains("newer version of the app: update this PC first"));
        assert!(matches!(restore("pos-3.db"), Err(PosError::NotFound)));
        // Still logged in, same data, no safety copy and no scratch files.
        assert_eq!(menu_names(&app, LOCAL), ["Pad thai"]);
        assert_eq!(files(dir.path()), ["pos-1.db", "pos-2.db"]);
    }

    #[test]
    fn a_file_from_another_pc_is_uploaded_in_pieces_then_restored() {
        use base64::Engine as _;
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let src = tempfile::tempdir().unwrap();
        other_pc_backup(src.path(), "pos-1700000000.db");
        let bytes = std::fs::read(src.path().join("pos-1700000000.db")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let app = till(dir.path());
        let id = "00112233445566778899aabbccddeeff";
        let chunk = |t: &str, offset: usize, data: &[u8]| dispatch(&app, t, "upload_backup_chunk", json!({ "upload": id, "offset": offset, "data": b64(data) }));
        // Admins only.
        assert!(matches!(chunk(LOCAL, 0, &bytes[..10]), Err(PosError::NotLoggedIn)));
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        assert!(matches!(chunk(LOCAL, 0, &bytes[..10]), Err(PosError::Forbidden)));
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        // A bad id is not a path.
        let err = dispatch(&app, LOCAL, "upload_backup_chunk", json!({ "upload": "../../x", "offset": 0, "data": "" })).unwrap_err();
        assert_eq!(err.to_string(), "bad upload id");
        // Three pieces, in order; a piece out of place is refused.
        let third = bytes.len() / 3;
        assert_eq!(chunk(LOCAL, 0, &bytes[..third]).unwrap(), third);
        assert!(chunk(LOCAL, third + 5, &bytes[third..2 * third]).unwrap_err().to_string().contains("interrupted"));
        chunk(LOCAL, third, &bytes[third..2 * third]).unwrap();
        assert_eq!(chunk(LOCAL, 2 * third, &bytes[2 * third..]).unwrap(), bytes.len());
        let up = dispatch(&app, LOCAL, "finish_backup_upload", json!({ "upload": id })).unwrap();
        let name = up["file"]["name"].as_str().unwrap().to_string();
        assert!(name.starts_with("pos-imported-"), "{name}");
        assert_eq!(up["file"]["size"], bytes.len());
        assert_eq!(up["summary"]["store_name"], "Old Shop");
        assert_eq!(std::fs::read(dir.path().join(&name)).unwrap(), bytes);
        // It is in the list like any backup, and restores like one.
        let list = dispatch(&app, LOCAL, "list_full_backups", json!({})).unwrap();
        assert_eq!(list[0]["name"], name.as_str());
        dispatch(&app, LOCAL, "restore_backup", json!({ "name": name, "pin": "1111", "confirm": "RESTORE" })).unwrap();
        assert_eq!(dispatch(&app, LOCAL, "login", json!({ "pin": "4321" })).unwrap()["name"], "Nok");

        // Not a backup: refused, and the uploaded piece is not kept.
        chunk(LOCAL, 0, b"PK\x03\x04 a zip file, say").unwrap();
        let err = dispatch(&app, LOCAL, "finish_backup_upload", json!({ "upload": id })).unwrap_err();
        assert!(err.to_string().contains("not a POS backup"), "{err}");
        assert!(!files(dir.path()).iter().any(|n| n.starts_with("upload-") || n.starts_with("restore-work-")), "{:?}", files(dir.path()));
        // Downloading (browsers): admins only, backups only.
        assert!(backup_download(&app, LOCAL, &name).unwrap().is_file());
        assert!(backup_download(&app, LOCAL, "../pos.db").is_err());
        dispatch(&app, LOCAL, "login", json!({ "pin": "5555" })).unwrap();
        assert!(matches!(backup_download(&app, LOCAL, &name), Err(PosError::Forbidden)));
    }

    #[test]
    fn a_restore_that_could_not_copy_changes_nothing_and_keeps_everyone_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        other_pc_backup(dir.path(), "pos-1700000000.db");
        let live = dir.path().join("live.db");
        let mut pos = Pos::open(&live).unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.create_product(&admin, curry("Pad thai")).unwrap();
        let backups = dir.path().join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::rename(dir.path().join("pos-1700000000.db"), backups.join("pos-1700000000.db")).unwrap();
        let app = App::new(pos, backups.clone());
        let tablet = "web-0123456789abcdef0123456789abcdef";
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        dispatch(&app, tablet, "login", json!({ "pin": "2222" })).unwrap();
        let id = dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["db_id"].clone();
        // Another program holds the database for writing.
        let other = Connection::open(&live).unwrap();
        other.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let err = dispatch(&app, LOCAL, "restore_backup", json!({ "name": "pos-1700000000.db", "pin": "1111", "confirm": "RESTORE" })).unwrap_err();
        assert!(err.to_string().contains("busy"), "{err}");
        other.execute_batch("ROLLBACK;").unwrap();
        // Nothing changed: same data, same logins, no safety copy left behind.
        assert_eq!(menu_names(&app, LOCAL), ["Pad thai"]);
        assert_eq!(dispatch(&app, tablet, "app_status", Value::Null).unwrap()["session"]["name"], "Sam");
        assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["db_id"], id);
        assert_eq!(files(&backups), ["pos-1700000000.db"]);
        // Once the other program lets go, it works (and the data id changes).
        dispatch(&app, LOCAL, "restore_backup", json!({ "name": "pos-1700000000.db", "pin": "1111", "confirm": "RESTORE" })).unwrap();
        let after = dispatch(&app, LOCAL, "app_status", Value::Null).unwrap();
        assert!(after["session"].is_null());
        assert_ne!(after["db_id"], id);
        assert_eq!(files(&backups).iter().filter(|n| n.starts_with("pos-before-restore-")).count(), 1);
    }

    #[test]
    fn uploads_tidy_up_after_themselves() {
        use base64::Engine as _;
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let src = tempfile::tempdir().unwrap();
        other_pc_backup(src.path(), "pos-1.db");
        let bytes = std::fs::read(src.path().join("pos-1.db")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let month_ago = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
        let old = |name: &str, age: SystemTime| std::fs::File::create(dir.path().join(name)).unwrap().set_modified(age).unwrap();
        for i in 0..4 {
            old(&format!("pos-imported-{}.db", 1_600_000_000 + i), month_ago + Duration::from_secs(i));
        }
        old("upload-ffffffffffffffffffffffffffffffff.part", SystemTime::now() - Duration::from_secs(2 * 60 * 60));
        old("upload-eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee.part", SystemTime::now());
        old("restore-work-1-2-3.tmp", SystemTime::now() - Duration::from_secs(2 * 60 * 60));
        let app = till(dir.path());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let id = "00112233445566778899aabbccddeeff";
        // A piece that would overflow is refused, not a crash.
        let err = dispatch(&app, LOCAL, "upload_backup_chunk", json!({ "upload": id, "offset": u64::MAX, "data": b64(b"xx") })).unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
        dispatch(&app, LOCAL, "upload_backup_chunk", json!({ "upload": id, "offset": 0, "data": b64(&bytes) })).unwrap();
        let up = dispatch(&app, LOCAL, "finish_backup_upload", json!({ "upload": id })).unwrap();
        let name = up["file"]["name"].as_str().unwrap().to_string();
        // Stale pieces and scratch files are gone, another upload in progress stays,
        // and only the newest 3 loaded files are kept (the new one first).
        let left = files(dir.path());
        assert_eq!(left, [
            "pos-imported-1600000002.db".to_string(), "pos-imported-1600000003.db".into(), name, "upload-eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee.part".into(),
        ]);
    }

    #[test]
    fn safety_copies_never_overwrite_each_other_and_do_not_count_as_daily_backups() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = unique_backup_path(dir.path(), "pos-before-restore-");
        std::fs::write(&a, b"1").unwrap();
        let (b, name) = unique_backup_path(dir.path(), "pos-before-restore-");
        assert_ne!(a, b);
        assert!(name.starts_with("pos-before-restore-"));
        // Only "pos-<digits>.db" makes the daily backup unnecessary.
        assert!(newest_regular_backup(dir.path()).is_none());
        std::fs::write(dir.path().join("pos-imported-5.db"), b"1").unwrap();
        assert!(newest_regular_backup(dir.path()).is_none());
        std::fs::write(dir.path().join("pos-5.db"), b"1").unwrap();
        assert!(newest_regular_backup(dir.path()).is_some());
    }
}

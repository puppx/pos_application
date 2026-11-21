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
    /// The database's PIN salt and cost, copied out so a PIN is hashed
    /// (20-60 ms) before taking `inner`, not while every terminal waits.
    /// Renewed by a restore; a stale one only costs hashing again.
    hasher: Mutex<PinHasher>,
    /// Held while the automatic backup copies the database, so a restore or
    /// an update waits for it. True once an update is installing: no
    /// automatic backup starts after that.
    backup_gate: Mutex<bool>,
    /// The live database file, for the automatic backups (set by main.rs).
    db_path: Mutex<Option<PathBuf>>,
    /// What the automatic backups did, for Backup & data and the admins' alert.
    backup_state: Mutex<BackupState>,
    /// `Pos::change_count` when the last backup started (None = none yet).
    backed_up_changes: Mutex<Option<i64>>,
    /// When the last scheduled backup was made (or the app started).
    last_scheduled: Mutex<Instant>,
    /// Each browser's own device name, as it last sent it (`X-Device`).
    devices: Mutex<HashMap<String, String>>,
    /// The background worker (main.rs): day close backups and log cleanups
    /// go there; without one (tests) they run at once.
    jobs: Mutex<Option<std::sync::mpsc::Sender<Job>>>,
    /// Why the database could not be opened at all (damaged): `inner.pos`
    /// is then an empty stand-in that nothing may use, and only the main
    /// till's recovery commands work, until a backup is restored.
    cannot_open: Mutex<Option<String>>,
    /// Wrong admin PINs typed on the recovery screen.
    recovery_throttle: Mutex<LoginThrottle>,
}

/// Work for the background worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// The automatic log and done-ticket cleanup (if due).
    LogCleanup,
    Backup(BackupKind),
}

/// Kinds of automatic backup, each its own file name and keep rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    /// Once a day after start ("pos-<time>.db", Keep N).
    Daily,
    /// "Backup now" ("pos-<time>.db").
    Manual,
    /// Every 1, 2 or 4 hours while the app runs, when something changed ("pos-auto-<time>.db").
    Scheduled,
    /// After the day is closed ("pos-<time>.db").
    DayClose,
    /// When the app closes normally, when something changed ("pos-exit-<time>.db", newest 3).
    Exit,
    /// On the start after the app did not close properly ("pos-emergency-<time>.db").
    Emergency,
}

impl BackupKind {
    fn prefix(self) -> &'static str {
        match self {
            BackupKind::Daily | BackupKind::Manual | BackupKind::DayClose => "pos-",
            BackupKind::Scheduled => "pos-auto-",
            BackupKind::Exit => "pos-exit-",
            BackupKind::Emergency => "pos-emergency-",
        }
    }
}

/// What the automatic backups did since the app started.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BackupState {
    /// The app did not close properly last time (crash, power cut, killed).
    pub unclean_shutdown: bool,
    /// The emergency backup made because of it.
    pub emergency_backup: Option<String>,
    /// The database failed its integrity check: no automatic backup runs
    /// (good older ones are never pushed out); restore one.
    pub damaged: bool,
    pub damaged_detail: Option<String>,
    /// Admins dismissed the "did not close properly" note.
    pub dismissed: bool,
    /// The newest backup made by this app run, and when (unix seconds).
    pub last_backup: Option<String>,
    pub last_backup_at: Option<u64>,
    pub last_backup_kind: Option<BackupKind>,
    /// The last automatic backup that failed, and why.
    pub last_error: Option<String>,
    pub last_error_at: Option<u64>,
    /// Second folder: the last good copy, or why the last copy failed.
    pub copy_ok_at: Option<u64>,
    pub copy_error: Option<String>,
    pub copy_error_at: Option<u64>,
}

/// `app_status.backup_alert` (admins only): something an admin must know.
#[derive(Debug, Clone, Serialize)]
pub struct BackupAlert {
    /// "damaged", "unclean" or "copy_failed" (the most serious first).
    kind: &'static str,
    message: String,
    emergency_backup: Option<String>,
}

impl App {
    pub fn new(pos: Pos, backup_dir: PathBuf) -> App {
        let hasher = Mutex::new(pos.pin_hasher());
        App {
            inner: Mutex::new(Inner { pos, logins: HashMap::new(), network_throttle: LoginThrottle::default(), local_throttle: LoginThrottle::default() }),
            backup_dir,
            hasher,
            backup_gate: Mutex::new(false),
            db_path: Mutex::new(None),
            backup_state: Mutex::new(BackupState::default()),
            backed_up_changes: Mutex::new(None),
            last_scheduled: Mutex::new(Instant::now()),
            jobs: Mutex::new(None),
            devices: Mutex::new(HashMap::new()),
            cannot_open: Mutex::new(None),
            recovery_throttle: Mutex::new(LoginThrottle::default()),
        }
    }

    /// The app when its database file could not be opened (damaged, `why`):
    /// only the recovery screen on the main till works (see [`RECOVERY_COMMANDS`]).
    pub fn cannot_open(why: String, backup_dir: PathBuf) -> Result<App> {
        let app = App::new(Pos::open_in_memory()?, backup_dir);
        {
            let mut st = app.backup_state();
            st.damaged = true;
            st.damaged_detail = Some(why.clone());
        }
        *app.cannot_open.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        Ok(app)
    }

    /// The database could not be opened (see [`App::cannot_open`]).
    pub fn opened_damaged(&self) -> Option<String> {
        self.cannot_open.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The data turned out damaged (a check, a backup or any command hit
    /// "database disk image is malformed"): no more automatic backups (good
    /// ones are never pushed out), admins are told, and the main till
    /// offers the recovery screen.
    pub fn note_damage(&self, detail: &str) {
        let mut st = self.backup_state();
        if !st.damaged {
            eprintln!("the database is damaged: {detail}");
            st.damaged = true;
            st.damaged_detail = Some(detail.to_string());
        }
    }

    /// The device name a browser sent with its request (server.rs), kept for
    /// its audit rows. None or empty forgets it.
    pub fn set_device(&self, t: &str, device: Option<&str>) {
        let mut d = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        match device.and_then(clean_device_label) {
            Some(name) if d.get(t) != Some(&name) => {
                if d.len() >= 4 * MAX_TERMINALS {
                    d.clear();
                }
                d.insert(t.to_string(), name);
            }
            Some(_) => {}
            None => {
                d.remove(t);
            }
        }
    }

    fn device(&self, t: &str) -> Option<String> {
        self.devices.lock().unwrap_or_else(|e| e.into_inner()).get(t).cloned()
    }

    /// The live database file (for the automatic backups).
    pub fn set_db_path(&self, db: PathBuf) {
        *self.db_path.lock().unwrap_or_else(|e| e.into_inner()) = Some(db);
    }

    fn db_path(&self) -> Option<PathBuf> {
        self.db_path.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn backup_state(&self) -> MutexGuard<'_, BackupState> {
        self.backup_state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hand background work to the worker (main.rs), or do it now without one.
    pub fn set_jobs(&self, tx: std::sync::mpsc::Sender<Job>) {
        *self.jobs.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    }

    pub fn request(&self, job: Job) {
        let sent = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|tx| tx.send(job).is_ok());
        if sent != Some(true) {
            run_job(self, job);
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hash a typed PIN without the database lock (see [`HashedPin`]).
    fn hash_pin(&self, pin: &str) -> HashedPin {
        let hasher = self.hasher.lock().unwrap_or_else(|e| e.into_inner()).clone();
        hasher.hash(pin)
    }

    fn backup_gate(&self) -> MutexGuard<'_, bool> {
        self.backup_gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An update is about to replace the app: wait for an automatic backup
    /// still copying, and start no other.
    pub fn stop_auto_backups(&self) {
        *self.backup_gate() = true;
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
                inner.pos.set_terminal_device(Some(t), self.device(t).as_deref());
                let out = f(inner, &s);
                inner.pos.set_terminal(None);
                out
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

    /// Re-read the sessions of everyone logged in as `user_id` (after their
    /// name, role or active flag changed); those who can no longer log in are logged out.
    fn refresh_logins_of(&mut self, user_id: i64) {
        let Inner { pos, logins, .. } = self;
        logins.retain(|_, l| {
            if l.session.user_id != user_id {
                return true;
            }
            match pos.refresh_session(&l.session) {
                Ok(s) => {
                    l.session = s;
                    true
                }
                Err(_) => false,
            }
        });
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
    /// Ticket badges for the logged-in user (None when logged out).
    tickets: Option<TicketBadges>,
    /// Admins only: a backup problem to show (None = nothing to say).
    backup_alert: Option<BackupAlert>,
    /// The main till only: the data is damaged, so the recovery screen can
    /// restore a backup without logging in (None = all well).
    recovery: Option<RecoveryInfo>,
}

/// `app_status.recovery` (main till only).
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryInfo {
    /// "cannot_open": the data could not be opened at all (nothing else
    /// works); "damaged": it opens but is damaged (logging in may still work).
    reason: &'static str,
    /// What SQLite said, for the support person.
    detail: Option<String>,
    message: String,
    /// The login screen can still be tried ("damaged" only).
    can_log_in: bool,
}

/// What `recovery_check` found: a backup that passed every check.
#[derive(Serialize)]
pub struct RecoveryCandidate {
    file: BackupFile,
    summary: BackupSummary,
}

#[derive(Serialize)]
pub struct RecoveryRestoreResult {
    #[serde(flatten)]
    restored: RestoreResult,
    /// Whose admin PIN was accepted: "backup" (an admin of the restored
    /// backup) or "current" (an admin of the damaged data, still readable).
    pin_from: &'static str,
    /// The data could not be opened when the app started: restart the app
    /// so the local network server and the automatic backups start too.
    restart_needed: bool,
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
    /// The same `idempotencyKey` came again (the till did not hear the
    /// answer and asked again): this is the first answer; nothing was
    /// taken or printed this time.
    replayed: bool,
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

#[derive(Serialize)]
pub struct BackupStatusView {
    settings: BackupSettings,
    state: BackupState,
    /// The second folder is there now (None = none set).
    copy_dir_ok: Option<bool>,
    /// The folder can be picked here (the main till only).
    can_pick_folder: bool,
}

#[derive(Serialize)]
pub struct CreatedTicket {
    ticket: Ticket,
    /// Managers and admins logged in on any terminal right now.
    answerers_online: usize,
    /// "Who staff call when urgent" (Settings → Login & staff), if set.
    call_text: Option<String>,
}

#[derive(Serialize)]
pub struct DeletedCount {
    deleted: usize,
}

#[derive(Serialize)]
pub struct ExportedCsv {
    csv: String,
    rows: usize,
    /// Suggested file name: activity-YYYY-MM-DD.csv.
    file_name: String,
    /// Where the main till saved it (Downloads), when asked to save.
    path: Option<String>,
}

/// The user's Downloads folder (the backup folder if there is none).
fn downloads_dir(fallback: &Path) -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    match home.map(|h| PathBuf::from(h).join("Downloads")) {
        Some(d) if d.is_dir() => d,
        _ => fallback.to_path_buf(),
    }
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

/// Today's date on this PC, YYYY-MM-DD.
fn chrono_today() -> String {
    let secs = unix_now() as i64;
    pos_core::local_date(secs)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The automatic daily backup: a copy of the database file at `db` when the
/// newest daily or manual backup is over a day old, then old ones pruned.
/// Run in the background after the window opens (on a big database it takes
/// a while). It copies through a connection of its own, so terminals keep
/// working; restores and updates wait for it (`backup_gate`). The copy is
/// written under a scratch name and renamed when complete, so a copy cut
/// off (the app closed) is never taken for a backup. Returns the new file.
pub fn auto_backup(app: &App, db: &Path, every: Duration) -> Option<PathBuf> {
    let gate = app.backup_gate();
    if *gate {
        return None;
    }
    let dir = &app.backup_dir;
    let set_up = !app.lock().pos.needs_setup().unwrap_or(true);
    // Only daily and manual backups count: a restore or a loaded file is not one.
    let fresh = newest_regular_backup(dir).and_then(|t| t.elapsed().ok()).is_some_and(|age| age < every);
    if fresh || !set_up {
        return None;
    }
    let made = backup_locked(app, db, BackupKind::Daily);
    drop(gate);
    made
}

/// An automatic backup of kind `kind` now (scheduled, day close, exit,
/// emergency): in the background, through a connection of its own, so
/// terminals keep working. Scheduled and exit backups are skipped when
/// nothing changed since the last backup. Never while the database is
/// damaged (good older backups are never pushed out). Returns the new file.
pub fn backup_kind(app: &App, kind: BackupKind) -> Option<PathBuf> {
    let db = app.db_path()?;
    let gate = app.backup_gate();
    if *gate {
        return None;
    }
    let set_up = !app.lock().pos.needs_setup().unwrap_or(true);
    if !set_up {
        return None;
    }
    if matches!(kind, BackupKind::Scheduled | BackupKind::Exit) && !changed_since_backup(app) {
        return None;
    }
    let made = backup_locked(app, &db, kind);
    drop(gate);
    made
}

/// Something was written since the last backup this app run made (or, with
/// none yet, since the app started).
fn changed_since_backup(app: &App) -> bool {
    let now = app.lock().pos.change_count().unwrap_or(i64::MAX);
    let last = *app.backed_up_changes.lock().unwrap_or_else(|e| e.into_inner());
    last != Some(now)
}

/// Remember the start of the app as "backed up": scheduled and exit backups
/// then only run once something changed. main.rs calls this after startup.
pub fn mark_unchanged(app: &App) {
    let now = app.lock().pos.change_count().ok();
    *app.backed_up_changes.lock().unwrap_or_else(|e| e.into_inner()) = now;
}

/// Make the backup (the caller holds `backup_gate`): copy under a scratch
/// name, check it (`quick_check`), rename it into place, prune, then copy
/// it to the second folder if one is set.
fn backup_locked(app: &App, db: &Path, kind: BackupKind) -> Option<PathBuf> {
    let dir = &app.backup_dir;
    if app.backup_state().damaged {
        return None;
    }
    let (settings, changes) = {
        let g = app.lock();
        (g.pos.backup_settings().ok(), g.pos.change_count().ok())
    };
    remove_stale_uploads(dir);
    let work = dir.join(format!("backup-work-{}-{}.tmp", unix_now(), std::process::id()));
    let made = Pos::backup_database_file(db, &work).and_then(|()| Pos::verify_backup_file(&work)).and_then(|()| {
        let (path, _) = unique_backup_path(dir, kind.prefix());
        std::fs::rename(&work, &path).map_err(|e| io_error(&e, "saving the backup"))?;
        Ok(path)
    });
    let made = match made {
        Ok(path) => {
            *app.backed_up_changes.lock().unwrap_or_else(|e| e.into_inner()) = changes;
            let mut st = app.backup_state();
            st.last_backup = path.file_name().map(|n| n.to_string_lossy().into_owned());
            st.last_backup_at = Some(unix_now());
            st.last_backup_kind = Some(kind);
            Some(path)
        }
        Err(e) => {
            eprintln!("{kind:?} backup failed: {e}");
            let _ = std::fs::remove_file(&work);
            if let Some(why) = damage_behind(db, &e) {
                app.note_damage(&why);
            }
            let mut st = app.backup_state();
            st.last_error = Some(e.to_string());
            st.last_error_at = Some(unix_now());
            None
        }
    };
    let keep = settings.as_ref().map_or(BACKUPS_KEPT_DEFAULT, |s| s.kept);
    let auto_keep = settings.as_ref().map_or(12, |s| s.hourly_keep);
    prune_all(dir, keep, auto_keep);
    if let (Some(path), Some(settings)) = (&made, &settings) {
        second_copy(app, settings, path);
    }
    made
}

/// Prune every kind: the daily rule ([`prune_backups`]), scheduled ones to
/// `auto_keep`, exit ones to the newest few, emergency ones like safety copies.
pub fn prune_all(dir: &Path, keep: usize, auto_keep: usize) {
    prune_backups(dir, keep);
    let all = list_backups(dir);
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let of_kind = |kind: &'static str| all.iter().filter(move |(_, p)| is_kind(&name(p), kind));
    let old_enough = |(t, _): &&(SystemTime, PathBuf)| t.elapsed().is_ok_and(|age| age > SAFETY_COPIES_MIN_AGE);
    let old = of_kind("pos-auto-")
        .skip(auto_keep.max(1))
        .chain(of_kind("pos-exit-").skip(SAFETY_COPIES_KEPT))
        .chain(of_kind("pos-emergency-").skip(SAFETY_COPIES_KEPT).filter(old_enough));
    for (_, p) in old {
        let _ = std::fs::remove_file(p);
    }
}

/// "Also copy backups to": copy a new, checked backup to the second folder
/// (scratch name, then rename) and keep the same number there. A missing or
/// read-only folder never fails the backup: the status says why, for the admins.
fn second_copy(app: &App, settings: &BackupSettings, path: &Path) {
    let Some(dir) = settings.copy_dir.as_deref().filter(|_| settings.copy_on) else { return };
    let dir = Path::new(dir);
    let result = (|| -> std::result::Result<(), String> {
        if !dir.is_dir() {
            return Err("folder not found".into());
        }
        let name = path.file_name().ok_or_else(|| "bad name".to_string())?;
        let tmp = dir.join(format!(".copy-{}.tmp", name.to_string_lossy()));
        let copied = std::fs::copy(path, &tmp).and_then(|_| std::fs::File::open(&tmp)?.sync_all()).and_then(|()| std::fs::rename(&tmp, dir.join(name)));
        if let Err(e) = copied {
            let _ = std::fs::remove_file(&tmp);
            // ERROR_DISK_FULL (Windows), ENOSPC (elsewhere).
            return Err(if matches!(e.raw_os_error(), Some(112 | 39 | 28)) {
                "the folder's disk is full".into()
            } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                "cannot write to the folder".into()
            } else {
                format!("cannot write to the folder ({e})")
            });
        }
        prune_all(dir, settings.kept, settings.hourly_keep);
        Ok(())
    })();
    let mut st = app.backup_state();
    match result {
        Ok(()) => {
            st.copy_ok_at = Some(unix_now());
            st.copy_error = None;
            st.copy_error_at = None;
        }
        Err(e) => {
            eprintln!("second backup copy failed: {e}");
            st.copy_error = Some(e);
            st.copy_error_at = Some(unix_now());
        }
    }
}

/// When a backup failed: is the live data damaged? SQLite said so
/// (SQLITE_CORRUPT), or the copy failed its check and the live file fails
/// `quick_check` too (a copy that is bad by itself is not the data's fault).
fn damage_behind(db: &Path, e: &PosError) -> Option<String> {
    if is_corrupt(e) {
        return Some(e.to_string());
    }
    if !matches!(e, PosError::Validation(m) if m.contains("damaged")) {
        return None;
    }
    match Pos::quick_check_file(db) {
        Ok(None) => None,
        Ok(Some(p)) => Some(p),
        Err(e) => Some(e.to_string()),
    }
}

/// "Back up now" when the data is damaged.
const DAMAGED_NO_BACKUP: &str =
    "The POS data is damaged, so no backup was made (your earlier backups are kept). Restore the newest good backup: Backup & data → Restore, or the recovery screen on the main till.";

/// Commands that work while the database cannot be opened (the rest are
/// refused): the recovery screen's, on the main till only.
const RECOVERY_COMMANDS: &[&str] = &["app_status", "recovery_status", "recovery_check", "recovery_restore"];

/// The recovery commands: the main till only, and only while the data is damaged.
fn require_recovery(app: &App, t: &str) -> Result<()> {
    if t != LOCAL {
        return Err(PosError::Forbidden);
    }
    if !app.backup_state().damaged {
        return Err(PosError::Validation("The POS data is not damaged: restore a backup from Backup & data instead.".into()));
    }
    Ok(())
}

/// The newest backups first, as the recovery screen lists them.
fn recovery_backups(app: &App) -> Vec<BackupFile> {
    list_backups(&app.backup_dir).into_iter().filter_map(|(_, p)| backup_file(&p)).filter(|b| is_full_backup_name(&b.name)).collect()
}

/// How many backups `recovery_check` tries, newest first, for a good one.
const RECOVERY_TRIES: usize = 10;

/// "Choose a backup…" on the recovery screen (main.rs, the native file
/// picker): check the file and keep a copy in the backup folder as
/// "pos-imported-<time>.db", ready for `recovery_restore`.
pub fn recovery_import(app: &App, path: &Path) -> Result<RecoveryCandidate> {
    require_recovery(app, LOCAL)?;
    let summary = Pos::check_backup(path, work_path(&app.backup_dir))?;
    let (dest, _) = unique_backup_path(&app.backup_dir, "pos-imported-");
    let tmp = app.backup_dir.join(format!("upload-{}.part", unix_now()));
    let copied = std::fs::copy(path, &tmp).and_then(|_| std::fs::rename(&tmp, &dest));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_error(&e, "cannot copy the backup"));
    }
    Ok(RecoveryCandidate { file: backup_file(&dest).ok_or(PosError::NotFound)?, summary })
}

/// The database a command found damaged: note it (see [`App::note_damage`]).
fn watch_damage<T>(app: &App, cmd: &str, r: Result<T>) -> Result<T> {
    match r {
        Err(e) if is_corrupt(&e) => {
            app.note_damage(&e.to_string());
            // A backup file being checked or restored may be the damaged one: its own words.
            if BACKUP_FILE_COMMANDS.contains(&cmd) {
                return Err(e);
            }
            // The tills (and browsers) know this answer: not SQLite's words.
            Err(PosError::Validation(DATA_DAMAGED.into()))
        }
        r => r,
    }
}

/// Any command that hit damaged data answers this (the browsers show "The main
/// till needs attention"; SQLite's words are kept for support, see `note_damage`).
const DATA_DAMAGED: &str = "The POS data is damaged. Restore a backup on the main till.";
/// Commands about a backup file, whose damage is the file's, not the live data's.
const BACKUP_FILE_COMMANDS: &[&str] =
    &["inspect_backup", "restore_backup", "upload_backup_chunk", "finish_backup_upload", "recovery_check", "recovery_restore", "restore_menu_backup", "import_menu"];

/// What admins are told by `app_status.backup_alert`.
fn backup_alert(app: &App) -> Option<BackupAlert> {
    let st = app.backup_state();
    if st.damaged {
        return Some(BackupAlert {
            kind: "damaged",
            message: "The POS data is damaged. Restore a backup now (Backup & data → Restore).".into(),
            emergency_backup: None,
        });
    }
    if st.unclean_shutdown && !st.dismissed {
        let message = if st.emergency_backup.is_some() {
            "POS did not close properly last time; a backup was made."
        } else {
            "POS did not close properly last time."
        };
        return Some(BackupAlert { kind: "unclean", message: message.into(), emergency_backup: st.emergency_backup.clone() });
    }
    st.copy_error.as_ref().map(|e| BackupAlert { kind: "copy_failed", message: format!("Second copy failed: {e}"), emergency_backup: None })
}

/// On start, after the app did not close properly (`unclean`): check the
/// database; if sound, take an emergency backup (switch on) before selling;
/// if damaged, make no backup at all and tell the admins to restore.
pub fn startup_check(app: &App, unclean: bool) {
    if !unclean {
        quick_check(app);
        return;
    }
    let (problem, on) = {
        let g = app.lock();
        (g.pos.integrity_problem(), g.pos.backup_settings().map(|s| s.emergency_on).unwrap_or(true))
    };
    {
        let mut st = app.backup_state();
        st.unclean_shutdown = true;
        match &problem {
            Ok(None) => {}
            Ok(Some(p)) => {
                st.damaged = true;
                st.damaged_detail = Some(p.clone());
            }
            Err(e) => {
                st.damaged = true;
                st.damaged_detail = Some(e.to_string());
            }
        }
    }
    if matches!(problem, Ok(None)) && on {
        let made = backup_kind(app, BackupKind::Emergency);
        app.backup_state().emergency_backup = made.and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    }
}

/// Every start after a proper close: `PRAGMA quick_check` of the database
/// file through a connection of its own (the till keeps working), so data
/// damaged since is found before a backup or a sale trips over it. The full
/// `integrity_check` only runs after a crash (it takes many times longer).
pub fn quick_check(app: &App) {
    let Some(db) = app.db_path() else { return };
    let started = Instant::now();
    let found = Pos::quick_check_file(&db);
    eprintln!("startup quick_check: {:?} in {:?}", found.as_ref().map(|p| p.as_deref().unwrap_or("ok")), started.elapsed());
    match found {
        Ok(None) => {}
        Ok(Some(p)) => app.note_damage(&p),
        Err(e) => app.note_damage(&e.to_string()),
    }
}

/// The scheduled backup, when due (the worker asks every minute).
pub fn scheduled_tick(app: &App) {
    let Ok(settings) = app.lock().pos.backup_settings() else { return };
    if !settings.hourly_on {
        return;
    }
    let every = Duration::from_secs(u64::from(settings.hourly_every) * 3600);
    if app.last_scheduled.lock().unwrap_or_else(|e| e.into_inner()).elapsed() < every {
        return;
    }
    if backup_kind(app, BackupKind::Scheduled).is_some() {
        *app.last_scheduled.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }
}

/// When the app closes normally (switch on): a backup when something
/// changed. The exit handler runs this on a thread of its own and waits a
/// bounded time for it.
pub fn exit_backup(app: &App) -> Option<PathBuf> {
    let on = app.lock().pos.backup_settings().map(|s| s.exit_on).unwrap_or(false);
    if !on {
        return None;
    }
    backup_kind(app, BackupKind::Exit)
}

/// The automatic log cleanup, in batches of [`CLEANUP_BATCH`] with the lock
/// let go between them (other terminals keep working). At most every 20 h.
pub fn run_log_cleanup(app: &App) -> Option<(usize, usize)> {
    if app.backup_state().damaged {
        return None;
    }
    let now = unix_now() as i64;
    let (cutoff, days) = app.lock().pos.log_cleanup_due(now).ok()??;
    let (mut count, mut tickets) = (0, 0);
    loop {
        let batch = app.lock().pos.log_cleanup_batch(cutoff);
        match batch {
            Ok((0, 0)) => break,
            Ok((n, t)) => {
                count += n;
                tickets += t;
            }
            Err(e) => {
                eprintln!("log cleanup: {e}");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Err(e) = app.lock().pos.log_cleanup_finish(cutoff, days, count, tickets) {
        eprintln!("log cleanup: {e}");
    }
    Some((count, tickets))
}

pub fn run_job(app: &App, job: Job) {
    match job {
        Job::LogCleanup => {
            run_log_cleanup(app);
        }
        Job::Backup(kind) => {
            backup_kind(app, kind);
        }
    }
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
        let scratch = (name.starts_with("restore-work-") || name.starts_with("backup-work-")) && name.ends_with(".tmp");
        if old && ((name.starts_with("upload-") && name.ends_with(".part")) || scratch) {
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

/// Copy a database file and its WAL as they are (the damaged data, which
/// SQLite may not be able to read to copy it properly).
fn copy_with_sidecars(db: &Path, dest: &Path) -> Result<()> {
    std::fs::copy(db, dest).map_err(|e| io_error(&e, "cannot keep a copy of the damaged data"))?;
    // Its WAL beside it ("…db-wal"): SQLite reads it with the copy.
    let (mut from, mut to) = (db.as_os_str().to_owned(), dest.as_os_str().to_owned());
    from.push("-wal");
    to.push("-wal");
    if Path::new(&from).is_file() {
        std::fs::copy(&from, &to).map_err(|e| io_error(&e, "cannot keep a copy of the damaged data"))?;
    }
    Ok(())
}

fn remove_backup_with_sidecars(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
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

/// Checking for and installing a new version of the app: admins only. The
/// desktop window's `check_update` and `install_update` call this first.
pub fn require_updates(app: &App, t: &str) -> Result<()> {
    app.with_session(t, |_, s| s.require(Permission::Updates))
}

/// This PC's own settings that only an admin at the desktop window may
/// change, like the local server (e.g. "Start with Windows"). Runs `f` as
/// that admin. Browsers on the network cannot call these commands at all
/// (they are not in [`dispatch`]); any terminal but [`LOCAL`] is refused here
/// too, so one added to `dispatch` by mistake still cannot change the PC.
pub fn with_device_settings<T>(app: &App, t: &str, f: impl FnOnce(&mut Pos, &Session) -> Result<T>) -> Result<T> {
    if t != LOCAL {
        return Err(PosError::Forbidden);
    }
    app.with_session(t, |pos, s| {
        s.require(Permission::ManageSettings)?;
        f(pos, s)
    })
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

/// A manager's or admin's PIN typed to approve one change, hashed before
/// the lock is taken; left out, null or empty = none.
fn approval_pin(app: &App, pin: Option<String>) -> Option<HashedPin> {
    pin.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(|p| app.hash_pin(p))
}

/// Run command `cmd` for terminal `t` (see [`commands!`]). While the
/// database cannot be opened only the recovery commands run; a command
/// that finds the data damaged marks it so (see [`App::note_damage`]).
pub fn dispatch(app: &App, t: &str, cmd: &str, args: Value) -> Result<Value> {
    if app.opened_damaged().is_some() && !RECOVERY_COMMANDS.contains(&cmd) {
        return Err(PosError::Validation("The POS data is damaged and cannot be opened. Restore a backup on the main till.".into()));
    }
    watch_damage(app, cmd, dispatch_command(app, t, cmd, args))
}

/// Answers kept for a repeated `idempotency_key` (a till that lost the
/// answer to a payment or a dish asks again with the same key), per app,
/// terminal and command, for [`Once::KEEP`].
struct Once;

/// Per key: when, what it was for (a hash of the command, the order and its
/// arguments: [`Once::what`]) and the first answer.
type OnceEntry = (Instant, u64, Order);
static ONCE: std::sync::LazyLock<Mutex<HashMap<String, OnceEntry>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// A key that came again with another order, command or arguments: a till
/// bug or a mix-up, never a lost answer. Nothing is done.
const KEY_REUSED: &str = "This request was already used for something else (another order, amount or dish): nothing was done. Try again.";

impl Once {
    const KEEP: Duration = Duration::from_secs(15 * 60);
    const MAX: usize = 2_000;

    /// The key a repeat is known by (per app and terminal); None without a key (no check).
    fn key(app: &App, t: &str, key: Option<&str>) -> Option<String> {
        let key = key.map(str::trim).filter(|k| !k.is_empty() && k.len() <= 100)?;
        Some(format!("{:p}|{t}|{key}", app as *const App))
    }

    /// What a request is: its command, order and arguments (`Debug` of them).
    fn what(cmd: &str, order_id: i64, args: &impl std::fmt::Debug) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (cmd, order_id, format!("{args:?}")).hash(&mut h);
        h.finish()
    }

    /// The first answer for `key`, if it came already for the same request
    /// (`what`); refused ([`KEY_REUSED`]) when it came for another one.
    /// Called with the database lock held, as is [`Once::remember`], so a
    /// repeat that arrives while the first is still running waits for it.
    fn seen(key: Option<&str>, what: u64) -> Result<Option<Order>> {
        let Some(key) = key else { return Ok(None) };
        let map = ONCE.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(key).filter(|(at, _, _)| at.elapsed() < Self::KEEP) {
            Some((_, w, o)) if *w == what => Ok(Some(o.clone())),
            Some(_) => Err(PosError::Conflict(KEY_REUSED.into())),
            None => Ok(None),
        }
    }

    fn remember(key: Option<String>, what: u64, o: &Order) {
        let Some(key) = key else { return };
        let mut map = ONCE.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, (at, _, _)| at.elapsed() < Self::KEEP);
        if map.len() >= Self::MAX {
            if let Some(oldest) = map.iter().min_by_key(|(_, (at, _, _))| *at).map(|(k, _)| k.clone()) {
                map.remove(&oldest);
            }
        }
        map.insert(key, (Instant::now(), what, o.clone()));
    }
}

/// `pay_order` and `add_payment`: one payment, checked against what the
/// till showed as left to pay, taken once per `idempotency_key`. The
/// receipt prints when the last part is paid; cash opens the drawer.
#[allow(clippy::too_many_arguments)]
fn take_payment(app: &App, t: &str, cmd: &str, order_id: i64, payment: &PaymentInput, pin: Option<String>, expected_left: Option<i64>, key: Option<String>) -> Result<PayResult> {
    let pin = approval_pin(app, pin);
    let key = Once::key(app, t, key.as_deref());
    let what = Once::what(cmd, order_id, &(payment, expected_left));
    let (order, job, replayed) = app.with_session(t, |pos, s| {
        if let Some(o) = Once::seen(key.as_deref(), what)? {
            return Ok((o, None, true));
        }
        let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
        let order = pos.add_payment_checked(s, approver.as_ref(), order_id, payment, expected_left)?;
        Once::remember(key, what, &order);
        let job = pos.payment_print_job(s, &order)?;
        Ok((order, job, false))
    })?;
    Ok(PayResult { order, print: print(job), replayed })
}

/// Declares the commands and generates [`dispatch`]. Each command lists its
/// arguments (read from a JSON object with camelCase keys; `Option` ones may
/// be left out) and a body returning `Result<T>`; `app` is the [`App`] and
/// `t` the calling terminal's id.
macro_rules! commands {
    ($( $(#[doc = $doc:literal])* fn $name:ident($app:ident, $t:ident $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty $body:block )*) => {
        /// Run command `cmd` with JSON `args` for terminal `t`.
        fn dispatch_command(app: &App, t: &str, cmd: &str, args: Value) -> Result<Value> {
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
    /// Never an error because the data is damaged: the main till must
    /// still reach the recovery screen (`recovery`), so each part falls back.
    fn app_status(app, t) -> AppStatus {
        let cannot_open = app.opened_damaged();
        let g = app.lock();
        let session = g.logins.get(t).map(|l| l.session.clone());
        fn or_damaged<T>(app: &App, r: Result<T>, default: T) -> Result<T> {
            match r {
                Ok(v) => Ok(v),
                Err(e) if is_corrupt(&e) || app.backup_state().damaged => {
                    if is_corrupt(&e) {
                        app.note_damage(&e.to_string());
                    }
                    Ok(default)
                }
                Err(e) => Err(e),
            }
        }
        let open = cannot_open.is_none();
        let status = AppStatus {
            version: env!("CARGO_PKG_VERSION"),
            // Never the first-admin setup over data that is only unreadable.
            needs_setup: open && or_damaged(app, g.pos.needs_setup(), false)?,
            store_name: if open { or_damaged(app, g.pos.setting("store_name"), None)? } else { None },
            plan_width: PLAN_WIDTH,
            plan_height: PLAN_HEIGHT,
            remote: t != LOCAL,
            auto_pin: if open { or_damaged(app, g.pos.auto_login_length(), None)? } else { None },
            auto_pin_on: open && or_damaged(app, g.pos.pin_auto_login(), false)?,
            db_id: if open { or_damaged(app, g.pos.database_id(), String::new())? } else { String::new() },
            tickets: match &session {
                Some(s) if open => or_damaged(app, g.pos.ticket_badges(s).map(Some), None)?,
                _ => None,
            },
            backup_alert: session.as_ref().filter(|s| s.role == Role::Admin).and_then(|_| backup_alert(app)),
            recovery: None,
            session,
        };
        drop(g);
        let st = app.backup_state().clone();
        let recovery = (t == LOCAL && st.damaged).then(|| match &cannot_open {
            Some(why) => RecoveryInfo {
                reason: "cannot_open",
                detail: Some(why.clone()),
                message: "The POS data is damaged and cannot be opened. Restore a backup to carry on.".into(),
                can_log_in: false,
            },
            None => RecoveryInfo {
                reason: "damaged",
                detail: st.damaged_detail.clone(),
                message: "The POS data is damaged. Restore a backup to carry on.".into(),
                can_log_in: true,
            },
        });
        Ok(AppStatus { recovery, ..status })
    }

    // ---- recovery (main till only, while the data is damaged) ----

    /// The recovery screen: why, and the full backups in the folder, newest first.
    fn recovery_status(app, t) -> Value {
        require_recovery(app, t)?;
        let st = app.backup_state().clone();
        Ok(serde_json::json!({
            "cannot_open": app.opened_damaged().is_some(),
            "detail": st.damaged_detail,
            "backups": recovery_backups(app),
            "folder": app.backup_dir.display().to_string(),
        }))
    }

    /// Check one backup by name, or (no name) find the newest good one: the
    /// newest backups are checked one by one (as for a restore: integrity,
    /// POS tables, version, an admin) until one passes.
    fn recovery_check(app, t, name: Option<String>) -> RecoveryCandidate {
        require_recovery(app, t)?;
        let names: Vec<String> = match name {
            Some(n) if is_full_backup_name(&n) => vec![n],
            Some(_) => return Err(PosError::Validation("that is not a backup".into())),
            None => recovery_backups(app).into_iter().take(RECOVERY_TRIES).map(|b| b.name).collect(),
        };
        let mut last = None;
        let mut found = None;
        for n in &names {
            let path = app.backup_dir.join(n);
            match Pos::check_backup(&path, work_path(&app.backup_dir)) {
                Ok(summary) => {
                    found = Some(RecoveryCandidate { file: backup_file(&path).ok_or(PosError::NotFound)?, summary });
                    break;
                }
                Err(e) => {
                    eprintln!("recovery: {n}: {e}");
                    last = Some(e);
                }
            }
        }
        found.ok_or_else(|| match last {
            Some(e) if names.len() == 1 => e,
            _ => PosError::Validation(format!("No good backup was found among the newest {}. Choose a backup file (a USB stick or the second folder).", names.len())),
        })
    }

    /// Restore a backup from the recovery screen, without logging in: RESTORE
    /// typed and an admin PIN. The PIN is checked against the backup's own
    /// admins (the damaged data's staff list may be unreadable, and the
    /// backup's admins are who run the till after), or the damaged data's
    /// admins while still readable. As a normal restore: the backup is
    /// checked first, the damaged data is kept as "pos-before-restore-…".
    fn recovery_restore(app, t, name: String, pin: String, confirm: String) -> RecoveryRestoreResult {
        require_recovery(app, t)?;
        if confirm.trim() != RESTORE_WORD {
            return Err(PosError::Validation(format!("type {RESTORE_WORD} to confirm")));
        }
        if !is_full_backup_name(&name) {
            return Err(PosError::Validation("that is not a backup".into()));
        }
        app.recovery_throttle.lock().unwrap_or_else(|e| e.into_inner()).check(Instant::now())?;
        let src = app.backup_dir.join(&name);
        if !src.is_file() {
            return Err(PosError::NotFound);
        }
        let _gate = app.backup_gate();
        let db = app.db_path().ok_or_else(|| PosError::Internal("no database path".into()))?;
        let cannot_open = app.opened_damaged().is_some();
        let mut g = app.lock();
        let inner = &mut *g;
        let prepared = if cannot_open {
            Pos::prepare_restore_file(&src, work_path(&app.backup_dir))?
        } else {
            inner.pos.prepare_restore(&src, work_path(&app.backup_dir))?
        };
        let current = if cannot_open { None } else { inner.pos.admin_with_pin(&pin).ok().flatten() };
        let (by, pin_from) = match (current, prepared.admin_with_pin(&pin)?) {
            (_, Some(n)) => (n, "backup"),
            (Some(n), None) => (n, "current"),
            (None, None) => {
                app.recovery_throttle.lock().unwrap_or_else(|e| e.into_inner()).record_failure(Instant::now());
                return Err(PosError::Validation("That is not the PIN of an admin in this backup.".into()));
            }
        };
        app.recovery_throttle.lock().unwrap_or_else(|e| e.into_inner()).record_success();
        // The damaged data is kept, as it is, in case anything can be saved from it.
        let (safety_path, safety_copy) = unique_backup_path(&app.backup_dir, "pos-before-restore-");
        let kept = if cannot_open { copy_with_sidecars(&db, &safety_path) } else { inner.pos.backup_unchecked(&safety_path).or_else(|_| copy_with_sidecars(&db, &safety_path)) };
        if let Err(e) = kept {
            remove_backup_with_sidecars(&safety_path);
            return Err(disk_full(e));
        }
        let summary = if cannot_open {
            prepared.install_as(&db)?;
            let pos = Pos::open(&db)?;
            let detail = restore_audit_detail(&prepared.summary, &format!("{by} (recovery screen, file {name})"));
            if let Err(e) = pos.audit_restored(&detail) {
                eprintln!("recovery: audit: {e}");
            }
            let summary = prepared.summary.clone();
            drop(prepared);
            inner.pos = pos;
            summary
        } else {
            match inner.pos.restore_by(&format!("{by} (recovery screen, file {name})"), prepared) {
                Ok(summary) => summary,
                Err(e) => {
                    remove_backup_with_sidecars(&safety_path);
                    return Err(e);
                }
            }
        };
        inner.log_everyone_out();
        *app.hasher.lock().unwrap_or_else(|e| e.into_inner()) = inner.pos.pin_hasher();
        let keep = inner.pos.backups_kept().unwrap_or(BACKUPS_KEPT_DEFAULT);
        drop(g);
        *app.cannot_open.lock().unwrap_or_else(|e| e.into_inner()) = None;
        {
            let mut st = app.backup_state();
            st.damaged = false;
            st.damaged_detail = None;
        }
        *app.backed_up_changes.lock().unwrap_or_else(|e| e.into_inner()) = None;
        prune_backups(&app.backup_dir, keep);
        Ok(RecoveryRestoreResult { restored: RestoreResult { summary, restored: name, safety_copy }, pin_from, restart_needed: cannot_open })
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
    /// The PIN is hashed before the lock is taken (other terminals go on
    /// meanwhile); a locked-out terminal is told at once, without hashing.
    fn login(app, t, pin: String, user_id: Option<i64>) -> Session {
        {
            let g = app.lock();
            if t == LOCAL { &g.local_throttle } else { &g.network_throttle }.check(Instant::now())?;
        }
        let pin = app.hash_pin(&pin);
        let device = app.device(t);
        let mut g = app.lock();
        let inner = &mut *g;
        inner.pos.set_terminal_device(Some(t), device.as_deref());
        let s = if t == LOCAL {
            let mut throttle = std::mem::take(&mut inner.local_throttle);
            let r = inner.pos.login_hashed(user_id, &pin, &mut throttle);
            inner.local_throttle = throttle;
            r
        } else {
            inner.pos.login_hashed(user_id, &pin, &mut inner.network_throttle)
        };
        inner.pos.set_terminal(None);
        let s = s?;
        g.sign_in(t, &s);
        Ok(s)
    }

    fn logout(app, t) -> () {
        let device = app.device(t);
        let mut g = app.lock();
        if let Some(l) = g.logins.remove(t) {
            g.pos.set_terminal_device(Some(t), device.as_deref());
            let r = g.pos.logout(&l.session);
            g.pos.set_terminal(None);
            r?;
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

    /// `fields`: only these details change (the ones edited on this till);
    /// left out, all of them, as before.
    fn update_order_details(app, t, id: i64, details: OrderDetails, fields: Option<Vec<String>>) -> Order {
        app.with_session(t, |pos, s| pos.update_order_details_fields(s, id, &details, fields.as_deref()))
    }

    /// Where an order that no longer exists went: the open bill it was
    /// joined or moved onto (None when it is not known or not open).
    fn order_moved_to(app, t, order_id: i64) -> Option<Order> {
        app.with_session(t, |pos, s| pos.moved_to(s, order_id))
    }

    fn move_order(app, t, id: i64, table_id: i64) -> Order {
        app.with_session(t, |pos, s| pos.move_order(s, id, table_id))
    }

    /// `starter`: order a starter as a main (false) or any dish as a starter
    /// (true); left out = the menu's setting. `team_id`: the kitchen team
    /// that makes it ("Which team makes this?"); left out = the menu's.
    /// `idempotency_key`: a random id per tap; the same key again (the till
    /// did not hear the answer) adds nothing and answers as the first time.
    fn add_item(app, t, order_id: i64, product_id: i64, qty: i64, note: Option<String>, variant_id: Option<i64>, option_ids: Option<Vec<i64>>, starter: Option<bool>, team_id: Option<i64>, idempotency_key: Option<String>) -> Order {
        let option_ids = option_ids.unwrap_or_default();
        let key = Once::key(app, t, idempotency_key.as_deref());
        let what = Once::what("add_item", order_id, &(product_id, qty, &note, variant_id, &option_ids, starter, team_id));
        app.with_session(t, |pos, s| {
            if let Some(o) = Once::seen(key.as_deref(), what)? {
                return Ok(o);
            }
            let o = pos.add_item_for(s, order_id, product_id, qty, note.as_deref(), variant_id, &option_ids, starter, team_id)?;
            Once::remember(key, what, &o);
            Ok(o)
        })
    }

    fn set_item_course(app, t, item_id: i64, starter: bool) -> Order {
        app.with_session(t, |pos, s| pos.set_item_course(s, item_id, starter))
    }

    /// Move a dish not yet sent to another kitchen team (None = its menu team).
    fn set_item_team(app, t, item_id: i64, team_id: Option<i64>) -> Order {
        app.with_session(t, |pos, s| pos.set_item_team(s, item_id, team_id))
    }

    /// Prints that failed after the till was told they were sent (a ticket
    /// sent slowly, a later copy), after number `after`; none = the last
    /// two minutes'. Every till shows them.
    fn print_failures(app, t, after: Option<u64>) -> PrintFailures {
        app.with_session(t, |_, s| s.require(Permission::Sell))?;
        Ok(pos_core::print_failures(after))
    }

    /// The teams "Which team makes this?" offers (empty = off).
    fn team_choice(app, t) -> Vec<i64> {
        app.with_session(t, |pos, s| {
            s.require(Permission::Sell)?;
            pos.team_choice()
        })
    }

    /// A dish not on the menu: name and price typed on the order. Staff
    /// pass `pin`, a manager's or admin's PIN approving it (it only
    /// approves; nobody is logged in or out).
    fn add_open_item(app, t, order_id: i64, name: String, price_cents: i64, qty: Option<i64>, note: Option<String>, starter: Option<bool>, pin: Option<String>) -> Order {
        let pin = approval_pin(app, pin);
        app.with_session(t, |pos, s| {
            let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
            pos.add_open_item_approved(s, approver.as_ref(), order_id, &name, price_cents, qty.unwrap_or(1), note.as_deref(), starter.unwrap_or(false))
        })
    }

    /// A change to a dish already sent tells the kitchen: fewer or removed
    /// prints CANCELLED (with the set's dishes for a set), a new note CHANGED.
    /// `pin`: a manager's or admin's PIN approving a discount over the staff
    /// limit, or staff reducing or removing a dish already sent (it only
    /// approves; nobody is logged in or out).
    ///
    /// `expected_qty` / `expected_discount_cents`: what the till showed; the
    /// line now differs = refused, "This dish changed on another till".
    /// `qty_delta`: +1 / -1 from what the line has now (the order screen's
    /// + and −); `update`'s own quantity, discount and note are then ignored.
    fn update_item(app, t, item_id: i64, update: ItemUpdate, pin: Option<String>, order_id: Option<i64>, expected_qty: Option<i64>, expected_discount_cents: Option<i64>, qty_delta: Option<i64>) -> Order {
        let pin = pin.as_deref().map(|p| app.hash_pin(p));
        let check = ItemCheck { expected_qty, expected_discount_cents, qty_delta };
        let (order, job) = app.with_session(t, |pos, s| {
            // The till says which order it shows: the dish must still be on it.
            if let Some(o) = order_id {
                pos.check_item_on_order(o, item_id)?;
            }
            let update = pos.resolve_item_update(item_id, &update, &check)?;
            let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
            pos.update_item_for_kitchen_approved(s, approver.as_ref(), item_id, &update)
        })?;
        // The change is saved either way; a printer problem must not undo it.
        let _ = print(job);
        Ok(order)
    }

    /// Change one line's price each. `pin`: a manager's or admin's PIN when
    /// staff change it (it only approves; nobody is logged in or out).
    fn set_item_price(app, t, item_id: i64, price_cents: i64, pin: Option<String>, order_id: Option<i64>) -> Order {
        let pin = pin.as_deref().map(|p| app.hash_pin(p));
        app.with_session(t, |pos, s| {
            if let Some(o) = order_id {
                pos.check_item_on_order(o, item_id)?;
            }
            let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
            pos.set_item_price(s, approver.as_ref(), item_id, price_cents)
        })
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
    /// `pin`: a manager's or admin's PIN when staff move them onto a bill
    /// with a lower service charge or tax (it only approves).
    fn move_items(app, t, order_id: i64, table_id: i64, items: Vec<(i64, i64)>, pin: Option<String>) -> Order {
        let pin = approval_pin(app, pin);
        app.with_session(t, |pos, s| {
            let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
            pos.move_items_approved(s, approver.as_ref(), order_id, table_id, &items)
        })
    }

    /// Join this order onto another table's bill (or just move there).
    /// `pin`: a manager's or admin's PIN when staff join a bill with a lower
    /// service charge or tax (it only approves).
    fn merge_into_table(app, t, order_id: i64, table_id: i64, pin: Option<String>) -> Order {
        let pin = approval_pin(app, pin);
        app.with_session(t, |pos, s| {
            let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
            pos.merge_into_table_approved(s, approver.as_ref(), order_id, table_id)
        })
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

    /// "Test Thai printing": the Thai sample in every image mode on one page,
    /// for any printer form (kitchen, receipt, other kitchen printers).
    fn test_thai_printer(app, t, config: PrinterConfig) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.thai_test_job(s, &config))?;
        Ok(print(Some(job)))
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

    /// `pin`: a manager's or admin's PIN when staff pay by Other on an
    /// order not from a delivery app.
    /// `expected_left_cents`: what the till showed as left to pay; another
    /// till changed the bill since = refused, "The bill changed on another
    /// till" (nothing taken). `idempotency_key`: a random id per confirm;
    /// the same key again answers as the first time and takes nothing.
    fn pay_order(app, t, order_id: i64, method: PaymentMethod, tendered_cents: i64, pin: Option<String>, expected_left_cents: Option<i64>, idempotency_key: Option<String>) -> PayResult {
        let payment = PaymentInput { method, amount_cents: None, tendered_cents, tip_cents: 0, items: None };
        take_payment(app, t, "pay_order", order_id, &payment, pin, expected_left_cents, idempotency_key)
    }

    /// One payment: the rest, an amount, or chosen dishes, with a tip.
    /// The receipt prints when the last part is paid; cash opens the drawer.
    /// `pin`: a manager's or admin's PIN when staff pay by Other on an
    /// order not from a delivery app (it only approves).
    /// `expected_left_cents` and `idempotency_key`: as for `pay_order`.
    fn add_payment(app, t, order_id: i64, payment: PaymentInput, pin: Option<String>, expected_left_cents: Option<i64>, idempotency_key: Option<String>) -> PayResult {
        take_payment(app, t, "add_payment", order_id, &payment, pin, expected_left_cents, idempotency_key)
    }

    /// Take back a part payment. Admins and managers need no PIN; staff
    /// pass `pin`, a manager's or admin's PIN approving it (it only
    /// approves; nobody is logged in or out). Logged with who approved it.
    fn remove_payment(app, t, order_id: i64, payment_id: i64, pin: Option<String>) -> Order {
        let pin = approval_pin(app, pin);
        app.with_session(t, |pos, s| {
            let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
            pos.remove_payment_approved(s, approver.as_ref(), order_id, payment_id)
        })
    }

    /// Cancel an open order. Admins and managers confirm with no PIN
    /// (`pin` left out or empty); anyone else needs a PIN, whose owner
    /// cancels it (an order already in the kitchen needs a manager's or
    /// admin's PIN). The user logged in here counts as the PIN's owner when
    /// it is theirs, even if they share it. Dishes still cooking get a
    /// CANCELLED ticket. See `Pos::cancel_order_confirmed`.
    fn cancel_order(app, t, order_id: i64, reason: String, pin: Option<String>) -> PrintOutcome {
        let pin = pin.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(|p| app.hash_pin(p));
        let job = app.with_session(t, |pos, s| pos.cancel_order_confirmed_hashed(s, order_id, &reason, pin.as_ref()))?;
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

    /// Also cancels open orders left empty for over half an hour, at most
    /// every ten minutes (every till asks for this often).
    fn business_day(app, t) -> BusinessDay {
        app.with_session(t, |pos, s| {
            if let Err(e) = pos.sweep_empty_orders_if_due() {
                eprintln!("empty orders: {e}");
            }
            pos.business_day(s)
        })
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
        // In the background on the main till: the day close backup (switch
        // on) and the log cleanup (at most every 20 hours).
        if app.lock().pos.backup_settings().is_ok_and(|b| b.dayclose_on) {
            app.request(Job::Backup(BackupKind::DayClose));
        }
        app.request(Job::LogCleanup);
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

    /// Every terminal logged in as this user gets the new name and role at
    /// once (app_status shows them), and is logged out if they can no longer log in.
    fn update_user(app, t, id: i64, name: String, role: Role, active: bool) -> User {
        app.with_session_inner(t, |inner, s| {
            let user = inner.pos.update_user(s, id, &name, role, active)?;
            inner.refresh_logins_of(id);
            Ok(user)
        })
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

    /// The service charge on one order. Removing or lowering it: staff pass
    /// `pin`, a manager's or admin's PIN approving it.
    fn set_service_charge(app, t, order_id: i64, rate_bp: i64, pin: Option<String>) -> Order {
        let pin = approval_pin(app, pin);
        app.with_session(t, |pos, s| {
            let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
            pos.set_service_charge_approved(s, approver.as_ref(), order_id, rate_bp)
        })
    }

    /// One tax rate for the whole order; null = each item's menu rate.
    /// A rate of the order's own, or menu rates that lower the tax on a
    /// dish: staff pass `pin`, a manager's or admin's PIN approving it.
    fn set_order_tax(app, t, order_id: i64, rate_bp: Option<i64>, pin: Option<String>) -> Order {
        let pin = approval_pin(app, pin);
        app.with_session(t, |pos, s| {
            let approver = pin.as_ref().map(|p| pos.pin_owner_hashed(p, Some(s.user_id))).transpose()?;
            pos.set_order_tax_approved(s, approver.as_ref(), order_id, rate_bp)
        })
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
        app.with_session(t, |_, s| s.require(Permission::Backup))?;
        if app.backup_state().damaged {
            return Err(PosError::Validation(DAMAGED_NO_BACKUP.into()));
        }
        let path = backup_path(&app.backup_dir);
        let made = app.with_session(t, |pos, s| {
            let changes = pos.change_count().ok();
            pos.backup_to(s, &path)?;
            Pos::verify_backup_file(&path)?;
            *app.backed_up_changes.lock().unwrap_or_else(|e| e.into_inner()) = changes;
            pos.backup_settings()
        });
        let settings = match made {
            Ok(s) => s,
            Err(e) => {
                remove_backup_with_sidecars(&path);
                // The copy failed because the data itself is damaged: say
                // that, not that "this backup file is damaged".
                if let Some(why) = app.db_path().and_then(|db| damage_behind(&db, &e)) {
                    app.note_damage(&why);
                    return Err(PosError::Validation(DAMAGED_NO_BACKUP.into()));
                }
                return Err(e);
            }
        };
        {
            let mut st = app.backup_state();
            st.last_backup = path.file_name().map(|n| n.to_string_lossy().into_owned());
            st.last_backup_at = Some(unix_now());
            st.last_backup_kind = Some(BackupKind::Manual);
        }
        prune_all(&app.backup_dir, settings.kept, settings.hourly_keep);
        second_copy(app, &settings, &path);
        Ok(path.display().to_string())
    }

    /// Backup & data: the backup switches, the second folder and what the
    /// automatic backups did (admins).
    fn backup_status(app, t) -> BackupStatusView {
        let settings = app.with_session(t, |pos, s| {
            s.require(Permission::Backup)?;
            pos.backup_settings()
        })?;
        let state = app.backup_state().clone();
        let copy_dir_ok = settings.copy_dir.as_deref().map(|d| Path::new(d).is_dir());
        Ok(BackupStatusView { settings, state, copy_dir_ok, can_pick_folder: t == LOCAL })
    }

    /// Hide the "did not close properly" note (admins). A damaged database
    /// or a failed second copy keeps showing until it is put right.
    fn dismiss_backup_alert(app, t) -> () {
        app.with_session(t, |_, s| s.require(Permission::Backup))?;
        app.backup_state().dismissed = true;
        Ok(())
    }

    /// Turn the second backup folder off and forget it (admins).
    fn clear_backup_copy_folder(app, t) -> () {
        app.with_session(t, |pos, s| {
            pos.set_backup_copy_dir(s, None)?;
            pos.set_setting(s, BACKUP_COPY_ON_KEY, "0")
        })?;
        let mut st = app.backup_state();
        st.copy_error = None;
        st.copy_error_at = None;
        Ok(())
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
        // Never while the automatic backup is copying the data being replaced.
        let _gate = app.backup_gate();
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
            // The data is replaced: sessions from the old staff list are void,
            // and PINs are hashed with the backup's salt.
            inner.log_everyone_out();
            *app.hasher.lock().unwrap_or_else(|e| e.into_inner()) = inner.pos.pin_hasher();
            let keep = inner.pos.backups_kept().unwrap_or(BACKUPS_KEPT_DEFAULT);
            Ok((RestoreResult { summary, restored: name, safety_copy }, keep))
        })?;
        {
            // The data is replaced by a checked backup: no longer damaged,
            // and the next automatic backup has something new to save.
            let mut st = app.backup_state();
            st.damaged = false;
            st.damaged_detail = None;
        }
        *app.backed_up_changes.lock().unwrap_or_else(|e| e.into_inner()) = None;
        prune_backups(&app.backup_dir, keep);
        Ok(restored)
    }

    // ---- tickets ("Contact admin") ----

    /// Open a ticket (anyone logged in). The server adds who, the terminal,
    /// the order's table, the version and recent print failures.
    fn create_ticket(app, t, category: String, urgency: Option<String>, body: String, order_id: Option<i64>, screen: Option<String>, screen_label: Option<String>, device: Option<String>) -> CreatedTicket {
        let input = NewTicket { category, urgency, body, order_id, screen, screen_label, device };
        app.with_session_inner(t, |inner, s| {
            let ticket = inner.pos.create_ticket(s, &input)?;
            let ids: Vec<i64> = inner
                .logins
                .iter()
                .filter(|(k, l)| k.as_str() == LOCAL || l.last_used.elapsed() < IDLE_LOGOUT)
                .map(|(_, l)| l.session.user_id)
                .collect();
            let answerers_online = inner.pos.answerers_among(&ids)?;
            let call_text = inner.pos.setting(TICKETS_CALL_TEXT_KEY)?.filter(|c| !c.trim().is_empty());
            Ok(CreatedTicket { ticket, answerers_online, call_text })
        })
    }

    /// `tab`: "mine" (anyone), or "open" / "waiting" / "done" (managers and
    /// admins; staff get "mine").
    fn list_tickets(app, t, tab: String, limit: Option<i64>, offset: Option<i64>) -> Vec<TicketRow> {
        app.with_session(t, |pos, s| pos.list_tickets(s, &tab, limit.unwrap_or(100), offset.unwrap_or(0)))
    }

    /// One ticket with its messages; marks it read. "not found" when the
    /// caller may not see it.
    fn get_ticket(app, t, id: i64) -> Ticket {
        app.with_session(t, |pos, s| pos.get_ticket(s, id))
    }

    fn reply_ticket(app, t, id: i64, body: String, set_status: Option<String>) -> Ticket {
        app.with_session(t, |pos, s| pos.reply_ticket(s, id, &body, set_status.as_deref()))
    }

    fn set_ticket_status(app, t, id: i64, status: String) -> Ticket {
        app.with_session(t, |pos, s| pos.set_ticket_status(s, id, &status))
    }

    /// Admins only.
    fn delete_ticket(app, t, id: i64) -> () {
        app.with_session(t, |pos, s| pos.delete_ticket(s, id))
    }

    // ---- activity log ----

    /// One page of the Activity log (managers and admins), newest first.
    fn list_activity(app, t, from: Option<i64>, to: Option<i64>, user_id: Option<i64>, kind: Option<String>, look: Option<bool>, q: Option<String>, before: Option<ActivityCursor>, limit: Option<usize>) -> ActivityPage {
        let f = ActivityFilter { from, to, user_id, kind, look, q };
        app.with_session(t, |pos, s| pos.list_activity(s, &f, before, limit))
    }

    /// An order's rows, oldest first (History detail; managers and admins).
    fn order_activity(app, t, order_id: i64) -> Vec<ActivityRow> {
        app.with_session(t, |pos, s| pos.order_activity(s, order_id))
    }

    fn activity_staff(app, t) -> Vec<ActivityStaff> {
        app.with_session(t, |pos, s| pos.activity_staff(s))
    }

    /// Delete chosen rows (admins, with their PIN).
    fn delete_activity(app, t, ids: Vec<i64>, pin: String) -> DeletedCount {
        app.with_session(t, |pos, s| Ok(DeletedCount { deleted: pos.delete_activity(s, &ids, &pin)? }))
    }

    /// What clearing before `before` would delete (admins).
    fn activity_count(app, t, before: i64) -> ActivityCount {
        app.with_session(t, |pos, s| pos.activity_count(s, before))
    }

    /// Delete every row older than `before` (admins, PIN and DELETE typed).
    /// `csv`: a CSV copy was saved first (recorded in the log).
    fn clear_activity(app, t, before: i64, pin: String, confirm: String, csv: Option<bool>) -> DeletedCount {
        app.with_session(t, |pos, s| Ok(DeletedCount { deleted: pos.clear_activity(s, before, &pin, &confirm, csv.unwrap_or(false))? }))
    }

    /// The rows of a filter, or every row older than `before`, as CSV
    /// (admins). `save`: on the main till, also write it to Downloads.
    fn export_activity(app, t, from: Option<i64>, to: Option<i64>, user_id: Option<i64>, kind: Option<String>, look: Option<bool>, q: Option<String>, before: Option<i64>, save: Option<bool>) -> ExportedCsv {
        let f = ActivityFilter { from, to, user_id, kind, look, q };
        // A big log takes seconds to write out: it is read on a connection of its
        // own (WAL: one consistent snapshot), so the tills are not held up meanwhile.
        // In memory (tests): on the shared one, as before.
        let s = app.with_session(t, |_, s| s.require(Permission::Delete).map(|_| s.clone()))?;
        let (csv, rows) = match app.db_path() {
            Some(db) => Pos::open(&db)?.export_activity(&s, &f, before)?,
            None => app.with_session(t, |pos, s| pos.export_activity(s, &f, before))?,
        };
        let file_name = format!("activity-{}.csv", chrono_today());
        let path = if save == Some(true) && t == LOCAL {
            let (mut path, mut n) = (downloads_dir(&app.backup_dir).join(&file_name), 1);
            while path.exists() {
                n += 1;
                path = downloads_dir(&app.backup_dir).join(format!("activity-{}-{n}.csv", chrono_today()));
            }
            std::fs::write(&path, csv.as_bytes()).map_err(|e| io_error(&e, "cannot save the CSV file"))?;
            Some(path.display().to_string())
        } else {
            None
        };
        Ok(ExportedCsv { csv, rows, file_name, path })
    }

    /// "Keep for" and the log's size (admins).
    fn log_retention(app, t) -> LogRetention {
        app.with_session(t, |pos, s| pos.log_retention(s))
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
    fn changing_yourself_updates_your_session_at_once() {
        let app = app();
        let tablet = "web-0123456789abcdef0123456789abcdef";
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        dispatch(&app, tablet, "login", json!({ "pin": "1111" })).unwrap();
        dispatch(&app, "web-ffffffffffffffffffffffffffffffff", "login", json!({ "pin": "2222" })).unwrap();
        let me = dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["session"]["user_id"].as_i64().unwrap();
        // A second admin, so the owner may step down.
        dispatch(&app, LOCAL, "create_user", json!({ "name": "Ann", "role": "admin", "pin": "3333" })).unwrap();
        let status = |t| dispatch(&app, t, "app_status", Value::Null).unwrap()["session"].clone();
        dispatch(&app, LOCAL, "update_user", json!({ "id": me, "name": "Boss", "role": "manager", "active": true })).unwrap();
        // Both terminals logged in as the owner see the new name and role without another call.
        for t in [LOCAL, tablet] {
            let s = status(t);
            assert_eq!((s["name"].clone(), s["role"].clone()), (json!("Boss"), json!("manager")), "{t}");
        }
        assert!(matches!(dispatch(&app, LOCAL, "list_users", json!({})), Err(PosError::Forbidden)));
        // Other people's logins are left as they were.
        assert_eq!(status("web-ffffffffffffffffffffffffffffffff")["name"], "Sam");
        // An admin turning themselves off is logged out everywhere.
        dispatch(&app, LOCAL, "logout", json!({})).unwrap();
        dispatch(&app, LOCAL, "login", json!({ "pin": "3333" })).unwrap();
        let ann = status(LOCAL)["user_id"].as_i64().unwrap();
        dispatch(&app, tablet, "logout", json!({})).unwrap();
        dispatch(&app, tablet, "login", json!({ "pin": "3333" })).unwrap();
        dispatch(&app, LOCAL, "update_user", json!({ "id": me, "name": "Boss", "role": "admin", "active": true })).unwrap();
        dispatch(&app, LOCAL, "update_user", json!({ "id": ann, "name": "Ann", "role": "admin", "active": false })).unwrap();
        assert!(status(LOCAL).is_null());
        assert!(status(tablet).is_null());
        assert!(matches!(dispatch(&app, LOCAL, "list_users", json!({})), Err(PosError::NotLoggedIn)));
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
    fn start_with_windows_is_for_an_admin_at_the_desktop_window_only() {
        let app = app();
        let tablet = "web-0123456789abcdef0123456789abcdef";
        let ran = std::cell::Cell::new(0);
        let change = |t| with_device_settings(&app, t, |_, _| { ran.set(ran.get() + 1); Ok(()) });
        // Nobody logged in.
        assert!(matches!(change(LOCAL), Err(PosError::NotLoggedIn)));
        // Staff at the till.
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        assert!(matches!(change(LOCAL), Err(PosError::Forbidden)));
        // An admin in a browser on the network: refused, and the commands do not exist there.
        dispatch(&app, tablet, "login", json!({ "pin": "1111" })).unwrap();
        assert!(matches!(change(tablet), Err(PosError::Forbidden)));
        for cmd in ["get_autostart", "set_autostart"] {
            for t in [tablet, LOCAL] {
                let err = dispatch(&app, t, cmd, json!({ "enabled": true })).unwrap_err();
                assert_eq!(err.to_string(), format!("unknown command {cmd}"));
            }
        }
        assert_eq!(ran.get(), 0);
        // An admin at the till.
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        change(LOCAL).unwrap();
        assert_eq!(ran.get(), 1);
    }

    #[test]
    fn cancelling_with_a_shared_pin_on_your_own_till_and_the_kitchen_is_told() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        let mia = pos.create_user(&admin, "Mia", Role::Manager, "2222").unwrap();
        let curry = pos
            .create_product(&admin, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 900, cost_cents: 0, tax_rate_bp: Some(0),
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap();
        let path = std::env::temp_dir().join(format!("pos-cancel-test-{}.bin", std::process::id()));
        pos.set_kitchen_printer(&admin, &PrinterConfig {
            target: PrinterTarget::File { path: path.display().to_string() }, copy_delay_secs: 0, app_font: false, slow_send: false, ..Default::default()
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
        // The File printer writes on a background thread: let it finish first.
        pos_core::wait_for_prints(Duration::from_secs(10));
        let printed = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).to_string();
        let _ = std::fs::remove_file(&path);
        assert!(printed.contains("CANCELLED") && printed.contains("Green curry"), "{printed}");
        assert_eq!(dispatch(&app, LOCAL, "get_order", json!({ "id": id })).unwrap()["status"], "cancelled");
        // Nothing in the kitchen: nothing printed.
        let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
        let r = dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": o["id"], "reason": "" })).unwrap();
        assert_eq!(r, json!({ "attempted": false, "error": null }));
    }

    /// "Send slowly" paces the bytes after the command has released the
    /// database lock: the till gets its answer at once and other terminals
    /// carry on while the ticket is still going out.
    #[test]
    fn slow_sending_happens_outside_the_lock() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        let curry = pos
            .create_product(&admin, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 900, cost_cents: 0, tax_rate_bp: Some(0),
                stock_qty: 0, track_stock: false, starter: false, name_alt: Some("แกงเขียวหวาน".into()), prep: false, team_id: None,
            })
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let printer = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut s, &mut buf).unwrap();
            (buf, Instant::now())
        });
        // Defaults: the app's font (a ticket of tens of KB) and "Send slowly" on.
        pos.set_kitchen_printer(&admin, &PrinterConfig { target: PrinterTarget::Network { host: "127.0.0.1".into(), port }, ..Default::default() })
            .unwrap();
        let app = App::new(pos, std::env::temp_dir());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
        let id = o["id"].as_i64().unwrap();
        dispatch(&app, LOCAL, "add_item", json!({ "orderId": id, "productId": curry.id, "qty": 2 })).unwrap();
        let started = Instant::now();
        let r = dispatch(&app, LOCAL, "send_to_kitchen", json!({ "orderId": id })).unwrap();
        assert_eq!(r["print"], json!({ "attempted": true, "error": null }));
        assert!(started.elapsed() < Duration::from_millis(300), "the till waited {:?}", started.elapsed());
        // Still sending, yet the lock is free and another terminal's command runs at once.
        assert!(app.inner.try_lock().is_ok());
        let other = Instant::now();
        dispatch(&app, LOCAL, "list_open_orders", json!({})).unwrap();
        assert!(other.elapsed() < Duration::from_millis(200));
        assert!(!printer.is_finished(), "the ticket is still going out");
        let (bytes, done) = printer.join().unwrap();
        assert!(bytes.starts_with(&[0x1B, b'@']) && bytes.ends_with(&[0x1D, b'V', 66, 0]));
        assert!(bytes.len() > 10 * pos_core::SLOW_CHUNK, "{} bytes", bytes.len());
        assert!(done - started >= pos_core::paced_duration(bytes.len()));
    }

    #[test]
    fn the_cancel_command_needs_a_pin_from_staff_only() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        let curry = pos
            .create_product(&admin, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 900, cost_cents: 0, tax_rate_bp: Some(0),
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
        // Staff: no PIN (left out or empty) is refused once the kitchen has it;
        // a mistake order (nothing sent or paid) is cancelled with just a confirm.
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        let sent = sent_order();
        for args in [json!({ "orderId": sent, "reason": "" }), json!({ "orderId": sent, "reason": "", "pin": "" }), json!({ "orderId": sent, "reason": "", "pin": null })] {
            assert!(matches!(dispatch(&app, LOCAL, "cancel_order", args), Err(PosError::Forbidden)));
        }
        assert_eq!(status(&sent), "open");
        for pin in [json!(null), json!("")] {
            let mistake = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap()["id"].clone();
            dispatch(&app, LOCAL, "add_item", json!({ "orderId": mistake, "productId": curry.id, "qty": 1 })).unwrap();
            dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": mistake, "reason": "", "pin": pin })).unwrap();
            assert_eq!(status(&mistake), "cancelled");
        }
        let unsent = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap()["id"].clone();
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
        // The File printer writes on a background thread: let it finish first.
        pos_core::wait_for_prints(Duration::from_secs(10));
        let printed = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).to_string();
        let _ = std::fs::remove_file(&path);
        assert!(printed.contains("CANCELLED") && printed.contains("Green curry"), "{printed}");
        // CANCELLED ticket switched off (a manager may): cancelled, nothing printed.
        dispatch(&app, LOCAL, "login", json!({ "pin": "4646" })).unwrap();
        dispatch(&app, LOCAL, "set_setting", json!({ "key": "print_cancelled_order", "value": "0" })).unwrap();
        let sent = sent_order();
        let r = dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": sent, "reason": "" })).unwrap();
        assert_eq!(r, json!({ "attempted": false, "error": null }));
        assert_eq!(status(&sent), "cancelled");
        // The File printer writes on a background thread: let it finish first.
        pos_core::wait_for_prints(Duration::from_secs(10));
        let printed = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).to_string();
        let _ = std::fs::remove_file(&path);
        assert!(printed.contains("NEW ORDER") && !printed.contains("CANCELLED"), "only the order went to the kitchen: {printed}");
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
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: Some(0),
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

    #[test]
    fn staff_need_a_manager_pin_for_sent_dishes_open_items_other_payments_and_the_limit() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        let curry = pos
            .create_product(&admin, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: Some(0),
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap();
        let app = App::new(pos, std::env::temp_dir());
        let new_order = |app: &App| {
            let o = dispatch(app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
            dispatch(app, LOCAL, "add_item", json!({ "orderId": o["id"], "productId": curry.id, "qty": 2 })).unwrap()
        };
        // Refused alone, wrong PIN, own PIN; then approved by Quinn's PIN.
        let check = |app: &App, cmd: &str, args: Value, needs: &str| {
            let err = dispatch(app, LOCAL, cmd, args.clone()).unwrap_err();
            assert_eq!(err.to_string(), format!("{needs} needs a manager PIN"), "{cmd}");
            let with = |pin: &str| { let mut a = args.clone(); a["pin"] = json!(pin); a };
            assert!(matches!(dispatch(app, LOCAL, cmd, with("9999")).unwrap_err(), PosError::InvalidPin), "{cmd}");
            assert_eq!(dispatch(app, LOCAL, cmd, with("2222")).unwrap_err().to_string(), format!("{needs} needs a manager's or admin's PIN"), "{cmd}");
            dispatch(app, LOCAL, cmd, with("4646")).unwrap()
        };

        // A manager sets the staff limit; staff may not.
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        assert!(matches!(
            dispatch(&app, LOCAL, "set_setting", json!({ "key": "discount_staff_max_pct", "value": "100" })).unwrap_err(),
            PosError::Forbidden
        ));
        dispatch(&app, LOCAL, "login", json!({ "pin": "4646" })).unwrap();
        dispatch(&app, LOCAL, "set_setting", json!({ "key": "discount_staff_max_pct", "value": "20" })).unwrap();
        let v = dispatch(&app, LOCAL, "get_settings", json!({ "keys": ["discount_staff_max_pct"] })).unwrap();
        assert_eq!(v["discount_staff_max_pct"], "20");

        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        let o = new_order(&app);
        let (id, item) = (o["id"].clone(), o["items"][0]["id"].clone());
        // Over Quinn's 20% limit.
        let upd = json!({ "qty": 2, "discount_cents": 500, "note": null });
        let o2 = check(&app, "update_item", json!({ "itemId": item, "update": upd }), "a discount over 20%");
        assert_eq!(o2["discount_cents"], 500);
        // A sent dish: one fewer.
        dispatch(&app, LOCAL, "send_to_kitchen", json!({ "orderId": id })).unwrap();
        let upd = json!({ "qty": 1, "discount_cents": 250, "note": null });
        let o2 = check(&app, "update_item", json!({ "itemId": item, "update": upd }), "removing a dish already sent to the kitchen");
        assert_eq!(o2["items"][0]["qty"], 1);
        // A dish not on the menu.
        let o2 = check(&app, "add_open_item", json!({ "orderId": id, "name": "Lobster", "priceCents": 1 }), "a dish not on the menu");
        assert_eq!(o2["items"].as_array().unwrap().len(), 2);
        // Other, not a delivery app: add_payment and pay_order.
        let pay = json!({ "method": "other", "amount_cents": null, "tendered_cents": 0, "tip_cents": 0, "items": null });
        let r = check(&app, "add_payment", json!({ "orderId": id, "payment": pay }), "a payment by Other (not a delivery app)");
        assert_eq!(r["order"]["status"], "paid");
        let o = new_order(&app);
        let r = check(&app, "pay_order", json!({ "orderId": o["id"], "method": "other", "tenderedCents": 0 }), "a payment by Other (not a delivery app)");
        assert_eq!(r["order"]["status"], "paid");
        // Card needs nothing; Sam is still the one logged in.
        let o = new_order(&app);
        dispatch(&app, LOCAL, "pay_order", json!({ "orderId": o["id"], "method": "card", "tenderedCents": 0 })).unwrap();
        assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["session"]["name"], "Sam");

        // Quinn and the owner are never asked.
        for pin in ["4646", "1111"] {
            dispatch(&app, LOCAL, "login", json!({ "pin": pin })).unwrap();
            let o = new_order(&app);
            let (id, item) = (o["id"].clone(), o["items"][0]["id"].clone());
            dispatch(&app, LOCAL, "send_to_kitchen", json!({ "orderId": id })).unwrap();
            dispatch(&app, LOCAL, "update_item", json!({ "itemId": item, "update": { "qty": 1, "discount_cents": 1000, "note": null } })).unwrap();
            dispatch(&app, LOCAL, "add_open_item", json!({ "orderId": id, "name": "Bread", "priceCents": 200 })).unwrap();
            let r = dispatch(&app, LOCAL, "pay_order", json!({ "orderId": id, "method": "other", "tenderedCents": 0 })).unwrap();
            assert_eq!(r["order"]["status"], "paid");
        }
    }

    /// The audit log's last entry with `action`: (user id, detail).
    fn last_audit(app: &App, action: &str) -> (Option<i64>, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copy.db");
        app.lock().pos.backup_unchecked(&path).unwrap();
        let c = Connection::open(&path).unwrap();
        c.query_row("SELECT user_id, detail FROM audit_log WHERE action = ?1 ORDER BY id DESC LIMIT 1", [action], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
    }

    #[test]
    fn staff_need_a_manager_pin_to_take_a_payment_back_or_cut_service_or_tax() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        let sam = pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        let quinn = pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        pos.set_service_settings(&admin, &ServiceSettings { rate_bp: 1000, ..Default::default() }).unwrap();
        let curry = pos.create_product(&admin, curry("Green curry")).unwrap();
        let floor = pos.create_floor(&admin, "Ground").unwrap();
        let table = pos
            .save_floor_layout(&admin, floor.id, &[TableLayout { is_table: Some(true), id: None, name: "1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
            .unwrap()[0]
            .id;
        let app = App::new(pos, std::env::temp_dir());
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "dine_in", "table_id": table } })).unwrap();
        let id = o["id"].clone();
        dispatch(&app, LOCAL, "add_item", json!({ "orderId": id, "productId": curry.id, "qty": 2 })).unwrap();
        let pay = |amount: i64| {
            let o = dispatch(&app, LOCAL, "add_payment", json!({ "orderId": id, "payment": { "method": "cash", "amount_cents": amount, "tendered_cents": amount } })).unwrap();
            o["order"]["payments"].as_array().unwrap().last().unwrap()["id"].clone()
        };
        let needs = "needs a manager PIN";
        let wrong = "needs a manager's or admin's PIN";

        // Taking a payment back.
        let p = pay(500);
        for pin in [json!(null), json!(""), json!("  ")] {
            let err = dispatch(&app, LOCAL, "remove_payment", json!({ "orderId": id, "paymentId": p, "pin": pin })).unwrap_err();
            assert_eq!(err.to_string(), format!("taking a payment back {needs}"));
        }
        let err = dispatch(&app, LOCAL, "remove_payment", json!({ "orderId": id, "paymentId": p })).unwrap_err();
        assert_eq!(err.to_string(), format!("taking a payment back {needs}"));
        let err = dispatch(&app, LOCAL, "remove_payment", json!({ "orderId": id, "paymentId": p, "pin": "9999" })).unwrap_err();
        assert!(matches!(err, PosError::InvalidPin), "{err}");
        let err = dispatch(&app, LOCAL, "remove_payment", json!({ "orderId": id, "paymentId": p, "pin": "2222" })).unwrap_err();
        assert_eq!(err.to_string(), format!("taking a payment back {wrong}"));
        let o = dispatch(&app, LOCAL, "remove_payment", json!({ "orderId": id, "paymentId": p, "pin": "4646" })).unwrap();
        assert_eq!(o["paid_cents"], 0);
        let (user, detail) = last_audit(&app, "order.payment_removed");
        assert_eq!(user, Some(sam.id));
        assert!(detail.ends_with(&format!("cash 500 tip 0 taken by Sam (PIN of Quinn #{}, manager)", quinn.id)), "{detail}");

        // Removing the service charge; putting it back is staff's own.
        let err = dispatch(&app, LOCAL, "set_service_charge", json!({ "orderId": id, "rateBp": 0 })).unwrap_err();
        assert_eq!(err.to_string(), format!("removing or lowering the service charge {needs}"));
        let err = dispatch(&app, LOCAL, "set_service_charge", json!({ "orderId": id, "rateBp": 0, "pin": "9999" })).unwrap_err();
        assert!(matches!(err, PosError::InvalidPin), "{err}");
        let o = dispatch(&app, LOCAL, "set_service_charge", json!({ "orderId": id, "rateBp": 0, "pin": "4646" })).unwrap();
        assert_eq!(o["service_bp"], 0);
        assert!(last_audit(&app, "order.service").1.ends_with(&format!("(was 1000) (PIN of Quinn #{}, manager)", quinn.id)));
        let o = dispatch(&app, LOCAL, "set_service_charge", json!({ "orderId": id, "rateBp": 1000 })).unwrap();
        assert_eq!(o["service_bp"], 1000);

        // A tax rate for the order; back to menu rates that do not lower it is staff's own.
        let err = dispatch(&app, LOCAL, "set_order_tax", json!({ "orderId": id, "rateBp": 0 })).unwrap_err();
        assert_eq!(err.to_string(), format!("changing the tax on an order {needs}"));
        let err = dispatch(&app, LOCAL, "set_order_tax", json!({ "orderId": id, "rateBp": 0, "pin": "2222" })).unwrap_err();
        assert_eq!(err.to_string(), format!("changing the tax on an order {wrong}"));
        let o = dispatch(&app, LOCAL, "set_order_tax", json!({ "orderId": id, "rateBp": 0, "pin": "1111" })).unwrap();
        assert_eq!(o["tax_bp"], 0);
        assert!(last_audit(&app, "order.tax").1.ends_with(&format!("(PIN of Owner #{}, admin)", admin.user_id)));
        dispatch(&app, LOCAL, "set_order_tax", json!({ "orderId": id, "rateBp": null })).unwrap();
        // Sam is still the one logged in.
        assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["session"]["name"], "Sam");

        // A manager logged in needs no PIN for any of it.
        dispatch(&app, LOCAL, "login", json!({ "pin": "4646" })).unwrap();
        let p = pay(500);
        dispatch(&app, LOCAL, "remove_payment", json!({ "orderId": id, "paymentId": p })).unwrap();
        assert!(last_audit(&app, "order.payment_removed").1.ends_with("(no PIN, manager)"));
        dispatch(&app, LOCAL, "set_service_charge", json!({ "orderId": id, "rateBp": 0 })).unwrap();
        let o = dispatch(&app, LOCAL, "set_order_tax", json!({ "orderId": id, "rateBp": 0 })).unwrap();
        assert_eq!((o["service_bp"].clone(), o["tax_bp"].clone()), (json!(0), json!(0)));
    }

    #[test]
    fn staff_need_a_manager_pin_to_move_dishes_onto_a_cheaper_bill() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        let sam = pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        let quinn = pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        pos.set_service_settings(&admin, &ServiceSettings { rate_bp: 1000, ..Default::default() }).unwrap();
        let curry = pos.create_product(&admin, NewProduct { tax_rate_bp: Some(700), ..curry("Green curry") }).unwrap();
        let floor = pos.create_floor(&admin, "Ground").unwrap();
        let layout: Vec<TableLayout> = (1..=8)
            .map(|n| TableLayout { is_table: Some(true), id: None, name: n.to_string(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 })
            .collect();
        let tables: Vec<i64> = pos.save_floor_layout(&admin, floor.id, &layout).unwrap().into_iter().map(|t| t.id).collect();
        let app = App::new(pos, std::env::temp_dir());
        let login = |pin: &str| dispatch(&app, LOCAL, "login", json!({ "pin": pin })).unwrap();
        // An order on `table` with two curries: (order id, line id).
        let open = |table: i64| {
            let o = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "dine_in", "table_id": table } })).unwrap();
            let o = dispatch(&app, LOCAL, "add_item", json!({ "orderId": o["id"], "productId": curry.id, "qty": 2 })).unwrap();
            (o["id"].clone(), o["items"][0]["id"].clone())
        };
        // A manager's PIN lowers a bill's service charge or sets its tax.
        let cut = |id: &Value, cmd: &str| dispatch(&app, LOCAL, cmd, json!({ "orderId": id, "rateBp": 0, "pin": "4646" })).unwrap();
        // Refused alone, wrong PIN, own PIN; then approved by Quinn's PIN.
        let check = |cmd: &str, args: Value, needs: &str| {
            let err = dispatch(&app, LOCAL, cmd, args.clone()).unwrap_err();
            assert_eq!(err.to_string(), format!("{needs} needs a manager PIN"), "{cmd}");
            let with = |pin: &str| { let mut a = args.clone(); a["pin"] = json!(pin); a };
            assert!(matches!(dispatch(&app, LOCAL, cmd, with("9999")).unwrap_err(), PosError::InvalidPin), "{cmd}");
            assert_eq!(dispatch(&app, LOCAL, cmd, with("2222")).unwrap_err().to_string(), format!("{needs} needs a manager's or admin's PIN"), "{cmd}");
            dispatch(&app, LOCAL, cmd, with("4646")).unwrap()
        };
        let quinn_note = format!("(PIN of Quinn #{}, manager)", quinn.id);
        let service = "moving dishes to a bill with a lower service charge";
        let tax = "moving dishes to a bill with a lower tax";

        login("2222");
        // Table 2's bill has no service charge (a manager took it off).
        let (a, line) = open(tables[0]);
        let (b, _) = open(tables[1]);
        cut(&b, "set_service_charge");
        let o = check("move_items", json!({ "orderId": a, "tableId": tables[1], "items": [[line, 1]] }), service);
        assert_eq!((o["id"].clone(), o["service_bp"].clone(), o["service_cents"].clone()), (b.clone(), json!(0), json!(0)));
        let (user, detail) = last_audit(&app, "order.move_items");
        assert_eq!(user, Some(sam.id));
        assert!(detail.ends_with(&format!("-> order {b}: 1 dish {quinn_note}")), "{detail}");
        let o = check("merge_into_table", json!({ "orderId": a, "tableId": tables[1] }), service);
        assert_eq!((o["id"].clone(), o["items"].as_array().unwrap().len()), (b.clone(), 3));
        assert!(last_audit(&app, "order.merge").1.ends_with(&format!("{a} -> order {b} {quinn_note}")));
        // Table 4's bill is tax-free: the dishes would take its 0%.
        let (c, line) = open(tables[2]);
        let (d, _) = open(tables[3]);
        cut(&d, "set_order_tax");
        let o = check("move_items", json!({ "orderId": c, "tableId": tables[3], "items": [[line, 1]] }), tax);
        assert!(o["items"].as_array().unwrap().iter().all(|i| i["tax_rate_bp"] == 0), "{o}");
        assert_eq!(o["tax_cents"], 0);
        assert!(last_audit(&app, "order.move_items").1.ends_with(&quinn_note));
        check("merge_into_table", json!({ "orderId": c, "tableId": tables[3] }), tax);

        // The same or a higher rate stays free: onto a free table (same
        // rates), from the bills with less onto ones with the full rates.
        let (e, line) = open(tables[4]);
        let o = dispatch(&app, LOCAL, "move_items", json!({ "orderId": e, "tableId": tables[5], "items": [[line, 1]] })).unwrap();
        assert_eq!(o["service_bp"], 1000);
        let (f, _) = open(tables[6]);
        let o = dispatch(&app, LOCAL, "merge_into_table", json!({ "orderId": e, "tableId": tables[6] })).unwrap();
        assert_eq!(o["id"], f);
        assert!(!last_audit(&app, "order.merge").1.contains("PIN"));
        let b_line = dispatch(&app, LOCAL, "get_order", json!({ "id": b })).unwrap()["items"][0]["id"].clone();
        let o = dispatch(&app, LOCAL, "move_items", json!({ "orderId": b, "tableId": tables[6], "items": [[b_line, 1]] })).unwrap();
        assert_eq!((o["id"].clone(), o["service_bp"].clone()), (f.clone(), json!(1000)));
        // Off the tax-free bill: the dishes are taxed again, at the menu's rate.
        let o = dispatch(&app, LOCAL, "merge_into_table", json!({ "orderId": d, "tableId": tables[6] })).unwrap();
        assert_eq!((o["id"].clone(), o["tax_bp"].clone()), (f.clone(), Value::Null));
        assert!(o["items"].as_array().unwrap().iter().all(|i| i["tax_rate_bp"] == 700), "{o}");
        // Sam is still the one logged in.
        assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["session"]["name"], "Sam");

        // Quinn and the owner are never asked.
        for (pin, note) in [("4646", "(no PIN, manager)"), ("1111", "(no PIN, admin)")] {
            login(pin);
            let (g, line) = open(tables[2]);
            let (h, _) = open(tables[3]);
            dispatch(&app, LOCAL, "set_service_charge", json!({ "orderId": h, "rateBp": 0 })).unwrap();
            dispatch(&app, LOCAL, "set_order_tax", json!({ "orderId": h, "rateBp": 0 })).unwrap();
            dispatch(&app, LOCAL, "move_items", json!({ "orderId": g, "tableId": tables[3], "items": [[line, 1]] })).unwrap();
            assert!(last_audit(&app, "order.move_items").1.ends_with(note), "{pin}");
            let o = dispatch(&app, LOCAL, "merge_into_table", json!({ "orderId": g, "tableId": tables[3] })).unwrap();
            assert!(last_audit(&app, "order.merge").1.ends_with(note), "{pin}");
            assert_eq!((o["service_cents"].clone(), o["tax_cents"].clone()), (json!(0), json!(0)));
            dispatch(&app, LOCAL, "cancel_order", json!({ "orderId": h, "reason": "test" })).unwrap();
        }
    }

    fn curry(name: &str) -> NewProduct {
        NewProduct {
            category_id: None, sku: None, barcode: None, name: name.into(), price_cents: 900, cost_cents: 0, tax_rate_bp: Some(0),
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        }
    }

    #[test]
    fn a_repeated_payment_or_dish_is_taken_once_and_a_stale_till_is_refused() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        let curry = pos.create_product(&admin, curry("Green curry")).unwrap();
        let app = App::new(pos, std::env::temp_dir());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let id = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap()["id"].clone();
        // A dish tapped once, the answer lost, asked again with the same key: one dish.
        let add = json!({ "orderId": id, "productId": curry.id, "qty": 1, "idempotencyKey": "tap-1" });
        let first = dispatch(&app, LOCAL, "add_item", add.clone()).unwrap();
        assert_eq!(dispatch(&app, LOCAL, "add_item", add).unwrap(), first);
        dispatch(&app, LOCAL, "add_item", json!({ "orderId": id, "productId": curry.id, "qty": 1, "idempotencyKey": "tap-2" })).unwrap();
        let line = first["items"][0]["id"].clone();
        // + and − as deltas: a stale update body does not matter.
        let stale = json!({ "qty": 1, "discount_cents": 0, "note": null });
        let o = dispatch(&app, LOCAL, "update_item", json!({ "itemId": line, "update": stale, "qtyDelta": 1, "orderId": id })).unwrap();
        assert_eq!(o["items"][0]["qty"], 3);
        // A note saved from a screen that still shows 2: refused, nothing lost.
        let e = dispatch(&app, LOCAL, "update_item", json!({ "itemId": line, "update": { "qty": 2, "discount_cents": 0, "note": "mild" }, "expectedQty": 2 })).unwrap_err();
        assert_eq!(e.to_string(), ITEM_CHANGED);
        assert_eq!(dispatch(&app, LOCAL, "get_order", json!({ "id": id })).unwrap()["items"][0]["qty"], 3);
        // The pay dialog showed 18.00 (two curries): the bill is 27.00 now, nothing is taken.
        let pay = |left: i64, key: &str| json!({ "orderId": id, "payment": { "method": "card", "amount_cents": left }, "expectedLeftCents": left, "idempotencyKey": key });
        let e = dispatch(&app, LOCAL, "add_payment", pay(1800, "pay-1")).unwrap_err();
        assert_eq!(e.to_string(), BILL_CHANGED);
        let e = dispatch(&app, LOCAL, "pay_order", json!({ "orderId": id, "method": "card", "tenderedCents": 0, "expectedLeftCents": 1800 })).unwrap_err();
        assert_eq!(e.to_string(), BILL_CHANGED);
        // Part of it by card, the answer lost, the same confirm again: one payment, and the same answer.
        let part = json!({ "orderId": id, "payment": { "method": "card", "amount_cents": 900 }, "expectedLeftCents": 2700, "idempotencyKey": "pay-2" });
        let r1 = dispatch(&app, LOCAL, "add_payment", part.clone()).unwrap();
        let r2 = dispatch(&app, LOCAL, "add_payment", part).unwrap();
        assert_eq!((r1["replayed"].clone(), r2["replayed"].clone()), (json!(false), json!(true)));
        assert_eq!(r2["order"], r1["order"]);
        let o = dispatch(&app, LOCAL, "get_order", json!({ "id": id })).unwrap();
        assert_eq!((o["payments"].as_array().unwrap().len(), o["paid_cents"].clone()), (1, json!(900)));
        // The same key for something else is refused, never replayed: another amount,
        // another command, another order, another dish.
        let other_amount = json!({ "orderId": id, "payment": { "method": "card", "amount_cents": 500 }, "expectedLeftCents": 1800, "idempotencyKey": "pay-2" });
        assert_eq!(dispatch(&app, LOCAL, "add_payment", other_amount).unwrap_err().to_string(), KEY_REUSED);
        let as_pay_order = json!({ "orderId": id, "method": "card", "tenderedCents": 0, "expectedLeftCents": 1800, "idempotencyKey": "pay-2" });
        assert_eq!(dispatch(&app, LOCAL, "pay_order", as_pay_order).unwrap_err().to_string(), KEY_REUSED);
        let id2 = dispatch(&app, LOCAL, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap()["id"].clone();
        dispatch(&app, LOCAL, "add_item", json!({ "orderId": id2, "productId": curry.id, "qty": 1 })).unwrap();
        let other_order = json!({ "orderId": id2, "payment": { "method": "card", "amount_cents": 900 }, "expectedLeftCents": 2700, "idempotencyKey": "pay-2" });
        assert_eq!(dispatch(&app, LOCAL, "add_payment", other_order).unwrap_err().to_string(), KEY_REUSED);
        let e = dispatch(&app, LOCAL, "add_item", json!({ "orderId": id2, "productId": curry.id, "qty": 1, "idempotencyKey": "tap-1" })).unwrap_err();
        assert_eq!(e.to_string(), KEY_REUSED);
        let e = dispatch(&app, LOCAL, "add_item", json!({ "orderId": id, "productId": curry.id, "qty": 2, "idempotencyKey": "tap-1" })).unwrap_err();
        assert_eq!(e.to_string(), KEY_REUSED);
        assert_eq!(dispatch(&app, LOCAL, "get_order", json!({ "id": id2 })).unwrap()["payments"].as_array().unwrap().len(), 0);
        assert_eq!(dispatch(&app, LOCAL, "get_order", json!({ "id": id2 })).unwrap()["items"][0]["qty"], 1);
        // A refused attempt is not remembered: its key works once the bill is right.
        let e = dispatch(&app, LOCAL, "pay_order", json!({ "orderId": id2, "method": "card", "tenderedCents": 0, "expectedLeftCents": 1, "idempotencyKey": "pay-3" })).unwrap_err();
        assert_eq!(e.to_string(), BILL_CHANGED);
        let r = dispatch(&app, LOCAL, "pay_order", json!({ "orderId": id2, "method": "card", "tenderedCents": 0, "expectedLeftCents": 900, "idempotencyKey": "pay-3" })).unwrap();
        assert_eq!((r["order"]["status"].clone(), r["replayed"].clone()), (json!("paid"), json!(false)));
        // Another terminal's key is its own.
        assert!(matches!(dispatch(&app, "web-other", "add_payment", pay(1800, "pay-2")), Err(PosError::NotLoggedIn)));
        // Without the new fields: as before.
        let r = dispatch(&app, LOCAL, "pay_order", json!({ "orderId": id, "method": "cash", "tenderedCents": 2000 })).unwrap();
        assert_eq!(r["order"]["status"], "paid");
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
        // A backup from two days ago: restoring it must not make it look new
        // (the daily backup and pruning go by the files' modified times).
        let backup = dir.path().join("pos-1700000000.db");
        let made = SystemTime::now() - Duration::from_secs(2 * 86_400);
        std::fs::File::options().write(true).open(&backup).unwrap().set_modified(made).unwrap();
        let modified = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
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
        assert_eq!(modified(&backup), made, "the restored file keeps its time");
        assert!(newest_regular_backup(dir.path()).is_some_and(|t| t == made));
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
    fn staff_are_refused_every_manage_command_but_still_sell() {
        let app = app();
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        dispatch(&app, LOCAL, "create_user", json!({ "name": "Quinn", "role": "manager", "pin": "4646" })).unwrap();
        let curry = dispatch(&app, LOCAL, "create_product", json!({ "product": {
            "category_id": null, "sku": null, "barcode": null, "name": "Green curry", "price_cents": 900, "cost_cents": 0, "tax_rate_bp": 0,
            "stock_qty": 0, "track_stock": false, "starter": false, "name_alt": null, "prep": false, "team_id": null,
        } }))
        .unwrap();
        let till = "web-0123456789abcdef0123456789abcdef";
        dispatch(&app, till, "login", json!({ "pin": "2222" })).unwrap();
        let refused = [
            // Staff could call these before (reports, End of day, Shift & cash).
            ("sales_summary", json!({ "from": 0, "to": 2_000_000_000 })),
            ("top_products", json!({ "from": 0, "to": 2_000_000_000, "limit": 10 })),
            ("close_day", json!({})),
            ("day_closes", json!({ "limit": 10 })),
            ("reprint_day_close", json!({ "id": 1 })),
            ("current_shift", json!({})),
            ("open_shift", json!({ "floatCents": 10_000 })),
            ("cash_move", json!({ "kind": "in", "amountCents": 500, "reason": "" })),
            ("close_shift", json!({ "countedCents": 0, "note": "" })),
            ("list_shifts", json!({ "limit": 10 })),
            ("print_shift", json!({ "id": 1 })),
            // And these stay refused (menu, floor plan, staff, settings, backups).
            ("list_products", json!({ "includeInactive": true, "limit": 10, "offset": 0 })),
            ("set_starter", json!({ "id": curry["id"], "starter": true })),
            ("create_floor", json!({ "name": "Garden" })),
            ("list_users", json!({})),
            ("save_teams", json!({ "teams": [] })),
            ("set_setting", json!({ "key": "store_name", "value": "Mine" })),
            ("set_setting", json!({ "key": "kds_beep", "value": "1" })),
            ("save_channels", json!({ "channels": [] })),
            ("backup_now", json!({})),
            ("list_full_backups", json!({})),
        ];
        for (cmd, args) in refused {
            let r = dispatch(&app, till, cmd, args);
            assert!(matches!(r, Err(PosError::Forbidden)), "{cmd}: {r:?}");
        }
        // Selling still works from the same till.
        let o = dispatch(&app, till, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
        dispatch(&app, till, "add_item", json!({ "orderId": o["id"], "productId": curry["id"], "qty": 1, "note": "mild" })).unwrap();
        dispatch(&app, till, "send_to_kitchen", json!({ "orderId": o["id"] })).unwrap();
        assert!(!dispatch(&app, till, "kitchen_board", json!({ "filter": "all" })).unwrap().as_array().unwrap().is_empty());
        let paid = dispatch(&app, till, "pay_order", json!({ "orderId": o["id"], "method": "cash", "tenderedCents": 1000 })).unwrap();
        assert_eq!(paid["order"]["status"], "paid");
        // A manager on the same till still has reports and the shift.
        dispatch(&app, till, "login", json!({ "pin": "4646" })).unwrap();
        dispatch(&app, till, "open_shift", json!({ "floatCents": 10_000 })).unwrap();
        dispatch(&app, till, "sales_summary", json!({ "from": 0, "to": 2_000_000_000 })).unwrap();
    }

    #[test]
    fn only_admins_check_for_and_install_updates() {
        let app = app();
        assert!(matches!(require_updates(&app, LOCAL), Err(PosError::NotLoggedIn)));
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        assert!(matches!(require_updates(&app, LOCAL), Err(PosError::Forbidden)));
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        dispatch(&app, LOCAL, "create_user", json!({ "name": "Quinn", "role": "manager", "pin": "4646" })).unwrap();
        require_updates(&app, LOCAL).unwrap();
        dispatch(&app, LOCAL, "login", json!({ "pin": "4646" })).unwrap();
        assert!(matches!(require_updates(&app, LOCAL), Err(PosError::Forbidden)));
    }

    /// A till on a database file (as the app runs), Owner 1111 and Sam 2222,
    /// backups in `dir/backups`; and a full backup from another PC (Nok
    /// 4321) in that folder as "pos-1700000000.db".
    #[test]
    fn the_activity_export_reads_on_its_own_connection_with_the_same_output() {
        let dir = tempfile::tempdir().unwrap();
        let (app, db) = till_on_disk(dir.path());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        dispatch(&app, LOCAL, "set_setting", json!({ "key": "store_name", "value": "Shop, \"quoted\"" })).unwrap();
        // On the shared connection (no file known yet), then on its own connection.
        let shared = dispatch(&app, LOCAL, "export_activity", json!({})).unwrap();
        app.set_db_path(db);
        let own = dispatch(&app, LOCAL, "export_activity", json!({})).unwrap();
        assert_eq!((own["csv"].clone(), own["rows"].clone()), (shared["csv"].clone(), shared["rows"].clone()));
        assert!(own["rows"].as_u64().unwrap() >= 3, "{own}");
        // Still admins only.
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        assert!(matches!(dispatch(&app, LOCAL, "export_activity", json!({})), Err(PosError::Forbidden)));
    }

    fn till_on_disk(dir: &Path) -> (App, PathBuf) {
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let mut other = Pos::open(dir.join("other.db")).unwrap();
        let nok = other.setup_first_admin("Nok", "4321").unwrap();
        other.set_setting(&nok, "store_name", "Old Shop").unwrap();
        other.backup_unchecked(backups.join("pos-imported-1700000000.db")).unwrap();
        let db = dir.join("pos.db");
        let mut pos = Pos::open(&db).unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.set_setting(&admin, "store_name", "This Shop").unwrap();
        (App::new(pos, backups), db)
    }

    fn store_name(path: &Path) -> String {
        let c = Connection::open(path).unwrap();
        c.query_row("SELECT value FROM settings WHERE key = 'store_name'", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn the_daily_backup_is_a_complete_copy_made_beside_the_till() {
        let dir = tempfile::tempdir().unwrap();
        let (app, db) = till_on_disk(dir.path());
        let day = Duration::from_secs(24 * 60 * 60);
        // The tills keep working while it copies: it never holds their lock.
        let made = std::thread::scope(|sc| {
            let backup = sc.spawn(|| auto_backup(&app, &db, day));
            for _ in 0..20 {
                dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
            }
            backup.join().unwrap()
        });
        let made = made.expect("a backup");
        let name = made.file_name().unwrap().to_string_lossy().into_owned();
        assert!(is_kind(&name, "pos-"), "{name}");
        assert_eq!(store_name(&made), "This Shop");
        // No scratch file is left; one a day is enough.
        assert_eq!(files(&app.backup_dir), [name.clone(), "pos-imported-1700000000.db".to_string()]);
        assert!(auto_backup(&app, &db, day).is_none());
        // A copy that cannot be made leaves nothing behind.
        assert!(auto_backup(&app, &dir.path().join("missing.db"), Duration::ZERO).is_none());
        assert_eq!(files(&app.backup_dir).len(), 2);
        // Once an update is installing, none starts.
        app.stop_auto_backups();
        assert!(auto_backup(&app, &db, Duration::ZERO).is_none());
        assert_eq!(files(&app.backup_dir).len(), 2);
    }

    #[test]
    fn a_restore_waits_for_the_daily_backup_and_pins_follow_the_restored_salt() {
        let dir = tempfile::tempdir().unwrap();
        let (app, db) = till_on_disk(dir.path());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let before = format!("{:?}", app.hasher.lock().unwrap().clone());
        // The daily backup is copying (it holds the gate) when the restore comes.
        let waited = std::thread::scope(|sc| {
            let gate = app.backup_gate();
            let restore = sc.spawn(|| {
                let t = Instant::now();
                dispatch(&app, LOCAL, "restore_backup", json!({ "name": "pos-imported-1700000000.db", "pin": "1111", "confirm": "RESTORE" })).unwrap();
                t.elapsed()
            });
            std::thread::sleep(Duration::from_millis(300));
            // The till still works meanwhile.
            assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["store_name"], "This Shop");
            drop(gate);
            restore.join().unwrap()
        });
        assert!(waited >= Duration::from_millis(250), "{waited:?}");
        assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["store_name"], "Old Shop");
        // The PIN salt came with the backup: the cached one is the new one.
        let after = format!("{:?}", app.hasher.lock().unwrap().clone());
        assert_ne!(before, after);
        assert_eq!(after, format!("{:?}", app.lock().pos.pin_hasher()));
        assert!(matches!(dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })), Err(PosError::InvalidPin)));
        dispatch(&app, LOCAL, "login", json!({ "pin": "4321" })).unwrap();
        // A stale cached salt only costs hashing again, under the lock.
        *app.hasher.lock().unwrap() = Pos::open_in_memory().unwrap().pin_hasher();
        dispatch(&app, LOCAL, "login", json!({ "pin": "4321" })).unwrap();
        assert!(matches!(dispatch(&app, LOCAL, "login", json!({ "pin": "9999" })), Err(PosError::InvalidPin)));
        // And the daily backup after it copies the restored data.
        let made = auto_backup(&app, &db, Duration::ZERO).unwrap();
        assert_eq!(store_name(&made), "Old Shop");
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
    #[test]
    fn tickets_from_a_tablet_reach_the_answerers_and_only_admins_delete() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        let app = App::new(pos, std::env::temp_dir());
        let tablet = "0123456789abcdef0123456789abcdef";
        let phone = "fedcba9876543210fedcba9876543210";
        dispatch(&app, tablet, "login", json!({ "pin": "2222" })).unwrap();
        // Nobody who answers is logged in: the sent screen says to phone them.
        app.with_session(tablet, |_, _| Ok(())).unwrap();
        dispatch(&app, LOCAL, "login", json!({ "pin": "2222" })).unwrap();
        let admin_id = app.lock().pos.login_users().unwrap().into_iter().find(|u| u.name == "Owner").unwrap().id;
        dispatch(&app, phone, "login", json!({ "pin": "1111", "userId": admin_id })).unwrap();
        dispatch(&app, phone, "set_setting", json!({ "key": "tickets_call_text", "value": "Quinn: 07700 900123" })).unwrap();
        dispatch(&app, phone, "logout", json!({})).unwrap();
        let made = dispatch(&app, tablet, "create_ticket", json!({
            "category": "printer", "urgency": "urgent", "body": "Not printing", "screen": "tables", "screenLabel": "Tables", "device": "Bar tablet"
        }))
        .unwrap();
        assert_eq!(made["answerers_online"], 0);
        assert_eq!(made["call_text"], "Quinn: 07700 900123");
        assert_eq!(made["ticket"]["terminal"], "web");
        let id = made["ticket"]["id"].as_i64().unwrap();
        // Quinn logs in on the phone: badge, banner info, and a count of one answerer.
        dispatch(&app, phone, "login", json!({ "pin": "4646" })).unwrap();
        let st = dispatch(&app, phone, "app_status", Value::Null).unwrap();
        assert_eq!((st["tickets"]["unread"].clone(), st["tickets"]["urgent"][0]["id"].clone()), (json!(1), json!(id)));
        assert!(st["backup_alert"].is_null(), "managers are never told about backups");
        let again = dispatch(&app, tablet, "create_ticket", json!({ "category": "other", "body": "and this" })).unwrap();
        assert_eq!(again["answerers_online"], 1);
        // Staff on the till cannot see Sam's... Sam is on both: his own on either.
        assert_eq!(dispatch(&app, LOCAL, "list_tickets", json!({ "tab": "open" })).unwrap().as_array().unwrap().len(), 2);
        dispatch(&app, phone, "reply_ticket", json!({ "id": id, "body": "On my way" })).unwrap();
        let st = dispatch(&app, tablet, "app_status", Value::Null).unwrap();
        assert_eq!(st["tickets"]["latest"]["by"], "Quinn");
        assert_eq!(st["tickets"]["unread"], 1);
        dispatch(&app, LOCAL, "get_ticket", json!({ "id": id })).unwrap();
        // Read on the till: gone on the tablet too.
        assert_eq!(dispatch(&app, tablet, "app_status", Value::Null).unwrap()["tickets"]["unread"], 0);
        // Deleting: the manager and staff are refused, the admin may.
        assert!(matches!(dispatch(&app, phone, "delete_ticket", json!({ "id": id })), Err(PosError::Forbidden)));
        assert!(matches!(dispatch(&app, tablet, "delete_ticket", json!({ "id": id })), Err(PosError::Forbidden)));
        dispatch(&app, phone, "logout", json!({})).unwrap();
        dispatch(&app, phone, "login", json!({ "pin": "1111", "userId": admin_id })).unwrap();
        dispatch(&app, phone, "delete_ticket", json!({ "id": id })).unwrap();
        assert!(matches!(dispatch(&app, tablet, "get_ticket", json!({ "id": id })), Err(PosError::NotFound)));
        // Logged out: no ticket info.
        assert!(dispatch(&app, "nobody-here-0000", "app_status", Value::Null).unwrap()["tickets"].is_null());
    }

    #[test]
    fn the_activity_log_is_for_managers_and_admins_and_its_management_for_admins() {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        let app = App::new(pos, std::env::temp_dir());
        let tablet = "0123456789abcdef0123456789abcdef";
        dispatch(&app, tablet, "login", json!({ "pin": "2222" })).unwrap();
        for cmd in ["list_activity", "activity_staff", "log_retention"] {
            assert!(matches!(dispatch(&app, tablet, cmd, json!({})), Err(PosError::Forbidden)), "{cmd}");
        }
        assert!(matches!(dispatch(&app, tablet, "order_activity", json!({ "orderId": 1 })), Err(PosError::Forbidden)));
        dispatch(&app, tablet, "logout", json!({})).unwrap();
        // A wrong PIN on the tablet is logged with its terminal and its own name.
        app.set_device(tablet, Some("Bar tablet"));
        assert!(dispatch(&app, tablet, "login", json!({ "pin": "9999" })).is_err());
        dispatch(&app, tablet, "login", json!({ "pin": "4646" })).unwrap();
        let page = dispatch(&app, tablet, "list_activity", json!({ "kind": "login", "look": true })).unwrap();
        let failed = &page["rows"][0];
        assert_eq!((failed["action"].clone(), failed["terminal"].clone(), failed["look"].clone()), (json!("login.failed"), json!("web-012345"), json!(true)));
        assert_eq!(page["look_count"], 1);
        assert_eq!(failed["device"], "Bar tablet");
        assert!(dispatch(&app, tablet, "activity_staff", json!({})).unwrap().as_array().unwrap().len() == 3);
        for (cmd, args) in [
            ("delete_activity", json!({ "ids": [1], "pin": "4646" })),
            ("clear_activity", json!({ "before": 2_000_000_000i64, "pin": "4646", "confirm": "DELETE" })),
            ("activity_count", json!({ "before": 2_000_000_000i64 })),
            ("export_activity", json!({})),
            ("log_retention", json!({})),
            ("set_setting", json!({ "key": "log_keep_days", "value": "30" })),
            ("backup_status", json!({})),
        ] {
            assert!(matches!(dispatch(&app, tablet, cmd, args), Err(PosError::Forbidden)), "{cmd}");
        }
        // The admin: delete, with a wrong PIN logged first.
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        assert!(matches!(dispatch(&app, LOCAL, "delete_activity", json!({ "ids": [1], "pin": "0000" })), Err(PosError::InvalidPin)));
        let r = dispatch(&app, LOCAL, "delete_activity", json!({ "ids": [1, 2], "pin": "1111" })).unwrap();
        assert_eq!(r["deleted"], 2);
        let csv = dispatch(&app, LOCAL, "export_activity", json!({ "kind": "login" })).unwrap();
        assert!(csv["csv"].as_str().unwrap().contains("log.bad_pin"));
        assert!(csv["path"].is_null(), "only saved when asked");
        assert!(dispatch(&app, LOCAL, "set_setting", json!({ "key": "log_keep_days", "value": "45" })).is_err());
        dispatch(&app, LOCAL, "set_setting", json!({ "key": "log_keep_days", "value": "90" })).unwrap();
        assert_eq!(dispatch(&app, LOCAL, "log_retention", json!({})).unwrap()["keep_days"], 90);
        // History: who cancelled it, and with whose PIN.
        dispatch(&app, tablet, "logout", json!({})).unwrap();
        dispatch(&app, tablet, "login", json!({ "pin": "2222" })).unwrap();
        let o = dispatch(&app, tablet, "open_order", json!({ "order": { "order_type": "takeaway" } })).unwrap();
        let id = o["id"].as_i64().unwrap();
        let curry = app.with_session(LOCAL, |pos, s| pos.create_product(s, curry("Curry"))).unwrap();
        dispatch(&app, tablet, "add_item", json!({ "orderId": id, "productId": curry.id, "qty": 1 })).unwrap();
        dispatch(&app, tablet, "send_to_kitchen", json!({ "orderId": id })).unwrap();
        dispatch(&app, tablet, "cancel_order", json!({ "orderId": id, "reason": "mistake", "pin": "4646" })).unwrap();
        let order = dispatch(&app, LOCAL, "get_order", json!({ "id": id })).unwrap();
        assert_eq!((order["closed_by_name"].clone(), order["close_approver_name"].clone()), (json!("Sam"), json!("Quinn")), "{order}");
        assert_eq!(order["close_approver_role"], "manager");
        let acts = dispatch(&app, LOCAL, "order_activity", json!({ "orderId": id })).unwrap();
        let cancel = acts.as_array().unwrap().iter().find(|r| r["action"] == "order.cancel").cloned().expect("the cancel row");
        assert_eq!((cancel["user_name"].clone(), cancel["approver_name"].clone(), cancel["look"].clone()), (json!("Sam"), json!("Quinn"), json!(true)), "{acts}");
    }

    #[test]
    fn automatic_backups_by_kind_skip_when_nothing_changed_and_copy_to_a_second_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (app, db) = till_on_disk(dir.path());
        app.set_db_path(db.clone());
        mark_unchanged(&app);
        // Nothing changed since the start: no scheduled or exit backup.
        assert!(backup_kind(&app, BackupKind::Scheduled).is_none());
        assert!(backup_kind(&app, BackupKind::Exit).is_none());
        // Day close backups are made whatever.
        let made = backup_kind(&app, BackupKind::DayClose).expect("a day close backup");
        assert!(is_kind(&made.file_name().unwrap().to_string_lossy(), "pos-"));
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let auto = backup_kind(&app, BackupKind::Scheduled).expect("something changed");
        assert!(auto.file_name().unwrap().to_string_lossy().starts_with("pos-auto-"));
        assert!(backup_kind(&app, BackupKind::Scheduled).is_none(), "and nothing since");
        // A second folder that is not there: the backup is still made, the status says why.
        let usb = dir.path().join("usb");
        app.with_session(LOCAL, |pos, s| pos.set_backup_copy_dir(s, Some(&usb.display().to_string()))).unwrap();
        dispatch(&app, LOCAL, "set_setting", json!({ "key": "backup_copy_on", "value": "1" })).unwrap();
        let name = dispatch(&app, LOCAL, "backup_now", json!({})).unwrap();
        assert!(Path::new(name.as_str().unwrap()).is_file());
        let st = dispatch(&app, LOCAL, "backup_status", json!({})).unwrap();
        assert_eq!(st["state"]["copy_error"], "folder not found");
        assert_eq!(st["copy_dir_ok"], false);
        assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["backup_alert"]["message"], "Second copy failed: folder not found");
        // Plugged in again: the next backup is copied there, and the alert goes.
        std::fs::create_dir_all(&usb).unwrap();
        dispatch(&app, LOCAL, "set_setting", json!({ "key": "store_name", "value": "Changed" })).unwrap();
        let auto = backup_kind(&app, BackupKind::Exit).expect("an exit backup");
        let copied = usb.join(auto.file_name().unwrap());
        assert_eq!(store_name(&copied), "Changed");
        assert_eq!(files(&usb), [auto.file_name().unwrap().to_string_lossy().into_owned()]);
        assert!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["backup_alert"].is_null());
        // Scheduled ones keep their own count.
        dispatch(&app, LOCAL, "set_setting", json!({ "key": "backup_hourly_keep", "value": "3" })).unwrap();
        for i in 0..5 {
            dispatch(&app, LOCAL, "set_setting", json!({ "key": "store_name", "value": format!("v{i}") })).unwrap();
            backup_kind(&app, BackupKind::Scheduled).expect("a scheduled backup");
        }
        let names = files(&app.backup_dir);
        assert_eq!(names.iter().filter(|n| n.starts_with("pos-auto-")).count(), 3, "{names:?}");
        assert_eq!(names.iter().filter(|n| is_kind(n, "pos-")).count(), 2, "the daily ones stay: {names:?}");
        assert!(!names.iter().any(|n| n.ends_with(".tmp") || n.ends_with("-wal") || n.ends_with("-shm")), "{names:?}");
    }

    #[test]
    fn after_an_unclean_shutdown_an_emergency_backup_is_made_or_a_damaged_database_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (app, db) = till_on_disk(dir.path());
        app.set_db_path(db.clone());
        startup_check(&app, false);
        assert!(!app.backup_state().unclean_shutdown);
        startup_check(&app, true);
        let st = app.backup_state().clone();
        assert!(st.unclean_shutdown && !st.damaged);
        let name = st.emergency_backup.expect("an emergency backup");
        assert!(name.starts_with("pos-emergency-") && app.backup_dir.join(&name).is_file());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let alert = dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["backup_alert"].clone();
        assert_eq!((alert["kind"].clone(), alert["message"].clone()), (json!("unclean"), json!("POS did not close properly last time; a backup was made.")));
        dispatch(&app, LOCAL, "dismiss_backup_alert", json!({})).unwrap();
        assert!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["backup_alert"].is_null());
        // Staff never see it.
        let tablet = "0123456789abcdef0123456789abcdef";
        dispatch(&app, tablet, "login", json!({ "pin": "2222" })).unwrap();
        assert!(matches!(dispatch(&app, tablet, "dismiss_backup_alert", json!({})), Err(PosError::Forbidden)));
        // Damaged: no backup of any kind, so good ones are never pushed out.
        {
            let mut st = app.backup_state();
            st.damaged = true;
        }
        let before = files(&app.backup_dir);
        assert!(backup_kind(&app, BackupKind::DayClose).is_none());
        assert!(backup_kind(&app, BackupKind::Emergency).is_none());
        assert!(dispatch(&app, LOCAL, "backup_now", json!({})).is_err());
        assert_eq!(files(&app.backup_dir), before);
        assert_eq!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["backup_alert"]["kind"], "damaged");
    }

    /// A till on disk with a good backup ("pos-<time>.db") and a long log,
    /// then its log pages overwritten on disk: damaged after a proper close.
    fn damaged_till(dir: &Path) -> (PathBuf, String) {
        let (app, db) = till_on_disk(dir);
        app.set_db_path(db.clone());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let good = dispatch(&app, LOCAL, "backup_now", json!({})).unwrap().as_str().unwrap().to_string();
        let good = Path::new(&good).file_name().unwrap().to_string_lossy().into_owned();
        drop(app);
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(
                "BEGIN;
                 WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 2999)
                 INSERT INTO audit_log (user_id, action, detail) SELECT 1, 'login', printf('%0200d', i) FROM n;
                 COMMIT;
                 PRAGMA wal_checkpoint(TRUNCATE);",
            )
            .unwrap();
        }
        let mut bytes = std::fs::read(&db).unwrap();
        let pages = bytes.len() / 4096;
        for b in &mut bytes[(pages - 60) * 4096..(pages - 20) * 4096] {
            *b = 0xA5;
        }
        std::fs::write(&db, bytes).unwrap();
        (db, good)
    }

    #[test]
    fn data_damaged_after_a_proper_close_is_found_at_start_and_backups_say_so() {
        let dir = tempfile::tempdir().unwrap();
        let (db, good) = damaged_till(dir.path());
        let app = App::new(Pos::open(&db).unwrap(), dir.path().join("backups"));
        app.set_db_path(db.clone());
        assert!(!app.backup_state().damaged);
        // A proper close last time: the quick check finds it all the same.
        startup_check(&app, false);
        let st = app.backup_state().clone();
        assert!(st.damaged && !st.unclean_shutdown, "{st:?}");
        let before = files(&app.backup_dir);
        assert!(backup_kind(&app, BackupKind::Scheduled).is_none());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let e = dispatch(&app, LOCAL, "backup_now", json!({})).unwrap_err().to_string();
        assert!(e.starts_with("The POS data is damaged, so no backup was made"), "{e}");
        assert_eq!(files(&app.backup_dir), before, "no backup, and the good ones stay");
        let st = dispatch(&app, LOCAL, "app_status", Value::Null).unwrap();
        assert_eq!((st["backup_alert"]["kind"].clone(), st["recovery"]["reason"].clone()), (json!("damaged"), json!("damaged")));
        assert!(st["recovery"]["can_log_in"].as_bool().unwrap());
        assert!(app.backup_dir.join(&good).is_file());

        // Not found at start (no check yet): "Back up now" itself finds it.
        let app = App::new(Pos::open(&db).unwrap(), dir.path().join("backups"));
        app.set_db_path(db.clone());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        let e = dispatch(&app, LOCAL, "backup_now", json!({})).unwrap_err().to_string();
        assert!(e.starts_with("The POS data is damaged, so no backup was made"), "{e}");
        assert!(app.backup_state().damaged);
        assert_eq!(files(&app.backup_dir), before);

        // Any command that reads a damaged page marks it too.
        let app = App::new(Pos::open(&db).unwrap(), dir.path().join("backups"));
        let read = dispatch(&app, LOCAL, "login", json!({ "pin": "1111" }))
            .and_then(|_| dispatch(&app, LOCAL, "export_activity", json!({})));
        assert!(read.is_err() && app.backup_state().damaged, "{read:?}");
    }

    #[test]
    fn the_recovery_screen_restores_a_backup_with_its_admin_pin_on_the_main_till_only() {
        let dir = tempfile::tempdir().unwrap();
        let (db, good) = damaged_till(dir.path());
        let app = App::new(Pos::open(&db).unwrap(), dir.path().join("backups"));
        app.set_db_path(db.clone());
        // Not while the data is fine.
        assert!(dispatch(&app, LOCAL, "recovery_status", json!({})).unwrap_err().to_string().contains("not damaged"));
        startup_check(&app, false);
        let tablet = "0123456789abcdef0123456789abcdef";
        for cmd in ["recovery_status", "recovery_check", "recovery_restore"] {
            let args = json!({ "name": good, "pin": "1111", "confirm": "RESTORE" });
            assert!(matches!(dispatch(&app, tablet, cmd, args), Err(PosError::Forbidden)), "{cmd} from a tablet");
        }
        assert!(dispatch(&app, tablet, "app_status", Value::Null).unwrap()["recovery"].is_null());
        let st = dispatch(&app, LOCAL, "recovery_status", json!({})).unwrap();
        assert!(st["backups"].as_array().unwrap().iter().any(|b| b["name"] == good.as_str()));
        // The newest good one: the imported file of another shop is older.
        let found = dispatch(&app, LOCAL, "recovery_check", json!({})).unwrap();
        assert_eq!((found["file"]["name"].clone(), found["summary"]["store_name"].clone()), (json!(good), json!("This Shop")));
        let args = |pin: &str, confirm: &str| json!({ "name": good, "pin": pin, "confirm": confirm });
        let e = dispatch(&app, LOCAL, "recovery_restore", args("1111", "restore")).unwrap_err().to_string();
        assert_eq!(e, "type RESTORE to confirm");
        // Staff PIN, or no admin's: refused.
        let e = dispatch(&app, LOCAL, "recovery_restore", args("2222", "RESTORE")).unwrap_err().to_string();
        assert!(e.contains("not the PIN of an admin"), "{e}");
        let done = dispatch(&app, LOCAL, "recovery_restore", args("1111", "RESTORE")).unwrap();
        assert_eq!((done["pin_from"].clone(), done["restart_needed"].clone(), done["restored"].clone()), (json!("backup"), json!(false), json!(good)));
        let safety = done["safety_copy"].as_str().unwrap();
        assert!(safety.starts_with("pos-before-restore-") && app.backup_dir.join(safety).is_file());
        assert!(!app.backup_state().damaged);
        assert_eq!(Pos::quick_check_file(&db).unwrap(), None, "the data is whole again");
        assert!(dispatch(&app, LOCAL, "app_status", Value::Null).unwrap()["recovery"].is_null());
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        assert!(dispatch(&app, LOCAL, "export_activity", json!({})).is_ok());
        // The restore's audit row says what came back, who, how and from which file.
        let (_, detail) = last_audit(&app, "backup.restore");
        assert!(detail.contains(" staff, by ") && detail.contains(&format!("(recovery screen, file {good})")), "{detail}");
        // Once restored, the recovery screen is gone.
        assert!(dispatch(&app, LOCAL, "recovery_restore", args("1111", "RESTORE")).is_err());
    }

    #[test]
    fn data_that_cannot_be_opened_only_offers_the_recovery_screen() {
        let dir = tempfile::tempdir().unwrap();
        let (db, good) = damaged_till(dir.path());
        // The header itself is gone: not a database any more.
        let mut bytes = std::fs::read(&db).unwrap();
        bytes[..100].fill(0x5A);
        std::fs::write(&db, bytes).unwrap();
        let e = Pos::open(&db).err().expect("it cannot be opened");
        assert!(is_corrupt(&e), "{e}");
        let app = App::cannot_open(e.to_string(), dir.path().join("backups")).unwrap();
        app.set_db_path(db.clone());
        let st = dispatch(&app, LOCAL, "app_status", Value::Null).unwrap();
        assert_eq!((st["needs_setup"].clone(), st["recovery"]["reason"].clone(), st["recovery"]["can_log_in"].clone()), (json!(false), json!("cannot_open"), json!(false)));
        // Nothing else runs, least of all the first-admin setup.
        assert!(dispatch(&app, LOCAL, "setup_admin", json!({ "name": "X", "pin": "9999" })).is_err());
        assert!(dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).is_err());
        let done = dispatch(&app, LOCAL, "recovery_restore", json!({ "name": good, "pin": "1111", "confirm": "RESTORE" })).unwrap();
        assert_eq!(done["restart_needed"], true);
        assert!(app.opened_damaged().is_none() && !app.backup_state().damaged);
        dispatch(&app, LOCAL, "login", json!({ "pin": "1111" })).unwrap();
        assert_eq!(dispatch(&app, LOCAL, "get_setting", json!({ "key": "store_name" })).unwrap(), "This Shop");
        drop(app);
        assert_eq!(store_name(&db), "This Shop");
    }
}

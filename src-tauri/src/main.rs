// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pos_core::*;
use serde::Serialize;
use tauri::{Manager, RunEvent, State};

const AUTO_BACKUP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const BACKUPS_KEPT: usize = 14;

/// One terminal = one database connection + at most one logged-in user.
/// The session lives here, never in the UI, so it cannot be forged.
struct Inner {
    pos: Pos,
    session: Option<Session>,
}

struct App {
    inner: Mutex<Inner>,
    backup_dir: PathBuf,
}

impl App {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` as the current user, re-validated against the database so a
    /// deactivated or demoted user loses access immediately.
    fn with_session<T>(&self, f: impl FnOnce(&mut Pos, &Session) -> Result<T>) -> Result<T> {
        let mut g = self.lock();
        let inner = &mut *g;
        let current = inner.session.as_ref().ok_or(PosError::NotLoggedIn)?;
        match inner.pos.refresh_session(current) {
            Ok(s) => {
                inner.session = Some(s.clone());
                f(&mut inner.pos, &s)
            }
            Err(_) => {
                inner.session = None;
                Err(PosError::NotLoggedIn)
            }
        }
    }
}

type Cmd<'a> = State<'a, App>;

#[derive(Serialize)]
struct AppStatus {
    needs_setup: bool,
    session: Option<Session>,
    store_name: Option<String>,
}

#[tauri::command(async)]
fn app_status(app: Cmd<'_>) -> Result<AppStatus> {
    let g = app.lock();
    Ok(AppStatus {
        needs_setup: g.pos.needs_setup()?,
        session: g.session.clone(),
        store_name: g.pos.setting("store_name")?,
    })
}

#[tauri::command(async)]
fn setup_admin(app: Cmd<'_>, name: String, pin: String) -> Result<Session> {
    let mut g = app.lock();
    let s = g.pos.setup_first_admin(&name, &pin)?;
    g.session = Some(s.clone());
    Ok(s)
}

#[tauri::command(async)]
fn login(app: Cmd<'_>, pin: String) -> Result<Session> {
    let mut g = app.lock();
    let s = g.pos.login(&pin)?;
    g.session = Some(s.clone());
    Ok(s)
}

#[tauri::command(async)]
fn logout(app: Cmd<'_>) -> Result<()> {
    let mut g = app.lock();
    if let Some(s) = g.session.take() {
        g.pos.logout(&s)?;
    }
    Ok(())
}

// ---- products ----

#[tauri::command(async)]
fn search_products(app: Cmd<'_>, query: String, limit: Option<i64>) -> Result<Vec<Product>> {
    app.with_session(|pos, s| pos.search_products(s, &query, limit.unwrap_or(50)))
}

#[tauri::command(async)]
fn find_by_code(app: Cmd<'_>, code: String) -> Result<Option<Product>> {
    app.with_session(|pos, s| pos.find_by_code(s, &code))
}

#[tauri::command(async)]
fn list_products(app: Cmd<'_>, include_inactive: bool, limit: i64, offset: i64) -> Result<Vec<Product>> {
    app.with_session(|pos, s| pos.list_products(s, include_inactive, limit, offset))
}

#[tauri::command(async)]
fn create_product(app: Cmd<'_>, product: NewProduct) -> Result<Product> {
    app.with_session(|pos, s| pos.create_product(s, product))
}

#[tauri::command(async)]
fn update_product(app: Cmd<'_>, id: i64, product: ProductUpdate) -> Result<Product> {
    app.with_session(|pos, s| pos.update_product(s, id, product))
}

#[tauri::command(async)]
fn adjust_stock(app: Cmd<'_>, id: i64, delta: i64, reason: String) -> Result<Product> {
    app.with_session(|pos, s| pos.adjust_stock(s, id, delta, &reason))
}

// ---- sales ----

#[tauri::command(async)]
fn quote_sale(app: Cmd<'_>, lines: Vec<CartLine>) -> Result<Quote> {
    app.with_session(|pos, s| pos.quote_sale(s, &lines))
}

#[tauri::command(async)]
fn complete_sale(
    app: Cmd<'_>,
    lines: Vec<CartLine>,
    method: PaymentMethod,
    tendered_cents: i64,
) -> Result<Sale> {
    app.with_session(|pos, s| pos.complete_sale(s, &lines, method, tendered_cents))
}

#[tauri::command(async)]
fn get_sale(app: Cmd<'_>, id: i64) -> Result<Sale> {
    app.with_session(|pos, s| pos.get_sale(s, id))
}

#[tauri::command(async)]
fn list_sales(app: Cmd<'_>, from: i64, to: i64, limit: i64, offset: i64) -> Result<Vec<SaleListItem>> {
    app.with_session(|pos, s| pos.list_sales(s, from, to, limit, offset))
}

#[tauri::command(async)]
fn void_sale(app: Cmd<'_>, id: i64, reason: String) -> Result<Sale> {
    app.with_session(|pos, s| pos.void_sale(s, id, &reason))
}

// ---- users ----

#[tauri::command(async)]
fn list_users(app: Cmd<'_>) -> Result<Vec<User>> {
    app.with_session(|pos, s| pos.list_users(s))
}

#[tauri::command(async)]
fn create_user(app: Cmd<'_>, name: String, role: Role, pin: String) -> Result<User> {
    app.with_session(|pos, s| pos.create_user(s, &name, role, &pin))
}

#[tauri::command(async)]
fn update_user(app: Cmd<'_>, id: i64, name: String, role: Role, active: bool) -> Result<User> {
    app.with_session(|pos, s| pos.update_user(s, id, &name, role, active))
}

#[tauri::command(async)]
fn reset_pin(app: Cmd<'_>, id: i64, pin: String) -> Result<()> {
    app.with_session(|pos, s| pos.reset_pin(s, id, &pin))
}

// ---- reports, settings, backup ----

#[tauri::command(async)]
fn sales_summary(app: Cmd<'_>, from: i64, to: i64) -> Result<SalesSummary> {
    app.with_session(|pos, s| pos.sales_summary(s, from, to))
}

#[tauri::command(async)]
fn top_products(app: Cmd<'_>, from: i64, to: i64, limit: i64) -> Result<Vec<TopProduct>> {
    app.with_session(|pos, s| pos.top_products(s, from, to, limit))
}

#[tauri::command(async)]
fn set_setting(app: Cmd<'_>, key: String, value: String) -> Result<()> {
    app.with_session(|pos, s| pos.set_setting(s, &key, &value))
}

#[tauri::command(async)]
fn backup_now(app: Cmd<'_>) -> Result<String> {
    let path = backup_path(&app.backup_dir);
    app.with_session(|pos, s| pos.backup_to(s, &path))?;
    prune_backups(&app.backup_dir);
    Ok(path.display().to_string())
}

fn backup_path(dir: &Path) -> PathBuf {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    dir.join(format!("pos-{ts}.db"))
}

fn list_backups(dir: &Path) -> Vec<(SystemTime, PathBuf)> {
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

fn prune_backups(dir: &Path) {
    for (_, old) in list_backups(dir).into_iter().skip(BACKUPS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
}

/// Daily safety copy, taken at startup when the newest one is over a day old.
fn auto_backup(pos: &Pos, dir: &Path) {
    let fresh = list_backups(dir)
        .first()
        .and_then(|(t, _)| t.elapsed().ok())
        .is_some_and(|age| age < AUTO_BACKUP_EVERY);
    if !fresh && !pos.needs_setup().unwrap_or(true) {
        if let Err(e) = pos.backup_unchecked(backup_path(dir)) {
            eprintln!("auto backup failed: {e}");
        }
        prune_backups(dir);
    }
}

fn main() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let backup_dir = data_dir.join("backups");
            std::fs::create_dir_all(&backup_dir)?;
            let pos = Pos::open(data_dir.join("pos.db"))?;
            auto_backup(&pos, &backup_dir);
            app.manage(App { inner: Mutex::new(Inner { pos, session: None }), backup_dir });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            setup_admin,
            login,
            logout,
            search_products,
            find_by_code,
            list_products,
            create_product,
            update_product,
            adjust_stock,
            quote_sale,
            complete_sale,
            get_sale,
            list_sales,
            void_sale,
            list_users,
            create_user,
            update_user,
            reset_pin,
            sales_summary,
            top_products,
            set_setting,
            backup_now,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start POS");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<App>() {
                let _ = state.lock().pos.optimize();
            }
        }
    });
}

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
    version: String,
    needs_setup: bool,
    session: Option<Session>,
    store_name: Option<String>,
    plan_width: i64,
    plan_height: i64,
}

#[tauri::command(async)]
fn app_status(app: Cmd<'_>, handle: tauri::AppHandle) -> Result<AppStatus> {
    let g = app.lock();
    Ok(AppStatus {
        version: handle.package_info().version.to_string(),
        needs_setup: g.pos.needs_setup()?,
        session: g.session.clone(),
        store_name: g.pos.setting("store_name")?,
        plan_width: PLAN_WIDTH,
        plan_height: PLAN_HEIGHT,
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

// ---- products (admin) ----

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

// ---- menu ----

#[tauri::command(async)]
fn menu(app: Cmd<'_>) -> Result<Vec<Product>> {
    app.with_session(|pos, s| pos.menu(s))
}

#[tauri::command(async)]
fn list_categories(app: Cmd<'_>, include_inactive: bool) -> Result<Vec<Category>> {
    app.with_session(|pos, s| pos.list_categories(s, include_inactive))
}

#[tauri::command(async)]
fn create_category(app: Cmd<'_>, name: String, color: Option<String>, sort_order: i64) -> Result<Category> {
    app.with_session(|pos, s| pos.create_category(s, &name, color.as_deref(), sort_order))
}

#[tauri::command(async)]
fn update_category(
    app: Cmd<'_>,
    id: i64,
    name: String,
    color: Option<String>,
    sort_order: i64,
    active: bool,
) -> Result<Category> {
    app.with_session(|pos, s| pos.update_category(s, id, &name, color.as_deref(), sort_order, active))
}

// ---- item options ----

#[tauri::command(async)]
fn menu_options(app: Cmd<'_>) -> Result<MenuOptions> {
    app.with_session(|pos, s| pos.menu_options(s))
}

#[tauri::command(async)]
fn list_option_groups(app: Cmd<'_>, include_inactive: bool) -> Result<Vec<OptionGroup>> {
    app.with_session(|pos, s| pos.list_option_groups(s, include_inactive))
}

#[tauri::command(async)]
fn create_option_group(app: Cmd<'_>, name: String, min_select: i64, max_select: i64) -> Result<OptionGroup> {
    app.with_session(|pos, s| pos.create_option_group(s, &name, min_select, max_select))
}

#[tauri::command(async)]
fn update_option_group(
    app: Cmd<'_>,
    id: i64,
    name: String,
    min_select: i64,
    max_select: i64,
    sort_order: i64,
    active: bool,
) -> Result<OptionGroup> {
    app.with_session(|pos, s| pos.update_option_group(s, id, &name, min_select, max_select, sort_order, active))
}

#[tauri::command(async)]
fn save_group_options(app: Cmd<'_>, group_id: i64, options: Vec<OptionInput>) -> Result<OptionGroup> {
    app.with_session(|pos, s| pos.save_group_options(s, group_id, &options))
}

#[tauri::command(async)]
fn set_product_option_groups(app: Cmd<'_>, product_id: i64, group_ids: Vec<i64>) -> Result<()> {
    app.with_session(|pos, s| pos.set_product_option_groups(s, product_id, &group_ids))
}

// ---- floor plan ----

#[tauri::command(async)]
fn list_floors(app: Cmd<'_>) -> Result<Vec<Floor>> {
    app.with_session(|pos, s| pos.list_floors(s))
}

#[tauri::command(async)]
fn create_floor(app: Cmd<'_>, name: String) -> Result<Floor> {
    app.with_session(|pos, s| pos.create_floor(s, &name))
}

#[tauri::command(async)]
fn update_floor(app: Cmd<'_>, id: i64, name: String, sort_order: i64) -> Result<Floor> {
    app.with_session(|pos, s| pos.update_floor(s, id, &name, sort_order))
}

#[tauri::command(async)]
fn delete_floor(app: Cmd<'_>, id: i64) -> Result<()> {
    app.with_session(|pos, s| pos.delete_floor(s, id))
}

#[tauri::command(async)]
fn list_tables(app: Cmd<'_>, floor_id: i64) -> Result<Vec<DiningTable>> {
    app.with_session(|pos, s| pos.list_tables(s, floor_id))
}

#[tauri::command(async)]
fn floor_status(app: Cmd<'_>, floor_id: i64) -> Result<Vec<TableStatus>> {
    app.with_session(|pos, s| pos.floor_status(s, floor_id))
}

#[tauri::command(async)]
fn save_floor_layout(app: Cmd<'_>, floor_id: i64, tables: Vec<TableLayout>) -> Result<Vec<DiningTable>> {
    app.with_session(|pos, s| pos.save_floor_layout(s, floor_id, &tables))
}

// ---- orders ----

#[tauri::command(async)]
fn open_order(app: Cmd<'_>, order: NewOrder) -> Result<Order> {
    app.with_session(|pos, s| pos.open_order(s, &order))
}

#[tauri::command(async)]
fn table_order(app: Cmd<'_>, table_id: i64) -> Result<Option<Order>> {
    app.with_session(|pos, s| pos.table_order(s, table_id))
}

#[tauri::command(async)]
fn get_order(app: Cmd<'_>, id: i64) -> Result<Order> {
    app.with_session(|pos, s| pos.get_order(s, id))
}

#[tauri::command(async)]
fn update_order_details(app: Cmd<'_>, id: i64, details: OrderDetails) -> Result<Order> {
    app.with_session(|pos, s| pos.update_order_details(s, id, &details))
}

#[tauri::command(async)]
fn move_order(app: Cmd<'_>, id: i64, table_id: i64) -> Result<Order> {
    app.with_session(|pos, s| pos.move_order(s, id, table_id))
}

#[tauri::command(async)]
fn add_item(
    app: Cmd<'_>,
    order_id: i64,
    product_id: i64,
    qty: i64,
    note: Option<String>,
    option_ids: Option<Vec<i64>>,
) -> Result<Order> {
    let option_ids = option_ids.unwrap_or_default();
    app.with_session(|pos, s| pos.add_item(s, order_id, product_id, qty, note.as_deref(), &option_ids))
}

#[tauri::command(async)]
fn update_item(app: Cmd<'_>, item_id: i64, update: ItemUpdate) -> Result<Order> {
    app.with_session(|pos, s| pos.update_item(s, item_id, &update))
}

/// Result of a print attempt. Printing problems never undo the action
/// (items stay "sent"); the UI shows the error and offers a reprint.
#[derive(Serialize)]
struct PrintOutcome {
    /// False when no kitchen printer is configured.
    attempted: bool,
    error: Option<String>,
}

/// Send a print job after the database lock has been released, so a slow or
/// offline printer never blocks other commands.
fn print(job: Option<PrintJob>) -> PrintOutcome {
    match job {
        None => PrintOutcome { attempted: false, error: None },
        Some(job) => PrintOutcome { attempted: true, error: job.send().err().map(|e| e.to_string()) },
    }
}

#[derive(Serialize)]
struct SendResult {
    items: Vec<OrderItem>,
    print: PrintOutcome,
}

#[tauri::command(async)]
fn send_to_kitchen(app: Cmd<'_>, order_id: i64) -> Result<SendResult> {
    let (items, job) = app.with_session(|pos, s| {
        let items = pos.send_to_kitchen(s, order_id)?;
        let job = pos.kitchen_job(s, order_id, &items, false)?;
        Ok((items, job))
    })?;
    Ok(SendResult { items, print: print(job) })
}

#[tauri::command(async)]
fn reprint_kitchen(app: Cmd<'_>, order_id: i64) -> Result<PrintOutcome> {
    let job = app.with_session(|pos, s| pos.kitchen_reprint_job(s, order_id))?;
    Ok(print(job))
}

// ---- kitchen printer setup (admin) ----

#[tauri::command(async)]
fn get_kitchen_printer(app: Cmd<'_>) -> Result<PrinterConfig> {
    app.with_session(|pos, _| pos.kitchen_printer())
}

#[tauri::command(async)]
fn set_kitchen_printer(app: Cmd<'_>, config: PrinterConfig) -> Result<()> {
    app.with_session(|pos, s| pos.set_kitchen_printer(s, &config))
}

#[tauri::command(async)]
fn test_kitchen_printer(app: Cmd<'_>, config: PrinterConfig) -> Result<PrintOutcome> {
    let job = app.with_session(|pos, s| pos.test_print_job(s, &config))?;
    Ok(print(Some(job)))
}

#[tauri::command(async)]
fn list_system_printers(app: Cmd<'_>) -> Result<Vec<String>> {
    app.with_session(|_, s| s.require(Permission::ManageSettings))?;
    Ok(system_printers())
}

#[tauri::command(async)]
fn pay_order(app: Cmd<'_>, order_id: i64, method: PaymentMethod, tendered_cents: i64) -> Result<Order> {
    app.with_session(|pos, s| pos.pay_order(s, order_id, method, tendered_cents))
}

#[tauri::command(async)]
fn cancel_order(app: Cmd<'_>, order_id: i64, reason: String) -> Result<()> {
    app.with_session(|pos, s| pos.cancel_order(s, order_id, &reason))
}

#[tauri::command(async)]
fn void_order(app: Cmd<'_>, order_id: i64, reason: String) -> Result<Order> {
    app.with_session(|pos, s| pos.void_order(s, order_id, &reason))
}

#[tauri::command(async)]
fn list_open_orders(app: Cmd<'_>) -> Result<Vec<OrderSummary>> {
    app.with_session(|pos, s| pos.list_open_orders(s))
}

#[tauri::command(async)]
fn list_orders(app: Cmd<'_>, from: i64, to: i64, limit: i64, offset: i64) -> Result<Vec<OrderSummary>> {
    app.with_session(|pos, s| pos.list_orders(s, from, to, limit, offset))
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
            menu,
            list_categories,
            create_category,
            update_category,
            list_floors,
            create_floor,
            update_floor,
            delete_floor,
            list_tables,
            floor_status,
            save_floor_layout,
            open_order,
            table_order,
            get_order,
            update_order_details,
            move_order,
            add_item,
            update_item,
            send_to_kitchen,
            reprint_kitchen,
            get_kitchen_printer,
            set_kitchen_printer,
            test_kitchen_printer,
            list_system_printers,
            menu_options,
            list_option_groups,
            create_option_group,
            update_option_group,
            save_group_options,
            set_product_option_groups,
            pay_order,
            cancel_order,
            void_order,
            list_open_orders,
            list_orders,
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

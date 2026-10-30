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
pub const BACKUPS_KEPT: usize = 14;
/// Menu-only backups kept (each is a small JSON file).
const MENU_BACKUPS_KEPT: usize = 30;

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
}

pub struct App {
    inner: Mutex<Inner>,
    pub backup_dir: PathBuf,
}

impl App {
    pub fn new(pos: Pos, backup_dir: PathBuf) -> App {
        App { inner: Mutex::new(Inner { pos, logins: HashMap::new(), network_throttle: LoginThrottle::default() }), backup_dir }
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` as the user logged in on terminal `t`, re-validated against
    /// the database so a deactivated or demoted user loses access at once.
    pub fn with_session<T>(&self, t: &str, f: impl FnOnce(&mut Pos, &Session) -> Result<T>) -> Result<T> {
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
                f(&mut inner.pos, &s)
            }
            Err(_) => {
                inner.logins.remove(t);
                Err(PosError::NotLoggedIn)
            }
        }
    }
}

impl Inner {
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
}

/// Result of a print attempt. Printing problems never undo the action
/// (items stay "sent"); the UI shows the error and offers a reprint.
#[derive(Serialize)]
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
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    dir.join(format!("pos-{ts}.db"))
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

pub fn prune_backups(dir: &Path) {
    for (_, old) in list_backups(dir).into_iter().skip(BACKUPS_KEPT) {
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
        })
    }

    fn setup_admin(app, t, name: String, pin: String) -> Session {
        let mut g = app.lock();
        let s = g.pos.setup_first_admin(&name, &pin)?;
        g.sign_in(t, &s);
        Ok(s)
    }

    fn login(app, t, pin: String) -> Session {
        let mut g = app.lock();
        let inner = &mut *g;
        let s = if t == LOCAL { inner.pos.login(&pin)? } else { inner.pos.login_with(&pin, &mut inner.network_throttle)? };
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

    fn add_item(app, t, order_id: i64, product_id: i64, qty: i64, note: Option<String>, variant_id: Option<i64>, option_ids: Option<Vec<i64>>) -> Order {
        let option_ids = option_ids.unwrap_or_default();
        app.with_session(t, |pos, s| pos.add_item(s, order_id, product_id, qty, note.as_deref(), variant_id, &option_ids))
    }

    fn update_item(app, t, item_id: i64, update: ItemUpdate) -> Order {
        app.with_session(t, |pos, s| pos.update_item(s, item_id, &update))
    }

    fn reprint_kitchen(app, t, order_id: i64) -> PrintOutcome {
        let job = app.with_session(t, |pos, s| pos.kitchen_reprint_job(s, order_id))?;
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

    fn cancel_order(app, t, order_id: i64, reason: String) -> () {
        app.with_session(t, |pos, s| pos.cancel_order(s, order_id, &reason))
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
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let backup = app.backup_dir.join(format!("pos-before-reset-{ts}.db"));
            pos.backup_unchecked(&backup)?;
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

    fn set_setting(app, t, key: String, value: String) -> () {
        app.with_session(t, |pos, s| pos.set_setting(s, &key, &value))
    }

    fn backup_now(app, t) -> String {
        let path = backup_path(&app.backup_dir);
        app.with_session(t, |pos, s| pos.backup_to(s, &path))?;
        prune_backups(&app.backup_dir);
        Ok(path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app() -> App {
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        App::new(pos, std::env::temp_dir())
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
}

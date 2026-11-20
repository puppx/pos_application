//! Restaurant POS business logic over a local SQLite database.
//!
//! Everything the app can do goes through [`Pos`]. Methods that need
//! authorisation take a [`Session`], which only `Pos::login` can produce.

mod auth;
mod channels;
mod customers;
mod dayclose;
mod error;
mod floors;
mod kds;
mod lan;
mod maintenance;
mod menu;
mod menu_tools;
mod options;
mod orders;
mod printing;
mod raster;
mod receipt;
mod products;
mod reports;
mod restore;
mod roles;
mod service;
mod settings;
mod shifts;
mod sets;
mod teams;
mod users;
mod variants;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};

pub use auth::{HashedPin, LoginThrottle, LoginUser, PinHasher};
pub use channels::{Channel, ChannelInput};
pub use dayclose::{BusinessDay, DayClose};
pub use customers::{normalize_postcode, phone_digits, Customer, CustomerInput};
pub use error::{PosError, Result};
pub use floors::{DiningTable, Floor, TableLayout, TableShape, TableStatus, PLAN_HEIGHT, PLAN_WIDTH};
pub use kds::{BoardItem, BoardOrder};
pub use lan::ServerSettings;
pub use maintenance::{MenuBackup, MenuCategory, MenuCounts, MenuItem, MenuOptionGroup, MenuPrice, MenuSetSection, MenuTeam, ResetOptions, ResetReport, SalesReset};
pub use menu::{Category, FLAG_OFF_DESSERT, FLAG_OFF_DRINK, FLAG_OFF_PREP, FLAG_OFF_STARTER};
pub use menu_tools::{ImportReport, ImportRow};
pub use options::{ItemOption, MenuOptions, OptionChoice, OptionGroup, OptionInput, ProductGroupLink};
pub use receipt::{Receipt, ReceiptLine, ReceiptSettings};
pub use printing::{
    paced_duration, print_failures, set_print_error_log, system_printers, wait_for_prints, write_paced, ImageMode, InFlight, PrintFailure, PrintFailures, KitchenRoute, KitchenTicket, PrintJob, PrintNames, PrinterConfig, PrinterTarget, TicketItem,
    PRINT_CANCELLED_ORDER_KEY, SLOW_CHUNK, SLOW_PAUSE,
};
pub use orders::{
    ItemUpdate, NewOrder, Order, OrderDetails, OrderItem, OrderSummary, OrderType, Payment, PaymentInput, PaymentMethod, EMPTY_ORDER_AGE, EMPTY_ORDER_SWEPT, OPEN_ITEM_SKU,
};
pub use products::{NewProduct, Product, ProductUpdate};
pub use reports::{GroupTotal, SalesSummary, TopProduct};
pub use restore::{io_error, is_disk_full, BackupSummary, PreparedRestore, DISK_FULL};
pub use service::{ServiceSettings, MAX_SERVICE_BP};
pub use settings::{
    discount_presets, PaymentNames, BACKUPS_KEPT_DEFAULT, BACKUPS_KEPT_KEY, BACKUPS_KEPT_RANGE, DISCOUNT_LIMIT_DEFAULT, DISCOUNT_LIMIT_KEY, MAX_DISCOUNT_PRESETS, MAX_PAY_NAME,
    PAY_NAME_CARD_KEY, PAY_NAME_CASH_KEY,
};
pub use shifts::{CashMove, Shift};
pub use sets::{SetChoice, SetPick, SetSection, SetSectionInput};
pub use teams::{Team, TeamInput, TEAM_CHOICE_KEY};
pub use roles::{Permission, Role, Session};
pub use users::User;
pub use variants::{Variant, VariantInput};

/// Database migrations, applied in order and tracked in `PRAGMA user_version`.
///
/// Released installs already contain these, so NEVER edit an entry once it has
/// shipped (v0.1.0 shipped `schema.sql`). To change the schema, append a new
/// file (e.g. `migrations/0004_xxx.sql`) holding only the ALTER/CREATE
/// statements; existing databases are upgraded on the next start.
const MIGRATIONS: &[&str] = &[
    include_str!("schema.sql"),
    include_str!("migrations/0002_delivery_customers.sql"),
    include_str!("migrations/0003_item_choices.sql"),
    include_str!("migrations/0004_courses_day_close.sql"),
    include_str!("migrations/0005_service_charge.sql"),
    include_str!("migrations/0006_deleted_items.sql"),
    include_str!("migrations/0007_menu_editing.sql"),
    include_str!("migrations/0008_thai_names.sql"),
    include_str!("migrations/0009_order_tax.sql"),
    include_str!("migrations/0010_prep_ahead.sql"),
    include_str!("migrations/0011_tax_included.sql"),
    include_str!("migrations/0012_teams.sql"),
    include_str!("migrations/0013_drinks.sql"),
    include_str!("migrations/0014_pin_length.sql"),
    include_str!("migrations/0015_set_menus.sql"),
    include_str!("migrations/0016_set_pairings.sql"),
    include_str!("migrations/0017_desserts.sql"),
    include_str!("migrations/0018_payments.sql"),
    include_str!("migrations/0019_shifts.sql"),
    include_str!("migrations/0020_sold_out.sql"),
    include_str!("migrations/0021_kitchen_screen.sql"),
    include_str!("migrations/0022_manager_role.sql"),
    include_str!("migrations/0023_bill_printed.sql"),
    include_str!("migrations/0024_shared_pins.sql"),
    include_str!("migrations/0025_sub_categories.sql"),
    include_str!("migrations/0026_sub_category_off.sql"),
    include_str!("migrations/0027_item_team.sql"),
    include_str!("migrations/0028_plan_labels.sql"),
    include_str!("migrations/0029_line_menu_price.sql"),
    include_str!("migrations/0030_order_activity.sql"),
    include_str!("migrations/0031_payment_ids.sql"),
];
/// The schema this app creates; backups up to this version can be restored.
pub const SCHEMA_VERSION: i32 = MIGRATIONS.len() as i32;

pub struct Pos {
    conn: Connection,
    pin_salt: [u8; 16],
    pin_params: argon2::Params,
    throttle: LoginThrottle,
    /// When forgotten empty orders were last cancelled (see `sweep_empty_orders_if_due`).
    last_empty_sweep: Option<std::time::Instant>,
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
        // Over a hundred statements are prepared once and reused; keep them all.
        conn.set_prepared_statement_cache_capacity(256);
        migrate(&conn)?;
        conn.execute_batch(receipt::BILL_STALE_TRIGGERS)?;
        conn.execute_batch(orders::ACTIVITY_TRIGGERS)?;
        let pin_salt = load_or_create_salt(&conn)?;
        if database_id(&conn)?.is_none() {
            new_database_id(&conn)?;
        }
        Ok(Pos { conn, pin_salt, pin_params, throttle: LoginThrottle::default(), last_empty_sweep: None })
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
        // Managers may change the kitchen settings and the staff discount
        // limit; the rest is for admins.
        const KITCHEN: &[&str] = &[
            "drinks_split", "desserts_split", "teams_on", "teams_split", "team_names", "quick_notes", "starter_choice", teams::TEAM_CHOICE_KEY,
            "kds_slow_mins", "kds_late_mins", "kds_refresh_secs", "kds_paid_hours", "kds_beep", printing::PRINT_CANCELLED_ORDER_KEY,
        ];
        // The staff discount limit: whoever may approve a bigger discount
        // (managers and admins) may set it.
        s.require(if KITCHEN.contains(&key) {
            Permission::KitchenSettings
        } else if key == settings::DISCOUNT_LIMIT_KEY {
            Permission::AnyDiscount
        } else {
            Permission::ManageSettings
        })?;
        if key.starts_with('_') {
            return error::invalid("reserved setting key");
        }
        settings::validate(key, value)?;
        let tidy;
        let value = if key == teams::TEAM_CHOICE_KEY {
            tidy = self.clean_team_choice(value)?;
            tidy.as_str()
        } else {
            value
        };
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

    /// Consistent copy of the database file at `src` into `dest` through a
    /// connection of its own, so it needs no [`Pos`] (nor the lock around
    /// one): in WAL mode the till keeps selling while it runs, and it sees
    /// the data as it was when it started. For the automatic daily backup.
    pub fn backup_database_file(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<()> {
        use rusqlite::OpenFlags;
        let src = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        src.busy_timeout(Duration::from_secs(5))?;
        let mut dest = Connection::open(dest)?;
        // One step: the whole copy in one read transaction. Steps of a few
        // pages would start again after every sale made meanwhile.
        let backup = rusqlite::backup::Backup::new(&src, &mut dest)?;
        for _ in 0..20 {
            match backup.step(-1)? {
                rusqlite::backup::StepResult::Done => return Ok(()),
                _ => std::thread::sleep(Duration::from_millis(250)),
            }
        }
        Err(PosError::Conflict("the database is busy; try again".into()))
    }

    /// Random id of this database's contents, changed by every restore, so
    /// terminals can tell the data was swapped under them and reload.
    pub fn database_id(&self) -> Result<String> {
        Ok(database_id(&self.conn)?.unwrap_or_default())
    }

    /// Run on shutdown: lets SQLite refresh planner statistics cheaply.
    pub fn optimize(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA optimize;")?;
        Ok(())
    }

    /// Record something the app layer did (e.g. deleted backup files).
    pub fn audit_event(&self, s: &Session, action: &str, detail: &str) -> Result<()> {
        self.audit(Some(s.user_id), action, detail)
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

const DB_ID_KEY: &str = "_db_id";

fn database_id(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT CAST(value AS TEXT) FROM settings WHERE key = ?1", [DB_ID_KEY], |r| r.get(0))
        .optional()?)
}

/// Give the database a new random id (see [`Pos::database_id`]).
fn new_database_id(conn: &Connection) -> Result<String> {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).map_err(|e| PosError::Internal(e.to_string()))?;
    let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [DB_ID_KEY, &id],
    )?;
    Ok(id)
}

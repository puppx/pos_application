//! Checks for the settings that have a fixed shape, and the ones the server
//! itself acts on (the staff discount limit). Everything else in `settings`
//! is free text the UI reads back.

use rusqlite::{Connection, OptionalExtension};

use crate::error::invalid;
use crate::{Pos, Result};

/// The biggest discount, in percent of the line, staff may give without a
/// manager's or admin's PIN. Not set or empty: no limit.
pub const DISCOUNT_LIMIT_KEY: &str = "discount_staff_max_pct";

/// At most this many quick discount buttons.
pub const MAX_DISCOUNT_PRESETS: usize = 8;

/// What Cash and Card are called on the till, receipts and reports (display
/// only: payments are still stored as "cash" / "card"). Empty = the default.
pub const PAY_NAME_CASH_KEY: &str = "pay_name_cash";
pub const PAY_NAME_CARD_KEY: &str = "pay_name_card";
/// The longest payment method name, in characters.
pub const MAX_PAY_NAME: usize = 24;

/// Automatic full backups kept in the backup folder (setting `backups_kept`).
pub const BACKUPS_KEPT_KEY: &str = "backups_kept";
pub const BACKUPS_KEPT_DEFAULT: usize = 14;
pub const BACKUPS_KEPT_RANGE: (usize, usize) = (3, 90);

/// `value` must fit `key` when the key has a fixed shape; other keys pass.
pub(crate) fn validate(key: &str, value: &str) -> Result<()> {
    let v = value.trim();
    match key {
        "start_screen" if !matches!(v, "" | "tables" | "orders" | "takeaway") => {
            invalid("start screen: choose Tables, Orders or New takeaway")
        }
        "show_takeaway" | "show_collection" | "show_delivery" if !matches!(value, "0" | "1") => invalid("use 1 (show) or 0 (hide)"),
        "button_size" if !matches!(v, "" | "normal" | "large") => invalid("button size: normal or large"),
        "auto_logout_warn_secs" if !whole_in(v, 0, 120) => invalid("warning: whole seconds from 0 to 120"),
        DISCOUNT_LIMIT_KEY if !v.is_empty() && !whole_in(v, 0, 100) => invalid("discount limit: a whole percentage from 0 to 100, or empty for no limit"),
        "discount_presets" => discount_presets(v).map(|_| ()),
        PAY_NAME_CASH_KEY | PAY_NAME_CARD_KEY if v.chars().count() > MAX_PAY_NAME || v.chars().any(char::is_control) => {
            invalid(format!("payment name: up to {MAX_PAY_NAME} characters on one line"))
        }
        BACKUPS_KEPT_KEY if !v.is_empty() && !whole_in(v, BACKUPS_KEPT_RANGE.0 as i64, BACKUPS_KEPT_RANGE.1 as i64) => invalid(format!(
            "backups kept: a whole number from {} to {}",
            BACKUPS_KEPT_RANGE.0, BACKUPS_KEPT_RANGE.1
        )),
        "kds_beep" if !matches!(value, "0" | "1") => invalid("use 1 (beep) or 0 (silent)"),
        // Switches: exactly 1 or 0 (" 0" would read differently on the screen).
        crate::printing::PRINT_CANCELLED_ORDER_KEY if !matches!(value, "0" | "1") => invalid("use 1 (print) or 0 (do not print)"),
        _ => Ok(()),
    }
}

fn whole_in(v: &str, lo: i64, hi: i64) -> bool {
    v.parse::<i64>().is_ok_and(|n| (lo..=hi).contains(&n))
}

/// "5, 10,15" -> [5, 10, 15]: whole percentages from 1 to 100, at most
/// [`MAX_DISCOUNT_PRESETS`]. Empty means no buttons.
pub fn discount_presets(v: &str) -> Result<Vec<i64>> {
    let parts: Vec<&str> = v.split([',', ';', ' ']).map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.len() > MAX_DISCOUNT_PRESETS {
        return invalid(format!("up to {MAX_DISCOUNT_PRESETS} discount buttons"));
    }
    parts
        .iter()
        .map(|p| match p.trim_end_matches('%').parse::<i64>() {
            Ok(n) if (1..=100).contains(&n) => Ok(n),
            _ => invalid("discount buttons: whole percentages from 1 to 100, separated by commas"),
        })
        .collect()
}

/// The staff discount limit in percent, if one is set.
pub(crate) fn discount_limit(conn: &Connection) -> Result<Option<i64>> {
    let raw: Option<String> = conn
        .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
        .query_row([DISCOUNT_LIMIT_KEY], |r| r.get(0))
        .optional()?;
    Ok(raw.and_then(|v| v.trim().parse::<i64>().ok()).filter(|n| (0..=100).contains(n)))
}

/// The names shown for the payment methods (see [`PAY_NAME_CASH_KEY`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentNames {
    pub cash: String,
    pub card: String,
}

impl Default for PaymentNames {
    fn default() -> Self {
        PaymentNames { cash: "Cash".into(), card: "Card".into() }
    }
}

impl PaymentNames {
    /// The name for a stored method ("cash", "card"), or None for others.
    pub fn of(&self, method: &str) -> Option<&str> {
        match method {
            "cash" => Some(&self.cash),
            "card" => Some(&self.card),
            _ => None,
        }
    }
}

impl Pos {
    /// What Cash and Card are called; not set or empty = "Cash" / "Card".
    pub fn payment_names(&self) -> Result<PaymentNames> {
        let get = |key: &str, def: &str| -> Result<String> {
            Ok(self.setting(key)?.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| def.to_string()))
        };
        Ok(PaymentNames { cash: get(PAY_NAME_CASH_KEY, "Cash")?, card: get(PAY_NAME_CARD_KEY, "Card")? })
    }

    /// How many automatic full backups to keep (setting `backups_kept`,
    /// 3 to 90; not set, empty or unreadable = 14).
    pub fn backups_kept(&self) -> Result<usize> {
        Ok(self
            .setting(BACKUPS_KEPT_KEY)?
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| (BACKUPS_KEPT_RANGE.0..=BACKUPS_KEPT_RANGE.1).contains(n))
            .unwrap_or(BACKUPS_KEPT_DEFAULT))
    }
}

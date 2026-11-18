//! Checks for the settings that have a fixed shape, and the ones the server
//! itself acts on (the staff discount limit). Everything else in `settings`
//! is free text the UI reads back.

use rusqlite::{Connection, OptionalExtension};

use crate::error::invalid;
use crate::Result;

/// The biggest discount, in percent of the line, staff may give without a
/// manager's or admin's PIN. Not set or empty: no limit.
pub const DISCOUNT_LIMIT_KEY: &str = "discount_staff_max_pct";

/// At most this many quick discount buttons.
pub const MAX_DISCOUNT_PRESETS: usize = 8;

/// `value` must fit `key` when the key has a fixed shape; other keys pass.
pub(crate) fn validate(key: &str, value: &str) -> Result<()> {
    let v = value.trim();
    match key {
        "start_screen" if !matches!(v, "" | "tables" | "orders" | "takeaway") => {
            invalid("start screen: choose Tables, Orders or New takeaway")
        }
        "show_takeaway" | "show_collection" | "show_delivery" if !matches!(v, "0" | "1") => invalid("use 1 (show) or 0 (hide)"),
        "button_size" if !matches!(v, "" | "normal" | "large") => invalid("button size: normal or large"),
        "auto_logout_warn_secs" if !whole_in(v, 0, 120) => invalid("warning: whole seconds from 0 to 120"),
        DISCOUNT_LIMIT_KEY if !v.is_empty() && !whole_in(v, 0, 100) => invalid("discount limit: a whole percentage from 0 to 100, or empty for no limit"),
        "discount_presets" => discount_presets(v).map(|_| ()),
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

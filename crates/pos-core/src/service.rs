//! Service charge settings: a percentage added to the bill for the chosen
//! order types (normally dine-in). Each order keeps its own rate, so staff
//! can change or remove it on one order without touching the others.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{OrderType, Permission, Pos, PosError, Result, Session};

const SETTING_KEY: &str = "_service_charge";
/// 30.00%: anything higher is surely a typing mistake.
pub const MAX_SERVICE_BP: i64 = 3_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSettings {
    /// Basis points: 1250 = 12.5%. 0 switches the service charge off.
    #[serde(default)]
    pub rate_bp: i64,
    /// Shown on the bill and receipt, e.g. "Service charge" or "Service".
    #[serde(default = "default_label")]
    pub label: String,
    /// New orders of these types get the charge.
    #[serde(default = "dine_in")]
    pub order_types: Vec<OrderType>,
}

fn default_label() -> String {
    "Service charge".into()
}

fn dine_in() -> Vec<OrderType> {
    vec![OrderType::DineIn]
}

impl Default for ServiceSettings {
    fn default() -> Self {
        ServiceSettings { rate_bp: 0, label: default_label(), order_types: dine_in() }
    }
}

/// Service on a pre-tax amount, rounded half up.
pub(crate) fn service_of(amount: i64, rate_bp: i64) -> i64 {
    (amount * rate_bp + 5_000) / 10_000
}

pub(crate) fn check_rate(rate_bp: i64) -> Result<()> {
    if !(0..=MAX_SERVICE_BP).contains(&rate_bp) {
        return invalid("service charge must be between 0% and 30%");
    }
    Ok(())
}

impl Pos {
    pub fn service_settings(&self) -> Result<ServiceSettings> {
        let raw: Option<String> = self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([SETTING_KEY], |r| r.get(0))
            .optional()?;
        Ok(raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
    }

    pub fn set_service_settings(&self, s: &Session, cfg: &ServiceSettings) -> Result<()> {
        s.require(Permission::ManageSettings)?;
        check_rate(cfg.rate_bp)?;
        let label = cfg.label.trim();
        if label.is_empty() || label.chars().count() > 30 {
            return invalid("the service charge name must be 1 to 30 characters");
        }
        let cfg = ServiceSettings { label: label.to_string(), ..cfg.clone() };
        let json = serde_json::to_string(&cfg).map_err(|e| PosError::Internal(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [SETTING_KEY, &json],
        )?;
        self.audit(Some(s.user_id), "service.settings", &json)
    }

    /// Rate for a new order of type `t`.
    pub(crate) fn service_rate_for(&self, t: OrderType) -> Result<i64> {
        let cfg = self.service_settings()?;
        Ok(if cfg.order_types.contains(&t) { cfg.rate_bp } else { 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::service_of;

    #[test]
    fn rounding() {
        assert_eq!(service_of(2000, 1250), 250);
        assert_eq!(service_of(1999, 1250), 250); // 249.875
        assert_eq!(service_of(4, 1250), 1); // 0.5 -> 1
        assert_eq!(service_of(0, 1250), 0);
        assert_eq!(service_of(5000, 0), 0);
    }
}

//! Settings for the optional local network server, which lets other
//! devices (tablets, laptops) use this POS in a web browser. The server
//! itself lives in the desktop app; this only stores its settings.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{Permission, Pos, PosError, Result, Session};

const SETTING_KEY: &str = "_lan_server";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSettings {
    /// Start the server when the POS app opens.
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    8080
}

impl Default for ServerSettings {
    fn default() -> Self {
        ServerSettings { auto_start: false, port: default_port() }
    }
}

impl Pos {
    pub fn server_settings(&self) -> Result<ServerSettings> {
        let raw: Option<String> = self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([SETTING_KEY], |r| r.get(0))
            .optional()?;
        Ok(raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
    }

    pub fn set_server_settings(&self, s: &Session, cfg: &ServerSettings) -> Result<()> {
        s.require(Permission::ManageSettings)?;
        if cfg.port < 1024 {
            return invalid("choose a port from 1024 to 65535 (8080 is usual)");
        }
        let json = serde_json::to_string(cfg).map_err(|e| PosError::Internal(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [SETTING_KEY, &json],
        )?;
        self.audit(Some(s.user_id), "server.settings", &json)
    }
}

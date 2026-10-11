use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Params, Version};
use rusqlite::{params, OptionalExtension};

use crate::error::invalid;
use crate::{Pos, PosError, Result, Role, Session};

/// Free attempts before lockout starts.
const FREE_ATTEMPTS: u32 = 5;
const BASE_LOCKOUT: Duration = Duration::from_secs(30);
const MAX_LOCKOUT: Duration = Duration::from_secs(15 * 60);

/// OWASP minimum for Argon2id (19 MiB, 2 passes): ~20-60 ms per login on
/// low-end CPUs, but makes offline guessing of a stolen database slow.
pub(crate) fn default_params() -> Params {
    Params::new(19 * 1024, 2, 1, Some(32)).expect("valid argon2 params")
}

pub(crate) fn test_params() -> Params {
    Params::new(8, 1, 1, Some(32)).expect("valid argon2 params")
}

pub(crate) fn validate_pin(pin: &str) -> Result<()> {
    if !(4..=8).contains(&pin.len()) || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return invalid("PIN must be 4 to 8 digits");
    }
    Ok(())
}

/// Brute-force protection for the PIN pad: after a few consecutive failures,
/// all logins on this terminal are locked out with exponential backoff.
#[derive(Debug, Default)]
pub struct LoginThrottle {
    failures: u32,
    locked_until: Option<Instant>,
}

impl LoginThrottle {
    pub fn check(&self, now: Instant) -> Result<()> {
        match self.locked_until {
            Some(until) if until > now => {
                Err(PosError::LockedOut((until - now).as_secs_f64().ceil() as u64))
            }
            _ => Ok(()),
        }
    }

    pub fn record_failure(&mut self, now: Instant) {
        self.failures += 1;
        if self.failures >= FREE_ATTEMPTS {
            let shift = (self.failures - FREE_ATTEMPTS).min(10);
            let wait = (BASE_LOCKOUT * (1 << shift)).min(MAX_LOCKOUT);
            self.locked_until = Some(now + wait);
        }
    }

    pub fn record_success(&mut self) {
        *self = LoginThrottle::default();
    }
}

impl Pos {
    pub(crate) fn hash_pin(&self, pin: &str) -> Result<Vec<u8>> {
        validate_pin(pin)?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, self.pin_params.clone());
        let mut out = vec![0u8; 32];
        argon
            .hash_password_into(pin.as_bytes(), &self.pin_salt, &mut out)
            .map_err(|e| PosError::Internal(e.to_string()))?;
        Ok(out)
    }

    /// True until the first admin has been created.
    pub fn needs_setup(&self) -> Result<bool> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        Ok(n == 0)
    }

    /// First-run only: create the initial admin and log them in.
    pub fn setup_first_admin(&mut self, name: &str, pin: &str) -> Result<Session> {
        let name = crate::users::clean_name(name)?;
        let hash = self.hash_pin(pin)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let n: i64 = tx.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        if n > 0 {
            return Err(PosError::Forbidden);
        }
        tx.execute(
            "INSERT INTO users (name, role, pin_hash, last_login_at) VALUES (?1, 'admin', ?2, unixepoch())",
            params![name, hash],
        )?;
        let user_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO audit_log (user_id, action) VALUES (?1, 'setup.admin_created')",
            [user_id],
        )?;
        tx.commit()?;
        Ok(Session { user_id, name, role: Role::Admin })
    }

    pub fn login(&mut self, pin: &str) -> Result<Session> {
        self.throttle.check(Instant::now())?;
        let found = match self.hash_pin(pin) {
            Ok(hash) => self
                .conn
                .prepare_cached("SELECT id, name, role FROM users WHERE pin_hash = ?1 AND active = 1")?
                .query_row([hash], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                })
                .optional()?,
            Err(_) => None,
        };
        let Some((user_id, name, role)) = found else {
            self.throttle.record_failure(Instant::now());
            self.audit(None, "login.failed", "")?;
            return Err(PosError::InvalidPin);
        };
        self.throttle.record_success();
        self.conn
            .prepare_cached("UPDATE users SET last_login_at = unixepoch() WHERE id = ?1")?
            .execute([user_id])?;
        self.audit(Some(user_id), "login", "")?;
        let role = Role::parse(&role).ok_or_else(|| PosError::Internal("bad role".into()))?;
        Ok(Session { user_id, name, role })
    }

    pub fn logout(&self, s: &Session) -> Result<()> {
        self.audit(Some(s.user_id), "logout", "")
    }

    /// Re-check a session against the database, e.g. after an admin edits
    /// users, so a deactivated or demoted user loses access immediately.
    pub fn refresh_session(&self, s: &Session) -> Result<Session> {
        let row = self
            .conn
            .prepare_cached("SELECT name, role FROM users WHERE id = ?1 AND active = 1")?
            .query_row([s.user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .optional()?;
        let (name, role) = row.ok_or(PosError::Forbidden)?;
        let role = Role::parse(&role).ok_or_else(|| PosError::Internal("bad role".into()))?;
        Ok(Session { user_id: s.user_id, name, role })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_format() {
        assert!(validate_pin("1234").is_ok());
        assert!(validate_pin("12345678").is_ok());
        assert!(validate_pin("123").is_err());
        assert!(validate_pin("123456789").is_err());
        assert!(validate_pin("12a4").is_err());
    }

    #[test]
    fn throttle_backoff() {
        let t0 = Instant::now();
        let mut t = LoginThrottle::default();
        for _ in 0..4 {
            t.record_failure(t0);
            assert!(t.check(t0).is_ok());
        }
        t.record_failure(t0);
        assert!(matches!(t.check(t0), Err(PosError::LockedOut(30))));
        assert!(t.check(t0 + Duration::from_secs(31)).is_ok());
        t.record_failure(t0);
        assert!(matches!(t.check(t0), Err(PosError::LockedOut(60))));
        t.record_success();
        assert!(t.check(t0).is_ok());
    }
}

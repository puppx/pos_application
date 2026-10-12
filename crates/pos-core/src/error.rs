use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum PosError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("invalid PIN")]
    InvalidPin,
    #[error("too many failed attempts, try again in {0} seconds")]
    LockedOut(u64),
    #[error("please log in")]
    NotLoggedIn,
    #[error("you do not have permission to do that")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    /// The printer could not be reached or refused the job.
    #[error("{0}")]
    Printer(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, PosError>;

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(PosError::Validation(msg.into()))
}

/// Errors cross the IPC boundary as plain messages; internals stay in logs.
impl Serialize for PosError {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// Map SQLite UNIQUE violations to a friendly conflict message.
pub(crate) fn map_unique(e: rusqlite::Error, msg: &str) -> PosError {
    match &e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            PosError::Conflict(msg.to_string())
        }
        _ => PosError::Db(e),
    }
}

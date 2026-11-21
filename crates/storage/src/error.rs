//! Error type shared by the storage foundation.

use std::fmt;

use rusqlite::ErrorCode;

/// Every failure surfaced by the storage foundation.
///
/// The variants deliberately avoid carrying raw setting values, secret
/// references or record payloads so that error messages can be logged without
/// leaking user data.
#[derive(Debug)]
pub enum StorageError {
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// SQLite reported an error that is not mapped to a more specific variant.
    Sqlite { code: i32, message: String },
    /// A WAL checkpoint or connection close failed before a destructive move.
    CheckpointFailed { reason: String },
    /// `SQLITE_FULL`: the disk or quota backing the database is exhausted.
    DiskFull,
    /// The database file is corrupt or is not an SQLite database.
    Corrupt { detail: String },
    /// `SQLITE_BUSY` / `SQLITE_LOCKED`: the database could not be locked in time.
    Busy,
    /// The writer actor has stopped; no further operations are possible.
    Closed,
    /// Another process (or another handle in this process) already holds the
    /// durable-store lock. Opening read-write would risk concurrent migration
    /// and restore, so the second open is refused.
    AlreadyLocked {
        /// Lock file that is held.
        path: std::path::PathBuf,
        /// PID written by the holder, when it could be read.
        holder_pid: Option<u32>,
    },
    /// A compare-and-swap expectation did not match the stored revision.
    Conflict {
        entity: &'static str,
        key: String,
        expected: Option<u64>,
        actual: Option<u64>,
    },
    /// A caller supplied a value that does not satisfy the setting grammar.
    InvalidSetting { key: &'static str, reason: String },
    /// A stored preference could not be parsed or validated; the safe default
    /// should be used and the row left untouched.
    InvalidStoredValue { key: &'static str },
    /// A credential reference was rejected. The attempted value is never
    /// included in the error.
    InvalidSecretRef { reason: &'static str },
    /// A durable user record failed validation.
    InvalidRecord { reason: String },
    /// A cache entry failed validation.
    InvalidCacheEntry { reason: String },
    /// A configuration value is out of range or otherwise unusable.
    InvalidConfig { reason: String },
    /// A migration plan is empty, duplicated or out of order.
    InvalidMigrationPlan { reason: String },
    /// The versions recorded in `schema_migrations` do not match the plan.
    InvalidMigrationState { reason: String },
    /// The database was written by a newer application version.
    SchemaTooNew { found: u32, supported: u32 },
    /// A backup or restore candidate failed verification.
    BackupVerification { reason: String },
    /// A deterministic fault injected by a test harness.
    InjectedFault { reason: &'static str },
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "storage I/O error: {err}"),
            Self::Sqlite { code, message } => {
                write!(f, "sqlite error {code}: {message}")
            }
            Self::CheckpointFailed { reason } => {
                write!(f, "storage checkpoint failed: {reason}")
            }
            Self::DiskFull => write!(f, "storage device is full"),
            Self::Corrupt { detail } => write!(f, "database is corrupt: {detail}"),
            Self::Busy => write!(f, "database is busy"),
            Self::Closed => write!(f, "storage writer has stopped"),
            Self::AlreadyLocked { path, holder_pid } => match holder_pid {
                Some(pid) => write!(
                    f,
                    "another Ansa instance (pid {pid}) already holds the storage lock at {}; close it and retry",
                    path.display()
                ),
                None => write!(
                    f,
                    "another Ansa instance already holds the storage lock at {}; close it and retry",
                    path.display()
                ),
            },
            Self::Conflict {
                entity,
                key,
                expected,
                actual,
            } => write!(
                f,
                "{entity} revision conflict for {key}: expected {expected:?}, actual {actual:?}"
            ),
            Self::InvalidSetting { key, reason } => {
                write!(f, "invalid setting {key}: {reason}")
            }
            Self::InvalidStoredValue { key } => {
                write!(f, "stored setting {key} is invalid; using the safe default")
            }
            Self::InvalidSecretRef { reason } => write!(f, "invalid secret reference: {reason}"),
            Self::InvalidRecord { reason } => write!(f, "invalid user record: {reason}"),
            Self::InvalidCacheEntry { reason } => write!(f, "invalid cache entry: {reason}"),
            Self::InvalidConfig { reason } => write!(f, "invalid storage configuration: {reason}"),
            Self::InvalidMigrationPlan { reason } => {
                write!(f, "invalid migration plan: {reason}")
            }
            Self::InvalidMigrationState { reason } => {
                write!(f, "invalid database migration state: {reason}")
            }
            Self::SchemaTooNew { found, supported } => write!(
                f,
                "database schema version {found} is newer than supported version {supported}"
            ),
            Self::BackupVerification { reason } => {
                write!(f, "backup verification failed: {reason}")
            }
            Self::InjectedFault { reason } => write!(f, "injected storage fault: {reason}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for StorageError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(value: rusqlite::Error) -> Self {
        map_sqlite_error(value)
    }
}

fn map_sqlite_error(error: rusqlite::Error) -> StorageError {
    match error {
        rusqlite::Error::SqliteFailure(code, message) => match code.code {
            ErrorCode::DiskFull => StorageError::DiskFull,
            ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => StorageError::Corrupt {
                detail: message.unwrap_or_else(|| "sqlite reported corruption".to_owned()),
            },
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => StorageError::Busy,
            _ => StorageError::Sqlite {
                code: code.extended_code,
                message: message.unwrap_or_else(|| format!("{code:?}")),
            },
        },
        other => StorageError::Sqlite {
            code: 0,
            message: other.to_string(),
        },
    }
}

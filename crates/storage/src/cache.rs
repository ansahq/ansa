//! Rebuildable cache store.
//!
//! The cache lives in a separate SQLite file with its own write connection.
//! Clearing it can never touch durable user records or preferences.
//!
//! Cache values are unencrypted. Do not put secrets or sensitive clipboard
//! bodies in the cache; this file is rebuildable and may be cleared at any
//! time.

use rusqlite::Connection;

use crate::error::StorageError;

const MAX_CACHE_KEY: usize = 256;
const MAX_CACHE_VALUE: usize = 1024 * 1024;

pub(crate) fn initialize(connection: &Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS cache_entries (
             key TEXT PRIMARY KEY,
             value BLOB NOT NULL,
             expires_at INTEGER
         ) STRICT;
         PRAGMA user_version = 1;",
    )?;
    Ok(())
}

pub(crate) fn validate_key(key: &str) -> Result<(), StorageError> {
    if key.is_empty() || key.len() > MAX_CACHE_KEY {
        return Err(StorageError::InvalidCacheEntry {
            reason: "cache key must be 1..=256 bytes".to_owned(),
        });
    }
    if key.chars().any(char::is_control) {
        return Err(StorageError::InvalidCacheEntry {
            reason: "cache key must not contain control characters".to_owned(),
        });
    }
    Ok(())
}

pub(crate) fn validate_value(value: &[u8]) -> Result<(), StorageError> {
    if value.len() > MAX_CACHE_VALUE {
        return Err(StorageError::InvalidCacheEntry {
            reason: "cache value exceeds the 1 MiB bound".to_owned(),
        });
    }
    Ok(())
}

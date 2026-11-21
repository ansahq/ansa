//! Durable user records.
//!
//! These records live in the main database and are never cleared by cache
//! maintenance. They are revisioned so concurrent edits can be detected.
//!
//! The payload is stored as unencrypted JSON. Callers must not place
//! clipboard bodies, note text, tokens or other sensitive content here until
//! the encrypted-payload layer exists; this crate is a structural foundation,
//! not a confidentiality boundary.

use std::fmt;

use crate::error::StorageError;

const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// A durable, application-owned record such as a note, quicklink or layout.
#[derive(Clone, PartialEq, Eq)]
pub struct UserRecord {
    /// Stable identifier.
    pub id: String,
    /// Application-defined record kind.
    pub kind: String,
    /// Starts at 1 and increments on every accepted update.
    pub revision: u64,
    /// Opaque JSON payload.
    pub payload_json: String,
    /// Creation time in Unix seconds.
    pub created_at: i64,
    /// Last accepted update time in Unix seconds.
    pub updated_at: i64,
}

impl fmt::Debug for UserRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserRecord")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("revision", &self.revision)
            .field("payload_bytes", &self.payload_json.len())
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

pub(crate) fn validate_id(id: &str) -> Result<(), StorageError> {
    if !is_identifier(id, 128) {
        return Err(StorageError::InvalidRecord {
            reason: "id must be 1..=128 letters, digits, '.', '_', ':' or '-'".to_owned(),
        });
    }
    Ok(())
}

pub(crate) fn validate_kind(kind: &str) -> Result<(), StorageError> {
    if !is_identifier(kind, 64) {
        return Err(StorageError::InvalidRecord {
            reason: "kind must be 1..=64 letters, digits, '.', '_', ':' or '-'".to_owned(),
        });
    }
    Ok(())
}

pub(crate) fn validate_payload(payload_json: &str) -> Result<(), StorageError> {
    if payload_json.len() > MAX_PAYLOAD_BYTES {
        return Err(StorageError::InvalidRecord {
            reason: "payload exceeds the 1 MiB bound".to_owned(),
        });
    }
    serde_json::from_str::<serde_json::Value>(payload_json).map_err(|_| {
        StorageError::InvalidRecord {
            reason: "payload is not valid JSON".to_owned(),
        }
    })?;
    Ok(())
}

fn is_identifier(value: &str, max_len: usize) -> bool {
    let length = value.len();
    if !(1..=max_len).contains(&length) {
        return false;
    }
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-'))
}

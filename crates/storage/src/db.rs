//! Connection opening and pragma configuration.

use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::error::StorageError;

/// Busy timeout applied to every connection.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// SQLite's default `max_page_count` (1 GiB at a 4 KiB page size).
const DEFAULT_MAX_PAGE_COUNT: u32 = 1_073_741_824;

/// Test-only capacity control.
///
/// Applied to the writer connection with `PRAGMA max_page_count`, which is the
/// real SQLite capacity limit: a write that needs a page beyond the limit fails
/// with `SQLITE_FULL` and the transaction rolls back.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityLimit {
    /// Freeze the database at its current page count; no growth is possible.
    CurrentPages,
    /// Set an explicit page cap.
    Pages(u32),
    /// Restore SQLite's default cap.
    Unlimited,
}

/// Connection pragmas as observed on the single writer connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PragmaReport {
    /// `PRAGMA foreign_keys` (1 when enabled).
    pub foreign_keys: i64,
    /// `PRAGMA journal_mode` (expected to be `wal`).
    pub journal_mode: String,
    /// `PRAGMA synchronous` (2 = FULL).
    pub synchronous: i64,
    /// `PRAGMA busy_timeout` in milliseconds.
    pub busy_timeout: i64,
}

pub(crate) fn open_writer(path: &Path) -> Result<Connection, StorageError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(path)?;
    configure_writer(&connection)?;
    Ok(connection)
}

fn configure_writer(connection: &Connection) -> Result<(), StorageError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

pub(crate) fn open_reader(path: &Path) -> Result<Connection, StorageError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(connection)
}

pub(crate) fn pragma_report(connection: &Connection) -> Result<PragmaReport, StorageError> {
    let foreign_keys = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    let journal_mode = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let synchronous = connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    let busy_timeout = connection.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
    Ok(PragmaReport {
        foreign_keys,
        journal_mode,
        synchronous,
        busy_timeout,
    })
}

pub(crate) fn table_exists(connection: &Connection, table: &str) -> Result<bool, StorageError> {
    let exists: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?;
    Ok(exists > 0)
}

/// Apply a test-only capacity limit to the writer connection.
///
/// `PRAGMA max_page_count` clamps an explicit value to the current page count,
/// so [`CapacityLimit::CurrentPages`] reads the writer's own committed page
/// count and freezes the database at that size.
pub(crate) fn apply_capacity_limit(
    connection: &Connection,
    limit: CapacityLimit,
) -> Result<(), StorageError> {
    let pages = match limit {
        CapacityLimit::CurrentPages => {
            let current: i64 = connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            u32::try_from(current).unwrap_or(u32::MAX)
        }
        CapacityLimit::Pages(pages) => pages,
        CapacityLimit::Unlimited => DEFAULT_MAX_PAGE_COUNT,
    };
    connection.pragma_update(None, "max_page_count", i64::from(pages))?;
    Ok(())
}

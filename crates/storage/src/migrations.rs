//! Versioned, transactional migrations.
//!
//! Each migration runs in its own `IMMEDIATE` transaction together with the
//! `schema_migrations` bookkeeping row. A migration that fails therefore rolls
//! back both its schema changes and its version record. Before an irreversible
//! migration is attempted, `VACUUM INTO` writes a verified, restorable snapshot
//! next to the database.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};

use crate::error::StorageError;
use crate::time;

/// One ordered schema migration.
pub struct Migration {
    /// Monotonically increasing, starting at 1.
    pub version: u32,
    /// Human-readable name recorded in `schema_migrations`.
    pub name: &'static str,
    /// When true a verified `VACUUM INTO` snapshot is taken before applying.
    pub irreversible: bool,
    /// The migration body. It runs inside an `IMMEDIATE` transaction.
    pub apply: fn(&Transaction<'_>) -> Result<(), StorageError>,
}

impl Migration {
    /// Construct a migration descriptor.
    #[must_use]
    pub const fn new(
        version: u32,
        name: &'static str,
        irreversible: bool,
        apply: fn(&Transaction<'_>) -> Result<(), StorageError>,
    ) -> Self {
        Self {
            version,
            name,
            irreversible,
            apply,
        }
    }
}

/// Ordered set of migrations owned by the application.
pub struct MigrationPlan {
    migrations: Vec<Migration>,
}

impl MigrationPlan {
    /// The production plan.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            migrations: vec![
                Migration::new(1, "initial", false, initial_schema),
                Migration::new(2, "record_tags", true, add_record_tags),
                Migration::new(3, "record_updated_index", false, add_record_updated_index),
            ],
        }
    }

    /// Build a custom plan. Used by tests that exercise rejection paths.
    #[must_use]
    pub fn new(migrations: Vec<Migration>) -> Self {
        Self { migrations }
    }

    /// The ordered migrations.
    #[must_use]
    pub fn migrations(&self) -> &[Migration] {
        &self.migrations
    }

    /// Highest supported schema version (0 for an empty plan).
    #[must_use]
    pub fn latest_version(&self) -> u32 {
        self.migrations
            .last()
            .map_or(0, |migration| migration.version)
    }

    /// Reject empty, duplicate or out-of-order plans before touching the DB.
    pub fn validate(&self) -> Result<(), StorageError> {
        let Some(first) = self.migrations.first() else {
            return Err(StorageError::InvalidMigrationPlan {
                reason: "migration plan is empty".to_owned(),
            });
        };
        if first.version != 1 {
            return Err(StorageError::InvalidMigrationPlan {
                reason: format!(
                    "first migration must have version 1, found {}",
                    first.version
                ),
            });
        }
        let mut previous = 0;
        for migration in &self.migrations {
            if migration.version <= previous {
                return Err(StorageError::InvalidMigrationPlan {
                    reason: format!(
                        "migration versions must strictly increase; {0} follows {previous}",
                        migration.version
                    ),
                });
            }
            if migration.name.trim().is_empty() {
                return Err(StorageError::InvalidMigrationPlan {
                    reason: format!("migration {} has an empty name", migration.version),
                });
            }
            previous = migration.version;
        }
        Ok(())
    }
}

/// A deterministic fault injected only by tests.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationFault {
    /// Abort before the migration body starts.
    FailBefore(u32),
    /// Run the migration body, then abort before commit (forces a rollback).
    RollbackAfterApply(u32),
    /// Commit the migration, then report failure (simulates a process halt
    /// exactly after the durable commit).
    HaltAfterCommit(u32),
}

/// Apply pending migrations. Returns the new schema version and appends every
/// snapshot taken before an irreversible migration to `snapshots`, including
/// snapshots taken before a migration that then fails.
pub(crate) fn apply_migrations(
    connection: &mut Connection,
    plan: &MigrationPlan,
    snapshot_dir: &Path,
    fault: Option<MigrationFault>,
    snapshots: &mut Vec<(u32, PathBuf)>,
) -> Result<u32, StorageError> {
    plan.validate()?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             applied_at INTEGER NOT NULL
         ) STRICT;",
    )?;

    let applied = applied_versions(connection)?;
    if let Some(found) = applied.iter().copied().max()
        && found > plan.latest_version()
    {
        return Err(StorageError::SchemaTooNew {
            found,
            supported: plan.latest_version(),
        });
    }
    if applied.len() > plan.migrations().len() {
        return Err(StorageError::SchemaTooNew {
            found: applied.last().copied().unwrap_or(0),
            supported: plan.latest_version(),
        });
    }
    for (index, version) in applied.iter().enumerate() {
        let expected = plan.migrations()[index].version;
        if *version != expected {
            return Err(StorageError::InvalidMigrationState {
                reason: format!(
                    "database has migration {version} at position {index}, plan expects {expected}"
                ),
            });
        }
    }

    for migration in plan.migrations().iter().skip(applied.len()) {
        if matches!(fault, Some(MigrationFault::FailBefore(version)) if version == migration.version)
        {
            return Err(StorageError::InjectedFault {
                reason: "migration fault before apply",
            });
        }
        if migration.irreversible {
            let snapshot = take_snapshot(connection, snapshot_dir, migration.version)?;
            snapshots.push((migration.version, snapshot));
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        (migration.apply)(&transaction)?;
        if matches!(fault, Some(MigrationFault::RollbackAfterApply(version)) if version == migration.version)
        {
            // Returning here drops the transaction without committing it.
            return Err(StorageError::InjectedFault {
                reason: "migration fault after apply before commit",
            });
        }
        transaction.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            params![migration.version, migration.name, time::now_unix()],
        )?;
        transaction.commit()?;
        if matches!(fault, Some(MigrationFault::HaltAfterCommit(version)) if version == migration.version)
        {
            return Err(StorageError::InjectedFault {
                reason: "migration fault after commit",
            });
        }
    }

    Ok(applied_versions(connection)?.last().copied().unwrap_or(0))
}

/// Versions recorded in `schema_migrations`, ordered ascending.
pub(crate) fn applied_versions(connection: &Connection) -> Result<Vec<u32>, StorageError> {
    let exists: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(StorageError::BackupVerification {
            reason: "database has no schema_migrations table".to_owned(),
        });
    }
    let mut statement =
        connection.prepare("SELECT version FROM schema_migrations ORDER BY version ASC")?;
    let rows = statement.query_map([], |row| row.get::<_, u32>(0))?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(row?);
    }
    Ok(versions)
}

/// Validate that the recorded versions are a strict prefix of `plan`.
pub(crate) fn validate_applied_prefix(
    applied: &[u32],
    plan: &MigrationPlan,
) -> Result<u32, StorageError> {
    if let Some(found) = applied.iter().copied().max()
        && found > plan.latest_version()
    {
        return Err(StorageError::SchemaTooNew {
            found,
            supported: plan.latest_version(),
        });
    }
    if applied.len() > plan.migrations().len() {
        return Err(StorageError::SchemaTooNew {
            found: applied.last().copied().unwrap_or(0),
            supported: plan.latest_version(),
        });
    }
    for (index, version) in applied.iter().enumerate() {
        let expected = plan.migrations()[index].version;
        if *version != expected {
            return Err(StorageError::InvalidMigrationState {
                reason: format!(
                    "database has migration {version} at position {index}, plan expects {expected}"
                ),
            });
        }
    }
    Ok(applied.last().copied().unwrap_or(0))
}

fn take_snapshot(
    connection: &Connection,
    snapshot_dir: &Path,
    version: u32,
) -> Result<PathBuf, StorageError> {
    fs::create_dir_all(snapshot_dir)?;
    let path = snapshot_dir.join(format!(
        "pre-migration-v{version}-{}.sqlite",
        time::now_millis()
    ));
    let path_text = path.to_str().ok_or_else(|| {
        StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "snapshot path is not valid UTF-8",
        ))
    })?;
    connection.execute("VACUUM INTO ?1", [path_text])?;

    let snapshot = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let integrity: String = snapshot.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(StorageError::Corrupt {
            detail: format!("pre-migration snapshot failed integrity_check: {integrity}"),
        });
    }
    snapshot.close().map_err(|(_, err)| err)?;
    Ok(path)
}

fn initial_schema(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    transaction.execute_batch(
        "CREATE TABLE settings (
             key TEXT PRIMARY KEY,
             value_json TEXT NOT NULL,
             revision INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         ) STRICT;

         CREATE TABLE user_records (
             id TEXT PRIMARY KEY,
             kind TEXT NOT NULL,
             revision INTEGER NOT NULL,
             payload_json TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         ) STRICT;

         CREATE TABLE secret_refs (
             alias TEXT PRIMARY KEY,
             provider TEXT NOT NULL,
             reference TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         ) STRICT;

         CREATE INDEX user_records_kind_idx ON user_records (kind);",
    )?;
    Ok(())
}

fn add_record_tags(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    transaction.execute(
        "ALTER TABLE user_records ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]'",
        [],
    )?;
    Ok(())
}

fn add_record_updated_index(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    transaction.execute(
        "CREATE INDEX user_records_updated_idx ON user_records (updated_at)",
        [],
    )?;
    Ok(())
}

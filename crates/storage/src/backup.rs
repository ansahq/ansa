//! Backup verification and atomic database replacement.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::db;
use crate::error::StorageError;
use crate::migrations::{self, MigrationPlan};
use crate::paths;
use crate::time;

const REQUIRED_TABLES: [&str; 4] = [
    "schema_migrations",
    "settings",
    "user_records",
    "secret_refs",
];

/// Verification report returned by [`crate::Store::backup_to`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupSummary {
    /// Destination file.
    pub path: PathBuf,
    /// Schema version recorded in the copy.
    pub schema_version: u32,
    /// Number of preference rows.
    pub settings_count: u64,
    /// Number of durable user records.
    pub user_records: u64,
    /// Number of credential-reference rows.
    pub secret_refs: u64,
    /// Result of `PRAGMA integrity_check` (`ok`).
    pub integrity: String,
}

/// Verification report returned by [`crate::Store::restore_from`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreSummary {
    /// Source file that was restored.
    pub path: PathBuf,
    /// Schema version before the restore.
    pub previous_schema_version: u32,
    /// Schema version after the restore and any pending migrations.
    pub schema_version: u32,
    /// Number of preference rows in the restored database.
    pub settings_count: u64,
    /// Number of durable user records in the restored database.
    pub user_records: u64,
    /// Number of credential-reference rows in the restored database.
    pub secret_refs: u64,
    /// Result of `PRAGMA integrity_check` (`ok`).
    pub integrity: String,
}

#[derive(Debug, Clone)]
pub(crate) struct DatabaseInfo {
    pub(crate) schema_version: u32,
    pub(crate) settings_count: u64,
    pub(crate) user_records: u64,
    pub(crate) secret_refs: u64,
    pub(crate) integrity: String,
}

/// Inspect a database without applying migrations. Rejects corrupt files,
/// schemas newer than `plan`, and databases that claim a schema version but
/// lack the required tables, columns or indexes.
pub(crate) fn verify_database(
    path: &Path,
    plan: &MigrationPlan,
) -> Result<DatabaseInfo, StorageError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(db::BUSY_TIMEOUT)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(StorageError::from)?;
    if integrity != "ok" {
        return Err(StorageError::Corrupt {
            detail: format!("integrity_check reported {integrity}"),
        });
    }
    let versions = migrations::applied_versions(&connection)?;
    let schema_version = migrations::validate_applied_prefix(&versions, plan)?;
    verify_schema(&connection, schema_version)?;
    let settings_count = verified_count(&connection, "settings")?;
    let user_records = verified_count(&connection, "user_records")?;
    let secret_refs = verified_count(&connection, "secret_refs")?;
    Ok(DatabaseInfo {
        schema_version,
        settings_count,
        user_records,
        secret_refs,
        integrity,
    })
}

fn verified_count(connection: &Connection, table: &str) -> Result<u64, StorageError> {
    count(connection, table).map_err(|error| StorageError::BackupVerification {
        reason: format!("could not count {table}: {error}"),
    })
}

fn verify_schema(connection: &Connection, schema_version: u32) -> Result<(), StorageError> {
    for table in REQUIRED_TABLES {
        if !db::table_exists(connection, table)? {
            return Err(StorageError::BackupVerification {
                reason: format!("missing required table `{table}`"),
            });
        }
    }
    if db::table_exists(connection, "cache_entries")? {
        return Err(StorageError::BackupVerification {
            reason: "durable database contains a cache_entries table".to_owned(),
        });
    }

    require_columns(
        connection,
        "schema_migrations",
        &["version", "name", "applied_at"],
    )?;
    require_columns(
        connection,
        "settings",
        &["key", "value_json", "revision", "updated_at"],
    )?;
    require_columns(
        connection,
        "secret_refs",
        &["alias", "provider", "reference", "created_at", "updated_at"],
    )?;
    let mut record_columns = vec![
        "id",
        "kind",
        "revision",
        "payload_json",
        "created_at",
        "updated_at",
    ];
    if schema_version >= 2 {
        record_columns.push("tags_json");
    }
    require_columns(connection, "user_records", &record_columns)?;

    require_index(connection, "user_records_kind_idx")?;
    if schema_version >= 3 {
        require_index(connection, "user_records_updated_idx")?;
    }
    Ok(())
}

fn require_columns(
    connection: &Connection,
    table: &str,
    columns: &[&str],
) -> Result<(), StorageError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| StorageError::BackupVerification {
            reason: format!("could not inspect {table}: {error}"),
        })?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| StorageError::BackupVerification {
            reason: format!("could not inspect {table} columns: {error}"),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| StorageError::BackupVerification {
            reason: format!("could not inspect {table} columns: {error}"),
        })?;
    for column in columns {
        if !names.iter().any(|name| name == column) {
            return Err(StorageError::BackupVerification {
                reason: format!("table `{table}` is missing column `{column}`"),
            });
        }
    }
    Ok(())
}

fn require_index(connection: &Connection, index: &str) -> Result<(), StorageError> {
    let exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
            [index],
            |row| row.get(0),
        )
        .map_err(|error| StorageError::BackupVerification {
            reason: format!("could not inspect index `{index}`: {error}"),
        })?;
    if exists == 0 {
        return Err(StorageError::BackupVerification {
            reason: format!("missing required index `{index}`"),
        });
    }
    Ok(())
}

fn count(connection: &Connection, table: &str) -> Result<u64, StorageError> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count: i64 = connection.query_row(&sql, [], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// A staged database replacement that can be committed or rolled back.
pub(crate) struct Replacement {
    main: PathBuf,
    old: Option<PathBuf>,
    /// Identity lock for the file currently installed at `main`.
    live_lock: Option<crate::lock::InstanceLock>,
}

impl Replacement {
    /// Path of the preserved pre-restore copy, when one was made.
    pub(crate) fn pre_restore_copy(&self) -> Option<&Path> {
        self.old.as_deref()
    }

    /// Take the identity lock for the file currently at `main`.
    ///
    /// The caller must adopt this lock so the installed file stays protected
    /// after the replacement object is dropped.
    pub(crate) fn take_live_lock(&mut self) -> Option<crate::lock::InstanceLock> {
        self.live_lock.take()
    }

    /// Remove the pre-restore copy. The live lock is left for the caller.
    pub(crate) fn commit(mut self) {
        if let Some(old) = self.old.take() {
            let _ = fs::remove_file(old);
        }
    }

    /// Put the original database back.
    ///
    /// The rollback copy's identity lock is acquired before the rename, so
    /// there is no window in which the restored file is live but unlocked. On
    /// success `take_live_lock` returns that lock; on failure it still returns
    /// the lock for whichever file remains at `main`.
    pub(crate) fn rollback(&mut self) -> Result<(), StorageError> {
        let Some(old) = self.old.take() else {
            return Ok(());
        };
        let restored_lock = crate::lock::InstanceLock::acquire_existing(&old)?;
        remove_sidecars(&self.main)?;
        fs::rename(old, &self.main)?;
        self.live_lock = Some(restored_lock);
        Ok(())
    }
}

/// Copy `candidate` beside `main`, verify the copy, then atomically rename it
/// over the live file. A copy of the previous file is kept for rollback.
///
/// The caller must already have closed the writer and confirmed that the main
/// WAL is checkpointed; this function refuses to proceed if a non-empty WAL or
/// rollback journal remains.
pub(crate) fn install_candidate(
    main: &Path,
    candidate: &Path,
    plan: &MigrationPlan,
) -> Result<Replacement, StorageError> {
    let parent = match main.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let file_name = main
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| StorageError::InvalidConfig {
            reason: "main database path has no usable file name".to_owned(),
        })?;
    let stamp = time::now_millis();
    let staged = parent.join(format!(".{file_name}.restore-{stamp}.tmp"));
    let old = parent.join(format!(".{file_name}.pre-restore-{stamp}"));

    ensure_sidecars_clear(main)?;
    fs::copy(candidate, &staged)?;
    if let Err(error) = sync_file(&staged) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    if let Err(error) = verify_database(&staged, plan) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }

    let had_original = main.exists();
    if had_original {
        if old.exists() {
            fs::remove_file(&old)?;
        }
        fs::copy(main, &old)?;
        sync_file(&old)?;
    }
    // Acquire the replacement file's identity lock *before* it becomes live.
    // The rename preserves the file identity, so there is no unlocked window in
    // which another store could open the new database and start a second WAL.
    let live_lock = match crate::lock::InstanceLock::acquire_existing(&staged) {
        Ok(lock) => lock,
        Err(error) => {
            let _ = fs::remove_file(&staged);
            if had_original {
                let _ = fs::remove_file(&old);
            }
            return Err(error);
        }
    };
    // Re-check immediately before the destructive step. If a checkpoint was
    // not complete, the original database and its WAL are left untouched.
    if let Err(error) = ensure_sidecars_clear(main) {
        let _ = fs::remove_file(&staged);
        if had_original {
            let _ = fs::remove_file(&old);
        }
        return Err(error);
    }
    if let Err(error) = remove_sidecars(main) {
        if had_original {
            let _ = fs::remove_file(&old);
        }
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    if let Err(error) = fs::rename(&staged, main) {
        let _ = fs::remove_file(&staged);
        if had_original {
            let _ = fs::remove_file(&old);
        }
        return Err(StorageError::Io(error));
    }

    Ok(Replacement {
        main: main.to_path_buf(),
        old: had_original.then_some(old),
        live_lock: Some(live_lock),
    })
}

pub(crate) fn sync_file(path: &Path) -> Result<(), StorageError> {
    let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    file.sync_all()?;
    Ok(())
}

/// Refuse to replace a live database while a non-empty WAL or rollback journal
/// could still hold acknowledged commits.
pub(crate) fn ensure_sidecars_clear(main: &Path) -> Result<(), StorageError> {
    for suffix in ["-wal", "-journal"] {
        let sidecar = paths::sidecar(main, suffix);
        match fs::metadata(&sidecar) {
            Ok(metadata) if metadata.len() > 0 => {
                return Err(StorageError::CheckpointFailed {
                    reason: format!(
                        "{} is still non-empty; refusing to replace the database",
                        sidecar.display()
                    ),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StorageError::Io(error)),
        }
    }
    Ok(())
}

fn remove_sidecars(main: &Path) -> Result<(), StorageError> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = paths::sidecar(main, suffix);
        match fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StorageError::Io(error)),
        }
    }
    Ok(())
}

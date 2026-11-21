#![forbid(unsafe_code)]
//! Transactional settings and user-record storage for Ansa.
//!
//! # Properties
//!
//! * One writer actor owns the durable database and the separate,
//!   rebuildable cache database. Reads use a small bounded pool of read-only
//!   connections against the same WAL database.
//! * `foreign_keys = ON`, `journal_mode = WAL`, `synchronous = FULL` and a
//!   busy timeout are applied to every writer.
//! * Schema changes are versioned migrations. Each migration and its
//!   `schema_migrations` row commit in one `IMMEDIATE` transaction; a failure
//!   rolls back the schema change and leaves the previous version usable.
//! * Irreversible migrations take a verified `VACUUM INTO` snapshot first.
//! * Settings are typed, validated and revisioned. Compare-and-swap updates
//!   return [`StorageError::Conflict`] on a stale revision.
//! * Durable records live in the main database; cache rows live in a separate
//!   `cache.sqlite` file and can be cleared without touching records.
//! * Only credential *references* are accepted through [`SecretRef`]; that
//!   type rejects raw tokens and redacts the reference in `Debug` output.
//!   `UserRecord` payloads and cache values are opaque, unencrypted caller
//!   data: this crate does not classify arbitrary bytes, so callers must not
//!   put secrets, clipboard bodies or note text into those paths until an
//!   encrypted-payload layer exists.
//!
//! # Crash boundary
//!
//! SQLite's WAL plus explicit transactions guarantee that a process halt
//! leaves either the pre-transaction or post-commit state. The test suite
//! includes real OS process kills during a write loop (with an external
//! acknowledged-revision log) and after migration DDL but before `COMMIT`.
//! Those tests exercise process termination, not physical power loss or a
//! failing storage device.
//!
//! # Process model
//!
//! Writes are serialized by one writer actor inside a process, and opening a
//! [`Store`] takes an exclusive advisory lock keyed to the *identity* of the
//! durable database file: volume serial plus file index on Windows
//! (`GetFileInformationByHandle`), device plus inode on Unix. Lock files live
//! in a fixed per-user lock directory, so hard links, symlinks and junctions
//! that resolve to the same file all agree on one lock. A second process, or a
//! second handle in the same process, fails with
//! [`StorageError::AlreadyLocked`] before SQLite opens a second writer or WAL.
//!
//! The OS releases the lock when the handle closes, including after a crash,
//! and the next open succeeds. Restores re-key the lock to the replacement
//! file's identity before the rename, so the store stays protected across the
//! swap. The lock is advisory: a raw SQLite client that does not use this crate
//! is not stopped by it. Platforms that cannot report a file identity fall back
//! to a normalized-path key, where hard links are not detectable.
//!
//! Recovery actions are reported rather than silent: pre-migration snapshots
//! and aborted restores are recorded as [`RecoveryInfo`] entries available from
//! [`Store::recovery_info`], naming the snapshot/backup file and any preserved
//! original copy.

mod actor;
mod backup;
mod cache;
mod db;
mod error;
mod lock;
mod migrations;
mod paths;
mod pool;
mod records;
mod recovery;
mod secrets;
mod settings;
mod time;

use std::path::{Path, PathBuf};

pub use backup::{BackupSummary, RestoreSummary};
pub use db::{CapacityLimit, PragmaReport};
pub use error::StorageError;
pub use migrations::{Migration, MigrationFault, MigrationPlan};
pub use records::UserRecord;
pub use recovery::{RecoveryInfo, RecoveryKind};
pub use secrets::SecretRef;
pub use settings::{LaunchMode, SettingKey, SettingSnapshot, SettingValue, Theme};

pub use actor::ShutdownFault;
use actor::StorageActor;
use lock::InstanceLock;
use pool::ReadPool;

/// Maximum number of records returned by a single [`Store::list_records`] or
/// [`Store::list_records_page`] call.
pub const MAX_RECORD_PAGE: usize = 1_000;

/// Test-only backup fault injection.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupFault {
    /// Replace the verified temporary copy with garbage before verification.
    CorruptCopy,
}

/// Storage configuration.
pub struct StorageConfig {
    /// Durable database file.
    pub main_path: PathBuf,
    /// Rebuildable cache database file. Must differ from `main_path`.
    pub cache_path: PathBuf,
    /// Directory for pre-migration snapshots.
    pub snapshot_dir: PathBuf,
    /// Number of read-only connections in each read pool (1..=16).
    pub read_connections: usize,
    /// Ordered migration plan. Defaults to [`MigrationPlan::standard`].
    pub migration_plan: MigrationPlan,
    /// Deterministic migration failure injection. Test-only.
    #[doc(hidden)]
    pub migration_fault: Option<MigrationFault>,
    /// Deterministic shutdown/checkpoint failure injection. Test-only.
    #[doc(hidden)]
    pub shutdown_fault: Option<ShutdownFault>,
    /// Deterministic backup failure injection. Test-only.
    #[doc(hidden)]
    pub backup_fault: Option<BackupFault>,
}

impl StorageConfig {
    /// Build a default configuration for the given database files.
    #[must_use]
    pub fn new(main_path: impl Into<PathBuf>, cache_path: impl Into<PathBuf>) -> Self {
        let main_path = main_path.into();
        let snapshot_dir = match main_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join("snapshots"),
            _ => PathBuf::from("snapshots"),
        };
        Self {
            main_path,
            cache_path: cache_path.into(),
            snapshot_dir,
            read_connections: 4,
            migration_plan: MigrationPlan::standard(),
            migration_fault: None,
            shutdown_fault: None,
            backup_fault: None,
        }
    }

    /// Validate every field before any database file is created or migrated.
    pub fn validate(&self) -> Result<(), StorageError> {
        if self.main_path.as_os_str().is_empty() || self.cache_path.as_os_str().is_empty() {
            return Err(StorageError::InvalidConfig {
                reason: "database paths must not be empty".to_owned(),
            });
        }
        if self.snapshot_dir.as_os_str().is_empty() {
            return Err(StorageError::InvalidConfig {
                reason: "snapshot directory must not be empty".to_owned(),
            });
        }
        if !(1..=16).contains(&self.read_connections) {
            return Err(StorageError::InvalidConfig {
                reason: "read_connections must be between 1 and 16".to_owned(),
            });
        }
        self.migration_plan.validate()?;
        if paths::equivalent(&self.main_path, &self.cache_path) {
            return Err(StorageError::InvalidConfig {
                reason: "cache and durable databases must be separate files".to_owned(),
            });
        }
        Ok(())
    }
}

struct Inner {
    actor: StorageActor,
    reads: ReadPool,
    cache_reads: ReadPool,
    schema_version: u32,
}

impl Inner {
    /// Open the databases. Recovery reports are appended to `recovery` as soon
    /// as the action happens, so they survive a later open failure.
    fn open(
        config: &StorageConfig,
        recovery: &mut Vec<RecoveryInfo>,
    ) -> Result<Self, StorageError> {
        config.validate()?;

        let mut main = db::open_writer(&config.main_path)?;
        if db::table_exists(&main, "cache_entries")? {
            return Err(StorageError::InvalidConfig {
                reason: format!(
                    "durable database {} contains a cache_entries table; a previous cache path \
                     resolved to the durable file. Back up the database, move cache data to a \
                     separate cache.sqlite, remove the cache_entries table, and reopen",
                    config.main_path.display()
                ),
            });
        }
        let mut snapshots = Vec::new();
        let schema_version =
            match migrations::apply_migrations(
                &mut main,
                &config.migration_plan,
                &config.snapshot_dir,
                config.migration_fault,
                &mut snapshots,
            ) {
                Ok(version) => version,
                Err(error) => {
                    // A snapshot can exist even though the migration failed; report
                    // it so the caller has an actionable recovery path.
                    recovery.extend(snapshots.into_iter().map(|(version, path)| {
                        RecoveryInfo::pre_migration_snapshot(path, version)
                    }));
                    return Err(error);
                }
            };
        recovery.extend(
            snapshots
                .into_iter()
                .map(|(version, path)| RecoveryInfo::pre_migration_snapshot(path, version)),
        );

        let cache = db::open_writer(&config.cache_path)?;
        for durable_table in [
            "schema_migrations",
            "settings",
            "user_records",
            "secret_refs",
        ] {
            if db::table_exists(&cache, durable_table)? {
                return Err(StorageError::InvalidConfig {
                    reason: format!(
                        "cache database {} contains durable table `{durable_table}`; choose a \
                         separate rebuildable cache file",
                        config.cache_path.display()
                    ),
                });
            }
        }
        cache::initialize(&cache)?;

        let actor = StorageActor::spawn(main, cache, config.shutdown_fault)?;
        let reads = ReadPool::open(&config.main_path, config.read_connections)?;
        let cache_reads = ReadPool::open(&config.cache_path, config.read_connections)?;

        Ok(Self {
            actor,
            reads,
            cache_reads,
            schema_version,
        })
    }

    fn shutdown(mut self) -> Result<(), StorageError> {
        // Close readers first so the writer can checkpoint the WAL when it is
        // closed, leaving a self-contained main database file.
        drop(self.reads);
        drop(self.cache_reads);
        self.actor.shutdown()
    }
}

/// Handle to the storage foundation.
///
/// The handle is cheap to pass between threads; writes are serialized by the
/// writer actor and reads use the bounded pool.
pub struct Store {
    config: StorageConfig,
    inner: Option<Inner>,
    schema_version: u32,
    recovery: Vec<RecoveryInfo>,
    /// Held for the whole store lifetime, including `restore_from`, so another
    /// instance cannot open the database between close and reopen.
    lock: InstanceLock,
}

impl Store {
    /// Open (or create) the databases, apply migrations and start the writer
    /// actor.
    ///
    /// An exclusive lock file beside the durable database is acquired first; a
    /// concurrent open fails with [`StorageError::AlreadyLocked`].
    pub fn open(config: StorageConfig) -> Result<Self, StorageError> {
        Self::open_with_recovery_log(config, &mut Vec::new())
    }

    /// Open the store and append recovery reports to `recovery`.
    ///
    /// Reports are appended as they happen, so they are available even when
    /// opening fails (for example a pre-migration snapshot written just before
    /// a migration aborts). [`Self::open`] is equivalent with a discarded log;
    /// the supplied vector is appended to, never cleared.
    pub fn open_with_recovery_log(
        config: StorageConfig,
        recovery: &mut Vec<RecoveryInfo>,
    ) -> Result<Self, StorageError> {
        config.validate()?;
        let lock = InstanceLock::acquire(&config.main_path)?;
        let mut collected = Vec::new();
        let inner = match Inner::open(&config, &mut collected) {
            Ok(inner) => inner,
            Err(error) => {
                recovery.extend(collected);
                return Err(error);
            }
        };
        recovery.extend(collected.iter().cloned());
        let schema_version = inner.schema_version;
        Ok(Self {
            config,
            inner: Some(inner),
            schema_version,
            recovery: collected,
            lock,
        })
    }

    /// Durable database path.
    #[must_use]
    pub fn main_path(&self) -> &Path {
        &self.config.main_path
    }

    /// Path of the current identity-keyed lock file (in the per-user lock
    /// directory; not beside the database).
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        self.lock.path()
    }

    /// Recovery actions taken while opening or restoring, oldest first.
    ///
    /// Entries name the snapshot/backup file and, for an aborted restore, the
    /// original database or preserved pre-restore copy to fall back to.
    #[must_use]
    pub fn recovery_info(&self) -> &[RecoveryInfo] {
        &self.recovery
    }

    /// Cache database path.
    #[must_use]
    pub fn cache_path(&self) -> &Path {
        &self.config.cache_path
    }

    /// Directory used for pre-migration snapshots.
    #[must_use]
    pub fn snapshot_dir(&self) -> &Path {
        &self.config.snapshot_dir
    }

    /// Current schema version.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Observe the writer connection's safety pragmas.
    pub fn pragma_report(&self) -> Result<PragmaReport, StorageError> {
        self.inner
            .as_ref()
            .ok_or(StorageError::Closed)?
            .actor
            .pragma_report()
    }

    /// Test-only: change the SQLite capacity limit on the writer connection.
    ///
    /// This is the real `PRAGMA max_page_count` path, not a simulated error.
    /// [`CapacityLimit::CurrentPages`] freezes the database at its current
    /// size, so the next write that needs a new page fails with
    /// [`StorageError::DiskFull`] and rolls back. Raise the limit again to
    /// prove the same store recovers.
    #[doc(hidden)]
    pub fn set_page_limit(&self, limit: CapacityLimit) -> Result<(), StorageError> {
        self.actor()?.set_capacity_limit(limit)
    }

    /// Read a setting. Returns an error if the stored value is invalid; use
    /// [`Self::get_or_default`] for a safe fallback with a diagnostic.
    pub fn get_setting(&self, key: SettingKey) -> Result<Option<SettingSnapshot>, StorageError> {
        let row = self.with_read(|connection| actor::read_setting_row(connection, key))?;
        match row {
            None => Ok(None),
            Some((value_json, revision, updated_at)) => {
                let value = settings::decode_value(key, &value_json)?;
                Ok(Some(SettingSnapshot {
                    key,
                    value,
                    revision: Some(revision),
                    updated_at: Some(updated_at),
                    diagnostic: None,
                }))
            }
        }
    }

    /// Read a setting, falling back to the safe default when the row is
    /// missing, corrupt or no longer valid. The stored row is never modified.
    pub fn get_or_default(&self, key: SettingKey) -> Result<SettingSnapshot, StorageError> {
        let row = self.with_read(|connection| actor::read_setting_row(connection, key))?;
        match row {
            None => Ok(SettingSnapshot {
                key,
                value: key.default_value(),
                revision: None,
                updated_at: None,
                diagnostic: None,
            }),
            Some((value_json, revision, updated_at)) => {
                match settings::decode_value(key, &value_json) {
                    Ok(value) => Ok(SettingSnapshot {
                        key,
                        value,
                        revision: Some(revision),
                        updated_at: Some(updated_at),
                        diagnostic: None,
                    }),
                    Err(diagnostic) => Ok(SettingSnapshot {
                        key,
                        value: key.default_value(),
                        revision: Some(revision),
                        updated_at: Some(updated_at),
                        diagnostic: Some(diagnostic),
                    }),
                }
            }
        }
    }

    /// Create a setting (`expected_revision = None`) or replace it when the
    /// stored revision matches. Returns the new revision.
    pub fn set_setting(
        &self,
        key: SettingKey,
        value: &SettingValue,
        expected_revision: Option<u64>,
    ) -> Result<u64, StorageError> {
        let value_json = settings::encode_value(key, value)?;
        self.actor()?
            .set_setting(key, value_json, expected_revision)
    }

    /// Delete a setting. With an expected revision the delete is a
    /// compare-and-swap.
    pub fn delete_setting(
        &self,
        key: SettingKey,
        expected_revision: Option<u64>,
    ) -> Result<bool, StorageError> {
        self.actor()?.delete_setting(key, expected_revision)
    }

    /// Insert a new durable record.
    ///
    /// `payload_json` is unencrypted caller data. Use [`SecretRef`] for
    /// credential references and never pass secrets, clipboard bodies or note
    /// text here until an encrypted-payload layer exists.
    pub fn create_record(
        &self,
        id: &str,
        kind: &str,
        payload_json: &str,
    ) -> Result<UserRecord, StorageError> {
        actor::validate_record_fields(id, kind, payload_json)?;
        self.actor()?
            .create_record(id.to_owned(), kind.to_owned(), payload_json.to_owned())
    }

    /// Read a durable record.
    pub fn get_record(&self, id: &str) -> Result<Option<UserRecord>, StorageError> {
        records::validate_id(id)?;
        self.with_read(|connection| actor::load_record(connection, id))
    }

    /// List records, optionally filtered by kind.
    ///
    /// At most [`MAX_RECORD_PAGE`] records are returned; use
    /// [`Self::list_records_page`] to page through a larger store.
    pub fn list_records(&self, kind: Option<&str>) -> Result<Vec<UserRecord>, StorageError> {
        self.list_records_page(kind, MAX_RECORD_PAGE, 0)
    }

    /// List a bounded page of records, optionally filtered by kind.
    pub fn list_records_page(
        &self,
        kind: Option<&str>,
        limit: usize,
        offset: u64,
    ) -> Result<Vec<UserRecord>, StorageError> {
        if let Some(kind) = kind {
            records::validate_kind(kind)?;
        }
        if !(1..=MAX_RECORD_PAGE).contains(&limit) {
            return Err(StorageError::InvalidRecord {
                reason: format!("record page limit must be 1..={MAX_RECORD_PAGE}"),
            });
        }
        self.with_read(|connection| actor::list_records(connection, kind, limit, offset))
    }

    /// Replace a record's payload when the stored revision matches.
    ///
    /// The payload is unencrypted caller data, as for [`Self::create_record`].
    pub fn update_record(
        &self,
        id: &str,
        payload_json: &str,
        expected_revision: u64,
    ) -> Result<UserRecord, StorageError> {
        records::validate_id(id)?;
        records::validate_payload(payload_json)?;
        self.actor()?
            .update_record(id.to_owned(), payload_json.to_owned(), expected_revision)
    }

    /// Delete a record, optionally with a revision expectation.
    pub fn delete_record(
        &self,
        id: &str,
        expected_revision: Option<u64>,
    ) -> Result<bool, StorageError> {
        records::validate_id(id)?;
        self.actor()?
            .delete_record(id.to_owned(), expected_revision)
    }

    /// Persist a validated credential reference. No plaintext secret can be
    /// represented by [`SecretRef`].
    pub fn put_secret_ref(&self, secret: SecretRef) -> Result<(), StorageError> {
        self.actor()?.put_secret_ref(secret)
    }

    /// Read a credential reference by alias.
    pub fn get_secret_ref(&self, alias: &str) -> Result<Option<SecretRef>, StorageError> {
        if alias.is_empty() {
            return Err(StorageError::InvalidSecretRef {
                reason: "alias must not be empty",
            });
        }
        self.with_read(|connection| actor::read_secret_ref(connection, alias))
    }

    /// Delete a credential reference for an alias and provider.
    pub fn delete_secret_ref(&self, alias: &str, provider: &str) -> Result<bool, StorageError> {
        self.actor()?
            .delete_secret_ref(alias.to_owned(), provider.to_owned())
    }

    /// Insert or replace a rebuildable cache entry.
    ///
    /// Values are unencrypted; do not cache secrets or sensitive clipboard
    /// bodies.
    pub fn cache_put(
        &self,
        key: &str,
        value: &[u8],
        expires_at: Option<i64>,
    ) -> Result<(), StorageError> {
        cache::validate_key(key)?;
        cache::validate_value(value)?;
        self.actor()?
            .cache_put(key.to_owned(), value.to_vec(), expires_at)
    }

    /// Read a non-expired cache entry.
    pub fn cache_get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        cache::validate_key(key)?;
        let inner = self.inner.as_ref().ok_or(StorageError::Closed)?;
        let connection = inner.cache_reads.acquire()?;
        connection.with(|connection| actor::cache_get(connection, key))
    }

    /// Remove every cache entry. Idempotent. Durable records and settings are
    /// untouched.
    pub fn clear_cache(&self) -> Result<u64, StorageError> {
        self.actor()?.clear_cache()
    }

    /// Write a consistent snapshot, then verify integrity, schema version and
    /// row counts against the live database.
    ///
    /// The copy is written to a unique temporary sibling, verified, synced and
    /// only then renamed over `path`. A failed refresh leaves the previous
    /// backup intact and removes the temporary file.
    pub fn backup_to(&self, path: impl AsRef<Path>) -> Result<BackupSummary, StorageError> {
        let path = path.as_ref();
        check_backup_destination(&self.config, path)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let temporary = backup_temp_path(path)?;
        let counts = match self.actor()?.backup(temporary.clone()) {
            Ok(counts) => counts,
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        };
        if matches!(self.config.backup_fault, Some(BackupFault::CorruptCopy))
            && let Err(error) = std::fs::write(&temporary, b"injected corrupt backup copy")
        {
            let _ = std::fs::remove_file(&temporary);
            return Err(StorageError::Io(error));
        }
        let info = match backup::verify_database(&temporary, &self.config.migration_plan) {
            Ok(info) => info,
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        };
        if info.schema_version != counts.schema_version
            || info.settings_count != counts.settings_count
            || info.user_records != counts.user_records
            || info.secret_refs != counts.secret_refs
        {
            let _ = std::fs::remove_file(&temporary);
            return Err(StorageError::BackupVerification {
                reason: format!(
                    "backup counts differ: live version {} / {} settings / {} records / {} secret refs, copy version {} / {} settings / {} records / {} secret refs",
                    counts.schema_version,
                    counts.settings_count,
                    counts.user_records,
                    counts.secret_refs,
                    info.schema_version,
                    info.settings_count,
                    info.user_records,
                    info.secret_refs
                ),
            });
        }
        if let Err(error) = backup::sync_file(&temporary) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(StorageError::Io(error));
        }
        Ok(BackupSummary {
            path: path.to_path_buf(),
            schema_version: info.schema_version,
            settings_count: info.settings_count,
            user_records: info.user_records,
            secret_refs: info.secret_refs,
            integrity: info.integrity,
        })
    }

    /// Replace the durable database with a verified backup.
    ///
    /// The candidate is copied beside the live database, verified again, then
    /// swapped in atomically after the writer actor has been shut down. A
    /// corrupt or too-new candidate is rejected before anything is replaced.
    /// The cache database is deliberately not restored.
    ///
    /// Every failed attempt is recorded in [`Self::recovery_info`] with the
    /// source backup and the original database (or preserved pre-restore copy)
    /// to fall back to.
    pub fn restore_from(
        &mut self,
        source: impl AsRef<Path>,
    ) -> Result<RestoreSummary, StorageError> {
        let source = source.as_ref().to_path_buf();
        let source_is_lock =
            lock::paths_for_existing(&self.config.main_path).is_some_and(|paths| {
                paths::equivalent(&source, &paths.lock) || paths::equivalent(&source, &paths.owner)
            });
        if paths::equivalent(&source, &self.config.main_path)
            || paths::equivalent(&source, &self.config.cache_path)
            || source_is_lock
            || lock::same_identity(&source, &self.config.main_path)
        {
            return Err(StorageError::InvalidConfig {
                reason: "restore source must differ from the live databases and lock file"
                    .to_owned(),
            });
        }

        let candidate = match backup::verify_database(&source, &self.config.migration_plan) {
            Ok(candidate) => candidate,
            Err(error) => {
                let copy = self.config.main_path.clone();
                self.record_restore_aborted(&source, Some(copy), &error);
                return Err(error);
            }
        };
        if candidate.schema_version == 0 {
            let error = StorageError::BackupVerification {
                reason: "candidate has no schema version".to_owned(),
            };
            let copy = self.config.main_path.clone();
            self.record_restore_aborted(&source, Some(copy), &error);
            return Err(error);
        }
        let previous_schema_version = self.schema_version;

        if let Err(error) = self.close_runtime() {
            // The checkpoint/close failed before any file was moved. Reopen the
            // original database (with its WAL intact) and report the failure.
            let copy = self.config.main_path.clone();
            self.record_restore_aborted(&source, Some(copy), &error);
            return Err(self.reopen_after_failure(error));
        }
        let mut replacement = match backup::install_candidate(
            &self.config.main_path,
            &source,
            &self.config.migration_plan,
        ) {
            Ok(replacement) => replacement,
            Err(error) => {
                let copy = self.config.main_path.clone();
                self.record_restore_aborted(&source, Some(copy), &error);
                return Err(self.reopen_after_failure(error));
            }
        };
        // Captured before `rollback` consumes the replacement: if rolling back
        // fails, this copy is the complete original database file.
        let preserved = replacement.pre_restore_copy().map(Path::to_path_buf);

        let reopened = match Inner::open(&self.config, &mut self.recovery) {
            Ok(inner) => inner,
            Err(error) => {
                return Err(match replacement.rollback() {
                    Ok(()) => {
                        if let Some(lock) = replacement.take_live_lock() {
                            self.lock = lock;
                        }
                        let copy = self.config.main_path.clone();
                        self.record_restore_aborted(&source, Some(copy), &error);
                        self.reopen_after_failure(error)
                    }
                    Err(rollback_error) => {
                        if let Some(lock) = replacement.take_live_lock() {
                            self.lock = lock;
                        }
                        self.rollback_failure(&source, error, rollback_error, preserved)
                    }
                });
            }
        };

        let verification =
            backup::verify_database(&self.config.main_path, &self.config.migration_plan);
        match verification {
            Ok(after)
                if after.settings_count == candidate.settings_count
                    && after.user_records == candidate.user_records
                    && after.secret_refs == candidate.secret_refs =>
            {
                let schema_version = reopened.schema_version;
                self.schema_version = schema_version;
                self.inner = Some(reopened);
                if let Some(lock) = replacement.take_live_lock() {
                    self.lock = lock;
                }
                replacement.commit();
                Ok(RestoreSummary {
                    path: source,
                    previous_schema_version,
                    schema_version: after.schema_version,
                    settings_count: after.settings_count,
                    user_records: after.user_records,
                    secret_refs: after.secret_refs,
                    integrity: after.integrity,
                })
            }
            other => {
                let shutdown_error = reopened.shutdown().err();
                let mut verification_error = match other {
                    Ok(after) => StorageError::BackupVerification {
                        reason: format!(
                            "restored counts differ: expected {} settings, {} records and {} secret refs, found {} and {} and {}",
                            candidate.settings_count,
                            candidate.user_records,
                            candidate.secret_refs,
                            after.settings_count,
                            after.user_records,
                            after.secret_refs
                        ),
                    },
                    Err(error) => error,
                };
                if let Some(shutdown_error) = shutdown_error {
                    verification_error = StorageError::BackupVerification {
                        reason: format!(
                            "{verification_error}; closing the rejected database failed: {shutdown_error}"
                        ),
                    };
                }
                match replacement.rollback() {
                    Ok(()) => {
                        if let Some(lock) = replacement.take_live_lock() {
                            self.lock = lock;
                        }
                        let copy = self.config.main_path.clone();
                        self.record_restore_aborted(&source, Some(copy), &verification_error);
                        Err(self.reopen_after_failure(verification_error))
                    }
                    Err(rollback_error) => {
                        if let Some(lock) = replacement.take_live_lock() {
                            self.lock = lock;
                        }
                        Err(self.rollback_failure(
                            &source,
                            verification_error,
                            rollback_error,
                            preserved,
                        ))
                    }
                }
            }
        }
    }

    fn actor(&self) -> Result<&StorageActor, StorageError> {
        self.inner
            .as_ref()
            .map(|inner| &inner.actor)
            .ok_or(StorageError::Closed)
    }

    fn with_read<T>(
        &self,
        operation: impl FnOnce(&rusqlite::Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let inner = self.inner.as_ref().ok_or(StorageError::Closed)?;
        let connection = inner.reads.acquire()?;
        connection.with(operation)
    }

    fn close_runtime(&mut self) -> Result<(), StorageError> {
        match self.inner.take() {
            Some(inner) => inner.shutdown(),
            None => Ok(()),
        }
    }

    fn record_restore_aborted(
        &mut self,
        source: &Path,
        recovery_copy: Option<PathBuf>,
        error: &StorageError,
    ) {
        self.recovery.push(RecoveryInfo::restore_aborted(
            source,
            recovery_copy,
            error.to_string(),
        ));
    }

    fn rollback_failure(
        &mut self,
        source: &Path,
        verification_error: StorageError,
        rollback_error: StorageError,
        preserved: Option<PathBuf>,
    ) -> StorageError {
        let copy = preserved.or_else(|| Some(self.config.main_path.clone()));
        let location = copy.as_ref().map_or_else(
            || self.config.main_path.display().to_string(),
            |path| path.display().to_string(),
        );
        let failure = StorageError::BackupVerification {
            reason: format!(
                "{verification_error}; rollback failed: {rollback_error}; the original database \
                 copy is preserved at {location}"
            ),
        };
        self.record_restore_aborted(source, copy, &verification_error);
        failure
    }

    fn reopen_after_failure(&mut self, original: StorageError) -> StorageError {
        match Inner::open(&self.config, &mut self.recovery) {
            Ok(inner) => {
                self.schema_version = inner.schema_version;
                self.inner = Some(inner);
                original
            }
            Err(reopen_error) => StorageError::BackupVerification {
                reason: format!(
                    "{original}; reopening the previous database failed: {reopen_error}"
                ),
            },
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.close_runtime();
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("main_path", &self.config.main_path)
            .field("cache_path", &self.config.cache_path)
            .field("schema_version", &self.schema_version)
            .finish_non_exhaustive()
    }
}

fn check_backup_destination(config: &StorageConfig, path: &Path) -> Result<(), StorageError> {
    let live_databases = [&config.main_path, &config.cache_path];
    for live in live_databases {
        if paths::equivalent(path, live) || lock::same_identity(path, live) {
            return Err(StorageError::InvalidConfig {
                reason: "backup destination must differ from the live databases".to_owned(),
            });
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            if paths::equivalent(path, &paths::sidecar(live, suffix)) {
                return Err(StorageError::InvalidConfig {
                    reason: "backup destination must not be a live SQLite sidecar".to_owned(),
                });
            }
        }
        if let Some(lock_paths) = lock::paths_for_existing(live)
            && (paths::equivalent(path, &lock_paths.lock)
                || paths::equivalent(path, &lock_paths.owner)
                || lock::same_identity(path, &lock_paths.lock)
                || lock::same_identity(path, &lock_paths.owner))
        {
            return Err(StorageError::InvalidConfig {
                reason: "backup destination must not be a single-instance lock file".to_owned(),
            });
        }
    }
    if paths::is_within(path, &config.snapshot_dir) {
        return Err(StorageError::InvalidConfig {
            reason: "backup destination must not be inside the snapshot directory".to_owned(),
        });
    }
    if path.is_dir() {
        return Err(StorageError::InvalidConfig {
            reason: "backup destination is a directory".to_owned(),
        });
    }
    Ok(())
}

fn backup_temp_path(path: &Path) -> Result<PathBuf, StorageError> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| StorageError::InvalidConfig {
            reason: "backup destination has no usable file name".to_owned(),
        })?;
    for attempt in 0..1_000_u32 {
        let candidate = parent.join(format!(
            ".{name}.backup-{}-{attempt}.tmp",
            time::now_millis()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(StorageError::BackupVerification {
        reason: "could not allocate a unique temporary backup path".to_owned(),
    })
}

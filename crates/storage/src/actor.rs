//! The single writer actor.
//!
//! All writes for the durable database and the rebuildable cache database are
//! funnelled through one dedicated thread that owns both write connections.
//! Callers send typed requests and receive typed replies; there is no shared
//! writer connection and therefore no lock held across an await (the crate is
//! synchronous and has no awaits at all).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::db::PragmaReport;
use crate::error::StorageError;
use crate::paths;
use crate::records::{self, UserRecord};
use crate::secrets::SecretRef;
use crate::settings::SettingKey;
use crate::time;

type Reply<T> = Sender<Result<T, StorageError>>;

/// Counts captured while the writer is quiescent, used to verify backups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BackupCounts {
    pub(crate) schema_version: u32,
    pub(crate) settings_count: u64,
    pub(crate) user_records: u64,
    pub(crate) secret_refs: u64,
}

/// Test-only shutdown fault injection.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownFault {
    /// Report a checkpoint failure without performing the explicit TRUNCATE
    /// checkpoint. Connections are still dropped normally.
    Checkpoint,
}

enum Request {
    SetSetting {
        key: SettingKey,
        value_json: String,
        expected_revision: Option<u64>,
        reply: Reply<u64>,
    },
    DeleteSetting {
        key: SettingKey,
        expected_revision: Option<u64>,
        reply: Reply<bool>,
    },
    CreateRecord {
        id: String,
        kind: String,
        payload_json: String,
        reply: Reply<UserRecord>,
    },
    UpdateRecord {
        id: String,
        payload_json: String,
        expected_revision: u64,
        reply: Reply<UserRecord>,
    },
    DeleteRecord {
        id: String,
        expected_revision: Option<u64>,
        reply: Reply<bool>,
    },
    PutSecretRef {
        secret: SecretRef,
        reply: Reply<()>,
    },
    DeleteSecretRef {
        alias: String,
        provider: String,
        reply: Reply<bool>,
    },
    CachePut {
        key: String,
        value: Vec<u8>,
        expires_at: Option<i64>,
        reply: Reply<()>,
    },
    CacheClear {
        reply: Reply<u64>,
    },
    Backup {
        path: PathBuf,
        reply: Reply<BackupCounts>,
    },
    PragmaReport {
        reply: Reply<PragmaReport>,
    },
    SetCapacityLimit {
        limit: crate::db::CapacityLimit,
        reply: Reply<()>,
    },
    Shutdown {
        reply: Reply<()>,
    },
}

pub(crate) struct StorageActor {
    sender: Option<Sender<Request>>,
    join: Option<JoinHandle<()>>,
}

impl StorageActor {
    pub(crate) fn spawn(
        main: Connection,
        cache: Connection,
        shutdown_fault: Option<ShutdownFault>,
    ) -> Result<Self, StorageError> {
        let (sender, receiver) = mpsc::channel();
        let join = thread::Builder::new()
            .name("ansa-storage-writer".to_owned())
            .spawn(move || run(main, cache, receiver, shutdown_fault))
            .map_err(StorageError::Io)?;
        Ok(Self {
            sender: Some(sender),
            join: Some(join),
        })
    }

    fn request<T>(&self, build: impl FnOnce(Reply<T>) -> Request) -> Result<T, StorageError> {
        let sender = self.sender.as_ref().ok_or(StorageError::Closed)?;
        let (reply, response) = mpsc::channel();
        sender
            .send(build(reply))
            .map_err(|_| StorageError::Closed)?;
        response.recv().map_err(|_| StorageError::Closed)?
    }

    pub(crate) fn set_setting(
        &self,
        key: SettingKey,
        value_json: String,
        expected_revision: Option<u64>,
    ) -> Result<u64, StorageError> {
        self.request(|reply| Request::SetSetting {
            key,
            value_json,
            expected_revision,
            reply,
        })
    }

    pub(crate) fn delete_setting(
        &self,
        key: SettingKey,
        expected_revision: Option<u64>,
    ) -> Result<bool, StorageError> {
        self.request(|reply| Request::DeleteSetting {
            key,
            expected_revision,
            reply,
        })
    }

    pub(crate) fn create_record(
        &self,
        id: String,
        kind: String,
        payload_json: String,
    ) -> Result<UserRecord, StorageError> {
        self.request(|reply| Request::CreateRecord {
            id,
            kind,
            payload_json,
            reply,
        })
    }

    pub(crate) fn update_record(
        &self,
        id: String,
        payload_json: String,
        expected_revision: u64,
    ) -> Result<UserRecord, StorageError> {
        self.request(|reply| Request::UpdateRecord {
            id,
            payload_json,
            expected_revision,
            reply,
        })
    }

    pub(crate) fn delete_record(
        &self,
        id: String,
        expected_revision: Option<u64>,
    ) -> Result<bool, StorageError> {
        self.request(|reply| Request::DeleteRecord {
            id,
            expected_revision,
            reply,
        })
    }

    pub(crate) fn put_secret_ref(&self, secret: SecretRef) -> Result<(), StorageError> {
        self.request(|reply| Request::PutSecretRef { secret, reply })
    }

    pub(crate) fn delete_secret_ref(
        &self,
        alias: String,
        provider: String,
    ) -> Result<bool, StorageError> {
        self.request(|reply| Request::DeleteSecretRef {
            alias,
            provider,
            reply,
        })
    }

    pub(crate) fn cache_put(
        &self,
        key: String,
        value: Vec<u8>,
        expires_at: Option<i64>,
    ) -> Result<(), StorageError> {
        self.request(|reply| Request::CachePut {
            key,
            value,
            expires_at,
            reply,
        })
    }

    pub(crate) fn clear_cache(&self) -> Result<u64, StorageError> {
        self.request(|reply| Request::CacheClear { reply })
    }

    pub(crate) fn backup(&self, path: PathBuf) -> Result<BackupCounts, StorageError> {
        self.request(|reply| Request::Backup { path, reply })
    }

    pub(crate) fn pragma_report(&self) -> Result<PragmaReport, StorageError> {
        self.request(|reply| Request::PragmaReport { reply })
    }

    pub(crate) fn set_capacity_limit(
        &self,
        limit: crate::db::CapacityLimit,
    ) -> Result<(), StorageError> {
        self.request(|reply| Request::SetCapacityLimit { limit, reply })
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), StorageError> {
        let mut result = Ok(());
        if let Some(sender) = self.sender.take() {
            let (reply, response) = mpsc::channel();
            match sender.send(Request::Shutdown { reply }) {
                Ok(()) => match response.recv() {
                    Ok(shutdown_result) => result = shutdown_result,
                    Err(_) => result = Err(StorageError::Closed),
                },
                Err(_) => result = Err(StorageError::Closed),
            }
        }
        if let Some(join) = self.join.take()
            && join.join().is_err()
            && result.is_ok()
        {
            result = Err(StorageError::Closed);
        }
        result
    }
}

impl Drop for StorageActor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn run(
    main: Connection,
    cache: Connection,
    receiver: Receiver<Request>,
    shutdown_fault: Option<ShutdownFault>,
) {
    let mut shutdown_reply = None;
    while let Ok(request) = receiver.recv() {
        match request {
            Request::SetSetting {
                key,
                value_json,
                expected_revision,
                reply,
            } => {
                let result = in_immediate_transaction(&main, |transaction| {
                    set_setting(transaction, key, &value_json, expected_revision)
                });
                let _ = reply.send(result);
            }
            Request::DeleteSetting {
                key,
                expected_revision,
                reply,
            } => {
                let result = in_immediate_transaction(&main, |transaction| {
                    delete_setting(transaction, key, expected_revision)
                });
                let _ = reply.send(result);
            }
            Request::CreateRecord {
                id,
                kind,
                payload_json,
                reply,
            } => {
                let result = in_immediate_transaction(&main, |transaction| {
                    create_record(transaction, &id, &kind, &payload_json)
                });
                let _ = reply.send(result);
            }
            Request::UpdateRecord {
                id,
                payload_json,
                expected_revision,
                reply,
            } => {
                let result = in_immediate_transaction(&main, |transaction| {
                    update_record(transaction, &id, &payload_json, expected_revision)
                });
                let _ = reply.send(result);
            }
            Request::DeleteRecord {
                id,
                expected_revision,
                reply,
            } => {
                let result = in_immediate_transaction(&main, |transaction| {
                    delete_record(transaction, &id, expected_revision)
                });
                let _ = reply.send(result);
            }
            Request::PutSecretRef { secret, reply } => {
                let result = in_immediate_transaction(&main, |transaction| {
                    put_secret_ref(transaction, &secret)
                });
                let _ = reply.send(result);
            }
            Request::DeleteSecretRef {
                alias,
                provider,
                reply,
            } => {
                let result = in_immediate_transaction(&main, |transaction| {
                    delete_secret_ref(transaction, &alias, &provider)
                });
                let _ = reply.send(result);
            }
            Request::CachePut {
                key,
                value,
                expires_at,
                reply,
            } => {
                let result = in_immediate_transaction(&cache, |transaction| {
                    cache_put(transaction, &key, &value, expires_at)
                });
                let _ = reply.send(result);
            }
            Request::CacheClear { reply } => {
                let result =
                    in_immediate_transaction(&cache, |transaction| clear_cache(transaction));
                let _ = reply.send(result);
            }
            Request::Backup { path, reply } => {
                let _ = reply.send(backup(&main, &path));
            }
            Request::PragmaReport { reply } => {
                let _ = reply.send(crate::db::pragma_report(&main));
            }
            Request::SetCapacityLimit { limit, reply } => {
                let _ = reply.send(crate::db::apply_capacity_limit(&main, limit));
            }
            Request::Shutdown { reply } => {
                shutdown_reply = Some(reply);
                break;
            }
        }
    }
    let result = shutdown_connections(main, cache, shutdown_fault);
    if let Some(reply) = shutdown_reply {
        let _ = reply.send(result);
    }
}

fn shutdown_connections(
    main: Connection,
    cache: Connection,
    shutdown_fault: Option<ShutdownFault>,
) -> Result<(), StorageError> {
    if matches!(shutdown_fault, Some(ShutdownFault::Checkpoint)) {
        // Dropping the connections still closes them, but we deliberately skip
        // the explicit checkpoint and report failure so the restore path must
        // abort without deleting sidecars.
        drop(main);
        drop(cache);
        return Err(StorageError::CheckpointFailed {
            reason: "injected shutdown checkpoint failure".to_owned(),
        });
    }

    let mut first_error = None;
    record_first(&mut first_error, checkpoint(&main, "durable"));
    record_first(&mut first_error, checkpoint(&cache, "cache"));
    record_first(
        &mut first_error,
        main.close().map_err(|(_, error)| StorageError::from(error)),
    );
    record_first(
        &mut first_error,
        cache
            .close()
            .map_err(|(_, error)| StorageError::from(error)),
    );
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn record_first(slot: &mut Option<StorageError>, result: Result<(), StorageError>) {
    if slot.is_none()
        && let Err(error) = result
    {
        *slot = Some(error);
    }
}

fn checkpoint(connection: &Connection, label: &str) -> Result<(), StorageError> {
    let (busy, _log, _checkpointed): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        return Err(StorageError::CheckpointFailed {
            reason: format!("{label} WAL checkpoint remained busy"),
        });
    }
    if !wal_is_clear(connection)? {
        return Err(StorageError::CheckpointFailed {
            reason: format!("{label} -wal file is still non-empty after checkpoint"),
        });
    }
    Ok(())
}

fn wal_is_clear(connection: &Connection) -> Result<bool, StorageError> {
    let Some(path) = connection.path() else {
        return Ok(true);
    };
    let wal = paths::sidecar(Path::new(path), "-wal");
    match fs::metadata(&wal) {
        Ok(metadata) => Ok(metadata.len() == 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(StorageError::Io(error)),
    }
}

/// Run one writer operation inside an explicit `IMMEDIATE` transaction.
///
/// A single SQL statement already has implicit atomicity, but wrapping every
/// acknowledged write in an explicit transaction makes the boundary uniform
/// and guarantees that a conflict check and its write see the same snapshot.
fn in_immediate_transaction<T>(
    connection: &Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let transaction = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    let value = operation(&transaction)?;
    transaction.commit()?;
    Ok(value)
}

fn revision_from_i64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn revision_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn current_setting_revision(
    connection: &Connection,
    key: SettingKey,
) -> Result<Option<u64>, StorageError> {
    let revision = connection
        .query_row(
            "SELECT revision FROM settings WHERE key = ?1",
            [key.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(revision.map(revision_from_i64))
}

fn current_record_revision(connection: &Connection, id: &str) -> Result<Option<u64>, StorageError> {
    let revision = connection
        .query_row(
            "SELECT revision FROM user_records WHERE id = ?1",
            [id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(revision.map(revision_from_i64))
}

fn set_setting(
    connection: &Connection,
    key: SettingKey,
    value_json: &str,
    expected_revision: Option<u64>,
) -> Result<u64, StorageError> {
    let now = time::now_unix();
    match expected_revision {
        None => {
            let inserted = connection.execute(
                "INSERT INTO settings (key, value_json, revision, updated_at)
                 VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(key) DO NOTHING",
                params![key.as_str(), value_json, now],
            )?;
            if inserted == 1 {
                Ok(1)
            } else {
                Err(StorageError::Conflict {
                    entity: "setting",
                    key: key.as_str().to_owned(),
                    expected: None,
                    actual: current_setting_revision(connection, key)?,
                })
            }
        }
        Some(expected) => {
            let updated = connection.execute(
                "UPDATE settings
                 SET value_json = ?1, revision = revision + 1, updated_at = ?2
                 WHERE key = ?3 AND revision = ?4",
                params![value_json, now, key.as_str(), revision_to_i64(expected)],
            )?;
            if updated == 1 {
                Ok(expected.saturating_add(1))
            } else {
                Err(StorageError::Conflict {
                    entity: "setting",
                    key: key.as_str().to_owned(),
                    expected: Some(expected),
                    actual: current_setting_revision(connection, key)?,
                })
            }
        }
    }
}

fn delete_setting(
    connection: &Connection,
    key: SettingKey,
    expected_revision: Option<u64>,
) -> Result<bool, StorageError> {
    let deleted = match expected_revision {
        None => connection.execute("DELETE FROM settings WHERE key = ?1", [key.as_str()])?,
        Some(expected) => connection.execute(
            "DELETE FROM settings WHERE key = ?1 AND revision = ?2",
            params![key.as_str(), revision_to_i64(expected)],
        )?,
    };
    if deleted == 1 {
        return Ok(true);
    }
    if let Some(expected) = expected_revision {
        let actual = current_setting_revision(connection, key)?;
        if actual != Some(expected) {
            return Err(StorageError::Conflict {
                entity: "setting",
                key: key.as_str().to_owned(),
                expected: Some(expected),
                actual,
            });
        }
    }
    Ok(false)
}

fn create_record(
    connection: &Connection,
    id: &str,
    kind: &str,
    payload_json: &str,
) -> Result<UserRecord, StorageError> {
    let now = time::now_unix();
    let inserted = connection.execute(
        "INSERT INTO user_records
             (id, kind, revision, payload_json, created_at, updated_at)
         VALUES (?1, ?2, 1, ?3, ?4, ?4)
         ON CONFLICT(id) DO NOTHING",
        params![id, kind, payload_json, now],
    )?;
    if inserted == 0 {
        return Err(StorageError::Conflict {
            entity: "user_record",
            key: id.to_owned(),
            expected: None,
            actual: current_record_revision(connection, id)?,
        });
    }
    Ok(UserRecord {
        id: id.to_owned(),
        kind: kind.to_owned(),
        revision: 1,
        payload_json: payload_json.to_owned(),
        created_at: now,
        updated_at: now,
    })
}

fn update_record(
    connection: &Connection,
    id: &str,
    payload_json: &str,
    expected_revision: u64,
) -> Result<UserRecord, StorageError> {
    let now = time::now_unix();
    let updated = connection.execute(
        "UPDATE user_records
         SET payload_json = ?1, revision = revision + 1, updated_at = ?2
         WHERE id = ?3 AND revision = ?4",
        params![payload_json, now, id, revision_to_i64(expected_revision)],
    )?;
    if updated == 0 {
        return Err(StorageError::Conflict {
            entity: "user_record",
            key: id.to_owned(),
            expected: Some(expected_revision),
            actual: current_record_revision(connection, id)?,
        });
    }
    load_record(connection, id)?.ok_or_else(|| StorageError::InvalidRecord {
        reason: "record disappeared after a successful update".to_owned(),
    })
}

fn delete_record(
    connection: &Connection,
    id: &str,
    expected_revision: Option<u64>,
) -> Result<bool, StorageError> {
    let deleted = match expected_revision {
        None => connection.execute("DELETE FROM user_records WHERE id = ?1", [id])?,
        Some(expected) => connection.execute(
            "DELETE FROM user_records WHERE id = ?1 AND revision = ?2",
            params![id, revision_to_i64(expected)],
        )?,
    };
    if deleted == 1 {
        return Ok(true);
    }
    if let Some(expected) = expected_revision {
        let actual = current_record_revision(connection, id)?;
        if actual != Some(expected) {
            return Err(StorageError::Conflict {
                entity: "user_record",
                key: id.to_owned(),
                expected: Some(expected),
                actual,
            });
        }
    }
    Ok(false)
}

fn put_secret_ref(connection: &Connection, secret: &SecretRef) -> Result<(), StorageError> {
    let now = time::now_unix();
    connection.execute(
        "INSERT INTO secret_refs (alias, provider, reference, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(alias) DO UPDATE SET
             provider = excluded.provider,
             reference = excluded.reference,
             updated_at = excluded.updated_at",
        params![secret.alias(), secret.provider(), secret.reference(), now],
    )?;
    Ok(())
}

fn delete_secret_ref(
    connection: &Connection,
    alias: &str,
    provider: &str,
) -> Result<bool, StorageError> {
    let deleted = connection.execute(
        "DELETE FROM secret_refs WHERE alias = ?1 AND provider = ?2",
        params![alias, provider],
    )?;
    Ok(deleted == 1)
}

fn cache_put(
    connection: &Connection,
    key: &str,
    value: &[u8],
    expires_at: Option<i64>,
) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO cache_entries (key, value, expires_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             expires_at = excluded.expires_at",
        params![key, value, expires_at],
    )?;
    Ok(())
}

fn clear_cache(connection: &Connection) -> Result<u64, StorageError> {
    let removed = connection.execute("DELETE FROM cache_entries", [])?;
    Ok(u64::try_from(removed).unwrap_or(0))
}

fn backup(connection: &Connection, path: &Path) -> Result<BackupCounts, StorageError> {
    let path_text = path.to_str().ok_or_else(|| StorageError::InvalidConfig {
        reason: "backup path is not valid UTF-8".to_owned(),
    })?;
    // VACUUM INTO requires the destination not to exist. The caller always
    // supplies a unique temporary path; never delete a previous backup here.
    connection.execute("VACUUM INTO ?1", [path_text])?;

    let versions = crate::migrations::applied_versions(connection)?;
    let schema_version = versions.last().copied().unwrap_or(0);
    Ok(BackupCounts {
        schema_version,
        settings_count: count(connection, "settings")?,
        user_records: count(connection, "user_records")?,
        secret_refs: count(connection, "secret_refs")?,
    })
}

fn count(connection: &Connection, table: &str) -> Result<u64, StorageError> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count: i64 = connection.query_row(&sql, [], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or(0))
}

pub(crate) fn read_setting_row(
    connection: &Connection,
    key: SettingKey,
) -> Result<Option<(String, u64, i64)>, StorageError> {
    let row = connection
        .query_row(
            "SELECT value_json, revision, updated_at FROM settings WHERE key = ?1",
            [key.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(row.map(|(json, revision, updated_at)| (json, revision_from_i64(revision), updated_at)))
}

pub(crate) fn load_record(
    connection: &Connection,
    id: &str,
) -> Result<Option<UserRecord>, StorageError> {
    let record = connection
        .query_row(
            "SELECT id, kind, revision, payload_json, created_at, updated_at
             FROM user_records WHERE id = ?1",
            [id],
            row_to_record,
        )
        .optional()?;
    Ok(record)
}

pub(crate) fn list_records(
    connection: &Connection,
    kind: Option<&str>,
    limit: usize,
    offset: u64,
) -> Result<Vec<UserRecord>, StorageError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let offset = i64::try_from(offset).unwrap_or(i64::MAX);
    let mut records = Vec::new();
    match kind {
        Some(kind) => {
            let mut statement = connection.prepare(
                "SELECT id, kind, revision, payload_json, created_at, updated_at
                 FROM user_records WHERE kind = ?1
                 ORDER BY created_at ASC, id ASC
                 LIMIT ?2 OFFSET ?3",
            )?;
            let rows = statement.query_map(params![kind, limit, offset], row_to_record)?;
            for row in rows {
                records.push(row?);
            }
        }
        None => {
            let mut statement = connection.prepare(
                "SELECT id, kind, revision, payload_json, created_at, updated_at
                 FROM user_records
                 ORDER BY created_at ASC, id ASC
                 LIMIT ?1 OFFSET ?2",
            )?;
            let rows = statement.query_map(params![limit, offset], row_to_record)?;
            for row in rows {
                records.push(row?);
            }
        }
    }
    Ok(records)
}

fn row_to_record(row: &rusqlite::Row<'_>) -> Result<UserRecord, rusqlite::Error> {
    Ok(UserRecord {
        id: row.get(0)?,
        kind: row.get(1)?,
        revision: revision_from_i64(row.get::<_, i64>(2)?),
        payload_json: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

pub(crate) fn read_secret_ref(
    connection: &Connection,
    alias: &str,
) -> Result<Option<SecretRef>, StorageError> {
    let row = connection
        .query_row(
            "SELECT alias, provider, reference FROM secret_refs WHERE alias = ?1",
            [alias],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    match row {
        Some((alias, provider, reference)) => {
            let secret =
                SecretRef::new(alias, provider, reference).map_err(|_| StorageError::Corrupt {
                    detail: "stored credential reference is invalid".to_owned(),
                })?;
            Ok(Some(secret))
        }
        None => Ok(None),
    }
}

pub(crate) fn cache_get(
    connection: &Connection,
    key: &str,
) -> Result<Option<Vec<u8>>, StorageError> {
    let now = time::now_unix();
    let value = connection
        .query_row(
            "SELECT value FROM cache_entries
             WHERE key = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
            params![key, now],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    Ok(value)
}

pub(crate) fn validate_record_fields(
    id: &str,
    kind: &str,
    payload_json: &str,
) -> Result<(), StorageError> {
    records::validate_id(id)?;
    records::validate_kind(kind)?;
    records::validate_payload(payload_json)
}

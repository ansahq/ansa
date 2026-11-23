//! Integration tests for the transactional storage foundation.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ansa_storage::{
    BackupFault, CapacityLimit, LaunchMode, MAX_RECORD_PAGE, Migration, MigrationFault,
    MigrationPlan, RecoveryKind, SecretRef, SettingKey, SettingValue, ShutdownFault, StorageConfig,
    StorageError, Store, Theme, UserRecord,
};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

fn config(dir: &TempDir) -> StorageConfig {
    StorageConfig::new(
        dir.path().join("ansa.sqlite"),
        dir.path().join("cache.sqlite"),
    )
}

fn open(dir: &TempDir) -> Store {
    // Tests run in parallel, and the identity lock is keyed by the durable
    // file's dev/inode rather than its path. A child/other test that is still
    // tearing down a lock for a recycled identity can briefly collide with a
    // fresh temporary database. Retry only that transient condition; a store
    // that remains locked still fails the test after the short deadline.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match Store::open(config(dir)) {
            Ok(store) => return store,
            Err(StorageError::AlreadyLocked { .. }) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("store should open: {error:?}"),
        }
    }
}

fn column_exists(connection: &Connection, table: &str, column: &str) -> bool {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("table info");
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("column names")
        .collect::<Result<Vec<_>, _>>()
        .expect("read columns");
    names.iter().any(|name| name == column)
}

fn applied_versions(path: &Path) -> Vec<u32> {
    let connection = Connection::open(path).expect("open raw database");
    let mut statement = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .expect("migration query");
    statement
        .query_map([], |row| row.get(0))
        .expect("migration rows")
        .collect::<Result<Vec<u32>, _>>()
        .expect("read migrations")
}

fn noop_migration(_transaction: &rusqlite::Transaction<'_>) -> Result<(), StorageError> {
    Ok(())
}

#[test]
fn pragmas_are_crash_safe_and_databases_are_separate() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);
    let report = store.pragma_report().expect("pragma report");

    assert_eq!(report.foreign_keys, 1, "foreign keys must be enabled");
    assert_eq!(report.journal_mode, "wal", "WAL journaling must be active");
    assert_eq!(report.synchronous, 2, "synchronous must be FULL");
    assert!(report.busy_timeout >= 5_000, "busy timeout must be set");
    assert!(store.main_path().exists());
    assert!(store.cache_path().exists());
    assert_ne!(
        fs::canonicalize(store.main_path()).expect("main canonical"),
        fs::canonicalize(store.cache_path()).expect("cache canonical")
    );
    assert_eq!(store.schema_version(), 3);
}

#[test]
fn open_rejects_identical_durable_and_cache_paths() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("same.sqlite");
    let config = StorageConfig::new(path.clone(), path);
    let error = Store::open(config).expect_err("must reject one-file configuration");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));
}

#[test]
fn noncanonical_cache_path_is_rejected_before_creating_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main = dir.path().join("ansa.sqlite");
    // Regression: `nested\..\ansa.sqlite` used to bypass the guard because
    // neither path existed yet, then created cache_entries inside the durable
    // database and failed permanently on the next open.
    let cache = dir.path().join("nested").join("..").join("ansa.sqlite");
    let error = Store::open(StorageConfig::new(main.clone(), cache))
        .expect_err("must reject non-canonical same-file cache path");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));
    assert!(
        !main.exists(),
        "validation must happen before creating the DB"
    );

    // Symmetric case: the durable path is the non-canonical spelling.
    let canonical = dir.path().join("ansa.sqlite");
    let noncanonical_main = dir.path().join("nested").join("..").join("ansa.sqlite");
    let error = Store::open(StorageConfig::new(noncanonical_main, canonical))
        .expect_err("must reject non-canonical durable path");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));
    assert!(!main.exists());
}

#[cfg(windows)]
#[test]
fn case_variant_cache_path_is_rejected_on_windows() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main = dir.path().join("ansa.sqlite");
    let cache = dir.path().join("ANSA.SQLITE");
    let error = Store::open(StorageConfig::new(main, cache))
        .expect_err("Windows case variants must compare equal");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));
}

#[test]
fn existing_durable_database_with_cache_table_fails_with_recovery_message() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main = dir.path().join("ansa.sqlite");
    let store = open(&dir);
    drop(store);
    let connection = Connection::open(&main).expect("raw open");
    connection
        .execute_batch(
            "CREATE TABLE cache_entries (
                 key TEXT PRIMARY KEY,
                 value BLOB NOT NULL,
                 expires_at INTEGER
             ) STRICT;",
        )
        .expect("create stale cache table");
    drop(connection);

    let error = Store::open(config(&dir)).expect_err("must refuse half-collided database");
    let reason = match error {
        StorageError::InvalidConfig { reason } => reason,
        other => panic!("expected InvalidConfig, got {other:?}"),
    };
    assert!(reason.contains("cache_entries"));
    assert!(reason.contains("Back up"));
}

#[test]
fn durable_tables_in_the_cache_database_are_rejected() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main = dir.path().join("ansa.sqlite");
    let wrong_cache = dir.path().join("wrong-cache.sqlite");
    let store = open(&dir);
    drop(store);
    fs::copy(&main, &wrong_cache).expect("copy durable database to cache path");
    let error = Store::open(StorageConfig::new(main, wrong_cache))
        .expect_err("cache file must not contain durable tables");
    let reason = match error {
        StorageError::InvalidConfig { reason } => reason,
        other => panic!("expected InvalidConfig, got {other:?}"),
    };
    assert!(reason.contains("durable table"));
}

#[test]
fn invalid_config_has_no_filesystem_side_effects() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut invalid_reads = config(&dir);
    invalid_reads.read_connections = 0;
    let error = Store::open(invalid_reads).expect_err("zero read connections must be rejected");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));
    assert!(!dir.path().join("ansa.sqlite").exists());
    assert!(!dir.path().join("cache.sqlite").exists());

    let other = tempfile::tempdir().expect("temp dir");
    let mut invalid_plan = config(&other);
    invalid_plan.migration_plan = MigrationPlan::new(vec![
        Migration::new(2, "bad-first", false, noop_migration),
        Migration::new(1, "late", false, noop_migration),
    ]);
    assert!(matches!(
        Store::open(invalid_plan),
        Err(StorageError::InvalidMigrationPlan { .. })
    ));
    assert!(!other.path().join("ansa.sqlite").exists());
    assert!(!other.path().join("cache.sqlite").exists());
}

#[test]
fn settings_are_revisioned_with_compare_and_swap() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);

    let revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        )
        .expect("create setting");
    assert_eq!(revision, 1);

    let snapshot = store
        .get_setting(SettingKey::Hotkey)
        .expect("read setting")
        .expect("setting exists");
    assert_eq!(snapshot.revision, Some(1));
    assert_eq!(
        snapshot.value,
        SettingValue::Hotkey("Ctrl+Space".to_owned())
    );
    assert!(snapshot.diagnostic.is_none());

    let revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Shift+K".to_owned()),
            Some(1),
        )
        .expect("update setting");
    assert_eq!(revision, 2);

    let conflict = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Alt+F4".to_owned()),
            Some(1),
        )
        .expect_err("stale revision must conflict");
    match conflict {
        StorageError::Conflict {
            expected, actual, ..
        } => {
            assert_eq!(expected, Some(1));
            assert_eq!(actual, Some(2));
        }
        other => panic!("expected conflict, got {other:?}"),
    }

    let duplicate_create = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Alt+F4".to_owned()),
            None,
        )
        .expect_err("create must conflict when a row exists");
    assert!(matches!(
        duplicate_create,
        StorageError::Conflict {
            actual: Some(2),
            ..
        }
    ));

    drop(store);
    let store = open(&dir);
    let snapshot = store
        .get_setting(SettingKey::Hotkey)
        .expect("read after reopen")
        .expect("setting exists");
    assert_eq!(snapshot.revision, Some(2));
    assert_eq!(
        snapshot.value,
        SettingValue::Hotkey("Ctrl+Shift+K".to_owned())
    );
}

#[test]
fn settings_validation_corpus() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);

    let valid_hotkeys = [
        "Ctrl+Space",
        "Ctrl+Shift+K",
        "Alt+F4",
        "Super+Enter",
        "Control+Alt+1",
        "Ctrl+PageDown",
        "shift+meta+z",
    ];
    let mut revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey(valid_hotkeys[0].to_owned()),
            None,
        )
        .expect("initial hotkey");
    for hotkey in &valid_hotkeys[1..] {
        revision = store
            .set_setting(
                SettingKey::Hotkey,
                &SettingValue::Hotkey((*hotkey).to_owned()),
                Some(revision),
            )
            .unwrap_or_else(|error| panic!("hotkey {hotkey} should be valid: {error}"));
    }
    assert_eq!(revision, valid_hotkeys.len() as u64);

    let invalid_hotkeys = [
        "",
        "Space",
        "Ctrl+",
        "+Space",
        "Ctrl+Shift",
        "Ctrl+Ctrl+K",
        "Ctrl+Control+K",
        "Hyper+K",
        "Ctrl+K+J",
        "Ctrl+NotAKey",
        "Ctrl+F25",
        "Ctrl+Space\n",
    ];
    for hotkey in invalid_hotkeys {
        let error = store
            .set_setting(
                SettingKey::Hotkey,
                &SettingValue::Hotkey(hotkey.to_owned()),
                Some(revision),
            )
            .expect_err("invalid hotkey must be rejected");
        assert!(
            matches!(error, StorageError::InvalidSetting { .. }),
            "hotkey {hotkey:?} produced {error:?}"
        );
    }

    let valid_aliases = ["go", "a1", "a.b_c-d", "Run"];
    let mut alias_revision = store
        .set_setting(
            SettingKey::AliasPrefix,
            &SettingValue::AliasPrefix(valid_aliases[0].to_owned()),
            None,
        )
        .expect("initial alias");
    for alias in &valid_aliases[1..] {
        alias_revision = store
            .set_setting(
                SettingKey::AliasPrefix,
                &SettingValue::AliasPrefix((*alias).to_owned()),
                Some(alias_revision),
            )
            .expect("valid alias");
    }

    let invalid_aliases = [
        "",
        " leading",
        "with space",
        "-dash",
        "semi;colon",
        "a/b",
        "abcdefghijklmnopqrstuvwxyz0123456",
    ];
    for alias in invalid_aliases {
        assert!(matches!(
            store.set_setting(
                SettingKey::AliasPrefix,
                &SettingValue::AliasPrefix(alias.to_owned()),
                Some(alias_revision),
            ),
            Err(StorageError::InvalidSetting { .. })
        ));
    }

    assert!(matches!(
        store.set_setting(
            SettingKey::Theme,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        ),
        Err(StorageError::InvalidSetting { .. })
    ));

    assert_eq!(
        store
            .set_setting(SettingKey::Theme, &SettingValue::Theme(Theme::Dark), None)
            .expect("theme"),
        1
    );
    assert_eq!(
        store
            .set_setting(
                SettingKey::LaunchMode,
                &SettingValue::LaunchMode(LaunchMode::Show),
                None,
            )
            .expect("launch mode"),
        1
    );
    assert_eq!(
        store
            .set_setting(
                SettingKey::MaximumResults,
                &SettingValue::MaximumResults(500),
                None,
            )
            .expect("maximum results"),
        1
    );
    for invalid in [0, 501, u32::MAX] {
        assert!(matches!(
            store.set_setting(
                SettingKey::MaximumResults,
                &SettingValue::MaximumResults(invalid),
                Some(1),
            ),
            Err(StorageError::InvalidSetting { .. })
        ));
    }
}

#[test]
fn invalid_stored_value_falls_back_without_deleting_data() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main_path = dir.path().join("ansa.sqlite");
    let store = open(&dir);
    store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        )
        .expect("hotkey");
    store
        .set_setting(
            SettingKey::AliasPrefix,
            &SettingValue::AliasPrefix("go".to_owned()),
            None,
        )
        .expect("alias");
    let record = store
        .create_record("note-1", "note", r#"{"text":"keep me"}"#)
        .expect("record");
    drop(store);

    let connection = Connection::open(&main_path).expect("raw open");
    connection
        .execute(
            "UPDATE settings SET value_json = ?1, revision = revision + 1 WHERE key = 'hotkey'",
            ["this is not json"],
        )
        .expect("corrupt setting");
    drop(connection);

    let store = open(&dir);
    let fallback = store
        .get_or_default(SettingKey::Hotkey)
        .expect("fallback read");
    assert_eq!(fallback.value, SettingKey::Hotkey.default_value());
    assert_eq!(fallback.revision, Some(2));
    assert!(matches!(
        fallback.diagnostic,
        Some(StorageError::InvalidStoredValue { key: "hotkey" })
    ));

    assert!(matches!(
        store.get_setting(SettingKey::Hotkey),
        Err(StorageError::InvalidStoredValue { key: "hotkey" })
    ));

    let alias = store
        .get_setting(SettingKey::AliasPrefix)
        .expect("alias survives")
        .expect("alias exists");
    assert_eq!(alias.value, SettingValue::AliasPrefix("go".to_owned()));
    assert_eq!(alias.revision, Some(1));

    assert_eq!(
        store.get_record("note-1").expect("record read"),
        Some(record.clone())
    );

    let connection = Connection::open(&main_path).expect("raw reopen");
    let stored: String = connection
        .query_row(
            "SELECT value_json FROM settings WHERE key = 'hotkey'",
            [],
            |row| row.get(0),
        )
        .expect("stored value");
    assert_eq!(stored, "this is not json", "reads must not rewrite the row");
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 2, "unrelated settings must survive");
    drop(connection);
    drop(store);

    let connection = Connection::open(&main_path).expect("raw reopen for invalid value");
    connection
        .execute(
            "UPDATE settings SET value_json = ?1, revision = revision + 1 WHERE key = 'alias_prefix'",
            [r#"{"type":"alias_prefix","value":"with space"}"#],
        )
        .expect("invalid stored value");
    drop(connection);

    let store = open(&dir);
    let alias = store
        .get_or_default(SettingKey::AliasPrefix)
        .expect("invalid value fallback");
    assert_eq!(alias.value, SettingKey::AliasPrefix.default_value());
    assert_eq!(alias.revision, Some(2));
    assert!(matches!(
        alias.diagnostic,
        Some(StorageError::InvalidStoredValue {
            key: "alias_prefix"
        })
    ));
    assert!(matches!(
        store
            .get_setting(SettingKey::Hotkey)
            .expect_err("hotkey row is still corrupt"),
        StorageError::InvalidStoredValue { key: "hotkey" }
    ));
    assert_eq!(
        store.get_record("note-1").expect("record read"),
        Some(record.clone())
    );
}

#[test]
fn secret_references_are_validated_and_debug_redacted() {
    let directory = tempfile::tempdir().expect("temp dir");
    let store = open(&directory);
    let reference =
        SecretRef::new("github", "vault", "secret://vault/github-token").expect("valid reference");
    assert_eq!(reference.alias(), "github");
    assert_eq!(reference.provider(), "vault");
    assert_eq!(reference.reference(), "secret://vault/github-token");

    let debug = format!("{reference:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("github-token"));

    store.put_secret_ref(reference.clone()).expect("store ref");
    assert_eq!(
        store.get_secret_ref("github").expect("read ref"),
        Some(reference)
    );

    let sentinel = "SK-PLAINTEXT-SENTINEL-7d2a1f";
    for (alias, provider, value) in [
        ("bad", "vault", sentinel),
        ("bad", "vault", "secret://"),
        ("bad", "vault", "secret://vault/with space"),
        ("bad", "vault", "secret://vault/with\ncontrol"),
        ("bad", "vault", "http://example.invalid/token"),
        ("bad", "vault", "keychain://"),
    ] {
        let error = SecretRef::new(alias, provider, value).expect_err("must reject");
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }

    let long = format!("secret://vault/{}", "x".repeat(600));
    assert!(SecretRef::new("bad", "vault", long).is_err());
}

/// This test covers the `SecretRef` acceptance path only. It proves that a
/// token offered as a credential reference is rejected and its bytes do not
/// appear in the main DB, cache DB, WAL or SHM files. It does **not** claim
/// detection in generic `UserRecord`/cache payloads; see
/// `generic_record_and_cache_payloads_are_unencrypted_caller_data`.
#[test]
fn secret_ref_sentinel_is_never_persisted() {
    let sentinel = "PLAINTEXT-SENTINEL-6f1e0c2a-DO-NOT-STORE";
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);

    let rejection = SecretRef::new("github", "vault", sentinel).expect_err("reject plaintext");
    assert!(!rejection.to_string().contains(sentinel));
    assert!(!format!("{rejection:?}").contains(sentinel));

    store
        .put_secret_ref(
            SecretRef::new("github", "vault", "secret://vault/github").expect("valid ref"),
        )
        .expect("store valid ref");
    store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        )
        .expect("setting");
    store
        .create_record("note-scan", "note", r#"{"text":"ordinary"}"#)
        .expect("record");
    store
        .cache_put("ordinary-cache-key", b"ordinary value", None)
        .expect("cache");

    let connection =
        Connection::open_with_flags(store.main_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read main");
    let stored_reference: String = connection
        .query_row(
            "SELECT reference FROM secret_refs WHERE alias = 'github'",
            [],
            |row| row.get(0),
        )
        .expect("reference row");
    assert_eq!(stored_reference, "secret://vault/github");
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('settings', 'user_records', 'secret_refs')",
            [],
            |row| row.get(0),
        )
        .expect("tables");
    assert_eq!(tables, 3);
    drop(connection);

    let main_wal = PathBuf::from(format!("{}-wal", store.main_path().display()));
    assert!(
        main_wal.exists(),
        "the WAL file should exist while acknowledged writes are open"
    );

    let mut scanned = 0;
    for entry in fs::read_dir(dir.path()).expect("list files") {
        let entry = entry.expect("entry");
        let path = entry.path();
        // The single-instance lock file is held with an exclusive OS lock, so
        // Windows refuses to read it from another handle. It contains only the
        // holder PID and is not part of the persisted data path.
        if path.is_file() && path != store.lock_path() {
            let bytes = fs::read(&path).expect("read database file");
            assert!(
                !contains_bytes(&bytes, sentinel.as_bytes()),
                "sentinel leaked into {}",
                path.display()
            );
            scanned += 1;
        }
    }
    assert!(scanned >= 3, "expected main, cache and WAL files");
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Documents the deliberate boundary: generic record/cache payloads are
/// unencrypted caller data. Storage does not guess whether arbitrary bytes are
/// secret-shaped, so callers must not put secrets, clipboard bodies or note
/// text there until an encrypted-payload layer exists. This test stores a
/// sentinel through those APIs and asserts the expected (persisted) result.
#[test]
fn generic_record_and_cache_payloads_are_unencrypted_caller_data() {
    let sentinel = "GENERIC-PAYLOAD-SENTINEL-3c9b-DO-NOT-USE";
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);

    let payload = format!(r#"{{"token":"{sentinel}"}}"#);
    let record = store
        .create_record("generic-note", "note", &payload)
        .expect("generic record is accepted");
    assert_eq!(record.payload_json, payload);
    assert!(
        store
            .get_record("generic-note")
            .expect("record read")
            .expect("record exists")
            .payload_json
            .contains(sentinel)
    );

    store
        .cache_put("generic-cache", sentinel.as_bytes(), None)
        .expect("generic cache value is accepted");
    assert_eq!(
        store.cache_get("generic-cache").expect("cache read"),
        Some(sentinel.as_bytes().to_vec())
    );

    // The credential-reference path still rejects the same plaintext.
    assert!(SecretRef::new("github", "vault", sentinel).is_err());
}

#[test]
fn cache_clear_is_isolated_and_idempotent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);
    store
        .create_record("durable", "note", r#"{"text":"keep"}"#)
        .expect("record");
    store
        .set_setting(SettingKey::Theme, &SettingValue::Theme(Theme::Dark), None)
        .expect("setting");

    store.cache_put("one", b"1", None).expect("cache one");
    store.cache_put("two", b"2", None).expect("cache two");
    assert_eq!(
        store.cache_get("one").expect("get one"),
        Some(b"1".to_vec())
    );

    assert_eq!(store.clear_cache().expect("first clear"), 2);
    assert_eq!(store.clear_cache().expect("second clear"), 0);
    assert_eq!(store.cache_get("one").expect("get cleared"), None);
    assert_eq!(store.cache_get("two").expect("get cleared"), None);

    assert!(store.get_record("durable").expect("read record").is_some());
    assert_eq!(
        store
            .get_setting(SettingKey::Theme)
            .expect("read setting")
            .expect("setting exists")
            .value,
        SettingValue::Theme(Theme::Dark)
    );

    store
        .cache_put("expired", b"old", Some(1))
        .expect("expired entry");
    assert_eq!(store.cache_get("expired").expect("expired read"), None);
    store
        .cache_put("future", b"new", Some(i64::MAX))
        .expect("future entry");
    assert_eq!(
        store.cache_get("future").expect("future read"),
        Some(b"new".to_vec())
    );
}

#[test]
fn record_crud_and_revision_conflicts() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);

    let record = store
        .create_record("note-1", "note", r#"{"text":"one"}"#)
        .expect("create");
    assert_eq!(record.revision, 1);
    assert_eq!(record.created_at, record.updated_at);

    let duplicate = store
        .create_record("note-1", "note", r#"{"text":"again"}"#)
        .expect_err("duplicate id");
    assert!(matches!(
        duplicate,
        StorageError::Conflict {
            expected: None,
            actual: Some(1),
            ..
        }
    ));

    let updated = store
        .update_record("note-1", r#"{"text":"two"}"#, 1)
        .expect("update");
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.payload_json, r#"{"text":"two"}"#);

    let stale = store
        .update_record("note-1", r#"{"text":"three"}"#, 1)
        .expect_err("stale update");
    assert!(matches!(
        stale,
        StorageError::Conflict {
            expected: Some(1),
            actual: Some(2),
            ..
        }
    ));

    let second = store
        .create_record(
            "link-1",
            "quicklink",
            r#"{"url":"https://example.invalid"}"#,
        )
        .expect("second record");
    assert_eq!(store.list_records(None).expect("list all").len(), 2);
    assert_eq!(
        store.list_records(Some("quicklink")).expect("list kind"),
        vec![second]
    );

    let stale_delete = store
        .delete_record("note-1", Some(1))
        .expect_err("stale delete");
    assert!(matches!(stale_delete, StorageError::Conflict { .. }));
    assert!(store.delete_record("note-1", Some(2)).expect("delete"));
    assert!(store.get_record("note-1").expect("read deleted").is_none());
    assert!(!store.delete_record("note-1", None).expect("second delete"));

    for (id, kind, payload) in [
        ("", "note", "{}"),
        ("bad id", "note", "{}"),
        ("ok", "", "{}"),
        ("ok", "bad kind", "{}"),
        ("ok", "note", "not json"),
    ] {
        assert!(matches!(
            store.create_record(id, kind, payload),
            Err(StorageError::InvalidRecord { .. })
        ));
    }
}

#[test]
fn backup_restore_round_trip_preserves_revisions_and_counts() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut store = open(&dir);
    let hotkey_revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        )
        .expect("hotkey create");
    let hotkey_revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Shift+Space".to_owned()),
            Some(hotkey_revision),
        )
        .expect("hotkey update");
    assert_eq!(hotkey_revision, 2);
    store
        .set_setting(SettingKey::Theme, &SettingValue::Theme(Theme::Light), None)
        .expect("theme");
    store
        .create_record("note-1", "note", r#"{"text":"one"}"#)
        .expect("record one");
    store
        .create_record("note-2", "note", r#"{"text":"two"}"#)
        .expect("record two");
    store
        .update_record("note-2", r#"{"text":"two updated"}"#, 1)
        .expect("record update");
    store
        .cache_put("cache-key", b"cache-value", None)
        .expect("cache");
    store
        .put_secret_ref(
            SecretRef::new("github", "vault", "secret://vault/github").expect("secret ref"),
        )
        .expect("secret ref store");

    let backup_path = dir.path().join("backup.sqlite");
    let backup = store.backup_to(&backup_path).expect("backup");
    assert_eq!(backup.schema_version, 3);
    assert_eq!(backup.settings_count, 2);
    assert_eq!(backup.user_records, 2);
    assert_eq!(backup.secret_refs, 1);
    assert_eq!(backup.integrity, "ok");
    assert!(backup_path.exists());

    store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Alt+Space".to_owned()),
            Some(2),
        )
        .expect("post-backup update");
    store
        .delete_record("note-1", None)
        .expect("post-backup delete");
    store
        .create_record("note-3", "note", r#"{"text":"three"}"#)
        .expect("post-backup insert");

    let restore = store.restore_from(&backup_path).expect("restore");
    assert_eq!(restore.previous_schema_version, 3);
    assert_eq!(restore.schema_version, 3);
    assert_eq!(restore.settings_count, 2);
    assert_eq!(restore.user_records, 2);
    assert_eq!(restore.secret_refs, 1);
    assert_eq!(restore.integrity, "ok");

    let hotkey = store
        .get_setting(SettingKey::Hotkey)
        .expect("read restored hotkey")
        .expect("hotkey exists");
    assert_eq!(hotkey.revision, Some(2));
    assert_eq!(
        hotkey.value,
        SettingValue::Hotkey("Ctrl+Shift+Space".to_owned())
    );
    let record = store
        .get_record("note-2")
        .expect("read restored record")
        .expect("record exists");
    assert_eq!(record.revision, 2);
    assert_eq!(record.payload_json, r#"{"text":"two updated"}"#);
    assert!(store.get_record("note-1").expect("note one").is_some());
    assert!(store.get_record("note-3").expect("note three").is_none());
    assert_eq!(
        store.cache_get("cache-key").expect("cache survives"),
        Some(b"cache-value".to_vec())
    );
    assert_eq!(
        store
            .get_secret_ref("github")
            .expect("secret ref read")
            .expect("secret ref exists")
            .reference(),
        "secret://vault/github"
    );

    drop(store);
    let store = open(&dir);
    assert_eq!(
        store
            .get_setting(SettingKey::Hotkey)
            .expect("reopened hotkey")
            .expect("hotkey exists")
            .revision,
        Some(2)
    );
    assert_eq!(store.list_records(None).expect("records").len(), 2);
}

#[test]
fn restore_rejects_corrupt_and_too_new_candidates_without_breaking_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut store = open(&dir);
    store
        .set_setting(SettingKey::Theme, &SettingValue::Theme(Theme::Dark), None)
        .expect("setting");

    let corrupt_path = dir.path().join("corrupt.sqlite");
    fs::write(&corrupt_path, b"this is not a sqlite database").expect("write corrupt");
    let corrupt = store
        .restore_from(&corrupt_path)
        .expect_err("must reject corrupt");
    assert!(
        matches!(corrupt, StorageError::Corrupt { .. }),
        "unexpected {corrupt:?}"
    );
    assert_eq!(
        store
            .get_setting(SettingKey::Theme)
            .expect("store still usable")
            .expect("setting exists")
            .value,
        SettingValue::Theme(Theme::Dark)
    );
    let rejected = store
        .recovery_info()
        .iter()
        .find(|entry| entry.path == corrupt_path)
        .unwrap_or_else(|| {
            panic!(
                "rejected restore must be reported: {:?}",
                store.recovery_info()
            )
        });
    assert_eq!(rejected.kind, RecoveryKind::RestoreAborted);
    assert_eq!(
        rejected.recovery_copy.as_deref(),
        Some(store.main_path()),
        "report must point at the untouched original database"
    );

    let too_new_path = dir.path().join("too-new.sqlite");
    let connection = Connection::open(&too_new_path).expect("raw create");
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at INTEGER NOT NULL
             ) STRICT;
             INSERT INTO schema_migrations VALUES (1, 'one', 0);
             INSERT INTO schema_migrations VALUES (2, 'two', 0);
             INSERT INTO schema_migrations VALUES (3, 'three', 0);
             INSERT INTO schema_migrations VALUES (4, 'four', 0);",
        )
        .expect("too-new schema");
    drop(connection);

    let too_new = store
        .restore_from(&too_new_path)
        .expect_err("must reject newer schema");
    assert!(
        matches!(
            too_new,
            StorageError::SchemaTooNew {
                found: 4,
                supported: 3
            }
        ),
        "unexpected {too_new:?}"
    );
    assert_eq!(
        store
            .get_setting(SettingKey::Theme)
            .expect("store still usable after reject")
            .expect("setting exists")
            .value,
        SettingValue::Theme(Theme::Dark)
    );
}

#[test]
fn failed_shutdown_checkpoint_aborts_restore_and_preserves_data() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main = dir.path().join("ansa.sqlite");
    let cache = dir.path().join("cache.sqlite");
    let mut faulted = StorageConfig::new(main.clone(), cache);
    faulted.shutdown_fault = Some(ShutdownFault::Checkpoint);
    let mut store = Store::open(faulted).expect("open with shutdown fault injection");
    let revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        )
        .expect("setting");
    store
        .create_record("ack", "note", r#"{"text":"acknowledged"}"#)
        .expect("record");
    let backup = dir.path().join("good.sqlite");
    store.backup_to(&backup).expect("backup before failure");
    let revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+K".to_owned()),
            Some(revision),
        )
        .expect("acknowledged update");
    assert_eq!(revision, 2);

    let error = store
        .restore_from(&backup)
        .expect_err("injected checkpoint failure must abort restore");
    assert!(
        matches!(error, StorageError::CheckpointFailed { .. }),
        "unexpected {error:?}"
    );

    let setting = store
        .get_setting(SettingKey::Hotkey)
        .expect("store reopens after aborted restore")
        .expect("acknowledged setting exists");
    assert_eq!(setting.revision, Some(2));
    assert_eq!(setting.value, SettingValue::Hotkey("Ctrl+K".to_owned()));
    assert!(store.get_record("ack").expect("record readable").is_some());
    assert!(backup.exists(), "backup must not be removed");

    let recovery = store.recovery_info();
    let entry = recovery
        .iter()
        .find(|entry| entry.kind == RecoveryKind::RestoreAborted)
        .unwrap_or_else(|| panic!("expected a restore recovery report: {recovery:?}"));
    assert_eq!(entry.path, backup, "report must name the failed source");
    assert_eq!(
        entry.recovery_copy.as_deref(),
        Some(main.as_path()),
        "report must point at the preserved original database"
    );
    assert!(
        entry.reason.contains("checkpoint"),
        "reason should describe the failure: {}",
        entry.reason
    );
}

#[test]
fn failed_backup_refresh_preserves_previous_backup_and_no_partial_destination() {
    let dir = tempfile::tempdir().expect("temp dir");
    let destination = dir.path().join("nightly.sqlite");
    {
        let store = open(&dir);
        store
            .set_setting(
                SettingKey::Hotkey,
                &SettingValue::Hotkey("Ctrl+Space".to_owned()),
                None,
            )
            .expect("setting");
        store.backup_to(&destination).expect("initial backup");
    }
    let previous = fs::read(&destination).expect("read previous backup");

    let mut config = config(&dir);
    config.backup_fault = Some(BackupFault::CorruptCopy);
    let store = Store::open(config).expect("open with backup fault");
    store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+K".to_owned()),
            Some(1),
        )
        .expect("post-backup update");

    let error = store
        .backup_to(&destination)
        .expect_err("injected corrupt copy must fail verification");
    assert!(
        matches!(error, StorageError::Corrupt { .. }),
        "unexpected {error:?}"
    );
    assert_eq!(
        fs::read(&destination).expect("previous backup survives"),
        previous,
        "a failed refresh must not destroy the previous backup"
    );
    for entry in fs::read_dir(dir.path()).expect("list dir") {
        let name = entry.expect("entry").file_name();
        let name = name.to_string_lossy();
        assert!(
            !(name.contains(".backup-") && name.ends_with(".tmp")),
            "temporary backup was not cleaned up: {name}"
        );
    }
    assert_eq!(
        store
            .get_setting(SettingKey::Hotkey)
            .expect("store usable")
            .expect("setting exists")
            .revision,
        Some(2)
    );

    drop(store);
    let store = open(&dir);
    let refreshed = store
        .backup_to(&destination)
        .expect("successful refresh replaces an existing backup");
    assert_eq!(refreshed.schema_version, 3);
    assert_eq!(refreshed.settings_count, 1);
}

#[test]
fn restore_rejects_structurally_incomplete_schema() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut store = open(&dir);
    store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        )
        .expect("setting");
    store
        .create_record("note", "note", r#"{"text":"ok"}"#)
        .expect("record");
    let good = dir.path().join("good.sqlite");
    store.backup_to(&good).expect("backup");

    let missing_table = dir.path().join("missing-table.sqlite");
    fs::copy(&good, &missing_table).expect("copy");
    let connection = Connection::open(&missing_table).expect("raw open");
    connection
        .execute_batch("DROP TABLE secret_refs;")
        .expect("drop table");
    drop(connection);
    let error = store
        .restore_from(&missing_table)
        .expect_err("missing table must be rejected");
    assert!(
        matches!(error, StorageError::BackupVerification { .. }),
        "unexpected {error:?}"
    );

    let missing_column = dir.path().join("missing-column.sqlite");
    fs::copy(&good, &missing_column).expect("copy");
    let connection = Connection::open(&missing_column).expect("raw open");
    connection
        .execute_batch("ALTER TABLE user_records DROP COLUMN tags_json;")
        .expect("drop column");
    drop(connection);
    let error = store
        .restore_from(&missing_column)
        .expect_err("missing column must be rejected");
    assert!(
        matches!(error, StorageError::BackupVerification { .. }),
        "unexpected {error:?}"
    );

    let missing_index = dir.path().join("missing-index.sqlite");
    fs::copy(&good, &missing_index).expect("copy");
    let connection = Connection::open(&missing_index).expect("raw open");
    connection
        .execute_batch("DROP INDEX user_records_updated_idx;")
        .expect("drop index");
    drop(connection);
    let error = store
        .restore_from(&missing_index)
        .expect_err("missing index must be rejected");
    assert!(
        matches!(error, StorageError::BackupVerification { .. }),
        "unexpected {error:?}"
    );

    assert_eq!(
        store
            .get_setting(SettingKey::Hotkey)
            .expect("original store usable")
            .expect("setting exists")
            .revision,
        Some(1)
    );
}

#[test]
fn corrupt_main_database_is_rejected_on_open() {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("ansa.sqlite"), b"definitely not sqlite").expect("write file");
    let error = Store::open(config(&dir)).expect_err("corrupt database must fail");
    assert!(
        matches!(error, StorageError::Corrupt { .. }),
        "unexpected {error:?}"
    );
}

#[test]
fn newer_schema_is_rejected_on_open() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("ansa.sqlite");
    let store = open(&dir);
    drop(store);
    let connection = Connection::open(&path).expect("raw open");
    connection
        .execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (4, 'future', 0)",
            [],
        )
        .expect("future migration");
    drop(connection);

    let error = Store::open(config(&dir)).expect_err("must reject future schema");
    assert!(
        matches!(
            error,
            StorageError::SchemaTooNew {
                found: 4,
                supported: 3
            }
        ),
        "unexpected {error:?}"
    );
}

#[test]
fn failed_migration_rolls_back_and_old_schema_stays_usable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main_path = dir.path().join("ansa.sqlite");
    let mut config = config(&dir);
    config.migration_fault = Some(MigrationFault::RollbackAfterApply(2));

    let mut recovery = Vec::new();
    let error = Store::open_with_recovery_log(config, &mut recovery)
        .expect_err("injected failure must abort open");
    assert!(matches!(error, StorageError::InjectedFault { .. }));
    let snapshot_report = recovery
        .iter()
        .find(|entry| entry.kind == RecoveryKind::PreMigrationSnapshot)
        .unwrap_or_else(|| panic!("failed open must report its snapshot: {recovery:?}"));
    assert!(
        snapshot_report
            .path
            .starts_with(dir.path().join("snapshots")),
        "report must name the snapshot path: {}",
        snapshot_report.path.display()
    );
    assert!(snapshot_report.path.exists());
    assert!(snapshot_report.reason.contains("v2"));
    assert!(snapshot_report.recovery_copy.is_none());

    assert_eq!(applied_versions(&main_path), vec![1]);
    let connection = Connection::open(&main_path).expect("raw old schema");
    assert!(column_exists(&connection, "settings", "key"));
    assert!(!column_exists(&connection, "user_records", "tags_json"));
    connection
        .execute(
            "INSERT INTO settings (key, value_json, revision, updated_at) VALUES ('probe', '{}', 1, 0)",
            [],
        )
        .expect("old schema remains writable");
    drop(connection);

    let snapshots = fs::read_dir(dir.path().join("snapshots"))
        .expect("snapshot directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("snapshot entries");
    assert!(
        snapshots.iter().any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("pre-migration-v2-")),
        "irreversible migration must take a snapshot"
    );
    let snapshot_path = snapshots
        .iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("pre-migration-v2-")
        })
        .expect("snapshot")
        .path();
    let snapshot = Connection::open(&snapshot_path).expect("open snapshot");
    let integrity: String = snapshot
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("snapshot integrity");
    assert_eq!(integrity, "ok");
    assert!(!column_exists(&snapshot, "user_records", "tags_json"));

    drop(snapshot);
    let store = open(&dir);
    assert_eq!(store.schema_version(), 3);
    let recovery = store.recovery_info();
    assert_eq!(
        recovery.len(),
        1,
        "expected one snapshot report: {recovery:?}"
    );
    let entry = &recovery[0];
    assert_eq!(entry.kind, RecoveryKind::PreMigrationSnapshot);
    assert!(
        entry.path.starts_with(dir.path().join("snapshots")),
        "snapshot report must name a file in the snapshot directory: {}",
        entry.path.display()
    );
    assert!(entry.path.exists(), "reported snapshot must exist");
    assert!(entry.reason.contains("v2"), "{}", entry.reason);
    assert!(entry.recovery_copy.is_none());
    assert!(column_exists(
        &Connection::open(&main_path).expect("reopened"),
        "user_records",
        "tags_json"
    ));
}

#[test]
fn halt_after_commit_leaves_a_consistent_new_prefix() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main_path = dir.path().join("ansa.sqlite");
    let mut config = config(&dir);
    config.migration_fault = Some(MigrationFault::HaltAfterCommit(2));

    let error = Store::open(config).expect_err("injected halt must abort open");
    assert!(matches!(error, StorageError::InjectedFault { .. }));
    assert_eq!(applied_versions(&main_path), vec![1, 2]);
    let connection = Connection::open(&main_path).expect("raw prefix schema");
    assert!(column_exists(&connection, "user_records", "tags_json"));
    drop(connection);

    let store = open(&dir);
    assert_eq!(store.schema_version(), 3);
    store
        .create_record("after-halt", "note", r#"{"text":"ok"}"#)
        .expect("new schema is usable");
}

#[test]
fn fail_before_first_migration_leaves_no_partial_tables() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main_path = dir.path().join("ansa.sqlite");
    let mut config = config(&dir);
    config.migration_fault = Some(MigrationFault::FailBefore(1));
    assert!(Store::open(config).is_err());

    let connection = Connection::open(&main_path).expect("raw open");
    let settings_table: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'settings'",
            [],
            |row| row.get(0),
        )
        .expect("table count");
    assert_eq!(settings_table, 0);
    assert!(applied_versions(&main_path).is_empty());

    let store = open(&dir);
    assert_eq!(store.schema_version(), 3);
}

#[test]
fn duplicate_and_out_of_order_migration_plans_are_rejected() {
    let duplicate_dir = tempfile::tempdir().expect("temp dir");
    let mut duplicate = config(&duplicate_dir);
    duplicate.migration_plan = MigrationPlan::new(vec![
        Migration::new(1, "one", false, noop_migration),
        Migration::new(1, "duplicate", false, noop_migration),
    ]);
    assert!(matches!(
        Store::open(duplicate),
        Err(StorageError::InvalidMigrationPlan { .. })
    ));

    let unordered_dir = tempfile::tempdir().expect("temp dir");
    let mut unordered = config(&unordered_dir);
    unordered.migration_plan = MigrationPlan::new(vec![
        Migration::new(1, "one", false, noop_migration),
        Migration::new(3, "three", false, noop_migration),
        Migration::new(2, "two", false, noop_migration),
    ]);
    assert!(matches!(
        Store::open(unordered),
        Err(StorageError::InvalidMigrationPlan { .. })
    ));

    let empty_dir = tempfile::tempdir().expect("temp dir");
    let mut empty = config(&empty_dir);
    empty.migration_plan = MigrationPlan::new(Vec::new());
    assert!(matches!(
        Store::open(empty),
        Err(StorageError::InvalidMigrationPlan { .. })
    ));
}

#[test]
fn concurrent_writers_serialize_without_losing_acknowledged_writes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(open(&dir));
    store
        .create_record("counter", "counter", r#"{"n":0}"#)
        .expect("counter");

    let mut handles = Vec::new();
    for worker in 0..6_u32 {
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            for index in 0..40_u32 {
                let id = format!("bulk-{worker}-{index}");
                let payload = format!(r#"{{"worker":{worker},"index":{index}}}"#);
                store
                    .create_record(&id, "bulk", &payload)
                    .unwrap_or_else(|error| panic!("acknowledged write failed: {error}"));
            }
        }));
    }
    for worker in 0..4_u32 {
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            for _ in 0..20_u32 {
                loop {
                    let record = store
                        .get_record("counter")
                        .expect("read counter")
                        .expect("counter exists");
                    let value: serde_json::Value =
                        serde_json::from_str(&record.payload_json).expect("counter json");
                    let next = value["n"].as_u64().expect("counter value") + 1;
                    let payload = format!(r#"{{"n":{next}}}"#);
                    match store.update_record("counter", &payload, record.revision) {
                        Ok(_) => break,
                        Err(StorageError::Conflict { .. }) => continue,
                        Err(error) => panic!("worker {worker} update failed: {error}"),
                    }
                }
            }
        }));
    }
    for handle in handles {
        handle.join().expect("worker thread");
    }

    assert_eq!(
        store.list_records(Some("bulk")).expect("bulk list").len(),
        240
    );
    let counter = store
        .get_record("counter")
        .expect("final counter read")
        .expect("counter exists");
    assert_eq!(counter.revision, 1 + 80);
    let value: serde_json::Value =
        serde_json::from_str(&counter.payload_json).expect("final counter json");
    assert_eq!(value["n"].as_u64(), Some(80));
    assert_eq!(store.list_records(None).expect("all records").len(), 241);
}

#[test]
fn low_disk_and_corruption_errors_are_mapped() {
    let full = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ffi::ErrorCode::DiskFull,
            extended_code: rusqlite::ffi::SQLITE_FULL,
        },
        None,
    );
    assert!(matches!(StorageError::from(full), StorageError::DiskFull));

    let corrupt = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ffi::ErrorCode::NotADatabase,
            extended_code: rusqlite::ffi::SQLITE_NOTADB,
        },
        None,
    );
    assert!(matches!(
        StorageError::from(corrupt),
        StorageError::Corrupt { .. }
    ));

    let busy = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ffi::ErrorCode::DatabaseBusy,
            extended_code: rusqlite::ffi::SQLITE_BUSY,
        },
        None,
    );
    assert!(matches!(StorageError::from(busy), StorageError::Busy));
}

#[test]
fn low_disk_page_limit_returns_disk_full_and_recovers() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);

    store
        .create_record("acked-a", "note", r#"{"text":"first"}"#)
        .expect("first acknowledged record");
    let revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        )
        .expect("acknowledged setting");
    store
        .create_record("acked-b", "note", r#"{"text":"second"}"#)
        .expect("second acknowledged record");

    // Freeze the database at its current page count. This is SQLite's real
    // capacity limit, so the next statement that needs a new page fails inside
    // SQLite with SQLITE_FULL; the actor never manufactures the error.
    store
        .set_page_limit(CapacityLimit::CurrentPages)
        .expect("apply a page limit to the writer");

    let payload = format!(r#"{{"blob":"{}"}}"#, "x".repeat(64 * 1024));
    let mut failure = None;
    // Keep writing until growth is refused. A fresh database should fail on the
    // first attempt; the loop tolerates any free pages left by migrations.
    for index in 0..32_u32 {
        let id = format!("overflow-{index}");
        match store.create_record(&id, "note", &payload) {
            Ok(record) => assert_eq!(record.revision, 1),
            Err(error) => {
                failure = Some((id, error));
                break;
            }
        }
    }
    let (failed_id, error) = failure.expect("a 64 KiB write must exceed the page limit");
    assert!(
        matches!(error, StorageError::DiskFull),
        "expected the real SQLite disk-full mapping, got {error:?}"
    );
    assert!(
        store
            .get_record(&failed_id)
            .expect("read after failure")
            .is_none(),
        "the failed transaction must be rolled back, not partially visible"
    );

    // Acknowledged rows written before the failure are still readable through
    // the same store.
    assert_eq!(
        store
            .get_setting(SettingKey::Hotkey)
            .expect("setting after disk full")
            .expect("setting exists")
            .revision,
        Some(revision)
    );
    assert!(store.get_record("acked-a").expect("a").is_some());
    assert!(store.get_record("acked-b").expect("b").is_some());

    // Raising the capacity lets the same store commit the retry.
    store
        .set_page_limit(CapacityLimit::Unlimited)
        .expect("raise the page limit");
    store
        .create_record(&failed_id, "note", &payload)
        .expect("write succeeds after the limit is raised");

    // Reopen without any limit: every acknowledged row, including the retry,
    // survives; the rejected statement left nothing behind.
    drop(store);
    let store = open(&dir);
    assert_eq!(
        store
            .get_setting(SettingKey::Hotkey)
            .expect("setting after reopen")
            .expect("setting exists")
            .revision,
        Some(revision)
    );
    assert_eq!(
        store
            .get_record("acked-a")
            .expect("a after reopen")
            .expect("record")
            .payload_json,
        r#"{"text":"first"}"#
    );
    assert_eq!(
        store
            .get_record(&failed_id)
            .expect("retry after reopen")
            .expect("retry record")
            .payload_json,
        payload
    );
    let backup = store
        .backup_to(dir.path().join("after-disk-full.sqlite"))
        .expect("integrity check after disk full");
    assert_eq!(backup.integrity, "ok");
}

#[test]
fn records_survive_reopen_with_revisions() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);
    store
        .create_record("layout", "layout", r#"{"width":800}"#)
        .expect("create");
    store
        .update_record("layout", r#"{"width":1024}"#, 1)
        .expect("update");
    drop(store);

    let store = open(&dir);
    let record: UserRecord = store
        .get_record("layout")
        .expect("read")
        .expect("record exists");
    assert_eq!(record.revision, 2);
    assert_eq!(record.kind, "layout");
    assert_eq!(record.payload_json, r#"{"width":1024}"#);
}

#[test]
fn record_listing_is_bounded_and_pageable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);
    for index in 0..5_u32 {
        store
            .create_record(&format!("p{index}"), "page", r#"{"n":0}"#)
            .expect("record");
    }
    let page = store.list_records_page(Some("page"), 2, 1).expect("page");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, "p1");
    assert_eq!(page[1].id, "p2");
    assert_eq!(store.list_records(Some("page")).expect("all").len(), 5);
    assert!(matches!(
        store.list_records_page(Some("page"), 0, 0),
        Err(StorageError::InvalidRecord { .. })
    ));
    assert!(matches!(
        store.list_records_page(Some("page"), MAX_RECORD_PAGE + 1, 0),
        Err(StorageError::InvalidRecord { .. })
    ));
}

#[test]
fn future_only_migration_row_is_reported_as_schema_too_new() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main = dir.path().join("ansa.sqlite");
    let connection = Connection::open(&main).expect("raw create");
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at INTEGER NOT NULL
             ) STRICT;
             INSERT INTO schema_migrations VALUES (4, 'future', 0);",
        )
        .expect("future schema");
    drop(connection);

    let error = Store::open(config(&dir)).expect_err("future-only schema must be rejected");
    assert!(
        matches!(
            error,
            StorageError::SchemaTooNew {
                found: 4,
                supported: 3
            }
        ),
        "unexpected {error:?}"
    );
}

#[test]
fn backup_destination_cannot_be_a_live_database() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);
    let main = store.main_path().to_path_buf();
    let error = store.backup_to(&main).expect_err("must reject live path");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));
    let cache = store.cache_path().to_path_buf();
    let error = store.backup_to(&cache).expect_err("must reject cache path");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));

    for suffix in ["-wal", "-shm", "-journal"] {
        for live in [&main, &cache] {
            let sidecar = PathBuf::from(format!("{}{suffix}", live.display()));
            let error = store
                .backup_to(&sidecar)
                .expect_err("must reject a live sidecar path");
            assert!(matches!(error, StorageError::InvalidConfig { .. }));
        }
    }
    let snapshot = store.snapshot_dir().join("manual.sqlite");
    let error = store
        .backup_to(&snapshot)
        .expect_err("must reject the snapshot directory");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));

    let lock_path = store.lock_path().to_path_buf();
    let error = store
        .backup_to(&lock_path)
        .expect_err("must reject the identity lock file");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));
    let owner_path = PathBuf::from(format!("{}.owner", lock_path.display()));
    let error = store
        .backup_to(&owner_path)
        .expect_err("must reject the owner note");
    assert!(matches!(error, StorageError::InvalidConfig { .. }));
}

#[test]
fn backup_destination_hard_link_to_live_database_is_rejected() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);
    let main = store.main_path().to_path_buf();
    let alias = dir.path().join("backup-alias.sqlite");
    if let Err(error) = fs::hard_link(&main, &alias) {
        #[cfg(windows)]
        panic!("Windows hard links are required for this backup-destination test: {error}");
        #[cfg(not(windows))]
        {
            eprintln!("skipping hard-link backup destination check: {error}");
            return;
        }
    }

    // Path names differ, so only the file-identity check can reject this.
    let error = store
        .backup_to(&alias)
        .expect_err("a hard-link alias of the live DB must be rejected");
    assert!(
        matches!(error, StorageError::InvalidConfig { .. }),
        "unexpected {error:?}"
    );

    // The alias is the live file; the rejection must not have written over it.
    store
        .create_record("after-rejected-alias-backup", "note", r#"{"text":"live"}"#)
        .expect("live database is still writable");
    assert!(
        store
            .get_record("after-rejected-alias-backup")
            .expect("read")
            .is_some()
    );
}

#[test]
fn second_open_is_rejected_and_lock_releases_on_drop() {
    let dir = tempfile::tempdir().expect("temp dir");
    let first = open(&dir);
    assert!(first.lock_path().exists(), "lock file must be created");

    let error = Store::open(config(&dir)).expect_err("a second handle must be refused");
    match error {
        StorageError::AlreadyLocked { path, holder_pid } => {
            assert_eq!(path, first.lock_path());
            assert_eq!(
                holder_pid,
                Some(std::process::id()),
                "the holder PID should be recorded"
            );
        }
        other => panic!("expected AlreadyLocked, got {other:?}"),
    }

    // The refused open must not have disturbed the live writer.
    first
        .create_record("while-locked", "note", r#"{"text":"ok"}"#)
        .expect("first store still writes");

    drop(first);
    let reopened = open(&dir);
    assert_eq!(reopened.schema_version(), 3);
    assert!(reopened.get_record("while-locked").expect("read").is_some());
}

#[test]
fn crashed_process_releases_the_instance_lock() {
    let dir = tempfile::tempdir().expect("temp dir");
    let marker = dir.path().join("lock-ready.txt");
    let mut child = spawn_worker(dir.path(), &marker, "lock-hold");
    if !wait_for_marker(&marker) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child never acquired the store lock");
    }

    let error = Store::open(config(&dir)).expect_err("the child owns the lock");
    assert!(
        matches!(error, StorageError::AlreadyLocked { .. }),
        "unexpected {error:?}"
    );

    child.kill().expect("kill the lock holder");
    let status = child.wait().expect("wait for the killed holder");
    assert!(!status.success(), "child should be terminated");

    // The OS closes the crashed process's handles, so the next open succeeds.
    // A short bounded retry tolerates handle teardown lag; it does not retry a
    // live holder forever.
    let store = open_after_crash(&dir);
    assert_eq!(store.schema_version(), 3);
    store
        .create_record("after-crash", "note", r#"{"text":"recovered"}"#)
        .expect("writer usable after the crashed holder released the lock");
}

fn open_after_crash(dir: &TempDir) -> Store {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match Store::open(config(dir)) {
            Ok(store) => return store,
            Err(StorageError::AlreadyLocked { .. }) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("reopen after a crashed holder failed: {error}"),
        }
    }
}

#[test]
fn hard_link_alias_is_rejected_by_identity_lock() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main = dir.path().join("ansa.sqlite");
    let store = open(&dir);
    store
        .create_record("original", "note", r#"{"text":"live"}"#)
        .expect("record before alias");

    let alias = dir.path().join("alias.sqlite");
    if let Err(error) = fs::hard_link(&main, &alias) {
        // The identity key is still exercised by the same-path and crash tests;
        // on Windows this alias case is required because the test host is
        // expected to be NTFS.
        #[cfg(windows)]
        panic!("Windows hard links are required for this identity test: {error}");
        #[cfg(not(windows))]
        {
            eprintln!("skipping hard-link alias check: {error}");
            return;
        }
    }

    // A different cache file and the default snapshot dir mean the only thing
    // that can reject the second store is the durable-file identity lock.
    let alias_cache = dir.path().join("alias-cache.sqlite");
    let error = Store::open(StorageConfig::new(alias.clone(), alias_cache))
        .expect_err("hard-link alias must be refused");
    match error {
        StorageError::AlreadyLocked { path, holder_pid } => {
            assert_eq!(path, store.lock_path());
            assert_eq!(holder_pid, Some(std::process::id()));
        }
        other => panic!("expected AlreadyLocked for a hard-link alias, got {other:?}"),
    }
    assert!(
        !PathBuf::from(format!("{}-wal", alias.display())).exists(),
        "the refused alias must not have created a second WAL"
    );

    // The live store is untouched and still authoritative.
    assert!(store.get_record("original").expect("live read").is_some());
    store
        .create_record("after-alias", "note", r#"{"text":"still live"}"#)
        .expect("live writer still works");
}

#[test]
fn distinct_databases_can_open_concurrently() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = open(&dir);
    let copy = dir.path().join("copy.sqlite");
    store.backup_to(&copy).expect("backup to a distinct file");
    let mut copy_config = StorageConfig::new(copy, dir.path().join("copy-cache.sqlite"));
    copy_config.snapshot_dir = dir.path().join("copy-snapshots");

    let copy_store = Store::open(copy_config).expect("a distinct file identity must not collide");
    copy_store
        .create_record("in-copy", "note", r#"{"text":"independent"}"#)
        .expect("copy writer works");
    assert!(
        store
            .get_record("in-copy")
            .expect("original read")
            .is_none(),
        "the copy must not be the original database"
    );
}

#[test]
fn restore_rekeys_the_identity_lock() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut store = open(&dir);
    store
        .create_record("before-restore", "note", r#"{"text":"one"}"#)
        .expect("record");
    let backup = dir.path().join("good.sqlite");
    store.backup_to(&backup).expect("backup");
    let old_lock = store.lock_path().to_path_buf();

    store.restore_from(&backup).expect("restore");

    // The rename installs a new file identity; the store must have re-keyed
    // its lock before the swap, not left the new file unlocked.
    assert_ne!(
        store.lock_path(),
        old_lock.as_path(),
        "restore should re-key the lock to the replacement file identity"
    );
    assert!(store.lock_path().exists());
    let error = Store::open(config(&dir)).expect_err("restored database must stay locked");
    match error {
        StorageError::AlreadyLocked { path, .. } => assert_eq!(path, store.lock_path()),
        other => panic!("expected AlreadyLocked after restore, got {other:?}"),
    }

    drop(store);
    let reopened = open(&dir);
    assert!(
        reopened
            .get_record("before-restore")
            .expect("read")
            .is_some()
    );
}

const KILL_ROOT: &str = "ANSA_STORAGE_KILL_ROOT";
const KILL_MARKER: &str = "ANSA_STORAGE_KILL_MARKER";
const KILL_MODE: &str = "ANSA_STORAGE_KILL_MODE";

fn slow_migration(transaction: &rusqlite::Transaction<'_>) -> Result<(), StorageError> {
    // Apply the DDL first, then signal the parent while the transaction is
    // still uncommitted. The parent kills the process in this window.
    transaction.execute("CREATE TABLE slow_data (id INTEGER PRIMARY KEY) STRICT", [])?;
    if let Ok(marker) = env::var(KILL_MARKER) {
        fs::write(marker, b"ddl applied, transaction uncommitted").expect("write migration marker");
    }
    thread::sleep(Duration::from_millis(1_500));
    Ok(())
}

fn slow_plan() -> MigrationPlan {
    MigrationPlan::new(vec![Migration::new(1, "slow", false, slow_migration)])
}

fn spawn_worker(root: &Path, marker: &Path, mode: &str) -> std::process::Child {
    let executable = env::current_exe().expect("test executable");
    Command::new(executable)
        .args(["--exact", "real_process_kill_worker", "--nocapture"])
        .env(KILL_ROOT, root)
        .env(KILL_MARKER, marker)
        .env(KILL_MODE, mode)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn storage child process")
}

fn wait_for_marker(path: &Path) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn real_process_kill_during_write_keeps_acknowledged_revisions() {
    let dir = tempfile::tempdir().expect("temp dir");
    let marker = dir.path().join("write-ready.txt");
    let ack_log = marker.with_extension("acks");
    let mut child = spawn_worker(dir.path(), &marker, "write");
    if !wait_for_marker(&marker) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child never acknowledged a durable write");
    }
    // Let the writer append several more acknowledged revisions before an
    // asynchronous kill. This proves recovery after abrupt process termination
    // during the write loop; it does not inject a failure at an exact
    // transaction boundary.
    thread::sleep(Duration::from_millis(25));
    child.kill().expect("kill child during writes");
    let status = child.wait().expect("wait for killed child");
    assert!(!status.success(), "child should be terminated");

    let acknowledged = fs::read_to_string(&ack_log).expect("read ack log");
    let revisions = acknowledged
        .lines()
        .filter_map(|line| line.trim().parse::<u64>().ok())
        .collect::<Vec<_>>();
    assert!(
        revisions.len() >= 10,
        "expected multiple acknowledged revisions, got {revisions:?}"
    );
    let maximum = revisions.iter().copied().max().expect("ack revision");

    let store = open(&dir);
    let snapshot = store
        .get_setting(SettingKey::Hotkey)
        .expect("reopen after kill")
        .expect("acknowledged setting survived");
    assert!(
        snapshot.revision.unwrap_or(0) >= maximum,
        "acknowledged revision {maximum} was lost (found {:?})",
        snapshot.revision
    );
    assert!(snapshot.diagnostic.is_none());
    let backup = store
        .backup_to(dir.path().join("after-kill.sqlite"))
        .expect("integrity check after kill");
    assert_eq!(backup.integrity, "ok");
    store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            Some(snapshot.revision.unwrap_or(maximum)),
        )
        .expect("writer is usable after kill");
}

#[test]
fn real_process_kill_during_migration_recovers_old_transaction() {
    let dir = tempfile::tempdir().expect("temp dir");
    let marker = dir.path().join("migration-ready.txt");
    let mut child = spawn_worker(dir.path(), &marker, "migration");
    if !wait_for_marker(&marker) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child never entered the slow migration");
    }
    child.kill().expect("kill child during migration");
    let status = child.wait().expect("wait for killed child");
    assert!(!status.success(), "child should be terminated");

    // The marker is written after CREATE TABLE inside the migration
    // transaction, so the process was killed after DDL but before COMMIT.
    let main_path = dir.path().join("ansa.sqlite");
    assert!(
        applied_versions(&main_path).is_empty(),
        "uncommitted migration row must not be visible"
    );
    let connection = Connection::open(&main_path).expect("raw open after kill");
    let table: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'slow_data'",
            [],
            |row| row.get(0),
        )
        .expect("slow table query");
    assert_eq!(
        table, 0,
        "the uncommitted DDL must not be visible after a real kill"
    );
    drop(connection);

    let mut config = StorageConfig::new(
        dir.path().join("ansa.sqlite"),
        dir.path().join("cache.sqlite"),
    );
    config.migration_plan = slow_plan();
    let store = Store::open(config).expect("reopen after mid-migration kill");
    assert_eq!(store.schema_version(), 1);
    assert_eq!(applied_versions(&main_path), vec![1]);
    let connection = Connection::open(dir.path().join("ansa.sqlite")).expect("raw reopen");
    let table: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'slow_data'",
            [],
            |row| row.get(0),
        )
        .expect("slow table query");
    assert_eq!(
        table, 1,
        "the migration must be retried, not left half-applied"
    );
}

#[test]
fn real_process_kill_worker() {
    let Ok(root) = env::var(KILL_ROOT) else {
        return;
    };
    let root = PathBuf::from(root);
    let marker = PathBuf::from(env::var(KILL_MARKER).expect("marker env"));
    let mode = env::var(KILL_MODE).expect("mode env");

    if mode == "lock-hold" {
        let store = open_root(&root);
        fs::write(&marker, store.lock_path().to_string_lossy().as_bytes()).expect("lock marker");
        loop {
            thread::sleep(Duration::from_millis(100));
        }
    }

    if mode == "migration" {
        let mut config = StorageConfig::new(root.join("ansa.sqlite"), root.join("cache.sqlite"));
        config.migration_plan = slow_plan();
        let _store = Store::open(config).expect("child migration open");
        fs::write(&marker, b"finished").expect("finished marker");
        loop {
            thread::sleep(Duration::from_millis(100));
        }
    }

    let store = open_root(&root);
    let mut revision = store
        .set_setting(
            SettingKey::Hotkey,
            &SettingValue::Hotkey("Ctrl+Space".to_owned()),
            None,
        )
        .expect("child setting");
    let ack_path = marker.with_extension("acks");
    let mut ack_log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ack_path)
        .expect("open ack log");
    writeln!(ack_log, "{revision}").expect("ack initial revision");
    ack_log.sync_all().expect("sync ack log");
    let mut acked = 1_u64;
    loop {
        revision = store
            .set_setting(
                SettingKey::Hotkey,
                &SettingValue::Hotkey(format!("Ctrl+Shift+F{}", revision % 24 + 1)),
                Some(revision),
            )
            .expect("child update");
        writeln!(ack_log, "{revision}").expect("ack revision");
        ack_log.sync_all().expect("sync ack revision");
        acked += 1;
        if acked == 10 {
            fs::write(&marker, revision.to_string()).expect("ready marker");
        }
    }
}

fn open_root(root: &Path) -> Store {
    let mut config = StorageConfig::new(root.join("ansa.sqlite"), root.join("cache.sqlite"));
    // Use the same standard plan as the parent.
    config.migration_plan = MigrationPlan::standard();
    Store::open(config).expect("child store")
}

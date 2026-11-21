# Review disposition — ANS-19 storage follow-up

Source review: `docs/reviews/2026-11-19-storage-quicklinks.md` (storage findings).
Scope of this follow-up: `crates/storage/**` only.

## S-1 (HIGH) — durable/cache path separation — Fixed

- `src/paths.rs` makes both paths absolute, resolves `.`/`..` lexically, uses the
  canonical path when it exists, and compares case-insensitively on Windows
  (verbatim `\\?\` prefixes are stripped for comparison).
- `StorageConfig::validate` runs before any database file is created.
- `Inner::open` rejects a durable DB containing `cache_entries` with a recovery
  message, and rejects a cache DB containing durable tables.
- Evidence: `noncanonical_cache_path_is_rejected_before_creating_files`
  (`nested\..` in both directions), `case_variant_cache_path_is_rejected_on_windows`,
  `existing_durable_database_with_cache_table_fails_with_recovery_message`,
  `durable_tables_in_the_cache_database_are_rejected`.

## S-2 (HIGH) — restore/WAL checkpoint ordering — Fixed

- Actor shutdown now returns a `Result`. It runs
  `PRAGMA wal_checkpoint(TRUNCATE)`, verifies the `-wal` file is absent or
  zero-length, and checks both `Connection::close` results.
- `Store::restore_from` aborts and reopens the original DB when checkpoint/close
  fails. Database replacement refuses to run while a non-empty `-wal` or
  `-journal` remains, and sidecar removal now returns errors.
- Evidence: `failed_shutdown_checkpoint_aborts_restore_and_preserves_data`
  (injected checkpoint failure, acknowledged revision 2 still readable),
  plus the existing restore round-trip and process-kill tests.

## S-3 (MEDIUM) — backup refresh destroys previous backup — Fixed

- `backup_to` writes to a unique temporary sibling, verifies it, syncs it, and
  only then renames it over the destination. A failed refresh removes the temp
  and leaves the previous file untouched.
- Evidence: `failed_backup_refresh_preserves_previous_backup_and_no_partial_destination`
  (injected corrupt copy; byte-for-byte old backup comparison; no temp left) and
  the same test's successful refresh over an existing destination.

## S-4 (MEDIUM) — verification only checked counts — Fixed

- `verify_database` now checks required tables (`schema_migrations`, `settings`,
  `user_records`, `secret_refs`), required columns (including `tags_json` for v2+),
  required indexes, and rejects `cache_entries` inside a durable DB. It also
  counts `secret_refs` and maps count/query failures to `BackupVerification`.
- Evidence: `restore_rejects_structurally_incomplete_schema` (missing table,
  missing column, missing index), `backup_restore_round_trip_preserves_revisions_and_counts`
  (secret-ref count), `secret_ref_sentinel_is_never_persisted`.

## S-5 (MEDIUM) — secret claim vs generic payloads — Fixed by narrowing the claim

- The crate-level docs no longer claim generic records/cache are secret-free.
  They state that `SecretRef` rejects plaintext, and that `UserRecord`/cache
  payloads are unencrypted caller data that must not contain secrets until an
  encrypted-payload layer exists.
- Evidence: the sentinel scan was renamed to
  `secret_ref_sentinel_is_never_persisted` and explicitly scoped; the honest
  `generic_record_and_cache_payloads_are_unencrypted_caller_data` test stores a
  sentinel through those APIs and asserts the documented persisted result. No
  fake secret detection was added.

## S-6 (MEDIUM) — real kill tests did not prove the claimed boundary — Fixed

- The slow migration now executes its DDL first, writes the marker from inside
  the open transaction, then sleeps; the parent kills after DDL and before
  COMMIT, then asserts the old schema on disk before reopening.
- The write-kill child appends every acknowledged revision (with `sync_all`) to
  an external ack log. The parent kills asynchronously, requires at least 10
  acknowledged revisions, and asserts the reopened revision is at least the
  maximum acked revision.
- Evidence: `real_process_kill_during_migration_recovers_old_transaction`,
  `real_process_kill_during_write_keeps_acknowledged_revisions`. The tests
  exercise OS process termination; they do not simulate power loss or a
  failing physical device.

## LOW findings

| Finding | Disposition | Evidence |
| --- | --- | --- |
| S-7 config validation after side effects | Fixed | `StorageConfig::validate` runs before opening files; `invalid_config_has_no_filesystem_side_effects`. |
| S-8 backup may target sidecars/snapshots; ignored sidecar removal | Fixed | `check_backup_destination`; `remove_sidecars` returns errors; `backup_destination_cannot_be_a_live_database`. |
| S-9 unbounded `list_records` | Fixed | `MAX_RECORD_PAGE`, `list_records_page(limit, offset)`; `record_listing_is_bounded_and_pageable`. |
| S-10 future-only schema misclassified | Fixed | max-version check before prefix check; `future_only_migration_row_is_reported_as_schema_too_new`. |
| S-11 stale/overbroad crate docs | Fixed | crate docs now match the enforced secret boundary and real kill tests. |
| S-12 single-writer is per process | Fixed | `src/lock.rs` takes an exclusive advisory lock keyed to the durable file's identity (volume serial + file index / dev + inode) in a fixed per-user lock directory; a second open (including a hard-link alias) fails with `AlreadyLocked`; the OS releases it on crash. Evidence: `second_open_is_rejected_and_lock_releases_on_drop`, `hard_link_alias_is_rejected_by_identity_lock`, `crashed_process_releases_the_instance_lock`, `restore_rekeys_the_identity_lock`. |
| Q-4/Q-5/Q-10/Q-11 (URL, Unicode URL, fingerprint, platform length) | Not applicable | outside `crates/storage`; owned by quicklinks/platform. |
| Q-9 alias grammar | Not applicable | storage alias grammar is already the strict `[A-Za-z0-9._-]` form; quicklink aliases are outside this crate. |
| Bidi/zero-width names | Partial | storage IDs/kinds/settings aliases are ASCII-only; arbitrary record payload rendering remains a caller/UI concern and is documented as unencrypted. |

## Residual risks not claimed fixed

- Real power-loss/physical fsync failure is not tested; low disk is now
  exercised through the real SQLite `max_page_count`/`SQLITE_FULL` path.
- Hard-linked main/cache files are not distinguished (canonical paths cannot
  resolve hard links without platform file-identity APIs).
- Restore accepts other SQLite schema objects (views/triggers) as long as the
  required schema is present; untrusted backups need an external trust decision.
- The single-instance lock is advisory: another SQLite client that does not use
  this crate can still open the files.
- Unencrypted `UserRecord`/cache payloads remain a caller contract.

## ANS-78 follow-up — low disk, single instance, recovery reports

Scope: `crates/storage/**` only. Status: **in review**, not closed.

### Low disk / `SQLITE_FULL`

- `db::CapacityLimit` + `Store::set_page_limit` (test-only, `#[doc(hidden)]`)
  apply the real `PRAGMA max_page_count` to the writer connection.
  `CurrentPages` reads the writer's committed `PRAGMA page_count` and freezes
  the database at that size; `Unlimited` restores SQLite's default cap. No
  error is manufactured in the actor.
- `low_disk_page_limit_returns_disk_full_and_recovers` writes acknowledged
  records, freezes the page count, writes 64 KiB payloads until SQLite refuses
  growth, and asserts `StorageError::DiskFull`. It also asserts the refused
  record is not visible (transaction rolled back), acknowledged rows remain
  readable, the same store commits the retry after the cap is raised, and the
  reopen (no cap) plus `VACUUM INTO` backup both succeed.

### Single instance / cross-process lock

- `src/lock.rs` acquires an exclusive, non-blocking OS lock (`File::try_lock`)
  keyed to the *identity* of the durable database file, not its path string.
  Identity is `GetFileInformationByHandle`'s volume serial + file index on
  Windows (via the safe `winapi-util` wrapper) and `dev` + `ino` on Unix. Lock
  files live in a fixed per-user directory (`%LOCALAPPDATA%\ansa-storage\locks`
  on Windows; `$XDG_RUNTIME_DIR` or a per-home temp directory on Unix), so hard
  links, symlinks and junctions resolving to the same file share one lock.
  Identity is read before SQLite is opened, so a refused alias never starts a
  second writer or `-wal`.
- The lock is held by `Store` for its lifetime, including across
  `restore_from`. `install_candidate` acquires the replacement file's identity
  lock **before** the rename, so there is no unlocked window; `Replacement`
  hands that lock to the store on commit/rollback. A refused open returns
  `StorageError::AlreadyLocked { path, holder_pid }`.
- The holder PID is written to a separate unlocked `<key>.lock.owner` note
  (Windows/Unix block reads of an exclusively locked file). The note is staged
  and published only after the lock is held (a losing attempt cannot clobber a
  live holder's note) and removed before unlocking (a successor cannot have
  written its own note yet). It is diagnostic and stale after a crash.
- `second_open_is_rejected_and_lock_releases_on_drop` proves same-process
  refusal, that the live writer is unaffected, and reopen after drop.
  `hard_link_alias_is_rejected_by_identity_lock` creates a real NTFS hard link
  to the live DB, asserts `AlreadyLocked` with the same lock path, no alias
  `-wal`, and a still-usable original store. On Windows the test fails if the
  hard link cannot be created; non-Windows skips with a printed limitation.
  `backup_destination_hard_link_to_live_database_is_rejected` creates a real
  hard link to the live DB and asserts `backup_to` rejects it with
  `InvalidConfig` through the identity check (the path names differ), then
  proves the live DB is still writable. Windows requires the link; non-Windows
  skips if unsupported.
  `crashed_process_releases_the_instance_lock` spawns the existing test
  executable, waits until the child holds the lock, asserts `AlreadyLocked`,
  kills the child, and reopens successfully (bounded 5 s retry for OS handle
  teardown). `restore_rekeys_the_identity_lock` proves the post-restore file
  identity is re-locked before the second open is attempted.
  `distinct_databases_can_open_concurrently` proves the identity key does not
  over-block distinct files. The existing real-kill tests still pass with the
  lock in place.
- Alias checks beyond the lock: `Store::restore_from` rejects a source whose
  identity equals the live DB, and `check_backup_destination` rejects a
  destination whose file identity equals a live database (or a live
  lock/owner file), so a hard-link alias cannot be overwritten by `VACUUM INTO`
  or the temp-rename swap.
- `try_lock` never blocks on a live holder; there is no unbounded wait.
- Dependency note: `winapi-util = "0.1.11"` is added as a Windows-only direct
  dependency. It is already in the workspace lockfile/graph (transitive through
  `same-file`), wraps exactly `GetFileInformationByHandle`, and keeps
  `#![forbid(unsafe_code)]`. Adding it updates only the `ansa-storage`
  dependency list in `Cargo.lock`; no new package or version was introduced.

### Recovery report

- `RecoveryInfo { kind, path, recovery_copy, reason }` and `RecoveryKind`
  (`PreMigrationSnapshot`, `RestoreAborted`) are public. `Store::recovery_info`
  returns the log for the open/restore lifetime. `Store::open_with_recovery_log`
  accepts an external log and appends reports even when opening fails;
  `Store::open` delegates with a discarded log.
- `apply_migrations` appends every snapshot to the report log as it is taken, so
  a migration that aborts after `VACUUM INTO` still names the snapshot.
- Every `restore_from` failure path records a `RestoreAborted` entry naming the
  source backup and pointing `recovery_copy` at the original database (or, when
  rollback fails, at the preserved `.pre-restore-*` copy, which the error text
  now also names). `install_candidate` now removes its staged/old files when the
  second sidecar check fails (previous residual S-2 item 3).
- Tests: `failed_migration_rolls_back_and_old_schema_stays_usable` asserts the
  snapshot report from a **failed** open (path exists, v2, no recovery copy) and
  from the subsequent successful open;
  `failed_shutdown_checkpoint_aborts_restore_and_preserves_data` asserts the
  failed-restore report names `good.sqlite`, points at `ansa.sqlite`, and keeps
  revision 2 readable; `restore_rejects_corrupt_and_too_new_candidates_without_breaking_store`
  asserts a pre-close rejection is reported and points at the untouched main DB.

### Commands and exact results

| Command | Result |
| --- | --- |
| `cargo test -p ansa-storage --offline` ×3 | **PASS**, 3/3 runs. Each run: 0 unit tests; `tests/storage.rs`: **42 passed, 0 failed, 0 ignored**; 0 doc-tests. (38 before L05a, 41 before the backup-destination closure, 42 now.) |
| `cargo clippy -p ansa-storage --all-targets --offline -- -D warnings` | **PASS**, exit 0. |
| `cargo fmt -p ansa-storage -- --check` | **PASS**, exit 0, no output. |

### Delta: identity-keyed lock (Max review MEDIUM)

The previous ANS-78 lock was a path-string lock beside the DB, so a hard link
opened a second store, a second WAL, and could diverge. The lock key is now the
file identity (volume serial + file index / dev + inode) and lives in a fixed
per-user directory, so all aliases agree on one lock. Restores re-key before
the rename. `Store::restore_from` rejects a source whose identity equals the
live DB, and `check_backup_destination` now rejects a destination whose file
identity equals a live database (or a lock/owner file), not just equal paths.
The hard-link, backup-destination hard-link and restore-rekey tests above cover
the reported gaps.

### Not safely provable here

- A real physical disk-full (ENOSPC on the device) is still not induced; the
  test exercises SQLite's own `SQLITE_FULL` capacity path via
  `max_page_count`, which is the same error code and rollback path.
- The lock is advisory. A raw SQLite client that ignores the lock file can
  still open the files. Platforms where `GetFileInformationByHandle`/`dev+ino`
  is unavailable fall back to a normalized-path key, where hard links are not
  detectable (documented in `src/lock.rs`).
- The lock directory is per-user, as requested. A data directory shared between
  two Windows accounts is not guarded across those accounts; the host should
  keep the durable directory user-private.
- The lock file name includes the volume serial and file index (or dev/ino);
  these are local identifiers, not secrets, but they are visible to the same
  user's lock directory.
- No deterministic in-tree fault forces `Replacement::rollback` to fail, so
  that branch is code-inspected only.
- Handle teardown immediately after `TerminateProcess`/kill is observed through
  a bounded retry; the exact instant the OS releases the handle is not asserted.

# Review: ANS-19 (L05 storage) and ANS-20 (L06 quicklinks)

**Date:** 2026-11-19
**Reviewer:** independent subagent (read-only review; no implementation changes)
**Repository:** `C:\Users\abhir\.local\share\opencode\worktree\1bbf61\playful-eagle`

**Tickets**

| Ticket | Task ID | Code under review |
| --- | --- | --- |
| ANS-19 / L05 — transactional settings and storage foundation | `omlit475x57m2elrpb9ycfbo` | `crates/storage/**` |
| ANS-20 / L06 — structured quicklinks with safe openers | `xxqkyaz58pk403en0c9f7xym` | `crates/quicklinks/**`; URL boundary compared with `crates/platform-api/src/lib.rs` and the adapter allowlist in `crates/platform-windows/src/url.rs` |

The review is against the Kaneo descriptions (goal, scope, acceptance criteria and test plan), not
against an implementation plan. Integration code (`crates/alpha`, `apps/cli`, `crates/ui-gpui`) was
inspected where it is the only consumer of these crates; those observations are marked
**integration** and are not counted in the crate severities.

---

## 1. Commands and exact results

All commands were run from the repository root with `--offline`.

| Command | Result |
| --- | --- |
| `cargo test -p ansa-storage -p ansa-quicklinks --offline` | **PASS.** `ansa-quicklinks`: 57 passed, 0 failed. `ansa-storage`: 0 unit tests; `tests/storage.rs`: 24 passed, 0 failed. Doc-tests: 0. |
| `cargo clippy -p ansa-storage -p ansa-quicklinks --all-targets --offline -- -D warnings` | **PASS.** Finished with no warnings (exit 0). |
| `cargo fmt -p ansa-storage -p ansa-quicklinks -- --check` | **PASS.** No output (exit 0). |
| `cargo test -p ansa-storage --offline --test storage` ×3 | **PASS** all three runs (24/24 each), so the timed process-kill tests were not flaky in this environment. |
| `cargo check -p ansa-alpha -p ansa-cli --offline` | **FAIL, unrelated in-tree WIP:** `crates/registry/src/registry.rs` has E0004 (non-exhaustive `OperationStatus::Failed`, `HostReconcile::Failed`), plus E0061/E0063. End-to-end CLI/alpha integration therefore could not be built or exercised. |

### Independent probes

A throwaway probe crate was created outside the repository at
`C:\Users\abhir\AppData\Local\Temp\opencode\review-probe` with path dependencies on both crates and
`rusqlite`/`tempfile`. It was used only to reproduce the findings below; it is not committed and no
crate file was modified. Evidence quoted in the findings is from that probe or from directly reading
the tests.

---

## 2. Findings

Severity scale: **BLOCKER / HIGH / MEDIUM / LOW**. No BLOCKER was found inside the two crates.

### 2.1 Storage (ANS-19)

#### S-1 — HIGH — The durable/cache separation guard is bypassable on a fresh install; the cache table is then created inside the durable database and the next open fails permanently

**Where:** `crates/storage/src/lib.rs:106-110` (guard), `crates/storage/src/lib.rs:575-580`
(`paths_equivalent`).

**Cause:** `paths_equivalent` canonicalises both paths. On a first run neither file exists, so both
`canonicalize` calls fail and the function falls back to a raw `Path` comparison. A cache path such
as `<data>/nested/../ansa.sqlite` (or a case variant on Windows) is therefore considered different
from `<data>/ansa.sqlite`. `open_writer` then creates the durable file, and `open_writer(cache)`
resolves the `..` (or case-insensitive filesystem) to the same file. `cache::initialize` creates
`cache_entries` inside the durable database. On the second run both paths canonicalise to the same
file, so `Store::open` now rejects the configuration and the application cannot start until the
configuration is corrected.

**Evidence (probe):**

```
main:  C:\...\ansa.sqlite
cache: C:\...\nested\..\ansa.sqlite
opened same-file config: main=...\ansa.sqlite cache=...\nested\..\ansa.sqlite
tables in main db: ["cache_entries", "schema_migrations", "secret_refs", "settings", "user_records"]
second open: Err(InvalidConfig { reason: "cache and durable databases must be separate files" })
```

`clear_cache` still deleted only `cache_entries`, so durable rows survive the normal clear path, but
the architectural invariant is broken: the cache is no longer rebuildable/separate, and a backup of
the main file now contains cache rows.

**Impact:** silent violation of "separate rebuildable caches from durable user records"; on a second
startup, a hard configuration failure. A first-run import/export/backup made while the two paths
resolve to one file is not a true cache/durable split.

**Minimal fix:** create/canonicalise the parent directory before the guard (or compare
`canonicalize(parent).join(file_name)`), use case-insensitive comparison on Windows, and after both
connections are open re-canonicalise and assert they are distinct files. Alternatively, open the
cache file first into a temporary name and compare file identity (volume serial + file index).

#### S-2 — HIGH (latent) — Restore deletes the live WAL/SHM before the rename, while the writer ignores close/checkpoint errors; a failed checkpoint can discard acknowledged writes on the restore-failure path

**Where:** `crates/storage/src/lib.rs:448-458` (`close_runtime` then `install_candidate`),
`crates/storage/src/lib.rs:135-141` (`Inner::shutdown`),
`crates/storage/src/actor.rs:337-339` (`let _ = main.close();`), and
`crates/storage/src/backup.rs:151-157` (`remove_sidecars(main)` before `rename`).

**Cause:** Restore assumes that after the actor has been shut down the main file is self-contained.
The actor discards both `close()` results, and `remove_sidecars` then unlinks `main-wal`/`main-shm`
without checking whether the final checkpoint actually completed. If another process/reader prevents
the checkpoint, or close fails, committed frames can still be in `main-wal`. They are deleted before
the new file is renamed in. The same path runs when the rename itself fails: the old `main` remains
but its WAL has already been deleted, and `reopen_after_failure` opens the old file without those
frames. Worse, the rollback copy is made by `fs::copy(main, &old)` before the sidecars are removed
(`backup.rs:143-149`), so `old` is a copy of `main` without the WAL as well; `Replacement::rollback`
(`backup.rs:102-109`) can therefore restore a database that is already missing the uncheckpointed
commits, even if the original `-wal` were still present.

**Impact:** a narrow but real path to losing acknowledged writes, directly against the L05
acceptance criterion "never silently lose acknowledged data". Preconditions are a failed final
checkpoint (concurrent reader, lock, I/O error) plus a restore attempt; the code currently has no
way to detect the failed checkpoint because the result is ignored.

**Minimal fix:** make the actor return the close/checkpoint result to `shutdown`; run
`PRAGMA wal_checkpoint(TRUNCATE)` and verify the `-wal` file is absent/empty before making the
`old` copy or removing sidecars; do not remove sidecars until the staged file has been verified and
is about to replace `main`; on any rename/verification failure, keep (or restore) the old sidecars.
Also surface a failed close as an error instead of silently proceeding.

#### S-3 — MEDIUM — Backup refresh destroys the previous backup before the new copy exists, and writes the final path non-atomically

**Where:** `crates/storage/src/actor.rs:601-618` (existing destination removed, then `VACUUM INTO`),
`crates/storage/src/lib.rs:379-419` (parent created, backup requested, verification after the fact).

**Cause:** `backup()` does:

```rust
if path.exists() { std::fs::remove_file(path)?; }
connection.execute("VACUUM INTO ?1", [path_text])?;
```

There is no temporary sibling file, no create-then-rename, and no preservation of the old backup
until the new one verifies. A disk-full, I/O error, or process kill during `VACUUM INTO` leaves the
previous backup gone and the destination either missing or partial. The probe confirmed that an
existing destination file is deleted and replaced with the new SQLite file; the failure half was
confirmed only by code inspection (forcing a `VACUUM INTO` failure safely would require a controlled
full disk).

**Impact:** the recovery path can be destroyed by a failed refresh. The main database is not touched,
so this is not primary data loss, but it can leave the user with no valid backup.

**Minimal fix:** `VACUUM INTO` a unique temporary file in the destination directory, `sync_all` it,
run `verify_database` on it, then `fs::rename` it over the destination (keeping the previous file
until the rename succeeds); clean up the temp file on every error.

#### S-4 — MEDIUM — Backup/restore verification checks only integrity, migration prefix, and two row counts; a database that claims v3 but lacks `secret_refs` or `tags_json` is accepted as a valid restore

**Where:** `crates/storage/src/backup.rs:55-79` (`verify_database`), `crates/storage/src/lib.rs:395-411`
(backup count comparison), `crates/storage/src/lib.rs:474-517` (post-restore comparison).

**Cause:** verification trusts `schema_migrations` plus `PRAGMA integrity_check` and counts
`settings`/`user_records`. It does not verify that the required tables/columns/indexes exist, does
not count `secret_refs`, and does not run `PRAGMA foreign_key_check` (there are no FKs today).
A database with migration rows 1,2,3 but no `secret_refs` table and no `tags_json` column passes
`verify_database` and is installed with a successful `RestoreSummary`.

**Evidence (probe):**

```
restore forged v3: Ok(RestoreSummary { schema_version: 3, settings_count: 1, user_records: 1, integrity: "ok", .. })
put secret into forged db: Err(Sqlite { code: 1, message: "no such table: secret_refs" })
user_records columns after restore: ["id","kind","revision","payload_json","created_at","updated_at"]
```

The same gap means `backup_to` can report success for a main database that was already structurally
incomplete (for example after a previous forged restore), because it only re-checks counts.

**Impact:** "verification report" is weaker than it appears: a structurally corrupt/incomplete file
is accepted, and credential operations fail only later with a generic SQLite error. If a restore
replaces a good database with such a file, the missing `secret_refs` rows are gone without any
verification failure.

**Minimal fix:** verify the expected schema objects (required tables, columns, and indexes), count
`secret_refs` in addition to settings/records, and map any count/query failure to
`StorageError::BackupVerification` rather than leaking `Sqlite { message: ... }`.

#### S-5 — MEDIUM — The "plaintext secrets are never persisted" claim is not enforced or tested for the public record/cache write paths

**Where:** `crates/storage/src/lib.rs:19-21` (top-level claim), `crates/storage/src/records.rs:1-9`
(payload is unencrypted; caller responsibility), `crates/storage/src/cache.rs:1-8` (cache is
unencrypted; caller responsibility), `crates/storage/tests/storage.rs:457-528` (sentinel test).

**Cause:** `SecretRef` itself is well designed (approved schemes only, no raw token, redacted
`Debug`, no echoing in errors). But `create_record` accepts an arbitrary JSON payload up to 1 MiB
and `cache_put` accepts arbitrary bytes up to 1 MiB; neither scans for secret-shaped content. The
test named `plaintext_sentinel_is_never_persisted` never attempts to persist the sentinel through
those APIs: it only calls `SecretRef::new` and stores ordinary values. The top-level doc says
plaintext secrets are "never written to preferences, caches or messages", which is stronger than the
enforced behavior.

**Evidence (probe):** storing `{"token":"SK-PLAINTEXT-SENTINEL-7d2a1f"}` via `create_record` and the
sentinel via `cache_put` produced:

```
sentinel in ...\ansa.sqlite: true
sentinel in ...\cache.sqlite: true
```

**Impact:** the L05 acceptance criterion "No secret or sensitive payload is written to logs, caches
or unencrypted preference tables" is a caller contract, not a property of this crate. The test gives
false confidence that the boundary is covered.

**Minimal fix:** either enforce it (an `EncryptedPayload` wrapper for sensitive records, a cache API
that refuses secret-shaped values, or a mandatory scan for the known secret formats) or narrow the
claim and rename the test to state exactly what it covers (the `SecretRef` acceptance path), with a
test that stores the sentinel through `create_record`/`cache_put` and documents the expected result.

#### S-6 — MEDIUM (test evidence) — The real process-kill tests do not prove the boundaries their names claim

**Where:** `crates/storage/tests/storage.rs:1133-1144` (`slow_migration`), `:1208-1241`
(`real_process_kill_during_migration_recovers_old_transaction`), `:1170-1206`
(`real_process_kill_during_write_keeps_acknowledged_revisions`), `:1262-1279` (worker).

**Migration kill:** the child writes the marker, then sleeps 1.5 s, then executes the only DDL
statement (`CREATE TABLE slow_data`). The parent kills during the sleep, i.e. **before any schema
change has been applied**. The test therefore proves rollback of an empty transaction and a retry,
not rollback of a partially applied migration. The in-process `RollbackAfterApply(2)` test at
`:862-920` does execute DDL and then abort, and the in-process `HaltAfterCommit(2)` test covers the
post-commit prefix, so the property is plausibly covered by the combination, but no real kill
exercises the post-DDL/pre-commit window. Also, the parent reopens with the custom `slow_plan`, not
the production standard plan; the test would still pass if the child had committed the migration.

**Write kill:** the child writes the marker once after the first acknowledged `set_setting`, then
loops writes without recording further acknowledgements. The parent only asserts
`revision >= acknowledged`. A lost acknowledged write after the marker would not be detected. This
does not prove the "no acknowledged write is lost" boundary, only that the first write survived.

**Minimal fix:** run the DDL before writing the migration marker (or use a second marker after the
DDL) and assert the pre-migration schema on disk immediately after the kill; have the writer append
each acknowledged revision (with `sync_all`) to an external ack log and assert the final revision is
at least the maximum acked after the kill.

#### S-7 — LOW — Configuration is validated after creating and migrating the databases

**Where:** `crates/storage/src/lib.rs:112-125`; `crates/storage/src/pool.rs:23-28`.

**Cause:** `read_connections` is checked inside `ReadPool::open`, after `open_writer(main)`,
`apply_migrations`, `open_writer(cache)`, `cache::initialize`, and `StorageActor::spawn`. A probe
with `read_connections = 0` returned `InvalidConfig` but left both `ansa.sqlite` and `cache.sqlite`
on disk, fully migrated.

**Impact:** a rejected configuration still creates/migrates durable files, which can surprise a
caller recovering from a bad config and makes initialisation non-atomic from the caller's view.
**Minimal fix:** validate all `StorageConfig` fields (paths, `read_connections`, migration plan)
before opening any file.

#### S-8 — LOW — Backup destinations do not reserve the SQLite sidecars or the snapshot directory

**Where:** `crates/storage/src/lib.rs:381-387` (only main/cache are rejected),
`crates/storage/src/actor.rs:605-609` (existing destination is removed), `crates/storage/src/backup.rs:171-176`
(`remove_sidecars` ignores errors).

**Impact:** `backup_to("ansa.sqlite-wal")` is not rejected. On platforms where unlinking an open file
is allowed, this can delete the live WAL; on Windows the open handle usually blocks the delete, but
the guard should not depend on that. A destination under `snapshots/` can overwrite a pre-migration
snapshot. A sidecar that cannot be removed is silently left behind, so a restarted DB can see a
stale WAL.
**Minimal fix:** reject `main`, `main-wal`, `main-shm`, `main-journal`, the cache equivalents, and
the snapshot directory as destinations; return an error from `remove_sidecars` instead of ignoring
it.

#### S-9 — LOW — Unbounded record listing and no pagination

**Where:** `crates/storage/src/actor.rs:661-689`, `crates/storage/src/lib.rs:293-299`.

`list_records` materialises every record (each up to 1 MiB) with no limit or cursor. A large durable
store can allocate heavily or stall the UI. **Minimal fix:** add a bounded limit/offset or a
streaming iterator and document the bound.

#### S-10 — LOW — A database with only future migration rows is reported as invalid state, not "schema too new"

**Where:** `crates/storage/src/migrations.rs:150-166`.

`applied.len() > plan.len()` is the only `SchemaTooNew` path before the prefix check. A foreign or
hand-edited database whose `schema_migrations` contains only `[4]` hits `InvalidMigrationState`
(index 0 expects 1) instead of `SchemaTooNew`. Not reachable from normal versions, but it makes
downgrade diagnostics wrong. **Minimal fix:** if any recorded version exceeds `plan.latest_version()`,
return `SchemaTooNew` before the positional prefix check.

#### S-11 — LOW / documentation — The crate-level crash-boundary note is stale and understates the test suite

`crates/storage/src/lib.rs:23-28` says real process-kill testing is "a separate release gate and is
not claimed here", while `tests/storage.rs` contains real subprocess-kill tests. Conversely, the
package docs at `lib.rs:19-21` overclaim secret non-persistence (see S-5). Reconcile both statements
with the enforced behavior.

#### S-12 — LOW / operational — The "single writer" guarantee is per process

`StorageActor` serialises writes inside one process, but there is no single-instance lock. Two
processes can open the same data directory; WAL allows one writer at a time, but a second process
with a newer migration plan can migrate while the first runs old SQL. Backups/restores also assume
exclusive access. **Minimal fix (host):** acquire a per-data-directory lock file or document that the
app must be single-instance.

---

### 2.2 Quicklinks (ANS-20)

#### Q-1 — MEDIUM — Import validates the model but not the target policy; unsafe or unusable quicklinks are imported with zero per-item errors and fail only at preview

**Where:** `crates/quicklinks/src/portable.rs:272-307` (only `link.validate()`), `:309-313`
(dispatch), `crates/quicklinks/src/model.rs:158-206` (`validate` checks only the literal prefix),
`crates/quicklinks/src/policy.rs:337-403` (`resolve_target` is never called by import).

**Cause:** `Link::validate` checks the static literal prefix for a non-web scheme and the model
invariants. It does not resolve the template with its defaults, check the opener/target-kind match,
or apply `OpenPolicy`. A template `{target:literal}` whose default is `javascript:alert(1)`, or a
`Browser` quicklink whose literal target is `C:\temp\x`, imports successfully. The preview gate
later rejects it with `BlockedScheme` / `OpenerMismatch`.

**Evidence (probe):**

```
unsafe-default import report: ImportReport { total: 1, applied: 1, skipped: 0, errors: [], replaced: true, .. }
unsafe-default preview: Err(Target(BlockedScheme { scheme: "javascript" }))
opener-mismatch import report: ImportReport { total: 1, applied: 1, errors: [], .. }
opener-mismatch preview: Err(Target(OpenerMismatch { opener: Browser, kind: LocalPath }))
```

**Impact:** a malicious or malformed export can be imported with a clean report and stored in the
library; the item is unusable (and, depending on the host, could be surfaced as a command that fails
at activation). The ticket explicitly asks for "malformed and malicious templates with per-item
errors rather than dropping the whole set"; this is a per-item error that is not reported.

**Minimal fix:** give `import_json`/`ImportOptions` an `OpenPolicy` (and optionally an
`OpenerProbe`), resolve each item with its defaults, and report `TargetError`/`ResolveError` as a
per-item `ImportItemError`. At minimum, reject defaults/aliases that cannot pass `resolve_target`
under the narrowest policy and document that import is model-only and must be followed by preview.

#### Q-2 — MEDIUM — Merge import bypasses the `MAX_QUICKLINKS` bound

**Where:** `crates/quicklinks/src/portable.rs:316-366` (`import_merge`), `crates/quicklinks/src/library.rs:16`
(`MAX_QUICKLINKS`), `:222-226` (the commit-path limit check).

**Cause:** `import_merge` inserts directly into `self.entries` and never compares
`self.entries.len() + accepted` with `MAX_QUICKLINKS`. The document-level check only limits the
*incoming* array. Starting from a full library, a one-item merge grows it past the cap. Normal
`preview`/`commit` then refuses any new quicklink because the library is already over the limit.

**Evidence (probe):**

```
fill import: total=10000 applied=10000 len=10000
merge past cap: ImportReport { total: 1, applied: 1, skipped: 0, errors: [], .. }
len after merge: 10001 (cap 10000)
preview at over-cap: Err(LimitReached { limit: 10000 })
```

**Impact:** the "bounded collection" invariant is not enforced by import; a large merge can double
the library to 20,000, increase export size and memory, and leave the UI unable to add anything.
**Minimal fix:** in `import_merge`, track the remaining capacity and report a per-item
`LimitReached` (or fail the import before mutating) once `MAX_QUICKLINKS` would be exceeded. Add a
regression test with a full library plus one merged item.

#### Q-3 — MEDIUM — Local-path policy is a single boolean; `DefaultApp` can open arbitrary executables/scripts, and UNC/device paths are accepted without a separate policy

**Where:** `crates/quicklinks/src/policy.rs:79-85` (`DefaultApp` supports both kinds), `:293-306`
(`is_absolute_local_path` accepts `\\`, `//`, `\\?\` and `\\.\`), `:337-403` (policy checks),
`:79-85` and `:100-122` (`allow_local_paths` is the only local gate).

**Evidence (probe):**

```
resolve_target("//evil.example/share", FileManager, with_local_paths) -> Ok(LocalPath)
resolve_target("\\\\?\\C:\\Windows", FileManager, with_local_paths) -> Ok(LocalPath)
resolve_target("\\\\?\\UNC\\srv\\share", FileManager, with_local_paths) -> Ok(LocalPath)
```

**Impact:** once local paths are enabled, a quicklink can target any absolute path, including
`\\.\PhysicalDrive0`, `\\attacker\share` (an SMB connection that may authenticate the user), and a
`.bat`/`.ps1`/`.exe` opened through `DefaultApp`. The preview shows the exact string, so the user
must confirm, but there is no "allowed roots", "network paths" or "executables" consent separate
from the generic local-path toggle. The L06 scope says local paths must be restricted by configured
policy and that a template must not invoke an arbitrary interpreter; a single boolean is weaker than
that wording.
**Minimal fix:** add `allowed_roots` (or an explicit directory chooser) and separate
`allow_network_paths` / `allow_executables` flags (both default false); reject device namespace
paths and script/executable extensions for `DefaultApp` unless explicitly enabled. Keep the exact
preview as the final confirmation.

#### Q-4 — LOW — URL validation accepts malformed authorities and userinfo; preview can promise an effect the browser will reject

**Where:** `crates/quicklinks/src/policy.rs:424-475` (`validate_web_url`).

The authority check only requires a non-empty host after `%`-escape validation. The probe accepted:

```
http://.
http://example.test:65536
http://example.test:-1
http://user@evil.test@good.test/
http://example.test/%00
http://example.test/%0d%0aSet-Cookie:x
```

**Impact:** the preview reports "Open in the system browser" for targets the browser will reject;
the platform allowlist (`crates/platform-windows/src/url.rs:41-48`) has the same authority rule, so
the two layers agree but both are lenient. Encoded CRLF/NUL is data in a URL path, not a launcher
bug, but it should not be presented as a valid target without validation. Userinfo in a template is
also stored/exported in plaintext.
**Minimal fix:** use a real URL parser (or validate host/port syntax, reject dot-only hosts and
ports outside 1..=65535), reject userinfo for quicklink templates or warn in the preview, and add
corresponding tests.

#### Q-5 — LOW — Raw Unicode in a literal web URL is rejected; only arguments are encoded

**Where:** `crates/quicklinks/src/policy.rs:447-473` (`is_url_byte` is ASCII-only), `:424-430`.

**Evidence (probe):** `resolve_target("https://de.wikipedia.org/wiki/München", Browser, web_only)`
returns `InvalidUrlCharacter { character: 'ü' }`. Arguments with Unicode work because
`encode_component` percent-encodes UTF-8, but a literal URL with a non-ASCII path cannot be saved.
**Impact:** a legitimate common URL is rejected; the user must manually percent-encode it.
**Minimal fix:** percent-encode non-ASCII bytes of literal URL text during target validation (or
accept IRIs and encode at handoff), leaving the argument encoder unchanged.

#### Q-6 — LOW — Unicode bidi/format controls are accepted in titles, aliases, tags and literal path segments, so the previewed effect and list entries can be visually spoofed

**Where:** `crates/quicklinks/src/model.rs:167`, `:180`, `:197` (`char::is_control()` only; U+202E is
Cf, not Cc), `crates/quicklinks/src/encoding.rs:81-86` (same for literal segments).

**Evidence (probe):** a local target `C:\temp\safe\u{202E}txt.exe` resolves to `Ok(LocalPath)` and a
title containing U+202E passes `Quicklink::validate()`. The display can reorder text around the
override so the "exact effect" or the title does not read as the actual path.
**Impact:** low-severity display spoofing for consequential local targets; the bytes stored/exported
are exact, but the user-facing rendering is ambiguous. **Minimal fix:** reject Unicode bidi controls
(`U+202A..U+202E`, `U+2066..U+2069`) and zero-width format characters in titles, aliases, tags, and
targets, or render them escaped in the preview.

#### Q-7 — LOW — Argument values are encoded without a pre-check against `MAX_RESOLVED_TARGET_CHARS`

**Where:** `crates/quicklinks/src/template.rs:872-905` (build then check),
`crates/quicklinks/src/encoding.rs:23-35` (allocate proportional to input).

**Evidence (probe):** a 5,000,000-byte text argument took ~95 ms and allocated roughly 15 MB before
returning `TargetTooLong`. **Impact:** a pasted/imported giant argument can create a momentary
memory spike even though the final error is correct. **Minimal fix:** bound each rendered argument
(e.g. reject values whose encoded length already exceeds the remaining target budget) or compute the
encoded length incrementally and fail before allocating the whole string.

#### Q-8 — LOW — `..` traversal is checked only as an exact segment; Windows normalization variants are accepted

**Where:** `crates/quicklinks/src/policy.rs:413-422`, `crates/quicklinks/src/encoding.rs:95-97`.

`validate_local_path` rejects only `segment == ".."`. My independent
`std::fs::canonicalize` probe did **not** resolve `.. ` or `...` as a parent on this Windows build
(it returned "path not found"), so this is not demonstrably exploitable today; however, the check is
narrower than the "no parent traversal" claim and depends on host path normalization. Since any
absolute path is already allowed by the current boolean policy, this is not a confinement boundary.
**Minimal fix:** reject segments consisting only of dots/spaces, and/or canonicalize the final path
at the host boundary before opening.

#### Q-9 — LOW — Alias grammar differs between quicklinks and settings

`Quicklink::validate` (`crates/quicklinks/src/model.rs:170-183`) allows spaces and any 64-character
non-control alias; `SettingKey::AliasPrefix` (`crates/storage/src/settings.rs:161-189`) allows only
1..=32 bytes of `[A-Za-z0-9._-]` starting with an alphanumeric. A quicklink alias `"my link"` is
accepted and committed (probe), but it does not match the launcher's alias-prefix grammar.
**Impact:** if aliases are parsed as single launcher tokens, space-containing aliases are unusable
or ambiguous. **Minimal fix:** align the grammar or document that quicklink aliases are labels, not
prefix tokens.

#### Q-10 — LOW — The preview fingerprint is computed but never checked at commit

`crates/quicklinks/src/model.rs:431-448` fingerprints the candidate + target + opener + revision, but
`Library::store` (`crates/quicklinks/src/library.rs:191-231`) only compares the library revision and
revalidates the model. Today `Preview` has private fields, no public constructor and no
`Deserialize`, so the candidate cannot be altered; the fingerprint is unused. **Impact:** future
API growth could make the stale check insufficient. **Minimal fix:** recompute and compare the
fingerprint in `store` (cheap) or remove it from the public API until it is used.

#### Q-11 — LOW — The platform boundary is stricter in some places and looser in others

**Where:** `crates/platform-api/src/lib.rs:280-285` delegates to
`crates/platform-windows/src/url.rs:8-51`.

Both layers allow only `http`/`https`, require `://` and a non-empty authority, and reject
whitespace/control characters/backslashes. Differences:

* quicklinks allows up to `MAX_RESOLVED_TARGET_CHARS` (16,384 chars); the platform adapter rejects
  URLs over `MAX_URL_LENGTH` (8,192 bytes). A preview can pass and `open_url` then fail.
* quicklinks rejects invalid `%` escapes and raw non-ASCII URL bytes; the platform adapter does not
  check `%` escapes and accepts raw Unicode.
* both accept the malformed authorities listed in Q-4.

**Impact:** the two "allowlists" are not equivalent, so a previewed target can fail at the host, and
a target the host would accept can be rejected by the quicklink crate. **Minimal fix:** share one
validator or add conformance tests that run both against the same corpus and document the maximum
length as the lower of the two.

#### Q-12 — LOW — `ReplaceConfirmed` is a naming convention, not an enforced confirmation

`crates/quicklinks/src/portable.rs:28-34` exposes `ImportMode::ReplaceConfirmed` as an enum variant;
`crates/quicklinks/src/portable.rs:368-387` replaces the whole library for any caller that selects
it. There is no confirmation token tied to a prior dry-run report. A trusted host can call it
directly. **Impact:** the "without confirmation" part of the acceptance criterion is a caller
contract, not a type-level guarantee. **Minimal fix:** require a `ReplaceConfirmation` value produced
from a `would_replace` dry-run report (or an explicit `confirm_replace: bool` documented as the
caller's responsibility).

---

## 3. Acceptance-criteria disposition

### ANS-19 / L05

| Criterion / scope item | Disposition | Evidence |
| --- | --- | --- |
| SQLite with foreign keys, single writer actor, bounded read pool, explicit transactions | **Met.** `PRAGMA foreign_keys=ON` is applied (no FK constraints exist yet, so it is observationally inert); all writes go through one actor; pool is 1..=16 per DB; every acknowledged write uses an `IMMEDIATE` transaction. | `db.rs:38-44`, `pool.rs:23-52`, `actor.rs:347-355`; pragma test. |
| Versioned, transactional migrations; duplicate/out-of-order rejection | **Met.** Plan validation, per-migration transaction with `schema_migrations` row, prefix validation, too-new test. | `migrations.rs:86-202`; tests `:966-998`, `:834-860`. |
| Restorable snapshot before every irreversible migration | **Met.** `VACUUM INTO` + `integrity_check` before the migration; test verifies a `pre-migration-v2-*` snapshot. | `migrations.rs:250-277`; test `:862-920`. |
| Kill during write/migration leaves old-or-new consistent schema | **Met in code**, but **test evidence is partial**: the real migration kill occurs before the DDL (S-6); the real write kill checks only the first ack (S-6). In-process fault injection covers DDL rollback and post-commit prefix. | S-6; `tests/storage.rs` cited there. |
| Revisioned settings with validation for hotkeys, aliases and enum values; reject invalid stored values safely | **Met.** CAS create/update/delete; per-key typed validation; `get_or_default` returns the default with a diagnostic and never rewrites the row; unrelated rows survive. | `settings.rs:88-159, 191-299`; `lib.rs:200-250`; tests `:85-417`; independent wrong-type probe. |
| Separate rebuildable cache; `clear_cache` cannot touch durable records; backup excludes/does not corrupt cache | **Met in the normal configuration** (`clear_cache` deletes only `cache_entries`; backup is a `VACUUM` of main only), but **bypassable** by a non-canonical cache path (S-1). | S-1; test `:536-580`; probe. |
| Credential references only; Debug redaction; no plaintext to prefs/caches/logs | **Partially met.** `SecretRef` enforces approved schemes/refs and redacts `Debug`; no logging exists; but records/cache accept arbitrary plaintext and the sentinel test does not exercise them (S-5). The doc overclaims. | S-5; `secrets.rs`; test `:419-528`. |
| Backup restores settings and records with a verification report | **Met for app-produced backups**, but **verification is incomplete** for structurally malformed files (S-4) and a failed refresh can destroy the previous backup (S-3). | S-3/S-4; test `:658-821`; probes. |
| Corrupt-database and low-disk behavior | **Corrupt rejection met** (`Corrupt` on open; corrupt/too-new restore rejected). **Low-disk is only error mapping** (`DiskFull`/`Busy`/`Corrupt` unit test); no real disk-full simulation and no test that a failed commit leaves the DB usable. | `error.rs:128-146`; test `:1062-1093`. |
| Backup/restore atomic replacement and Windows file locking | **Mostly met.** Candidate is staged, synced, verified, then renamed; old copy is kept until final verification; reader/writer connections are closed first. **Caveat:** sidecar removal happens before the rename and ignores failures; close results are ignored (S-2/S-8). Not tested under a real Windows file lock/antivirus. | `backup.rs:112-176`; S-2/S-8. |
| No lost acknowledged writes | **Met on the normal path** (`commit` then reply, WAL + `synchronous=FULL`; concurrent test and first-ack kill test pass). **Caveat:** restore's sidecar deletion in a failed-checkpoint path can lose acknowledged writes (S-2), and the kill test does not prove all later acks (S-6). | `actor.rs:347-355`; S-2/S-6. |

### ANS-20 / L06

| Criterion / scope item | Disposition | Evidence |
| --- | --- | --- |
| Structured template with named arguments, defaults, validation and URL/path encoding | **Met.** Grammar, typed args (`text/number/boolean/choice`), defaults, per-placeholder encoding, limits. | `template.rs:684-797`; tests `:289-538`. |
| RFC 3986 encoding (unreserved set, UTF-8 percent encoding, spaces, percent, plus, Unicode) | **Met.** Only `A-Za-z0-9-._~` pass through; UTF-8 bytes become uppercase `%XX`; space is `%20`; `+` is `%2B`; `%` is `%25`; round-trip property test and independent probes pass. | `encoding.rs:13-35`; tests `:192-257`; probe. |
| Single-pass substitution, no re-expansion; literal braces | **Met.** Values are appended once and never rescanned; `{{`/`}}` are literal; a lone brace is a parse error. Note: a literal `{` in a *web URL* still fails URL validation unless written as `%7B`, so the brace escape is only useful for local/text targets. | `template.rs:713-797, 861-906`; tests `:289-377, 602-682`. |
| Scheme/path policy: reject javascript/data/vbscript/file/shell; case/whitespace/control/encoded-scheme tricks; opener mismatch | **Met for the enumerated schemes and tricks.** Case-insensitive blocking, `%`-encoded/whitespace-obscured schemes are not recognized as schemes and are rejected, control/leading/trailing whitespace rejected, drive-relative rejected, `..` segments rejected, opener mismatch rejected at preview. **Partial for UNC/device paths and `DefaultApp` executables** (Q-3) and import does not apply the policy (Q-1). | `policy.rs:19, 337-475`; tests `:716-940`; probes. |
| Arguments with spaces, quotes, percent signs and Unicode resolve to literal values | **Met.** Verified with `a+b%20 c/é`, quotes, braces, `$()` and Unicode; percent-encoding round-trips. | `tests.rs:192-257, 602-682`; probe. |
| Preview the resolved target and opener before save; exact effect | **Met in the crate.** `Preview::build` resolves, classifies, probes, describes, fingerprints; `Library::commit` only stores the previewed candidate and rejects a stale library revision. **Caveat:** fingerprint is not checked (Q-10), and `DefaultApp`/local effects are broad (Q-3). | `model.rs:339-448`, `library.rs:142-231`; tests `:946-1056`. |
| A template cannot invoke a shell, arbitrary interpreter or unapproved scheme | **Met at the crate boundary** (no process launch, no shell interpolation, only `http(s)` + explicit local paths). **Partial** for `DefaultApp` opening local executables/scripts and UNC/device paths (Q-3). The host adapter's `open_url` uses `ShellExecuteW` with a single argument, not `cmd.exe`. | `policy.rs`, `platform-windows/src/win32.rs:47-111`; Q-3. |
| Aliases, tags, preferred opener | **Met.** Case-insensitive alias uniqueness (rechecked at commit and import), normalized/sorted tags, opener selection. Alias grammar differs from storage (Q-9). | `model.rs:60-231`, `library.rs:233-243`. |
| Import/export with versioned JSON; per-item errors; no partial overwrite without confirmation; deterministic export | **Mostly met.** `schema_version` 1 is required and unknown versions are rejected; items are parsed/validated individually; merge never overwrites; replace is all-or-nothing and only via `ReplaceConfirmed`; export is sorted/canonical and round-trips. **Gaps:** policy is not applied at import (Q-1), merge can exceed the cap (Q-2), `ReplaceConfirmed` is a convention (Q-12), and `ImportReport::is_complete` is true for a dry run although nothing was applied. | `portable.rs:25, 210-387`; tests `:1267-1521`; probes. |
| Opener availability tests; real-browser and file-manager checks | **Not fully met.** The crate's mechanism is tested with a synthetic probe only; the CLI uses `StaticOpenerProbe::all_available()` and explicitly does not open. The desktop does not implement quicklink authoring. There is no real browser/file-manager probe in this slice. | `policy.rs:247-285`; `apps/cli/src/main.rs:324-339`; integration section. |

---

## 4. Verified vs. unverified

### Verified

* All scoped tests, clippy and fmt pass (Section 1); the storage integration suite passed three
  consecutive runs with the timed kill tests.
* Storage: pragma configuration, single writer actor under 10 concurrent threads with CAS counters,
  revision persistence across reopen, migration plan rejection, rollback of a migration body,
  post-commit prefix, snapshot creation/verification, backup/restore round trip, corrupt and too-new
  restore rejection, cache clear isolation in the normal configuration, `SecretRef` validation and
  Debug redaction, wrong-type stored-value fallback without touching unrelated rows.
* Independent storage probes: same-file cache configuration (S-1), forged v3 restore (S-4),
  sentinel persistence via record/cache (S-5), `read_connections=0` side effects (S-7), restore of
  an older-schema database, backup replacing an existing destination file (S-3).
* Quicklinks: full test suite plus independent probes for RFC 3986 output, single-pass braces,
  blocked/encoded schemes, drive-relative and `..` targets, opener mismatch, import/export
  round-trip with arguments/defaults/tags/Unicode, stale preview rejection, case-insensitive alias
  uniqueness, unsafe-default import (Q-1), merge cap bypass (Q-2), UNC/device path acceptance (Q-3),
  malformed URLs (Q-4), literal Unicode rejection (Q-5), bidi acceptance (Q-6), unbounded argument
  allocation (Q-7).
* The platform adapter's URL validator and `ShellExecuteW` call were inspected: no shell or `cmd.exe`
  interpolation; only `http(s)` is accepted; a single URL/path argument is passed.

### Unverified

* **Real low-disk behavior.** No actual disk-full was simulated. `DiskFull` is mapped correctly, but
  there is no evidence that a failed `COMMIT`/migration/backup leaves the old state usable.
* **A real process kill after the migration DDL but before commit.** The real kill test kills before
  the DDL; the DDL rollback is proven only by in-process fault injection.
* **Restore rollback when `Inner::open` fails after the swap.** The code path exists but no test
  exercises it; `install_candidate`/`Replacement` are private.
* **A failed checkpoint / concurrent reader during restore (S-2).** Not tested under a real Windows
  file lock, antivirus scan, or second process. The sidecar-removal failure path is ignored.
* **Real opener availability and real browser/file-manager launches.** The crate is inert by design;
  the CLI uses a static all-available probe; the desktop does not author/open quicklinks.
* **Cross-process concurrency.** The single-writer guarantee is per process; no single-instance lock
  was found.
* **Untrusted SQLite restore.** A restored file may contain triggers/views; verification does not
  reject them, and `trusted_schema`/defensive mode is not set.
* **End-to-end CLI/alpha integration.** `cargo check -p ansa-alpha -p ansa-cli --offline` fails in
  `crates/registry` (unrelated WIP), so the integrated surface could not be built or exercised.

---

## 5. Integration observations (outside the two crate directories, affects ticket acceptance)

These were found by static inspection; the alpha/CLI targets do not currently compile because of the
unrelated `crates/registry` errors.

1. **The catalog flattens structured quicklinks back to a raw template string, and the desktop
   opens it without preview.** `crates/alpha/src/catalog.rs:110-139` builds
   `CommandKind::Quicklink { url: link.template().source() }`, and `crates/ui-gpui/src/lib.rs:367`
   calls `ansa_platform_windows::open_url(&url)` directly. Any consumer of the catalog therefore
   bypasses `Library::preview`, argument resolution and the opener policy. A parameterized quicklink
   would be handed to the browser as a literal `{name}` URL; a local-path quicklink would be rejected
   by `open_url`. The desktop currently does not compose quicklinks ("quicklink authoring ... not
   implemented in the desktop"), so this is latent, but it means L06 is not end-to-end wired.
2. **Valid quicklink libraries can exceed the storage record cap.** Quicklinks allows 10,000 entries
   and a 4 MiB import document (`quicklinks/src/portable.rs:25`, `library.rs:16`), while storage
   rejects record payloads over 1 MiB (`storage/src/records.rs:15`). `DailyDriver::save_quicklinks`
   (`crates/alpha/src/daily_driver.rs:297-311`) commits to the in-memory library first and then
   fails the durable save, leaving memory and disk divergent. Cap the export/library size, compress
   the record, or raise the storage cap with an explicit bound.
3. **A corrupt quicklink record is replaced on the next save.** `DailyDriver::reload_quicklinks`
   (`daily_driver.rs:268-295`) reports the warning, uses an empty library, and keeps
   `quicklink_record_revision = Some(record.revision)`. A later `save_quicklinks` successfully
   `update_record`s over the corrupt (possibly recoverable) record with only the new content. Keep
   the original record, or require an explicit repair confirmation before overwriting it.
4. **The CLI library file is written non-atomically.** `apps/cli/src/main.rs:654-657` uses
   `fs::write`; a crash during import/save can truncate the JSON library. The storage-backed path is
   safer; write to a temp file + rename for the CLI too.
5. **Opener probes are synthetic in the CLI.** `apps/cli/src/main.rs:324-339` uses
   `StaticOpenerProbe::all_available()`; the preview can therefore claim an opener is available when
   no browser/file manager is registered. This is acceptable for a headless "preview only" command
   only if the output labels availability as unverified.

---

## 6. Residual risks

* **Backup/snapshot disk growth.** Pre-migration snapshots are never pruned
  (`migrations.rs:250-277`); each irreversible migration can double the data directory. `backup_to`
  can also overwrite a snapshot (S-8).
* **No backup durability barrier.** `VACUUM INTO` writes the destination, but there is no explicit
  `sync_all`/parent-directory fsync before the verification report; a power loss immediately after
  `backup_to` could still lose the newest backup (S-3's temp+rename+sync fix covers this).
* **Restore accepts arbitrary SQLite schema objects.** A crafted backup with the expected migration
  prefix and counts can add triggers/views; `trusted_schema` is not disabled. Do not restore
  untrusted files without an explicit warning, or run verification under `PRAGMA trusted_schema=OFF`
  / a defensive connection.
* **Local-path policy** is a single boolean; no allowed roots, network-path or executable policy
  (Q-3). UNC paths can initiate SMB authentication when opened.
* **Quicklink templates can contain credentials in URLs.** `https://user:pass@host` is accepted;
  `Quicklink`'s derived `Debug` and `export_json` include the template, so a host that logs the
  library or exports it leaks the userinfo. There is no redaction/warning for quicklink templates.
* **Bidi/zero-width formatting** in display text is not rejected (Q-6).
* **No cross-process single-instance lock** for the data directory (S-12).
* **`paths_equivalent` does not resolve hard links**, and on Windows it compares canonical paths
  case-sensitively after the first check; hard-linked main/cache files can still merge the databases.
* **Windows file-lock/antivirus behavior** is not exercised; the restore path depends on the OS
  allowing the rename after all in-process handles are closed.
* **`ImportReport` semantics:** `is_complete()` is true for a dry run even though nothing was
  applied (`portable.rs:142-150`); callers must check `dry_run` first.

---

## 7. Recommendation

The two crates are substantially implemented and their scoped test suites, clippy and fmt are green.
Within the crate boundaries I found **no BLOCKER**, but I would not close either ticket as-is:

* **Storage (ANS-19):** fix S-1 (path-equivalence guard) and S-2 (WAL/sidecar ordering and ignored
  close results) before sign-off; S-3 and S-4 are medium-severity recovery/verification gaps; S-5 is
  a claim/test mismatch that should be resolved by enforcement or an honest narrowing of the
  acceptance statement. Strengthen S-6 so the "real kill" claim is actually proven.
* **Quicklinks (ANS-20):** fix Q-1 (import must apply the target policy or be explicitly documented
  as model-only) and Q-2 (merge cap), and decide the Q-3 policy question (allowed roots / network
  paths / executables) before claiming "a template cannot invoke an arbitrary interpreter". The
  encoding, single-pass substitution, scheme blocking, preview/commit revision gate and
  import/export versioning are in good shape.
* **Integration:** the end-to-end desktop/alpha path is not wired (Section 5). If the ticket is
  judged on the desktop experience, item 1 is a functional blocker; if it is judged on the crates
  plus CLI, it is a follow-up. The workspace `ansa-registry` build failure prevented an integrated
  check.

---

*End of review.*

---

# Storage re-review after fixes (ANS-19)

**Re-review date:** 2026-11-19
**Owner disposition:** `crates/storage/REVIEW-DISPOSITION.md`
**Scope of this round:** `crates/storage/**` only. The original findings above are preserved; this
section records only the follow-up verification.

## Commands and exact results

| Command | Result |
| --- | --- |
| `cargo test -p ansa-storage --offline` ×3 | **PASS.** 35 passed, 0 failed, 0 ignored on each run (0 unit tests, 35 integration tests). The timed process-kill tests were stable in all three runs. |
| `cargo clippy -p ansa-storage --all-targets --offline -- -D warnings` | **PASS**, exit 0. |
| `cargo fmt -p ansa-storage -- --check` | **PASS**, no output, exit 0. |
| `cargo check -p ansa-cli -p ansa-alpha --offline` | **PASS**, exit 0. The earlier `ansa-registry` E0004/E0061/E0063 blockers observed in the first review are no longer present. |

An independent probe crate (outside the repository) reused the crate by path and exercised the
path-alias, busy-reader, and restore-failure paths below.

## Finding-by-finding verification

### S-1 — fixed for the reported cases; one LOW NTFS alias residual

* `StorageConfig::validate` now runs before any database is opened (`src/lib.rs:124-148`) and uses
  `paths::normalized`/`paths::equivalent` (`src/paths.rs:12-57`), which makes paths absolute,
  resolves `.`/`..` lexically, canonicalises existing paths, strips verbatim `\\?\` prefixes for
  comparison, and compares case-insensitively on Windows.
* `noncanonical_cache_path_is_rejected_before_creating_files`
  (`tests/storage.rs:87-109`) covers both `nested\..` directions and asserts the main file was not
  created. `case_variant_cache_path_is_rejected_on_windows` (`:111-120`) covers the Windows case
  variant. The probe additionally confirmed trailing-space and trailing-dot cache spellings are
  rejected before either file exists.
* `existing_durable_database_with_cache_table_fails_with_recovery_message` (`:122-147`) proves an
  existing durable DB containing `cache_entries` returns an `InvalidConfig` recovery message rather
  than a half-open store. `Inner::open` checks this before migrations (`src/lib.rs:162-172`) and
  also rejects durable tables inside the cache DB (`:180-197`).
* **Residual (LOW):** an NTFS alternate-data-stream spelling such as `ansa.sqlite::$DATA` is not
  equated by `paths::normalized`. The probe returned `Sqlite { code: 3338, "disk I/O error" }` after
  creating the main (and the cache spelling); no half-open `Store` was produced, because opening the
  cache fails before the actor is spawned. The old permanent-failure mode is therefore closed, but
  an invalid config can still create the main file. Hard links remain indistinguishable (already
  documented as a residual).

### S-2 — fixed for the practical failure path; no data-loss sequence found

* `StorageActor::shutdown` now sends a `Shutdown` request and returns the actor's `Result`
  (`src/actor.rs:242-261`). `shutdown_connections` runs `PRAGMA wal_checkpoint(TRUNCATE)` on both
  databases, checks `busy`, verifies the `-wal` is absent/zero-length (`src/actor.rs:382-453`), and
  checks both `Connection::close` results. `Store::restore_from` aborts and reopens the original DB
  when `close_runtime` returns an error (`src/lib.rs:578-582`), and `install_candidate` refuses to
  proceed while a non-empty `-wal`/`-journal` remains (`src/backup.rs:237-297, 305-325`).
* `failed_shutdown_checkpoint_aborts_restore_and_preserves_data` (`tests/storage.rs:985-1030`)
  exercises the **injected `ShutdownFault::Checkpoint`** path: the restore aborts, revision 2 is
  still readable, and the backup survives. This is the `close_runtime` error branch, not the
  non-empty-WAL branch inside `install_candidate`.
* Independent probe with a **real** held read transaction on the main DB (so the writer's
  `wal_checkpoint(TRUNCATE)` is busy): `restore_from` returned
  `CheckpointFailed { reason: "durable WAL checkpoint remained busy" }`; the acknowledged revision 2
  remained readable, the backup file survived, and a reopen after releasing the reader also showed
  revision 2. I could not construct a sequence that loses acknowledged data or leaves the DB
  unusable in the single-process model.
* The migration `ensure_sidecars_clear` branch and a non-empty WAL appearing after a successful
  checkpoint are **not exercised by the test suite**; they require a second process/unsupported
  concurrency. The source correctly returns an error if the sidecar is non-empty.
* **Residual (LOW):** if `Replacement::rollback` itself fails (for example a locked `-shm`), the
  error path at `src/lib.rs:654-661` leaves the `Store` closed (`inner = None`) and reports only
  that the database "may be at <main>"; the preserved `.pre-restore-*` copy is not named. A restart
  can recover, and the old copy remains, but the message is not a complete recovery instruction.
  Also, the second `ensure_sidecars_clear(main)?` at `src/backup.rs:277` returns without removing the
  staged/old files on failure, leaving temporary litter.

### S-3 — fixed

`backup_to` writes to a unique temporary sibling (`src/lib.rs:483-540`, `backup_temp_path` at
`:752-775`), verifies the temporary, syncs it, and only then `rename`s it over the destination; a
failed `VACUUM INTO`, corrupt copy, sync, or rename removes the temporary and leaves the old
destination untouched. `failed_backup_refresh_preserves_previous_backup_and_no_partial_destination`
(`tests/storage.rs:1032-1096`) uses `BackupFault::CorruptCopy` and compares the destination
byte-for-byte while asserting no `.tmp` remains; the successful refresh over an existing destination
is also covered. The independent probe confirmed an existing destination is replaced only by a
complete SQLite file. **Residual (LOW):** the directory is not fsynced after the rename, so the
rename's metadata durability is not proven against power loss; that is outside the stated guarantee
and not tested.

### S-4 — fixed

`verify_database` now checks the required tables (`schema_migrations`, `settings`, `user_records`,
`secret_refs`), required columns (including `tags_json` for v2+), the required indexes, and rejects
`cache_entries` in a durable DB (`src/backup.rs:66-198`). It counts `secret_refs` as well as
settings/records, and maps count failures to `BackupVerification` (`:98-102`).
`restore_rejects_structurally_incomplete_schema` (`tests/storage.rs:1098-1168`) covers a missing
table, missing column, and missing index; the round-trip test now covers the secret-ref count
(`:805-917`). **Residual (LOW):** columns and indexes are checked by name, not type/definition; a
fabricated DB could reuse an index name on the wrong columns. Extra views/triggers are still
accepted; the trust decision is documented in `REVIEW-DISPOSITION.md`, not in the public
`verify_database` doc comment.

### S-5 — fixed by narrowing the claim

The crate-level claim now says `SecretRef` rejects raw tokens and that `UserRecord`/cache payloads
are unencrypted caller data that must not contain secrets (`src/lib.rs:19-24`).
`secret_ref_sentinel_is_never_persisted` (`tests/storage.rs:562-638`) is explicitly scoped to the
`SecretRef` path and scans the main/cache/WAL files, so it can fail if that path regresses.
`generic_record_and_cache_payloads_are_unencrypted_caller_data` (`:646-681`) deliberately stores a
sentinel through `create_record`/`cache_put` and asserts the documented persisted result.
**Residual (LOW):** generic record/cache payloads remain an unenforced caller contract; this is now
documented honestly rather than claimed away.

### S-6 — fixed

`slow_migration` now executes the DDL, writes the marker from inside the still-open transaction, and
then sleeps (`tests/storage.rs:1549-1558`). The parent kills after the marker and, **before
reopening**, asserts `applied_versions` is empty and `slow_data` does not exist (`:1643-1698`); it
then reopens with the same custom plan and verifies the migration is retried. The write worker
appends every acknowledged revision to an external ack log with `sync_all` (`:1719-1750`); the
parent requires at least ten acked revisions and asserts the reopened revision is at least the
maximum acked (`:1588-1641`). The tests passed on all three runs. **Residual (LOW):** the migration
kill uses a custom plan rather than the standard one, and the tests exercise process termination,
not power loss or a failing device (which the crate docs state).

### S-7 through S-12

| Finding | Re-review result |
| --- | --- |
| S-7 config side effects | **Fixed.** `StorageConfig::validate` runs before `open_writer`; `invalid_config_has_no_filesystem_side_effects` (`tests/storage.rs:167-188`) asserts no main/cache files for invalid `read_connections` and an invalid plan. |
| S-8 sidecars/snapshots | **Fixed.** `check_backup_destination` rejects main/cache, their `-wal`/`-shm`/`-journal`, and the snapshot directory (`src/lib.rs:723-750`); `remove_sidecars` now returns errors (`src/backup.rs:327-337`); sidecar/snapshot rejection is tested (`tests/storage.rs:1519-1543`). |
| S-9 bounded listing | **Fixed.** `MAX_RECORD_PAGE = 1000`; `list_records` is bounded and `list_records_page(limit, offset)` exists (`src/lib.rs:68-70, 373-397`); tested (`tests/storage.rs:1465-1486`). **Residual (LOW):** callers that used `list_records` expecting all rows now silently receive at most 1000; the doc states this, and no workspace consumer except tests relies on the old semantic. |
| S-10 future-only schema | **Fixed.** The max-version check precedes the prefix check in both `apply_migrations` and `validate_applied_prefix` (`src/migrations.rs:150-164, 239-251`); `future_only_migration_row_is_reported_as_schema_too_new` (`tests/storage.rs:1489-1516`) covers `[4]` against latest 3. |
| S-11 stale docs | **Fixed.** `src/lib.rs:19-39` now states the narrowed secret boundary, the real process-kill coverage, and the per-process single-writer model. |
| S-12 cross-process lock | **Partial, as declared.** No lock file was added; the per-process limitation is documented at `src/lib.rs:35-39`. This is an acceptable host responsibility (LOW residual). |

## Remaining storage findings

No **BLOCKER**, **HIGH**, or **MEDIUM** finding remains in the changed storage code. Residual LOW
items:

1. **S-1 NTFS ADS alias** (`src/paths.rs:12-57`): `ansa.sqlite::$DATA` is not equated before open;
   the main file can be created before the invalid configuration is rejected. No half-open store.
2. **S-2 rollback-failure recovery message** (`src/lib.rs:654-661`): a failed rollback leaves the
   `Store` closed and does not name the `.pre-restore-*` copy; a restart/reopen is required.
3. **S-2 staged/old cleanup** (`src/backup.rs:277-284`): the second `ensure_sidecars_clear(main)?`
   returns without removing the temporary/staged files.
4. **S-3 directory fsync** (`src/lib.rs:537-540`): the rename is atomic, but the parent directory is
   not fsynced; power-loss metadata durability is unverified.
5. **S-4 structural verification is name-based** (`src/backup.rs:153-198`), and extra SQLite objects
   (triggers/views) are accepted by trust decision documented only in `REVIEW-DISPOSITION.md`.
6. **S-9 truncation semantics** (`src/lib.rs:373-379`): `list_records` is now a 1000-row page; this
   is a behavioral change for callers that expected a complete list.
7. **S-12** no cross-process lock file; documented host responsibility.

## Storage verdict

**ANS-19 is acceptable with residual risks.** The two original HIGH findings (S-1 and S-2) are
closed for the reported configurations and practical failure paths; S-3 through S-10 are fixed and
tested; S-11 is reconciled; S-12 is explicitly partial. The independent real busy-reader probe
supports the S-2 claim beyond the injected fault test. I would add a real
`wal_checkpoint`-busy/non-empty-WAL test and either document or close the ADS/rollback-recovery
residuals before calling the storage foundation final.

---

# Quicklinks re-review after fixes (ANS-20)

**Re-review date:** 2026-11-19
**Owner disposition:** `crates/quicklinks/REVIEW-DISPOSITION.md`
**Scope of this round:** `crates/quicklinks/**` only. `SCHEMA_VERSION` remains 1. The original
findings above are preserved; this section records only the follow-up verification.

## Commands and exact results

| Command | Result |
| --- | --- |
| `cargo test -p ansa-quicklinks --offline` | **PASS.** 71 passed, 0 failed, 0 ignored; 0 doc-tests. |
| `cargo clippy -p ansa-quicklinks --all-targets --offline -- -D warnings` | **PASS**, exit 0. |
| `cargo fmt -p ansa-quicklinks -- --check` | **PASS**, no output, exit 0. |
| `cargo check -p ansa-cli -p ansa-alpha --offline` | **PASS**, exit 0; the owner's claim that the consumer crates still compile against the extended `OpenPolicy` is correct. |

Independent probes (path-reduced to the relevant boundaries) were run in the same temporary crate
described in the first review.

## Finding-by-finding verification

### Q-1 — fixed for defaults, required-argument samples, merge and replace

`Library::import_json_with_policy` resolves every parsed item with `import_sample_values` (declared
defaults, or `"sample"`/`1`/`true`/first choice for required arguments) and applies
`resolve_target` before accepting it (`src/portable.rs:302-344, 444-480`). `import_json` supplies a
`with_local_paths` baseline and documents that hosts can pass a narrower policy
(`src/portable.rs:214-243`).

Probe results:

* `javascript:alert(1)`, `file:C:` and `ssh:host` defaults → one per-item error each, `applied: 0`,
  library empty, `replaced: false`.
* Browser + `C:\temp\report.txt` → one per-item `OpenerMismatch`, library empty.
* Merge of an unsafe item and a safe item → unsafe skipped with per-item error, safe applied.
* Replace with any unsafe item → all-or-nothing, no replacement.
* A safe required-argument template (`query_link`) imports normally.

**Residual (LOW):** the gate is sample-based, not exhaustive. A required `Choice` whose *first*
choice is safe but a *later* choice resolves to a blocked target still imports; the unsafe choice
fails only at preview. Likewise arbitrary text values cannot be enumerated. This is documented in
`import_sample_values` and is a reasonable coarse gate, but should be stated in the import docs.
`import_json`'s default `with_local_paths` also lets an untrusted export introduce ordinary local
paths without an explicit host opt-in; the host should pass its actual policy (the doc says so).
This is a policy default, not the old no-policy bypass.

### Q-2 — fixed

`import_merge` computes the projected size from new identities and returns a document-level error
before any mutation if it would exceed `MAX_QUICKLINKS` (`src/portable.rs:353-374`).
`merge_import_refuses_to_exceed_the_library_cap` (`src/tests.rs:1700-1727`) fills exactly 10,000,
merges one more, asserts `applied == 0`, one document-level error, and `len == 10,000`. The code
returns before the alias/insert loop, so no partial mutation occurs.

### Q-3 — network/device/opt-ins fixed; executable denylist still incomplete (MEDIUM)

`OpenPolicy` now has independent `allow_network_paths`, `allow_device_paths`, `allow_executables`,
each defaulting to false and serde-compatible with older policies (`src/policy.rs:103-202`).
`is_network_path` distinguishes `\\host\share`, `//host/share` and `\\?\UNC\...`; `is_device_path`
covers `\\?\`, `\\.\` and forward-slash spellings (`:409-427`). `resolve_target` returns
`NetworkPathsDisallowed`, `DevicePathsDisallowed`, and `ExecutableDisallowed` with typed errors
(`:501-524`); `\\?\UNC\...` requires both device and network opt-ins (verified by tests and probe).

What works (tests `src/tests.rs:1733-1841` and probe):

* `\\server\share`, `//host/share` → `NetworkPathsDisallowed` without `with_network_paths`; allowed
  with it.
* `\\?\C:\...`, `\\.\PhysicalDrive0`, `//?/C:/...` → `DevicePathsDisallowed` without
  `with_device_paths`; allowed with it.
* `\\?\UNC\server\share` requires device + network; case variants work.
* `C:\temp\run.exe` is `ExecutableDisallowed` for `DefaultApp`; `FileManager` can still reveal it;
  `with_executables` allows it. Case, trailing dot, and ADS (`run.exe:stream`, `run.exe::$DATA`) are
  normalized to `exe`; trailing-space spelling is rejected as surrounding whitespace. Network and
  device executable paths require the corresponding opt-in plus `with_executables`.

**Remaining MEDIUM (Q-3):** `EXECUTABLE_EXTENSIONS` (`src/policy.rs:25-28`) is a **denylist** and
misses common interpreter/installer/HTA/ClickOnce/management-console extensions. The probe accepted
all of these under `DefaultApp` with `allow_executables = false`:

```
C:\temp\run.py, run.pyw, run.jar, run.hta, run.pl, run.rb, run.sh,
run.application, run.pif, run.scf, run.msc, run.settingcontent-ms,
run.search-ms, run.diagcab
```

A `DefaultApp` quicklink for `C:\temp\run.py` imported with `applied: 1` and previewed successfully
(`LocalPath`, "Open with the default application"); `.exe` was rejected. With a Python/Java/mshta
association, `ShellExecute` will launch an interpreter without the explicit `allow_executables`
opt-in, which still violates the L06 acceptance wording "a template cannot invoke ... an arbitrary
interpreter" for common cases. **Minimal fix:** invert the policy to a safe allowlist of
document/image/media/archive extensions for `DefaultApp` (or require `with_executables` for any
extension not explicitly classified safe); at minimum add the listed common interpreter, installer,
HTA, ClickOnce, shell-command and management-console extensions. `FileManager` reveal can remain
unrestricted. Also add tests for these extensions and for the import path.

### Q-4/Q-5 — fixed for the requested cases; LOW residuals

**Q-4:** `validate_authority` now checks host labels, dot-only hosts, userinfo, port range
1..=65,535 and bracketed IPv6 shape (`src/policy.rs:629-786`); `%00`, `%0d`, `%0a` and `%7f` are
rejected as encoded control bytes (`:788-841`). Tests `src/tests.rs:1847-1917` cover dot hosts,
`65536`, `-1`, userinfo, `:80:90`, `%00`, CRLF and a valid `[::1]:8080`. The probe confirmed valid
`https://example.com`, trailing-dot hosts, `:80`, `:65535`, `[::1]`, `München` paths and ordinary
query URLs are accepted.

**LOW remaining in Q-4:** bracketed IPv6 validation is shape-only. The probe accepted `[:::]`,
`[1:2:3:4:5:6:7:8:9]` and `[::ffff:999.1.1.1]` (the platform adapter also accepts them); a real
IPv6 parser would reject them. IPvFuture (`[v1.fe]`) and zone IDs (`[::1%25eth0]`) are rejected even
though syntactically valid; these are rare over-rejections. Normal URLs are not over-rejected.

**Q-5:** `literal_unicode_urls_are_accepted_verbatim` (`src/tests.rs:1919-1931`) verifies
`https://de.wikipedia.org/wiki/München` previews with exact bytes; the test passes.

**LOW remaining in Q-5:** `validate_url_characters` accepts non-ASCII **whitespace** inside a URL
(`U+00A0`, `U+2000`, `U+2028` were accepted by quicklinks), while the platform `validate_url`
rejects it. A preview can therefore pass a target the host `open_url` rejects. Add
`character.is_whitespace()` rejection for non-ASCII URL characters (or align the host). This is a
preview/host mismatch, not a scheme bypass.

### Q-6 — fixed for bidi/zero-width controls; LOW residual

`text::is_unsafe_format_character` rejects Arabic letter mark, U+200B, U+200E/F, U+202A–E,
U+2060, U+2066–69 and U+FEFF (`src/text.rs:10-22`); it is applied to titles, aliases, tags,
template literals, literal path segments and resolved targets (`src/model.rs:168-235`,
`src/template.rs:818-826`, `src/encoding.rs:104-109`). The test covers U+202E in all those places
(`src/tests.rs:1933-1984`). U+200C/U+200D remain allowed for legitimate language/emoji text.

**LOW residual:** U+2028/U+2029 line separators and U+00AD soft hyphen are not in the set and were
accepted in titles; they are invisible/line-breaking display risks but not bidi reordering or
zero-width controls. Consider adding them (and a regression test for allowed ZWJ sequences).

### Q-7 — fixed

`resolve` checks the raw value's byte length against the remaining 8,192-byte target budget before
`render()`/encoding (`src/template.rs:935-948`); `resolve_text` checks raw input length before
cloning (`:980-984`); defaults over 8,192 bytes are rejected at parse (`:336-341`); the final target
is bounded in bytes and characters (`:952-958`). Probe:

* 5,000,000-byte text argument → `TargetTooLong` in ~5.5 µs (no large clone/encode).
* 8,000 spaces with URL-query encoding (would expand beyond 8,192) → `TargetTooLong` in ~552 µs;
  the allocation is bounded by the remaining budget (~24 KB worst case).
* 8,192-byte text with the URL prefix → error before encoding.

No unbounded path found.

### Q-8/Q-9/Q-10

* **Q-8 fixed.** `validate_local_path` rejects `..` and any non-empty segment consisting only of
  `.`/space, after stripping `\\?\`, `\\.\`, `//?/`, `//./` prefixes (`src/policy.rs:598-627`).
  Probe: `...`, `.. `, `.`, ` . ` rejected; `.config` and `a..b` allowed. `a ` is rejected as
  surrounding whitespace; no traversal. **LOW residual:** trailing-dot/space segments such as `a.`
  are allowed (Windows may normalize them), but they cannot traverse.
* **Q-9 fixed.** Aliases are 1..=32 ASCII chars, start alphanumeric, then `[A-Za-z0-9._-]`
  (`src/model.rs:177-209`), matching the storage alias prefix. Probe: `"my link"` →
  `InvalidAliasCharacter`; `"my.link_1"` valid; length/start tests pass (`src/tests.rs:2030-2054`).
* **Q-10 implemented.** `Library::store` checks `Preview::verify_fingerprint()` before saving and
  returns `CommitError::FingerprintMismatch` (`src/library.rs:203-212`); `Preview::verify_fingerprint`
  recomputes `fingerprint_of` over the candidate, target and revision (`src/model.rs:484-491`).
  The existing preview test asserts a true verification (`src/tests.rs:999-1000`). **LOW residual:**
  the mismatch branch is not reachable through the public API (private fields, no public
  constructor/Deserialize) and has no test; it is defense-in-depth only.

### Q-11 and Q-12 — partial/accepted residuals

* **Q-11 partial (LOW, acceptable).** The quicklink target is now capped at 8,192 bytes, matching
  the host URL limit; raw Unicode letters are accepted by both layers; quicklinks is stricter on
  escapes/authorities/userinfo. The platform adapter still has a separate, looser validator and
  rejects Unicode whitespace that quicklinks accepts (Q-5 residual); sharing one validator remains a
  follow-up owned by `crates/platform-windows`. The current boundary is safe in the quicklink →
  preview direction; the host adapter remains the last line for direct `open_url` callers.
* **Q-12 not fixed (LOW, acceptable).** `ImportMode::ReplaceConfirmed` remains a caller contract, now
  documented on the variant (`src/portable.rs:31-40`). The CLI `--replace` flag is the user
  confirmation; a type-level token would be cleaner but would require editing `ansa-alpha`/CLI
  callers outside the crate. I accept this as a residual for a trusted host, but it should stay
  explicit in the UI/docs.

### Versioning residual (LOW)

`SCHEMA_VERSION` remains 1 and the exported JSON shape is unchanged, but validation is materially
stricter: aliases are now at most 32 characters (previously 64), targets are capped at 8,192
bytes/chars, and unsafe defaults/bidi formats are rejected. A v1 export produced by the previous
build can therefore contain items the new build rejects **per item** on import. This is not silent
data loss (the report names each item), but the unchanged version number does not signal the semantic
tightening. Consider a schema version bump or a documented v1-to-v1 migration note if old exports
must round-trip.

## Remaining quicklinks findings

* **MEDIUM — Q-3 executable denylist incomplete** (`src/policy.rs:25-28, 457-467, 518-523`):
  `DefaultApp` opens `.py`, `.pyw`, `.jar`, `.hta`, `.pl`, `.rb`, `.sh`, `.application`, `.pif`,
  `.scf`, `.msc`, `.settingcontent-ms`, `.search-ms`, `.diagcab` without `allow_executables`; import
  accepts such items and preview succeeds. Fix with a safe allowlist or a default-deny for unknown
  extensions, plus tests.
* **LOW — Q-1 sample-based import gate** (`src/portable.rs:444-480`): non-default choices and
  arbitrary text values are not exhaustively checked; the item is rejected at preview instead.
* **LOW — Q-1 default local import policy** (`src/portable.rs:214-243`): `import_json` defaults to
  `with_local_paths`; untrusted imports can add ordinary local links without host opt-in. Pass the
  host's actual policy for untrusted files.
* **LOW — Q-4 IPv6 shape validation / rare over-rejection** (`src/policy.rs:745-771`).
* **LOW — Q-5 Unicode whitespace accepted by quicklinks, rejected by the host** (`src/policy.rs:788-841`).
* **LOW — Q-6 U+2028/U+2029/U+00AD not rejected** (`src/text.rs:10-22`).
* **LOW — Q-10 mismatch branch untested/unreachable** (`src/library.rs:210-212`, `src/model.rs:484-491`).
* **LOW — Q-11 platform validator not shared; Q-12 confirmation is a contract** (accepted residuals).
* **LOW — unchanged `SCHEMA_VERSION` with stricter validation** (`src/portable.rs:25`,
  `src/model.rs:26`).

## Quicklinks verdict

**ANS-20 is conditionally acceptable.** All original encoding, single-pass substitution, scheme
blocking, preview/commit, alias, import/export and determinism findings are fixed or reduced to
LOW residuals; Q-2 and Q-9 are fixed, Q-1 is fixed for defaults/required samples, and Q-8/Q-10 are
implemented. The one substantive remaining issue is **Q-3's incomplete executable denylist**, which
still lets `DefaultApp` launch common interpreters (`.py`, `.jar`, `.hta`, `.application`, ...)
without the explicit `allow_executables` opt-in. If that gap is accepted explicitly as a residual
(or the host restricts `DefaultApp` to web URLs), the ticket is acceptable; otherwise it should be
fixed with a safe allowlist before closing. No BLOCKER or HIGH finding remains.

---

## Quicklinks delta re-review — Q-3 and LOWs

**Date:** 2026-11-19. This supersedes the Q-3 MEDIUM and the LOW bullets in the previous section;
the original text is retained for history. Only `crates/quicklinks/**` was inspected.

**Commands:** `cargo test -p ansa-quicklinks --offline` — **77 passed, 0 failed**; clippy
`--all-targets -- -D warnings` — **exit 0**; `cargo fmt -- --check` — **exit 0**;
`cargo check -p ansa-cli -p ansa-alpha --offline` — **exit 0**.

**Q-3 is now fixed for the requested policy.** `SAFE_DEFAULT_APP_EXTENSIONS` is a default-deny
allowlist (`src/policy.rs:94-111`); `validate_default_app_file` (`:602-622`) checks safe extensions
first, then `allow_executables`, then known executable/script/installer types, then
`allow_unknown_file_types`. A probe confirmed:

* `.py .pyw .jar .hta .pl .rb .sh .application .pif .scf .msc .settingcontent-ms .search-ms
  .diagcab` (and `.exe .bat .ps1 .xlsm .pptm`) are `ExecutableDisallowed` for `DefaultApp`, while
  `FileManager` still reveals them.
* Case (`.PY`), trailing dots (`.py.`, `.py..`), space-before-dot, ADS (`:stream`, `::$DATA`,
  `:.txt`), and double extensions (`run.txt.exe`, `run.exe.py`) are blocked. `run.exe`/`run.py`
  opened via `DefaultApp` with `allow_executables` succeed; `allow_unknown_file_types` does **not**
  bypass a known executable.
* `report.pdf`, `report.txt`, `report.docx`, images, audio/video and archives are allowed; unknown
  `.xyz` is `UnknownFileTypeDisallowed` unless `allow_unknown_file_types`; folders, extensionless
  paths, and trailing-separator paths are allowed by design (the host must stat directory status).
* Import now rejects a `DefaultApp` `.py` link per item and accepts a `.pdf` link
  (`import_rejects_executable_and_unknown_default_app_targets`, `tests.rs:1993-2038`; probe
  confirmed `applied: 0` for `.py`, `applied: 1` for `.pdf`).

**Remaining Q-3 LOWs (not MEDIUM):** the safe allowlist includes active/legacy content
(`html`, `htm`, `xhtml`, `svg`, `xml`, `rtf`, `doc`, `xls`, `ppt`) that can run scripts or macros in
the handler; extensionless/no-extension paths (including a native executable with no extension) rely
on the host adapter to stat the target and verify it is a directory/file before `ShellExecute`; a
folder whose name has an unknown dotted extension is treated as an unknown file (conservative).
`FileManager` reveal, device/network opt-ins and the `allow_unknown`/`allow_executables` separation
work as claimed.

**LOWs.** Unicode whitespace/soft hyphen are rejected consistently **at the resolved-target
boundary** for both URLs and local paths (test `tests.rs:2040-2062`; probe confirmed U+00A0, U+2000,
U+2028, U+2029, U+00AD → `UnsafeWhitespace`), while titles, aliases, tags and raw template literals
still accept them until preview/import resolves the target — no opening bypass, but display-text
consistency is not complete. Invalid IPv6 is mostly rejected (`[:::]`, group-count, multiple `::`,
invalid IPv4 octets: `tests.rs:2064-2089`), but `[::1.2.3.4:5]` and `[1.2.3.4::]` (IPv4 tail not
last) are still accepted and `[v1.fe]` is rejected (LOW; use a full IPv6 parser if exactness
matters). `SCHEMA_VERSION` remains 1 and `MIN_SUPPORTED_SCHEMA_VERSION` is 1; every v1 item is
revalidated at import, and the export document contains `schema_version: 1` (the
`MIN_SUPPORTED_SCHEMA_VERSION` constant is exported from the crate, not serialized as a field).
This is acceptable for the unchanged JSON shape, but old v1 exports containing now-invalid items
(e.g. 64-char aliases or unsafe defaults) are rejected **per item** without a version signal; a v2
bump or a documented v1 semantic-tightening note is advisable if old exports must round-trip.
Fingerprint mismatch is now testable via the `#[cfg(test)]` helper and
`fingerprint_mismatch_is_rejected_at_commit` (`tests.rs:1066-1084`); both the true and mismatch paths
pass.

**ANS-20 verdict:** **acceptable with residual risks.** No BLOCKER/HIGH/MEDIUM remains from Q-3 or
the reviewed LOWs. Residuals: no real browser/file-manager probe (the crate is inert and the CLI
uses a static probe), the Q-12 caller-confirmation contract, the unchanged v1 schema with stricter
validation, active/legacy extensions in the safe allowlist, extensionless host-stat responsibility,
and the remaining IPv6-shape edge. These are bounded and documented; I would close Q-3 and the LOW
items as fixed/partial-accepted.

---

# Storage delta re-review — ANS-78 / L05a

**Date:** 2026-11-19. Scope: `crates/storage/**` only. This is a delta on top of the ANS-19 storage
re-review above; previous sections are preserved.

## Commands and results

| Command | Result |
| --- | --- |
| `cargo test -p ansa-storage --offline` ×3 | **PASS.** 38 passed, 0 failed, 0 ignored on each run (0 unit tests; `tests/storage.rs` 38). |
| `cargo clippy -p ansa-storage --all-targets --offline -- -D warnings` | **PASS**, exit 0. |
| `cargo fmt -p ansa-storage -- --check` | **PASS**, no output, exit 0. |
| `cargo check -p ansa-cli -p ansa-alpha --offline` | **PASS**, exit 0 (consumer crates still compile against the new storage API). |

## Low disk (`SQLITE_FULL`)

The test uses SQLite's real capacity limit: `CapacityLimit::CurrentPages` reads
`PRAGMA page_count` and applies `PRAGMA max_page_count` on the **writer** connection
(`src/db.rs:94-112`, actor request `src/actor.rs:381-382`; test `tests/storage.rs:1511-1617`). It
writes acknowledged records/settings first, freezes the page count, writes 64 KiB payloads until
SQLite refuses growth, and asserts `StorageError::DiskFull` (the real `SQLITE_FULL` mapping). It also
asserts the refused record is absent (rollback), the acknowledged rows and setting still read, the
same store commits the retry after `Unlimited`, and both reopen and a `VACUUM INTO` backup pass an
integrity check. **No fake hook or manufactured error is involved.**

**LOW:** the loop may return `Ok` for some `overflow-*` records before the first failure; those are
acknowledged writes but the test only re-asserts the pre-limit rows, the setting and the final retry.
Tracking every successful id and asserting it after reopen would close that test gap. Physical
ENOSPC (a genuinely full device, as opposed to SQLite's page cap) is still not induced; this is the
documented residual.

## Single-instance lock

`src/lock.rs:52-90` opens `<durable>.lock` and calls `File::try_lock` (non-blocking); the lock is
acquired before the database in `Store::open_with_recovery_log` (`src/lib.rs:287-310`) and stored in
`Store.lock` (`src/lib.rs:261-269`), so it is held across `restore_from`'s close/reopen window. A
refused open returns `AlreadyLocked { path, holder_pid }`; the holder PID comes from the unlocked
`.lock.owner` note. `second_open_is_rejected_and_lock_releases_on_drop`
(`tests/storage.rs:1722-1750`) verifies the same-process refusal, live-writer continuity and reopen
after drop. `crashed_process_releases_the_instance_lock` (`:1752-1794`) starts a child that holds the
lock, asserts `AlreadyLocked`, kills it, and reopens after a bounded 5 s handle-teardown retry.
`try_lock` never blocks on a live holder.

**MEDIUM — path-alias bypass of the instance lock (hard link / junction alias).** The lock path is
derived from the configured `main_path` string (`src/lock.rs:52-53`, `src/lib.rs:292`), not from file
identity or a canonicalised data path. A local alias with a different final path gets a different
`.lock` file. Independent probe: with a first store holding `ansa.sqlite.lock`, create an NTFS hard
link `linked.sqlite` to the same DB and open `Store::open(linked.sqlite, linked-cache.sqlite)`; the
second store **opens successfully** (lock file `linked.sqlite.lock`), reads `None` for a record the
first store already acknowledged, and the first store cannot see the second's write. Two separate
`-wal` files exist for the same main DB, so the stores have divergent views and a checkpoint by
either can lose the other's committed pages. A junction/symlink to the data directory can produce the
same alias effect (canonicalisation would fix those; hard links need file identity). **Impact:**
single-instance policy bypass plus potential acknowledged-write loss on a WAL database; normal
single-path opens are correctly locked. **Minimal fix:** derive the lock path and opened database path
from `paths::normalized(main_path)` for symlink/junction resolution, and use file identity
(volume serial + file index on Windows, dev/ino on Unix) or explicitly reject hard-link aliases;
document hard links as unsupported if not handled. This is the only new finding above LOW.

**LOW residuals**
- The owner PID is written after the lock is acquired; a second open in that tiny window can return
  `AlreadyLocked { holder_pid: None }`.
- `Drop` unlocks and then removes `.lock.owner`; a new holder that acquires the lock between those
  steps can have its owner note removed by the previous holder (lock remains held; diagnostics lose
  the PID). The `.lock` file itself is never removed; that is harmless.
- The lock/owner paths are reserved by `check_backup_destination` (`src/lib.rs:898-904`) and
  `restore_from`'s source guard (`:662-666`), but `backup_destination_cannot_be_a_live_database`
  does not test the lock/owner paths, and there is no test for a second open while `restore_from` is
  in progress despite the lock field proving the intent.
- A `cache_path` equal to `main.lock`/`main.lock.owner` is not rejected by `StorageConfig::validate`;
  the former turns the cache into the lock file (works, but conflates two files), the latter fails to
  open as a database. LOW configuration-hygiene gap.
- The lock is advisory: a raw SQLite client or a Store opened through a hard-link alias is not
  stopped (the alias case is the MEDIUM above; a raw client is the documented residual).

## Recovery reports

`RecoveryInfo`/`RecoveryKind` and `Store::open_with_recovery_log`/`recovery_info` are public
(`src/recovery.rs`, `src/lib.rs:281-310, 324-331`). `apply_migrations` appends each snapshot as it is
taken, including snapshots from a migration that then fails
(`src/migrations.rs:134-214`). Tests verify: a failed open reports its pre-migration snapshot (path
exists, v2, `recovery_copy: None`) both from the failed open and the later successful one
(`tests/storage.rs:1246-1336`); a rejected corrupt restore reports `RestoreAborted` with the source
path and the untouched main DB as `recovery_copy` (`:948-963`); and the injected shutdown-checkpoint
failure reports the source backup and main DB while revision 2 stays readable (`:1051-1059`).
`restore_from` records every pre-replacement failure; when rollback itself fails, `rollback_failure`
names the preserved `.pre-restore-*` copy and reports it (`src/lib.rs:815-848`), and
`install_candidate` now cleans staged/old files on the second sidecar-check failure
(`src/backup.rs:282-287`). No acknowledged data loss was found in the covered paths.

**LOW residuals**
- A `PreMigrationSnapshot` report names the snapshot but leaves `recovery_copy: None`, so it does not
  itself name the original/durable DB (callers have `Store::main_path()`); the tests assert this
  intentional shape. `RestoreAborted` reports do name both source and original/preserved copy.
- The tests assert readable data after a failed restore but do not directly assert the original
  `-wal`/`-shm` files were preserved; the shutdown-fault path relies on the connection close/drop.
- A restore source equal to the live DB/lock/owner is rejected before `record_restore_aborted`, so it
  produces no recovery entry; it is a configuration rejection rather than a destructive attempt.
- The rollback-failure branch (`Replacement::rollback` failing) has no deterministic in-tree fault;
  it is code-inspected only, and the Store is left closed (`inner = None`) with the error naming the
  preserved copy. This is the declared untested-rollback residual.
- A normal fresh open reports the pre-migration v2 snapshot even though the DB was empty; this is
  accurate but may be noise for a UI recovery prompt.

## Verdict

**L05a is acceptable with residual risks**, with one **MEDIUM** path-alias lock finding to resolve
or explicitly accept. The core claims are verified: the low-disk test uses real
`PRAGMA max_page_count`/`SQLITE_FULL` and proves rollback/recovery; the non-blocking lock works for
normal same-path opens, drop and a killed child; recovery reports name snapshots/backups and the
original/preserved DB on the tested failure paths. Accepted residuals are physical ENOSPC (page-cap
simulation only), the advisory nature of the lock, and the untested rollback-failure branch. The
hard-link/junction alias bypass should be fixed by canonicalising the lock/open path and/or using
file identity before declaring the single-instance guarantee complete; if the host guarantees a
canonical data path and hard links are out of scope, it can be recorded as an accepted residual.

---

## L05a identity-lock delta (hard-link fix)

**Date:** 2026-11-19. Supersedes the MEDIUM path-alias finding above.

**Commands:** `cargo test -p ansa-storage --offline` ×3 — **41 passed, 0 failed** each run
(the suite grew from 38); clippy `--all-targets -- -D warnings` — **exit 0**;
fmt `-- --check` — **exit 0**. A targeted
`cargo test -p ansa-storage --offline hard_link_alias_is_rejected_by_identity_lock -- --nocapture`
also passed with no “skipping” output.

**Verified.** `src/lock.rs:207-226` derives the key from the **file identity**: on Windows
`winapi_util::file::information` (safe wrapper; the crate remains `#![forbid(unsafe_code)]`) gives
volume serial + 64-bit file index; on Unix `metadata.dev()`/`ino()`. Lock files live in a fixed
per-user directory outside the data directory (`:170-199`), so hard links, symlinks and junctions
resolve to one lock. `InstanceLock::acquire` opens the durable file with raw `OpenOptions` only to
read identity and drops it before locking (`:69-93`); SQLite is opened only later in
`Inner::open`, after `Store::open_with_recovery_log` has the lock (`src/lib.rs:292-309`). The
hard-link test creates a real `fs::hard_link` to the live DB and asserts `AlreadyLocked` with the
same lock path/`holder_pid`, no `alias-wal`, and that the live writer remains usable
(`tests/storage.rs:1808-1847`). On this Windows NTFS temp it ran; the source still has a silent
“skip if `fs::hard_link` fails” branch, so a non-NTFS/network temp could be green without exercising
it.

**Restore/recovery.** `install_candidate` acquires the staged file's identity lock **before** the
rename (`src/backup.rs:297-309`); `Replacement::rollback` acquires the preserved copy's lock
**before** restoring it (`:235-249`), and `restore_from` adopts the returned lock on success or
rollback (`src/lib.rs:731-761, 796-808`), leaving no unlocked live-main window in code.
`restore_from` rejects a source whose **identity** equals the live main, including a hard link
(`src/lib.rs:670-678`), and the backup/destination guards use identity for lock/owner files
(`:927-933`). `restore_rekeys_the_identity_lock` verifies the post-restore lock path changes and a
second open is refused (`tests/storage.rs:1872-1906`).

**Remaining LOWs:** the hard-link test's silent skip (make it mandatory/`expect` on Windows if the
case must always run); no direct test for `source == live identity`, rollback re-keying, or the
no-unlocked-window claim (code-inspected); identity fallback to a normalized-path key if the
platform cannot report identity (documented, hard links then undetectable); per-user/per-environment
lock directory means different `LOCALAPPDATA`/`XDG_RUNTIME_DIR`/users can still bypass; and restoring
through a file-level alias may leave other alias names pointing at the old file (expected file
replacement semantics, worth documenting). `REVIEW-DISPOSITION.md` still says 38 tests; the current
suite is 41.

**Verdict:** ANS-78/L05a is **acceptable with residual risks**. The previous path-alias MEDIUM is
closed for normal identity-capable filesystems; no BLOCKER/HIGH/MEDIUM remains. Accepted residuals
are physical ENOSPC (the page cap is the real SQLite error path but not a full device), the advisory
lock (raw SQLite clients), and the untested rollback-failure branch; the LOWs above are test/documentation
hardening rather than implementation blockers.

---

## L05a hard-link fix — final delta

**Commands re-run on the current tree:** `cargo test -p ansa-storage --offline` ×3 — **41 passed,
0 failed** each run (the current suite is 41, not the 38 recorded in `REVIEW-DISPOSITION.md`);
clippy `--all-targets -- -D warnings` — **exit 0**; fmt `-- --check` — **exit 0**.

**Identity key verified.** `src/lock.rs:207-226` uses the Windows `winapi_util::file::information`
safe wrapper (volume serial + 64-bit file index) and Unix `dev`+`ino`; the lock key is not a path
hash unless identity retrieval fails. Lock files live in a fixed per-user directory outside the data
directory (`:170-199`). `InstanceLock::acquire` opens the durable file with raw `OpenOptions` only
to read identity, drops that handle, then takes the lock (`:69-93`); SQLite is opened only later in
`Inner::open`. The hard-link test creates a real `fs::hard_link` and asserts `AlreadyLocked` with the
same lock path/`holder_pid`, no alias `-wal`, and a still-usable live writer
(`tests/storage.rs:1808-1847`). A targeted `--nocapture` run passed with no “skipping” output, so it
**did run on this Windows NTFS temp**. The test still silently returns if `fs::hard_link` fails, so
a non-NTFS/network CI could be green without exercising it — **LOW test-robustness**.

**Restore/re-key verified in code and by test.** `install_candidate` acquires the staged file's
identity lock before the rename (`src/backup.rs:297-309`); `rollback` acquires the preserved copy's
lock before restoring it (`:235-249`); `restore_from` adopts the returned lock on success and on
rollback (`src/lib.rs:731-761, 796-808`). My independent probe confirmed `restore_from` on a
hard-link source returns `InvalidConfig` and leaves the live DB usable; the existing
`restore_rekeys_the_identity_lock` test proves the post-restore lock path changes and a second open
is refused (`tests/storage.rs:1872-1906`). No unlocked live-main window is apparent in the code.
**LOW gaps:** no direct test for `source == live identity`, rollback re-keying, or the no-unlocked-
window claim (code-inspected); identity fallback is a normalized-path key if the platform cannot
report identity, so hard links are undetectable there; the per-user/per-environment lock directory
means different `LOCALAPPDATA`/`XDG_RUNTIME_DIR`/users can still bypass; and restoring through a
file-level alias may leave other alias names pointing at the old file (expected replacement
semantics; worth documenting).

**Adjacent LOW found in the guard:** `backup_to` rejects a **path** equal to the live DB/cache and
lock/owner paths, and `same_identity` for lock/owner, but does not use `same_identity` for the live
main/cache themselves. My probe successfully backed up to an NTFS **hard link of the live main**:
the operation replaces that alias with the backup file while leaving the live main intact. This is
not corruption, but the destination is a live-DB identity and should be rejected for consistency
(`src/lib.rs:883-916`); add `lock::same_identity(path, live)` to that guard.

**Final verdict:** ANS-78/L05a is **acceptable with residual risks**. The prior path-alias MEDIUM
is closed; no BLOCKER/HIGH/MEDIUM remains. Residuals: physical ENOSPC (real SQLite `SQLITE_FULL`
path, not a full device), advisory lock (raw SQLite clients), untested rollback-failure branch, and
the LOW test/guard items above.

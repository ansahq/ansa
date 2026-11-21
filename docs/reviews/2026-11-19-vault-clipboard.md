# Review: ANS-26 (P03 vault), ANS-27 (P04 clipboard capture), ANS-29 (P05 insertion)

**Date:** 2026-11-19
**Reviewer:** independent subagent (read-only review; no implementation changes)
**Repository:** `C:\Users\abhir\.local\share\opencode\worktree\1bbf61\playful-eagle`
**Written file:** this review only.

| Ticket | Task ID | Code under review |
| --- | --- | --- |
| ANS-26 / P03 — vault and secrets | `ig1tmsjihiwnh21yufh1b315` | `crates/vault/**` |
| ANS-27 / P04 — clipboard capture | `oohr4v94ybngl3a0zb8umxol` | `crates/clipboard/**` (capture, filtering, storage) |
| ANS-29 / P05 — insertion | `txkwq727oyipid3gnmlpz2rg` | `crates/clipboard/**` (insertion transaction) |

The review is against the ticket scope in `outputs/implementation-backlog.md` and the security
criteria in the review brief. No production consumer of either crate exists in the working tree
(`grep` finds no `ansa_vault` / `ansa_clipboard` use outside the two crates), so acceptance below is
at crate/API level, not end-to-end desktop acceptance.

---

## 1. Commands and exact results

All commands were run from the repository root.

| Command | Result |
| --- | --- |
| `cargo test -p ansa-vault -p ansa-clipboard --offline` | **PASS.** `ansa_clipboard` unit: **71 discovered, 70 passed, 0 failed, 1 ignored**. `tests/public_api.rs`: 2 passed. `ansa_vault` unit: 3 passed. `tests/vault.rs`: 13 passed (including a real child-process DPAPI reopen test). Doc-tests: 1 passed. **Combined: 89 passed, 0 failed, 1 ignored.** |
| `cargo clippy -p ansa-vault -p ansa-clipboard --all-targets --offline -- -D warnings` | **PASS.** no warnings. |
| `cargo fmt --check -p ansa-vault -p ansa-clipboard` | **PASS.** no output. |
| Ignored clipboard test | `windows::tests::real_clipboard_read_does_not_modify_it` is `#[ignore]`; it was **not run** because it reads the real clipboard. The Windows adapter therefore has no automated live test. |

The claimed **73 tests** is exact for the clipboard crate (71 unit + 2 integration), of which 72 ran
and 1 is the ignored manual clipboard test. The vault adds 16 passing tests, and the clipboard
doctest adds one more: **89 passing tests + 1 ignored** in total.

### Independent probes

A throwaway crate at `C:\Users\abhir\AppData\Local\Temp\opencode\ansa-review-probe` (path
dependencies on both crates, `tempfile`) was used only to confirm filter and cleanup behavior below.
The probe modified no repository file. A Windows symlink probe could not run because this host
requires `SeCreateSymbolicLinkPrivilege`; symlink/reparse behavior below is therefore static review
only.

Probe results that matter:

```text
json-password:            sensitive=false   {"password": "hunter2-hunter2"}
json-api-key:             sensitive=false   {"api_key": "abcd1234efgh5678"}
env-password:             sensitive=true    password=hunter2-hunter2
yaml-password:            sensitive=true    password: hunter2-hunter2
quoted-env:               sensitive=false   PASSWORD="hunter2-hunter2"
configured-cyrillic-upper sensitive=false   marker "ПАРОЛЬ", text "ПАРОЛЬ"
configured-cyrillic-lower sensitive=true    marker "ПАРОЛЬ", text "пароль"
luhn-valid order id       sensitive=true    1000000000000008
prefixed-unrelated-secret survives=false    vault secret "ansa-clip-vault-note" deleted by store open
```

---

## 2. Findings

Severity: **BLOCKER / HIGH / MEDIUM / LOW**. No BLOCKER was found. There is **no clipboard-content
plaintext at rest** path in either crate: the vault has only DPAPI, the non-Windows backend is
explicit `Unsupported`, and the production store constructor requires a `Vault` (metadata is
unencrypted by design; see the note in C-11).

| ID | Severity | Finding |
| --- | --- | --- |
| C-1 | **HIGH** | `ExcludeClipboardContentFromMonitorProcessing` Windows opt-out is ignored |
| C-2 | **MEDIUM** | Orphan cleanup deletes prefixed non-clipboard secrets; empty history deletes all seals |
| C-3 | **MEDIUM** | No cross-process lock; concurrent instances lose captures or delete fresh seals |
| C-4 | **MEDIUM** | Configured sensitive markers are not case-insensitive for non-ASCII text |
| C-5 | LOW | Heuristic false negatives (JSON/quoted credentials) and Luhn false positives |
| C-6 | LOW | Preview cache is not wiped on store drop |
| C-7 | LOW | `SnapshotContent`/filter scratch copies are not wiped; `content.rs` overclaims |
| C-8 | LOW | Non-standard marker DWORD fails open |
| C-9 | LOW | `CF_HDROP` has no aggregate memory bound |
| C-10 | LOW | Delete/clear report success while ciphertext deletion is best-effort |
| C-11 | LOW | Pause/private mode and capture policy are session/in-memory state |
| C-12 | LOW | Public unencrypted `MemorySealBackend` / `open_with_backend` footgun |
| C-13 | LOW/scope | No production capture listener or consumer; pull-only |
| C-14 | LOW | `next_id` saturation from tampered metadata can duplicate IDs |
| V-1 | LOW | `rotate` does not wipe plaintext if `seal` fails |
| V-2 | LOW | Concurrent `put_secret`/`rotate` are last-writer-wins |
| V-3 | LOW | Symlink/temp/ACL/directory-fsync hardening residuals |
| V-4 | LOW/scope | No backup/recovery; P03 backup clause not delivered |
| V-5 | LOW | Sentinel/rotation/edge test coverage is narrow |
| V-6 | LOW | Raw Windows error code on decryption failure |
| I-1 | LOW | Sequence/owner check is not last; `Unknown` owner and sequence 0 pass |
| I-2 | LOW | `InsertionTarget.foreground` is never read |
| I-3 | LOW/scope | No Windows insertion host; secure-input/IME/evidence are host contracts |
| I-4 | LOW | No tests for owner `Unknown`, sequence 0, or a copy after the final check |

### 2.1 Clipboard capture and storage (ANS-27)

#### C-1 — HIGH — `ExcludeClipboardContentFromMonitorProcessing` is not honored, so an explicit Windows opt-out is ignored

**Where:** `crates/clipboard/src/windows.rs:171-175` (only `CanIncludeInClipboardHistory`,
`CanUploadToCloudClipboard`, `Clipboard Viewer Ignore` are read), `crates/clipboard/src/filter.rs:26-39`
(the marker model has no field for it), `crates/clipboard/src/filter.rs:452-460` (rejections).

**Cause:** Windows documents `ExcludeClipboardContentFromMonitorProcessing`,
`CanIncludeInClipboardHistory` and `CanUploadToCloudClipboard` for clipboard history/cloud control;
the adapter also honors the legacy `Clipboard Viewer Ignore`. The adapter reads the last three but
never registers or checks `ExcludeClipboardContentFromMonitorProcessing`, whose **presence** prevents
all formats on the clipboard from being included in clipboard history or synchronized. A producer
that relies on this marker alone (rather than `CanIncludeInClipboardHistory = 0`) is silently
captured.

**Impact:** a password manager or other application using the documented marker can have its
clipboard content sealed into Ansa's history and shown in previews. The content is DPAPI-sealed at
rest and still subject to the content heuristics, but the producer's explicit history opt-out is
violated. This is a direct failure of the P04 "sensitive flags / Windows markers" scope.

**Fix:** add a marker field (or map it to `viewer_ignore`), check
`marker_is_present("ExcludeClipboardContentFromMonitorProcessing")` in `read()`, and reject it in
`evaluate` as a full capture exclusion. Add a test that constructs the marker state; an adapter
test can be a mocked `ExclusionMarkers`, since the real clipboard test is ignored.

#### C-2 — MEDIUM — Orphan cleanup deletes unexpected `ansa-clip-*` vault secrets, and an empty history deletes every clipboard seal

**Where:** `crates/clipboard/src/store.rs:812-827` (`cleanup_orphans`), `crates/clipboard/src/store.rs:53`
(`ALIAS_PREFIX`), `crates/clipboard/src/store.rs:467` (called on open), `crates/clipboard/src/store.rs:787`
(called on reload), `crates/clipboard/src/store.rs:812-813` (the doc claim "Non-clipboard vault
secrets are never touched"), and `crates/clipboard/src/store.rs:1471-1491` (the test only seeds an
unprefixed `"unrelated-secret"`).

**Cause:** the store takes a general-purpose `Vault` and treats the `ansa-clip-` prefix as its
exclusive namespace. Any other secret whose name happens to start with that prefix is deleted on
`ClipboardStore::open` / `reload_from_disk`. If `history.json` is missing or contains an empty item
list (for example, the store root was reset while the vault was kept), `load_history` returns an
empty history and the same cleanup deletes every `ansa-clip-*` sealed payload.

**Evidence (probe):** seeding a `MemorySealBackend` with `ansa-clip-vault-note` and opening a store
with an empty history returned `prefixed-unrelated-survives: false`. The existing test
`opening_removes_orphaned_seals_but_not_unrelated_vault_secrets` uses `unrelated-secret`, so it does
not exercise the collision.

**Impact:** silent loss of a non-clipboard vault secret, or of all clipboard ciphertext, on a
normal open. Encrypted data only; no plaintext exposure. The fixed prefix is predictable and not
documented as reserved.

**Fix:** isolate the clipboard namespace: put clipboard seals in a dedicated store subdirectory /
separate vault, or embed a random store identifier in the alias (for example
`ansa-clip-<store-id>-<id>`) and delete only aliases carrying this store's identifier. Do not run
destructive cleanup when the metadata file is absent; treat that case as "unknown" rather than
"all orphans".

#### C-3 — MEDIUM — No cross-process lock: concurrent instances can lose captures or delete a freshly sealed payload

**Where:** `crates/clipboard/src/store.rs:438-469` (`open_with_backend`), `crates/clipboard/src/store.rs:528-621`
(`capture` does seal → metadata write → in-memory commit), `crates/clipboard/src/store.rs:814-827`
(destructive cleanup on every open).

**Cause:** the store's serialization is `&mut self` only. Two `ClipboardStore` instances over the
same root and vault perform independent read-modify-write cycles on `history.json` and independently
run orphan cleanup. A second instance can read stale metadata, delete a sealed payload that the
first instance just wrote but has not yet referenced in `history.json`, or overwrite the first
instance's metadata with its own candidate (both allocate the same `next_id`). No file lock,
singleton guard, or generation counter is present.

**Impact:** silently lost or permanently undecryptable clipboard items. No plaintext exposure, and
no overwrite of a newer *clipboard* value; this is storage concurrency. The launcher may be
single-instance by policy, but neither crate enforces it and the ticket explicitly asks about
concurrent capture.

**Fix:** take an exclusive lock file (or a named mutex) around open/capture/reload, or make the
store a single-instance service. Make orphan cleanup generation-aware or grace-based (for example,
do not delete seals younger than the last metadata commit). Add a two-process test.

#### C-4 — MEDIUM — Configured sensitive markers are not case-insensitive for non-ASCII text

**Where:** `crates/clipboard/src/filter.rs:519-528` (haystack lowered with `to_ascii_lowercase`),
`crates/clipboard/src/model.rs:360-370` (`normalize_list` lowers markers with Unicode-aware
`to_lowercase`).

**Cause:** the configured marker is stored in Unicode lowercase, but the clipboard text is lowered
with ASCII-only `to_ascii_lowercase`. A marker such as `ПАРОЛЬ` becomes `пароль`, while the copied
text `ПАРОЛЬ` is unchanged by the ASCII lowercase and never matches. File-list paths have the same
problem.

**Evidence (probe):** with `sensitive_markers = ["ПАРОЛЬ"]`, `"ПАРОЛЬ"` returned `sensitive=false`
and `"пароль"` returned `sensitive=true`.

**Impact:** a user-configured security marker for a non-Latin language can silently fail on
uppercase or mixed-case content. The built-in heuristics are English-centric, so for such users the
configured marker may be the only content-specific guard. This is a correctness bug in a security
control, not merely a heuristic limitation.

**Fix:** lower the haystack with a bounded Unicode `to_lowercase` (reject first on the existing size
bound), or use a case-folding comparison for markers; alternatively restrict configured markers to
ASCII and document that. Add uppercase/mixed-case tests for a non-ASCII marker.

#### C-5 — LOW — Content heuristics have documented-by-implication but not tested false negatives (JSON/quoted credentials) and nontrivial false positives

**Where:** `crates/clipboard/src/filter.rs:588-631` (`contains_sensitive_assignment` requires `=` or
`:` immediately after the key, before any quote), `crates/clipboard/src/filter.rs:640-675` (Luhn
detection accepts any 13–19 digit run, including 16-digit order numbers).

**Cause / evidence (probe):**
- `{"password": "hunter2-hunter2"}` and `{"api_key": "abcd1234efgh5678"}` are **not** flagged,
  because after `password` the next character is `"`, not `=`/`:`.
- `PASSWORD="hunter2-hunter2"` is not flagged for the same reason.
- `1000000000000008` (a Luhn-valid 16-digit order-like number) is flagged as a payment card. Any
  16-digit value has roughly a 10% chance of a Luhn pass, so ordinary order/account numbers are
  occasionally rejected.

**Impact:** JSON is a very common clipboard payload, so a built-in credential rule misses a
realistic case. The Luhn rule occasionally rejects harmless content. Both fail closed (no secret is
leaked) and the rustdoc does say "conservative heuristic, not a proof" and "false positives are
preferred to silent capture"; this is therefore LOW, but the specific false-negative risk is not
documented and there are no regression tests for JSON/quoted forms.

**Fix:** skip a leading `"`/`'` after the key before looking for `=`/`:` and/or parse a JSON
key/value pair; add fixtures. Either document the Luhn false-positive rate or require a card-like
prefix/context. This is not a blocker for a heuristic, but it should be an explicit limitation.

#### C-6 — LOW — Preview-cache plaintext is not wiped when the store is dropped

**Where:** `crates/clipboard/src/store.rs:343-396` (`PreviewCache`), `crates/clipboard/src/store.rs:398-419`
(`ClipboardStore` has no `Drop`), `crates/clipboard/src/store.rs:652-657` (`lock` is the only wipe
path).

**Cause:** `PreviewEntry` has no `Drop`/wipe, and `ClipboardStore` does not implement `Drop`.
`lock()`, `unlock()`, `reload_index()`, eviction and delete wipe the cache, but simply dropping the
store (app shutdown, error path, test end) leaves up to 16 KiB of preview plaintext in freed heap
memory. A crash dump can contain it.

**Impact:** best-effort in-memory plaintext retention. The crate documents lock-time wiping and
best-effort limits; the drop path is simply not covered. Same-user threat only; not at rest.

**Fix:** implement `Drop` for `PreviewCache`/`ClipboardStore` that calls `clear()`, and add a drop
test with a sentinel (allocator retention makes the assertion best-effort, but the wipe call is
testable).

#### C-7 — LOW — `SnapshotContent` and filter scratch copies hold plaintext without wiping; the `content.rs` doc overclaims

**Where:** `crates/clipboard/src/filter.rs:73-103` (`SnapshotContent` has no `Drop`),
`crates/clipboard/src/filter.rs:520` (`lower = value.to_ascii_lowercase()`),
`crates/clipboard/src/filter.rs:609-616` (`candidate: String` per key occurrence),
`crates/clipboard/src/content.rs:1-7` ("`ClipboardContent` is the only type in this crate that owns
plaintext").

**Cause:** rejected snapshots—which are often the sensitive ones—are dropped normally. The adapter's
temporary `Vec<u16>`/`String` units and the filter's lowercase/candidate copies are also dropped
unwiped. Only `ClipboardContent` wipes on drop.

**Impact:** rejected secrets and paths remain in freed process memory longer than the documentation
implies. Same-user memory disclosure only; no persistence. The `content.rs` statement is factually
wrong.

**Fix:** give `SnapshotContent` a drop wipe, use bounded wiped scratch buffers for the marker/
assignment scan (or document the copies), and correct the `content.rs` claim to name the types that
can hold plaintext.

#### C-8 — LOW — A readable but non-standard marker DWORD is treated as permission, not fail-closed

**Where:** `crates/clipboard/src/windows.rs:231-245` (`boolean_marker` returns
`Some(value != 0)`), `crates/clipboard/src/windows.rs:247-273` (`read_dword` accepts any block of
size ≥ 4).

**Cause:** the code comment says "A present marker whose DWORD cannot be read is treated as an
opt-out so a malformed producer marker fails closed." A producer that writes a string such as `"0"`
instead of a DWORD yields bytes `0x30 0x00 0x00 0x00`, which is readable and nonzero, so it is
treated as `Some(true)` (allowed). Any unexpected nonzero value has the same effect.

**Impact:** a malformed opt-out marker fails open. Only out-of-contract producers are affected, and
the value is not clipboard content. LOW.

**Fix:** accept exactly `1` as allowed; treat `0` and every other value as an opt-out (or return an
"unknown, fail closed" state). Add a unit test for a short/garbage marker block if the FFI can be
factored enough to test.

#### C-9 — LOW — `CF_HDROP` has per-path bounds but no aggregate memory bound

**Where:** `crates/clipboard/src/windows.rs:313-359`, constants at `crates/clipboard/src/windows.rs:41-44`.

**Cause:** the adapter limits to 1024 paths and 32,768 UTF-16 units per path, but not the sum. A
malicious clipboard producer can make the adapter allocate roughly 1024 × 64 KiB ≈ 64 MiB of
UTF-16 before `evaluate` applies the much smaller `max_item_bytes` policy, plus the resulting
`String` copies. The content is then rejected, but the allocation already happened.

**Impact:** bounded local memory-amplification/DoS from a clipboard producer; no plaintext
persistence. 64 MiB is not a crash on a typical desktop, but it is far above the documented
"bounded" impression.

**Fix:** add an aggregate byte/unit budget in the adapter (stop and set `truncated` once it is
reached), or reuse `ABSOLUTE_MAX_ITEM_BYTES` as an adapter cap.

#### C-10 — LOW — Delete/clear commit metadata before deleting ciphertext and report success even if deletion fails

**Where:** `crates/clipboard/src/store.rs:723-735` (`delete_item`), `crates/clipboard/src/store.rs:737-749`
(`clear_all`), `crates/clipboard/src/store.rs:609-614` (eviction deletion is also best-effort).

**Cause/impact:** the metadata write is authoritative; payload deletion is `let _ = ...`. If a
delete fails (lock, I/O, sharing violation), the user sees a successful delete/clear while the
ciphertext remains on disk until a later `open`/`reload` orphan cleanup. DPAPI still protects it,
and no plaintext remains, but "delete" is not immediate. A crash between the two steps has the same
effect.

**Fix:** treat payload deletion as part of the operation: report partial failure and retry, or
persist a tombstone and sweep on open. Do not silently return success while the ciphertext remains.
Document that deletion is logical until the sweep.

#### C-11 — LOW — Pause, private mode and the capture policy are session/in-memory state

**Where:** `crates/clipboard/src/model.rs:418-457` (`from_stored` resets `paused`/`private_mode`),
`crates/clipboard/src/store.rs:767-789` (reload preserves only the in-memory flags),
`crates/clipboard/src/store.rs:426-442` (the policy is supplied by the caller).

**Cause/impact:** a restart or crash resumes capture and loses pause/private mode. If the app opens
the store with `CapturePolicy::default()` each time, configured exclusions also disappear. This is
not a crate-level hidden failure (the docs call them session flags), but a user who paused before a
crash can have the next copy captured. The policy is presumably L05 settings; ensure it is wired
and documented.

**Fix:** persist session flags/policy in settings (with explicit semantics) or document the reset
prominently and have the store refuse to open without the user's settings.

**Note — metadata is unencrypted by design:** `history.json` contains capture timestamps, sequence
numbers, byte/character counts and the sanitized source process name. It contains no clipboard text
or file paths, but the metadata exposure (especially source app names) should be an explicit product
decision, not assumed harmless.

#### C-12 — LOW — `MemorySealBackend` is a public, unencrypted test double shipped in release

**Where:** `crates/clipboard/src/store.rs:142-160`, `crates/clipboard/src/store.rs:438-442`
(`open_with_backend`), `crates/clipboard/src/lib.rs:20-22`.

**Cause/impact:** a caller can construct a store whose payloads are plaintext in memory (or
implement a custom plaintext-on-disk `SealBackend`) and the type is compiled into release builds.
The docs say it "must not be used for real data", but the API does not enforce it. No current
consumer uses it outside tests (`grep` confirms). Not a hidden fallback, but a footgun.

**Fix:** feature-gate the test double (`#[cfg(test)]` or a `test-support` feature), or make
`open_with_backend` crate-private/test-only. Keep `ClipboardStore::open(Vault)` as the only
production constructor.

#### C-13 — LOW / scope — No production capture listener or consumer; the adapter is pull-only

**Where:** `crates/clipboard/src/lib.rs:5-6`, `crates/clipboard/src/store.rs:528`; no
`AddClipboardFormatListener` / message loop and no app wiring in the tree.

**Impact:** the crate is a library only. No end-to-end capture runs on a real clipboard, and no
self-copy feedback suppression exists (a future listener could re-capture the launcher's own
insertion copy). This is expected for a crate-level ticket but must not be described as a working
clipboard history.

**Fix (later ticket):** wire a `WM_CLIPBOARDUPDATE` listener, suppress snapshots whose owner is the
launcher, and add a live integration test.

#### C-14 — LOW — `next_id` saturation can create duplicate IDs from tampered metadata

**Where:** `crates/clipboard/src/model.rs:542-621`, especially `self.next_id.saturating_add(1)` at
`model.rs:619`, and the `next_id <= max_id` check at `model.rs:442-445`.

**Cause/impact:** `from_stored` accepts `next_id == u64::MAX` when `max_id == u64::MAX - 1`.
`insert` then uses `u64::MAX` and never increments; the next insert reuses the same id, creating
duplicate metadata IDs (and `seal` overwrites the earlier payload). The history will fail
`from_stored` on the next open ("duplicate item identifier"). Requires tampered metadata or an
astronomically large number of captures; no secret exposure.

**Fix:** reject `next_id == u64::MAX` (or reserve it as exhausted) and assert uniqueness on every
insert.

---

### 2.2 Vault (ANS-26)

The DPAPI layer itself is correct and there is no plaintext fallback. I found no BLOCKER/HIGH/MEDIUM
in this crate.

**DPAPI FFI check (`crates/vault/src/dpapi_windows.rs:16-197`):**

- `DataBlob` is `#[repr(C)] { cb_data: u32, pb_data: *mut u8 }`; it matches `DATA_BLOB` on 32- and
  64-bit Windows, including the 4/8-byte alignment padding.
- Both imports use `#[link(name = ...)]` plus `unsafe extern "system"`; the signatures match the SDK
  (`BOOL` = `i32`, `DWORD` = `u32`, `PVOID`/prompt structures = pointers, description = `LPCWSTR`).
- Both calls pass `CRYPTPROTECT_UI_FORBIDDEN`; neither passes `CRYPTPROTECT_LOCAL_MACHINE` or
  `CRYPTPROTECT_SYSTEM`, so the protection stays user-bound. The reserved and prompt pointers are
  null; the optional entropy is the only optional input.
- Entropy is `CONTEXT_DOMAIN || validated context`. `context_tag` is compared first for a clear
  error, but DPAPI's entropy binding is what actually prevents a rewritten tag from decrypting; the
  tampered/rewritten-tag test passes.
- Input lengths are converted with `u32::try_from`; the input and entropy slices are live for the
  call and are only read. `protect`/`unprotect` never truncate a length.
- Output is passed by value; a null pointer with zero length is handled, a null pointer with a
  nonzero length is rejected, and the output length is checked against `MAX_SEALED_FILE_BYTES` /
  `MAX_PLAINTEXT_BYTES` **before** `from_raw_parts`. `LocalFree` is called exactly once on the
  success path and on the over-limit path; the bytes are copied before freeing.
- No failure-path leak is visible: DPAPI does not allocate `pDataOut` on failure, and errors are
  mapped to `Dpapi`/`DecryptionFailed` with `last_os_error` captured immediately.
- The crate uses `#![deny(unsafe_op_in_unsafe_fn)]`; every FFI call is in an explicit `unsafe`
  block with a safety comment. There is no unsafe operation hidden in a safe function.
- Empty plaintext round-trips (tested), and the plaintext/file/context/alias/secret-count bounds
  are enforced on both write and read (tests cover the main ones).

#### V-1 — LOW — `rotate` does not wipe the decrypted plaintext when `seal` fails

**Where:** `crates/vault/src/lib.rs:511-513`:

```rust
let mut plaintext = self.open_payload(&payload, &context)?;
let replacement = self.seal(&plaintext, &context)?;
wipe(&mut plaintext);
```

If `seal` returns an error (DPAPI failure), `?` drops the `Vec` without the `wipe` call used on the
success path and in the crate's own `wipe` helper.

**Impact:** a secret survives in freed heap memory on a protection failure. No disk leak; same-user
memory only. The crate otherwise takes care to wipe here, so this is an inconsistency.

**Fix:** compute the result, wipe unconditionally, then propagate:

```rust
let replacement = self.seal(&plaintext, &context);
wipe(&mut plaintext);
let replacement = replacement?;
```

Add a test with an injected failing backend if feasible.

#### V-2 — LOW — Concurrent `put_secret`/`rotate` are last-writer-wins; `rotate` can revert a newer secret

**Where:** `crates/vault/src/lib.rs:380-411` (`put_secret`: find, seal, rename), `crates/vault/src/lib.rs:495-532`
(`rotate`: read, open, seal, rename), `crates/vault/src/lib.rs:390-396` (4096-secret check is a
TOCTOU).

**Cause/impact:** `Vault` is `Sync` and its methods take `&self`, but nothing coordinates a
`rotate` with a concurrent `put_secret` for the same alias. `rotate` can read an old ciphertext,
then a `put_secret` can write a new value, and `rotate` can rename the resealed old value over it.
Two concurrent puts can also exceed `MAX_SECRETS`. The result is a silent revert/lost update, not a
plaintext leak. The crate docs say "safe to share behind a reference"; that is Rust memory safety,
not transactional semantics, but a caller may read it as more.

**Fix:** document a single-writer requirement, or add a per-vault lock file / in-process mutex and
a generation check before rename (compare-and-swap). Add a concurrent put/rotate test.

#### V-3 — LOW — Filesystem hardening and durability residuals (symlink/reparse, temp collision, ACLs, directory fsync)

**Where:** `crates/vault/src/lib.rs:214-267` (no explicit ACL or reparse-point checks on root/
`secrets`/`vault.json`), `crates/vault/src/lib.rs:552-578` (`find_secret_file` skips non-regular
entries; good), `crates/vault/src/lib.rs:698-717` (`atomic_write`).

**Details:**
- `DirEntry::file_type().is_file()` is a non-following check in Rust (a file symlink is
  `is_symlink`, not `is_file`), so symlinked `.sealed` entries are skipped on list/get/delete. This
  is correct but untested here because the host lacks symlink privilege. A junction/symlink swap
  between the scan and `File::open` remains a same-user/directory-write TOCTOU; it is not a
  privilege boundary because DPAPI is user-bound.
- `atomic_write` uses a predictable `.tmp-<pid>-<counter>` name and, if `create_new` fails, removes
  that path unconditionally (`lib.rs:702-704`). A pre-existing file with the same name in the
  secrets directory would be deleted. Only the vault directory writer can plant it, so impact is
  low, but the cleanup should remove only the temp it actually created.
- No explicit DACL is set; confidentiality relies on DPAPI, not directory permissions. An
  attacker with write access can delete, roll back, or swap ciphertext (DPAPI cannot prevent that)
  and can read the non-secret metadata/aliases.
- `write_temp_file` syncs the file, but the directory entry is not flushed; a crash after rename
  may lose the update. No power-loss test exists.

**Impact:** rollback/deletion/metadata tampering by a same-user or vault-directory writer; a
predictable-temp collision can delete an unrelated file in that directory; crash durability is not
proven. No plaintext or cross-user exposure.

**Fix:** create the temp with a random name and do not delete on `create_new` failure; consider a
DACL check and a directory flush; add symlink/reparse and crash-consistency tests where possible.

#### V-4 — LOW / scope — No backup/recovery; P03's backup-recovery clause is explicitly not delivered

**Where:** `crates/vault/src/lib.rs:28-32` ("No backup/migration support ... explicit future
design, not a claim of this crate"), `crates/vault/src/lib.rs:33-35` (rotation cannot rotate OS
master keys).

**Impact:** a vault copied to another Windows user or machine normally cannot be decrypted; the
payload remains opaque. This is honest and secure (fail closed), but the ticket's definition of done
mentions backup recovery. It is a scope gap, not a vulnerability. The locked-vault UX is also only
an API (`lock`/`unlock`), not a UI.

**Fix:** future recovery design (credential-manager-backed export or explicit user-bound backup),
with a recovery drill; do not promise recovery in the meantime.

#### V-5 — LOW — Sentinel/test coverage is narrower than the claim

**Where:** `crates/vault/tests/vault.rs:295-315` (sentinel test), `crates/vault/tests/vault.rs:271-293`
(rotation happy path).

**Details:**
- The sentinel test scans all files under the vault directory (including the secrets temp location)
  for the raw sentinel, checks `Debug` for the payload and vault, serialized payload JSON, and one
  missing-secret error. It does not scan process logs (there is no logging subsystem), other error
  paths, or encoded leaks (a hypothetical hex/base64 plaintext write would evade a raw-byte scan).
- Rotation is tested only on success; no DPAPI failure injection, no interrupted rotation, no
  MAX_SECRETS boundary, no cross-user/machine test (not feasible in this session), and no symlink
  test.
- The non-Windows test is `#[cfg(not(windows))]`, so it did not run in this Windows validation.

**Impact:** the main properties are covered, but the failure/edge claims rest on code inspection
rather than tests. This is acceptable for a low-level OS wrapper if documented, but the review
should not overstate the sentinel coverage.

**Fix:** add a failing-backend rotate test if the DPAPI layer can be abstracted, a symlink test on a
host with privilege, and a MAX_SECRETS boundary test. Document that "no logs" means the crate has no
logging, not that logs were scanned.

#### V-6 — LOW / informational — Decryption errors expose the raw Windows error code

**Where:** `crates/vault/src/error.rs:116-120`, `crates/vault/src/dpapi_windows.rs:186-190`.

**Impact:** a local caller can distinguish DPAPI failure modes (bad data, wrong user, etc.) from the
`os_code`. No plaintext is returned. If the vault is ever exposed through a remote service, collapse
this to a unit error at that boundary.

---

### 2.3 Insertion (ANS-29)

The transaction never claims `Inserted` without `InsertionEvidence::Confirmed`, never writes the
clipboard after the initial copy, and defaults to `CopyOnly`. The automatic host is absent exactly as
the docs say (`crates/clipboard/src/insertion.rs:22-25`; `crates/platform-windows/src/focus/native.rs`
explicitly does not inject input). The remaining findings are LOW.

#### I-1 — LOW — The final clipboard sequence/owner check is not the last check before `send_paste`; `Unknown` ownership and sequence 0 are accepted

**Where:** `crates/clipboard/src/insertion.rs:446-465`: the sequence check is at 448, the owner check
at 451, then `observe_target` at 457 and a final verify at 461, then `send_paste` at 465.
`ClipboardOwner::Unknown` passes at 451 (only `Other` is rejected); `write_clipboard` returning 0 is
not rejected at 375-389, and `0 == 0` would satisfy the check at 448.

**Cause/impact:** a user copy that lands during the final `observe_target` is not overwritten (there
are no further writes), but the paste can insert the newer clipboard content rather than the
selected item. The docs say the sequence and owner must "still be ours immediately before input";
`Unknown` is not "ours", and the check is not immediately before input. A host that cannot
determine the sequence can return 0 and pass the equality check.

**Fix:** reject 0 and `Unknown` for automatic insertion; move the sequence/owner check to
immediately before `send_paste` (after the final target observation), or re-check after observation
and before input. Add fake-host tests for Unknown owner, zero sequence, and a copy after the final
check.

#### I-2 — LOW — `InsertionTarget.foreground` is never read

**Where:** `crates/clipboard/src/insertion.rs:88-89` (field), `crates/clipboard/src/insertion.rs:160-193`
(`verify` uses only `observed.foreground`).

**Impact:** a target that was background at summon can still be automatically focused and pasted
(if `allow_focus_request` is enabled). That may be intended, but the field is advertised as captured
state and has no policy effect. No test covers it.

**Fix:** either use it to deny/require confirmation for a background-at-summon target, or remove it
and document that only the fresh observation matters.

#### I-3 — LOW / scope — Secure-input, IME, permission and destination evidence are host-supplied; no Windows implementation or end-to-end test

**Where:** `crates/clipboard/src/insertion.rs:270-289` (trait), `insertion.rs:22-25` (docs).

**Impact:** the policy code is correct against a fake host, but on Windows there is no
`InsertionHost`, no `SendInput` adapter, no real foreground/secure-input/IME observation, and no
real destination evidence. `UIPI`/elevation, focus races and IME behavior are therefore unverified
by this crate. This is explicitly accepted by the brief ("automatic host is explicitly absent"), not
a defect of the policy layer.

**Fix (future ticket):** implement the host with `SetForegroundWindow` verification, integrity-level
handling, `SendInput` failure mapping and a destination evidence mechanism; add a real acceptance
test. Do not describe the automatic path as production-ready before that.

#### I-4 — LOW — Test coverage does not exercise owner `Unknown`, zero sequence, or a copy after the final check

**Where:** `crates/clipboard/src/insertion.rs:648-911`, `crates/clipboard/tests/public_api.rs:128-159`.

**Impact:** the branch matrix is well covered (copy-only, disabled switches, permission, focus,
IME/secure, target identity/generation, evidence, failures), but the three gaps above are exactly
the cases a real host is most likely to hit. LOW test-quality issue, not a code defect.

---

## 3. Does the claimed 73-test suite cover the risk?

| Risk | Evidence | Assessment |
| --- | --- | --- |
| DPAPI round trip, tamper, wrong context, rewritten tag, empty payload | `crates/vault/tests/vault.rs:56-115` | Good. Real DPAPI on Windows. A truncated hex case is `InvalidPayload`, not `DecryptionFailed`; accurate enough. |
| Cross-process/user-bound behavior | `crates/vault/tests/vault.rs:233-269` | Good for same-user cross-process. Cross-user/machine is not testable here; DPAPI semantics documented. |
| Lock gates every vault operation | `crates/vault/tests/vault.rs:119-154` | Good. |
| Alias traversal, device names, case replacement, delete | `crates/vault/tests/vault.rs:156-214`, `319-366` | Good. Symlink/reparse is not tested; the static `file_type` guard is correct. |
| Plaintext sentinel in vault files/Debug/serde/errors | `crates/vault/tests/vault.rs:295-315` | Good for raw bytes and the listed surfaces; no logs/other error paths/encoded-leak variants. |
| Rotation preserves values | `crates/vault/tests/vault.rs:271-293` | Happy path only; no failure injection or concurrent write. |
| Filter before persistence, no attribution bypass | `filter.rs:713-944`, `store.rs:1084-1138` | Good for hand-built snapshots; no adapter-level marker test. |
| Windows exclusion markers | `filter.rs:748-781` | Tests three markers only through `ExclusionMarkers`; **no test for `ExcludeClipboardContentFromMonitorProcessing`**, and the adapter's real marker reads are untested. |
| Sensitive heuristics | `filter.rs:862-913`, `store.rs:1084-1112` | Good fixtures for the covered forms; no JSON/quoted forms, no non-ASCII marker test. |
| Quota/age/pin/eviction determinism | `model.rs:808-1016`, `store.rs:1141-1375` | Good. |
| Lock/private/pause/cache wipe | `store.rs:1223-1300` | Good for the explicit paths; no drop-without-lock test. |
| Delete/clear and orphan cleanup | `store.rs:1303-1323`, `1471-1491` | Delete paths tested; the orphan test does not exercise a prefixed unrelated secret or a missing history. |
| Windows adapter retry/close/GlobalLock/HDROP | `windows.rs:449-457` only | **No automated live test**; the manual test is ignored. Static review found the FFI pairing correct, but this is unverified at runtime. |
| Insertion policy branches | `insertion.rs:648-911`, `public_api.rs:128-159` | Good branch coverage; no real host, no Unknown owner/zero sequence/late copy. |

The 73 clipboard tests are real and mostly meaningful; the claim should not be read as covering the
Windows marker set or live adapter behavior. The vault's 16 tests are stronger than the ticket
probably requires, except for failure injection.

---

## 4. Ticket verdicts

### ANS-26 / P03 vault — **Accepted at crate/API level, with residual risks**

No BLOCKER/HIGH/MEDIUM. The DPAPI FFI is correct (layout, `extern "system"`, `CRYPTPROTECT_UI_FORBIDDEN`,
optional entropy, `LocalFree` on all output paths, length checks, no leak on the limit path), the
non-Windows path is explicit `Unsupported` before filesystem writes, payload versions and context
binding are enforced, aliases/devices/case are handled, writes are temp+sync+rename, rotation
preserves data on success, Debug/error output is content-free, and lock is honestly documented as a
session gate. Residual risks: no backup/recovery (explicit scope gap), concurrent put/rotate is
last-writer-wins, no ACL/symlink/crash tests, rotate error-path plaintext is not wiped, and sentinel
coverage is narrower than a broad claim.

### ANS-27 / P04 clipboard capture — **Not acceptable as-is**

One HIGH (C-1: a documented Windows opt-out marker is ignored) and three MEDIUM (C-2 namespace-
destructive orphan cleanup, C-3 no cross-process lock, C-4 non-ASCII configured markers). The
encryption-before-persistence, metadata-only history, content filtering, quota/pin/age, lock/cache
wipe and DPAPI round trip are implemented and tested. After C-1 is fixed and C-2/C-3/C-4 are either
fixed or explicitly accepted with tests/documentation, the ticket can be accepted at crate level.
The real adapter still has no automated live test (ignored manual test).

### ANS-29 / P05 insertion — **Accepted at policy/API level, with residual risks**

The target identity/creation-time/generation/permission/fresh-focus checks, secure-input/IME gates,
clipboard sequence/owner checks, copy-first behavior, no-restore, evidence-only `Inserted` and
copy-only default are all present and well covered by fake-host tests. The automatic Windows host is
explicitly absent, so this is not end-to-end acceptance. Residual risks: the final sequence check is
not the last check before input, `ClipboardOwner::Unknown` and sequence 0 pass, `target.foreground`
is unused, and secure-input/IME/evidence remain host contracts.

---

## 5. Overall verdict and residual risks

**No BLOCKER.** The vault is a solid, honest DPAPI wrapper with no plaintext fallback and no
dangerous cipher. The clipboard storage path correctly filters before sealing, seals before writing
metadata, persists only metadata plus DPAPI ciphertext, and wipes the preview cache on lock. The
insertion policy is conservative and truthful about the missing automatic host.

**Unresolved security-relevant work before caller-facing release:**

1. **HIGH:** honor `ExcludeClipboardContentFromMonitorProcessing` (C-1).
2. **MEDIUM:** isolate the clipboard alias namespace and stop deleting unrecognized prefixed
   secrets; fix the missing-history destructive cleanup (C-2).
3. **MEDIUM:** add a cross-process guard or single-instance contract for capture (C-3).
4. **MEDIUM:** make configured sensitive markers Unicode-case-insensitive (C-4).
5. Confirm the app persists pause/private mode and `CapturePolicy` settings; otherwise a restart
   silently resumes capture with default exclusions (C-11).
6. Decide whether `MemorySealBackend` and `open_with_backend` should exist in release builds (C-12).
7. Add failure-injection tests for rotation and a live (or controlled) test for the Windows adapter;
   the current manual test is ignored.
8. Do not claim backup/recovery or automatic paste for these tickets yet; neither is delivered.

**Not verified here:** cross-user/cross-machine DPAPI failure, symlink/reparse behavior (host lacks
symlink privilege), physical power-loss durability, a real Windows clipboard listener or paste host,
and any desktop/UI integration. The workspace validation report (`docs/validation-2026-11-19.md`,
535 tests) predates and does not include these two crates; its count should not be cited as covering
them.

---

# Clipboard re-review after fixes

**Date:** 2026-11-19
**Scope:** re-audit of ANS-27/P04 after the fixes for C-1..C-14. No implementation code was changed by
this re-review. Only this appended section and a throwaway probe outside the repository were used.
This section supersedes the ANS-27 verdict in section 4 for the current working tree.

## 1. Commands and results

| Command | Result |
| --- | --- |
| `cargo test -p ansa-clipboard --offline` | **PASS.** Unit tests: **82 discovered, 81 passed, 1 ignored** (the manual real-clipboard test). `tests/public_api.rs`: **4 passed**, including a child-process lock test (the child invocation is an extra run of one of the four, not an additional test). Doc-tests: 1 passed. **87 discovered, 86 passed, 1 ignored.** |
| `cargo clippy -p ansa-clipboard --all-targets --offline -- -D warnings` | **PASS.** No warnings. |
| `cargo fmt --check -p ansa-clipboard` | **PASS.** No output. |

The clipboard crate now has 87 discovered tests including its doctest (82 unit + 4 integration +
1 doctest), up from the previous 73 unit/integration tests. The ignored test remains the manual
`windows::tests::real_clipboard_read_does_not_modify_it`; it was not run. The real Windows adapter
path (`RegisterClipboardFormatW`/`IsClipboardFormatAvailable`/`GetClipboardData`) is still covered
by unit decision tests and static review rather than a live automated test.

### Independent probes

The probe crate at `C:\Users\abhir\AppData\Local\Temp\opencode\ansa-review-probe` was updated to
exercise the public API and a custom failing `SealBackend`. Relevant output:

```text
marker exclude-only:            Reject(MonitorProcessingExcluded)
marker exclude+all-true:        Reject(MonitorProcessingExcluded)
marker exclude+history-false:   Reject(MonitorProcessingExcluded)
marker exclude+viewer:          Reject(ViewerIgnore)
marker exclude+cloud-false:     Reject(MonitorProcessingExcluded)
marker exclude+empty-content:   Reject(MonitorProcessingExcluded)
marker exclude+sensitive:       Reject(MonitorProcessingExcluded)
marker all-allowed:             Accept
marker cloud-false-captured-explicitly: Accept
unicode-marker "ПАРОЛЬ": true
unicode-marker "ПарОль": true
unicode-marker "пароль": true
json-form "{\"password\": \"hunter2-hunter2\"}": true
json-form "PASSWORD=\"hunter2-hunter2\"": true
saturation from_stored(MAX):     Corrupt("item identifier space is exhausted")
saturation insert(MAX-1):        Ok(18446744073709551614)
saturation insert(after MAX-1):  Err(IdExhausted)
legacy items=1
legacy decrypt=Err(ItemNotFound)
legacy old-alias-survives=true
legacy old-alias-after-clear=true
legacy backend-aliases=["ansa-clip-1"]
logical delete-report=true
logical ciphertext-remains=true
logical manifest-has-tombstone=true
logical clear-report=0
logical ciphertext-after-clear=true
crash-lock second-open-while-child-alive: Some(AlreadyLocked)
crash-lock reopen-after-kill: ok=true
```

## 2. Fix verification

### C-1 — Fixed: the monitor-processing marker is read and enforced

`windows.rs:232-239` now reads `ExcludeClipboardContentFromMonitorProcessing` with
`marker_is_present`; `filter.rs:39-42` models it; `filter.rs:486-488` rejects it before history,
cloud, format and content checks. `windows.rs:96-122` and `295-310` implement the boolean-marker
decision: only a present, readable DWORD value `1` is `Allowed`; `0`, every other value and an
unreadable/absent DWORD are `Excluded`. Tests at `windows.rs:538-564` cover `0`, `2`, `0x30` and an
unreadable value; `filter.rs:849-858` covers the presence marker. The probe confirms that the
exclusion wins over empty content, sensitive content, history/cloud opt-outs and an explicit
cloud-capture policy. `ExclusionMarkers::is_history_opt_out` also includes it.

Residual LOW: if `RegisterClipboardFormatW` fails (for example a full atom table),
`marker_is_present`/`boolean_marker` treat the format as absent; the boolean DWORD path is fail-closed
once the format is present, but registration/availability failure remains fail-open. The live
adapter test is still ignored.

### C-2 — Fixed for the requested destructive cases; one legacy-migration MEDIUM remains

The store now derives a 32-hex random `store_id` and writes aliases as
`ansa-clip-<store_id>-<id>` (`store.rs:86-88`, `471-513`, `1121-1123`). `manifest.json` records the
aliases owned by that store and is written **before** the sealed payload
(`store.rs:776-831`). Cleanup:
- runs only when metadata is present, history is non-empty and the manifest is present
  (`store.rs:1125-1161`);
- considers only aliases in this store's manifest;
- deletes only aliases that belong to the current store ID and are unreferenced by history;
- preserves non-clipboard and foreign aliases.

Tests at `store.rs:1852-1957` cover an unreferenced manifest orphan, an `ansa-clip-vault-note`
unrelated secret, a missing history and an empty history; all pass. The probe separately confirms
the legacy-format case below. The prior C-2 destructive behavior is fixed for a fresh store.

**Remaining M-1 — MEDIUM:** a store written by the **pre-namespace layout** (old
`ansa-clip-<id>` aliases, `history.json` without `store_id`, no `manifest.json`) opens successfully
but its existing items are no longer decryptable. `load_manifest` invents a fresh store ID when the
manifest is absent (`store.rs:515-519`), `resolve_store_id` returns that ID when metadata has no
`store_id` (`store.rs:1190-1210`), and `alias_for` now looks up
`ansa-clip-<new-id>-<item-id>` (`store.rs:1121-1123`). The old ciphertext is not deleted (cleanup is
suppressed without a manifest) and is not reachable by `decrypt_item`; `delete_item`/`clear_all` also
address only the new namespace. Probe evidence: `legacy items=1`,
`legacy decrypt=Err(ItemNotFound)`, `legacy old-alias-survives=true`,
`legacy old-alias-after-clear=true`.

Impact: silent loss of access to existing clipboard history after an upgrade; no plaintext exposure,
and the old ciphertext remains recoverable if a migration is added later. The `HistoryMetadata`
comment at `store.rs:1175-1179` ("the manifest supplies it") describes only the case where a
manifest exists; it does not handle the no-manifest legacy store.

Fix: on open, detect `metadata_present && history non-empty && manifest absent && metadata.store_id
absent` and either (a) migrate each old `ansa-clip-<id>` envelope under the new namespace with
`backend.open` + `backend.seal` + `backend.delete`, updating history/manifest atomically, or (b)
reject with a clear `MigrationRequired`/`CorruptHistory` error instead of showing undecryptable items.
Add a legacy-layout test. If the prior working-tree layout is explicitly test-only/unreleased, this
can be accepted as a documented migration limitation; otherwise it is a release blocker for upgrade.

### C-3 — Fixed: non-blocking cross-process lock, child rejection and crash release

`lock.rs:1-156` acquires `<store>/.lock` with `CreateFileW(..., share_mode = 0, OPEN_ALWAYS)` on
Windows. It does not retry or sleep; a second open gets `AlreadyLocked` for
`ERROR_SHARING_VIOLATION`/`ERROR_LOCK_VIOLATION`. The lock is held by `_lock` in
`ClipboardStore` (`store.rs:562`) and released by `StoreLock`'s `Drop` (`lock.rs:111-118`). The
public API child test (`public_api.rs:24-53`) launches the test binary in a child process and
asserts `AlreadyLocked`; it passes. The same-process double-open test at `store.rs:1988-2008` also
passes. The external crash probe held the store in a child, confirmed `AlreadyLocked` from the parent,
killed the child and reopened successfully (`crash-lock reopen-after-kill: ok=true`), so the Windows
OS releases the no-share handle on process termination. Capture is written seal → history with
manifest tombstones, and a second store cannot interleave; no committed-capture loss path was found
by inspection, but no crash-injection test was run.

Residual LOW: the non-Windows fallback is an exclusive-create file removed on drop; a crash leaves a
stale file and permanently blocks a later open (documented, and the crate is Windows-focused). The
lock is per store root, not vault-wide. No in-repo crash/kill test or physical power-loss test.

### C-4 — Fixed: Unicode lowercase marker matching

`is_sensitive_text` now builds `let mut lower = value.to_lowercase()` and wipes it
(`filter.rs:546-565`); `contains_sensitive_assignment` also wipes its candidate
(`filter.rs:668-688`). `CapturePolicy::normalized` already uses Unicode `to_lowercase` for markers
(`model.rs:360-370`). The unit test `filter.rs:959-977` covers uppercase, mixed-case and lowercase
Cyrillic; the probe confirms all three. The rustdoc explicitly says this is lowercase matching, not
full Unicode case folding (`filter.rs:556-558`), which matches the requested scope.

Residual LOW: combining-sequence/normalization and locale-specific special cases (`ß`/`SS`, Turkish
`İ`) are not folded; this is documented as lowercase, not full caseless matching.

### C-5..C-14 — Dispositions

| ID | Status | Remaining notes |
| --- | --- | --- |
| C-5 | **Fixed for JSON/quoted forms** | `contains_sensitive_assignment` now strips the key quote, the separator and the value quote (`filter.rs:634-696`); tests at `filter.rs:979-1001` and the probe cover JSON, quoted env and YAML-like forms. Luhn false positives remain and are now documented at `filter.rs:546-554`; other heuristic false negatives remain by design. |
| C-6 | **Fixed** | `PreviewCache` has `Drop` → `clear`/wipe (`store.rs:415-419`); `ClipboardStore` has `Drop` → clear previews and lock (`store.rs:1164-1169`). No automated Drop test; `preview()` still returns a caller-owned plaintext copy, documented. |
| C-7 | **Substantially fixed** | `SnapshotContent` has a wiping `Drop` (`filter.rs:190-202`); the text lowercase buffer and assignment candidate are wiped; the adapter wipes `Vec<u16>` units/buffers (`windows.rs:374-376`, `428-430`). Residual LOW: `is_placeholder` and `looks_like_credential_path` still allocate transient lowercase copies; `decode_payload`'s partially built file-list paths are dropped unwiped on an error; `ClipboardContent::preview`'s intermediate file-list `joined` is dropped unwiped. `content.rs:1-9` now says "best-effort" and no longer claims exclusivity. |
| C-8 | **Fixed** | `BooleanMarkerDecision` accepts only a present, readable `1`; `0`, nonstandard values and unreadable data are `Excluded` (`windows.rs:96-122`, `295-310`; test `538-564`). Residual LOW: registration/availability failure is treated as "absent" (fail-open) in `marker_is_present`/`boolean_marker`; no live adapter test. |
| C-9 | **Fixed** | `MAX_HDROP_UNITS` (512K units = 1 MiB) plus `hdrop_budget_allows` caps the aggregate (`windows.rs:66-70`, `124-128`, `380-433`; test `566-572`). Residual LOW: the adapter can still read up to 1 MiB before the default 128 KiB policy rejects it; bounded and no persistence. |
| C-10 | **Claim honest; behavior verified** | `delete_item`/`clear_all` commit metadata first and keep manifest tombstones when ciphertext deletion fails (`store.rs:955-1007`, docs `53-55`). The probe with a failing backend shows `delete_item = true`, ciphertext remains, manifest tombstone present; `clear_all` attempts the sweep and leaves the tombstone. Residual LOW: `persist_manifest` errors are ignored, so a tombstone is not guaranteed durable on a manifest write failure; no in-repo failure-injection test; `clear_all`'s return value is the history-item count, not the manifest sweep count. |
| C-11 | **Honest** | Pause, private mode and the capture policy are explicitly session/caller state (`lib.rs:34-35`, `store.rs:43-46`, `model.rs:456-462`, `511-553`); `from_stored` resets pause/private, and the host must re-apply settings. Residual LOW: a restarted host that calls `capture` with defaults will not have the user's pause/private/exclusion settings unless it restores them; that is now a documented host contract. |
| C-12 | **Mitigated, not enforced** | `MemorySealBackend` and `open_with_backend` are `#[doc(hidden)]` (`store.rs:167-176`, `596-601`, `lib.rs:98-99`); the lib docs call it a hidden test double never selected by `ClipboardStore::open` (`lib.rs:25-28`). Residual LOW: both remain public API and a deliberately miswired embedder can still supply a plaintext backend; no production consumer does so. |
| C-13 | **Honest** | `lib.rs:31-33` explicitly says capture is pull-only, with no `WM_CLIPBOARDUPDATE` listener, message loop or self-copy suppression; a host must call `read_clipboard_snapshot` then `capture`. Residual scope: no end-to-end listener and a future listener/insertion integration must add self-copy suppression. |
| C-14 | **Fixed** | `from_stored` rejects `next_id == u64::MAX` (`model.rs:446-453`), `insert` returns `IdExhausted` at `u64::MAX` (`model.rs:566-569`), and `reset_next_id` allows recovery from a manifest max but still fails insertion (`model.rs:511-518`). Unit test `model.rs:980-992` and the probe both pass; no duplicate-ID reuse. |

## 3. Remaining severity

- **BLOCKER:** none.
- **HIGH:** none.
- **MEDIUM:** **M-1**, legacy pre-namespace store migration (existing items become undecryptable
  and are not cleaned by normal operations).
- **LOW:** the residuals listed above (marker registration/availability fail-open, manifest tombstone
  durability, transient scratch copies, non-Windows stale lock, Luhn false positives, logical-delete
  test gap, public doc-hidden test backend, pull-only scope, no live adapter test).

## 4. ANS-27 verdict after fixes

**Acceptable for a fresh store with residual risks; conditionally acceptable as a release.**

The requested C-1 fix is present and fail-closed for the boolean markers; C-2's destructive cleanup
cases are fixed and tested; C-3's non-blocking lock, child rejection and crash release are verified;
C-4's Unicode lowercase matching is fixed and tested; C-5's JSON/quoted false negatives and C-14 are
fixed; C-6/C-7 are substantially fixed; C-8/C-9 are fixed; C-10/C-11/C-13 claims are now honest;
C-12 is minimized to a hidden, still-public test double. There is **no remaining BLOCKER/HIGH**.

One **MEDIUM (M-1)** remains if existing working-tree clipboard history from the prior
`ansa-clip-<id>` layout must survive. If that layout is test-only/unreleased (as it is in this
working tree), a fresh-store declaration plus an explicit "no migration" limitation could accept M-1
as a residual. Otherwise, ANS-27 is **not acceptable as a drop-in upgrade** until the legacy aliases
are migrated or the store refuses to open with a clear migration error.

**Not verified:** a live automated Windows clipboard read, physical power-loss/atomic-write testing,
a real pre-fix store produced by the old binary (the probe reconstructed the documented old layout),
and any desktop/UI consumer. The child-process and crash-lock probes are external to the repository;
the repository does contain the child-process integration test.

## 5. M-1 migration delta (2026-11-20)

**Status: M-1 is closed for the normal pre-namespace layout; ANS-27 is acceptable for fresh and
legacy stores with LOW residual risks.**

- **Detection:** `store.rs:629-635` treats history as legacy only when metadata exists and has no
  `store_id`; legacy history plus any namespaced `manifest.json` fails `LegacyLayout` before writes
  (`630-634`). Invalid or mismatched `store_id` still fails `CorruptHistory`.
- **Order:** `migrate_legacy` (`store.rs:1187-1256`) preflight-opens and format-checks every old
  alias (no writes on failure), writes every new alias, persists history, attempts the manifest, and
  only then deletes the old `ansa-clip-<id>` aliases. A rewrite or history failure deletes the
  newly written aliases; old history and old aliases stay intact. A manifest-write failure is
  recoverable because a later open rebuilds ownership from the non-empty history (`store.rs:674-686`).
  Thus normal error-return paths cannot present unusable items, and old ciphertext is deleted only
  after the new aliases and history exist.
- **Fail-closed:** missing/unreadable/undecodable/format-mismatched legacy payloads, legacy plus
  manifest, corrupt manifest, invalid store ID and store-ID mismatch all fail without migrating.
- **Cleanup:** `delete_item` (`store.rs:1014-1021`) and `clear_all` (`1035-1056`) remove both new and
  legacy aliases; eviction paths also delete legacy aliases (`663-669`, `869`, `978`, `1114`).
- **Tests:** all four migration tests pass: `legacy_store_is_migrated_and_legacy_aliases_are_cleaned`
  (two items, decrypt, old aliases gone, history store ID/manifest, delete/clear leftovers),
  `corrupt_legacy_store_refuses_without_modifying_the_old_store` (LegacyLayout, history bytes
  unchanged, no manifest/new aliases, then repair and migrate),
  `legacy_store_with_an_unexpected_manifest_is_refused`, and
  `vault_legacy_store_migrates_with_real_dpapi` (real DPAPI old alias → new alias, decrypt, old gone).
  Refusal is exercised for missing/unreadable legacy payloads and unexpected manifests; the
  readable-but-malformed/format-mismatch branch and eviction legacy cleanup are code-only (test gaps).
- **Commands:** `cargo test -p ansa-clipboard --offline`: 86 unit (85 passed, 1 ignored), 4
  `public_api`, 1 doctest — **91 discovered, 90 passed, 1 ignored**. The migration filter ran 4/4
  passing, including the real-DPAPI test. Clippy with `-D warnings` and `cargo fmt --check` pass.
- **Residual LOW:** no directory fsync/power-loss test, so the logical write/commit/delete order is
  not proven durable across power loss; a history with a `store_id` but missing new aliases
  (restored/mismatched) can open with items that fail to decrypt because the manifest-rebuild path
  does not verify alias existence; old-alias deletion failures during migration are not tracked
  (leak until item delete/clear/eviction); the `usable=false` refusal branch and eviction legacy
  cleanup lack dedicated tests.

**ANS-27 verdict:** acceptable for fresh and normal legacy stores; no BLOCKER/HIGH/MEDIUM remains
for M-1, with the LOW residuals above.

# ANS-11 / F07 — native hotkey adapters and conflict lifecycle

Date: 2026-11-18
Ticket: ANS-11 `[F07] Implement native hotkey adapters and conflict lifecycle`
Branch: `ans-foundation-evidence`
Status: implementation and automated Windows evidence complete; **F07 remains open**.
No macOS/Linux adapter exists, no automatic sleep/lock/session observer is wired, and
the desktop has no rebinding/recorder UI yet. Do not read this document as a claim
that F07 is closed.

## Scope of this change

Files owned and changed:

- `crates/platform-api/src/hotkey.rs` (new: neutral model, persistence, manager,
  recorder, Windows backend glue)
- `crates/platform-api/src/lib.rs` (module/re-exports, capability notes and test
  expectation)
- `crates/platform-windows/src/hotkey.rs` (typed chord grammar, typed errors,
  arbitrary-chord worker, suspend/resume/rearm, probe/capability, tests)
- `crates/platform-windows/src/lib.rs` (additive exports and capability notes only)
- `apps/cli/src/main.rs` (`ansa hotkey validate|status|list|fallback`, help and tests)
- this document

No new crates were added. No root workspace file, `crates/ui-gpui/**`,
`crates/ui-components/**`, `crates/core/**`, `crates/host/**`, or another agent's
document was edited. `crates/platform-windows/src/lib.rs` also contains another
agent's concurrent, additive `lifecycle` module/export; those lines were preserved.

## Implemented contracts

### 1. Platform-neutral chord model

`ansa_platform_api::KeyChord` (alias `Hotkey`) is a modifier set plus exactly one
non-modifier key:

- modifiers: Control, Alt, Shift, Meta/Super;
- keys: `a`–`z`, `0`–`9`, `f1`–`f24`, Space, Enter, Tab, Escape, Backspace, Delete,
  Insert, Home, End, PageUp, PageDown, arrows, and the supported punctuation
  `- = [ ] \ ; ' , . / \``;
- stable persisted form: lowercase modifiers in the fixed order
  `ctrl+alt+shift+meta`, then one key token, joined by `+`;
- native rendering:
  - Windows `Ctrl+Alt+Shift+Win`
  - macOS `Control+Option+Shift+Command`
  - Linux `Ctrl+Alt+Shift+Super`
- typed `ChordParseError` for empty input, empty `+` tokens, modifier-only chords,
  duplicate modifiers, multiple keys and unsupported tokens.

The Windows adapter has a separate copy of the grammar because the dependency
direction is API -> Windows. A parity test parses every supported token with both
implementations and compares persisted and native output.

### 2. Typed outcomes and capability reporting

`RegistrationOutcome` (alias `HotkeyStatus`) has `Registered`, `Conflict { chord,
detail }`, `Denied { chord, detail }`, `Unsupported { chord, fallback }` and
`Degraded { chord, reason, alternative }`. `is_registered` is true only for
registered/degraded outcomes; `capability_status()` maps to the host
`CapabilityStatus` vocabulary.

`HostCapabilities::global_hotkey` is no longer a constant on Windows. Each
`host_capabilities()` call performs a transient `RegisterHotKey` probe of the
development-default `Ctrl+Space` under the process-global mutex:

- free -> `Available`;
- owned by the IME/another application/this process -> `Degraded` with the reason
  and a rebind/desktop-shortcut alternative;
- access denied -> `Degraded` with the denial detail (not a silent success);
- API unusable/invalid -> `Unsupported`.

On non-Windows, `global_hotkey` remains `Unsupported` and the notes explicitly say
no adapter exists and that a desktop shortcut/direct launch is the fallback. The
probe is real, but it is still a point-in-time observation; callers must not cache
it indefinitely.

### 3. Persistence model

`HotkeyConfig` is a versioned text model:

```text
ansa-hotkeys 1
summon ctrl+space
```

- one binding per line (`<action> <chord>`), blank/comment (`#`) lines ignored;
- `summon` is required; `command:<id>` style actions are allowed by the action-name
  grammar for the future per-command binding work;
- validation rejects unsupported versions, duplicate actions, malformed lines,
  invalid actions, missing summon and unparsable chords with typed errors and
  1-based line numbers;
- unknown key tokens (for example `ctrl+hyper`) produce `InvalidChord` wrapping the
  typed chord error; malformed input never panics.

`HotkeyStore` takes an injected path (no location policy in the domain layer):

- `save` validates first, writes a same-directory temp file, `sync_all`s it, copies
  the verified content to `<name>.bak`, then atomically renames the temp file over
  the main file;
- `load` never writes. It tries the main file, then the backup, then the documented
  default `ctrl+space`; every fallback returns a `HotkeyRecovery` record explaining
  why and whether the backup was used;
- an invalid/missing/stale main file is therefore not overwritten by a load, and
  the most recent verified binding remains recoverable from the backup.

The desktop does not yet call this store at startup; see “Persistence/restart”.

### 4. Manager state machine

`HotkeyManager<B: HotkeyBackend>` implements:

- `activate` — register the configured summon chord;
- `check_conflict` — non-destructive probe before saving;
- `rebind` / `rebind_and_save` — probe first, keep the previous active binding on
  conflict/denial/unsupported, and restore it if a post-release registration fails;
- persistence only after successful registration; if the save fails, the runtime
  binding is rolled back to the previous working chord and the config restored;
- `suspend` / `resume` — release and re-register exactly once; a rebind while
  suspended persists and waits for `resume` instead of silently re-acquiring the
  shortcut;
- `rearm` — unregister + register once as the explicit sleep/unlock recovery hook;
  if re-arm loses the chord, the manager exposes the suspended state so `resume`
  can retry the last working binding instead of reporting a false success;
- `shutdown` — release once and remain stopped; repeated shutdown returns
  `AlreadyStopped` rather than an error or a silent success;
- typed errors for not-active/already-active/already-suspended/suspended/shutdown,
  backend, conflict, denial, unsupported and persistence failures.

`HotkeyBackend` is injectable; tests use `FakeBackend`, which records duplicate
registration attempts. The real adapter is available as `WindowsHotkeyBackend`
(Windows builds only) and converts through the persisted grammar.

### 5. Shortcut recorder

`ShortcutRecorder` is UI-independent:

- consumes key-down/up events (`RecorderEvent`/`RecorderKey`);
- modifier-only presses never produce a chord;
- displays native modifier names (`modifier_display`) and the native candidate;
- state is `Idle`/`Capturing`/`Candidate`/`Verified`;
- `verify(ConflictCheck)` refuses conflict/denied/unsupported and keeps the
  candidate retryable;
- `commit()` only succeeds from `Verified`, so an unverified candidate cannot be
  saved through the recorder contract;
- `cancel()` returns the abandoned candidate/verified chord and resets.

This wave deliberately does **not** add a GPUI recorder/rebinding view.

### 6. Windows adapter

- `HotkeyRegistration::register(callback)` is unchanged for existing callers and
  registers `Ctrl+Space`.
- `HotkeyRegistration::register_chord(chord, callback)` registers any validated
  chord with its VK + `MOD_*` + `MOD_NOREPEAT`.
- registration errors are typed: 1409 -> `Conflict`, 87 -> `InvalidChord`, 5 ->
  `Denied`, other worker failures -> `Worker`; every message renders the chord.
- the worker handshake is bounded (5 s) and two-phase (thread id, then registration
  result). A failure joins the worker, so no thread is left running.
- `Drop` posts `WM_QUIT`, joins the worker and releases the OS registration before
  returning.
- process-global registration, probing and lifecycle commands are serialized by a
  mutex; a process-global active record prevents the capability probe from
  misreading our own binding as an external conflict.
- `suspend`, `resume`, `rearm` are implemented on the same worker and verified by
  real-worker tests.
- **Automatic** sleep/lock/session handling is not implemented. No message-only
  window, `WM_POWERBROADCAST` handler or `WTSRegisterSessionNotification` call is
  created. `rearm()` is the explicit hook; the capability notes disclose this.

### 7. CLI fallback surface

`ansa-cli` now supports, without changing its default search/apps/calc/open
behaviour:

```text
ansa hotkey validate <chord>   parse/display, never register
ansa hotkey status             real capability probe
ansa hotkey list               built-in binding and supported grammar
ansa hotkey fallback           desktop-shortcut/direct-launch fallback
```

The fallback message never simulates input and never requests elevation. The CLI
does not read/write the desktop's persisted binding yet; it says so explicitly.

## Verification commands and results

Run on Windows 11, Rust 1.99.1, branch `ans-foundation-evidence`.

| Command | Result |
| --- | --- |
| `cargo fmt -p ansa-platform-api -p ansa-platform-windows -p ansa-cli -- --check` | exit 0 |
| `cargo test -p ansa-platform-api` | 28 passed, 0 failed, 0 ignored |
| `cargo test -p ansa-platform-windows` | 60 passed, 0 failed, 0 ignored |
| `cargo test -p ansa-cli` | 6 passed, 0 failed, 0 ignored |
| `cargo clippy -p ansa-platform-api -p ansa-platform-windows -p ansa-cli --all-targets -- -D warnings` | exit 0 |
| `cargo run -p ansa-cli -- hotkey validate ctrl+space` | exit 0; `valid: ctrl+space`, `native: Ctrl+Space`, `registration: not attempted (validation only)` |
| `cargo run -p ansa-cli -- hotkey status` | exit 0; `capability: degraded` because Ctrl+Space is owned in this session (exact detail below) |
| `cargo test -p ansa-platform-windows real_worker_survives_1000_register_unregister_cycles -- --nocapture` | passed; 1,000 real cycles on the fallback chord (see below) |

Observed capability probe in this environment:

```text
capability: degraded
  the default Ctrl+Space summon chord is unavailable: the key combination is already registered by another application, the IME or this process
  alternative: Rebind the summon shortcut, use a desktop shortcut, or launch ansa-desktop directly.
```

This is the honest degraded state, not a claim that global hotkeys are available on
`Ctrl+Space`.

## 1,000-cycle results

### Manager suspend/resume (fake adapter)

`hotkey::tests::suspend_and_resume_do_not_duplicate_registrations_across_1000_cycles`
runs 1,000 suspend/resume pairs. It asserts exactly one active registration after
each resume, no duplicate registration, `AlreadySuspended`/`AlreadyActive` on
repeated calls, and that the fake backend never observed a duplicate.

### Real Windows worker register/unregister

`Ctrl+Space` is owned in this environment. The test first tries it, then reports and
uses `Ctrl+Alt+Shift+F24`:

```text
Ctrl+Space is unavailable; using Ctrl+Alt+Shift+F24 for the real-worker hotkey test
completed 1000 register/unregister cycles with the real worker on Ctrl+Alt+Shift+F24
```

The test fails (not skips) if a cycle after the initial probe fails. It skips with
the exact probe error only when all candidate chords are owned.

## Conflict matrix

| Scenario | Automated? | Result |
| --- | --- | --- |
| Same-process duplicate registration | Yes | Typed `Conflict`; second registration fails while the first holds a free chord; after drop, re-registration succeeds |
| Another process/IME owns Ctrl+Space | Partly | Observed in this session: capability probe reports `Degraded` and the real-worker test falls back to F24. A controlled second external app is not scripted (manual) |
| Invalid chord | Yes | Parser tests reject empty/modifier-only/duplicate/multi-key/unsupported tokens; Win32 error 87 maps to `InvalidChord` in `win32_errors_map_to_distinct_typed_variants` |
| Access denied | Yes (mapping) | Error 5 maps to `Denied`; a real denied session was not forced |
| Shutdown cleanup | Yes | `Drop` joins and a subsequent registration succeeds; manager shutdown is idempotent |
| Suspend/resume/rearm | Yes | Real worker test exercises suspend, repeated suspend, resume, repeated resume and rearm on one worker |
| Sleep/lock/wake/session change | No | Automatic observer not implemented; explicit `rearm()` is the only supported path |
| Win-key or IME-reserved combinations | No | Manual only; the OS may reserve combinations even when `RegisterHotKey` accepts them |

## Persistence/restart note

The versioned store, atomic save, backup recovery and validation are tested in
`ansa-platform-api`. A rebind is only persisted after the backend confirms
registration, and a save failure rolls the runtime binding back. However, the
desktop startup path still registers the fixed `Ctrl+Space` default through
`HotkeyRegistration::register`; it does not yet load `HotkeyStore` or call
`HotkeyManager::activate`. Therefore “rebindings persist after restart” is
implemented and tested at the contract level but **not wired end-to-end in the
desktop**. The GPUI rebinding UI is also absent.

## Unverified — do not claim these pass

- A controlled real-machine conflict matrix with a second external application
  owning a chord (only the observed Ctrl+Space/IME conflict and the probe are
  documented).
- Automatic re-registration after sleep, lock, unlock, fast user switching or
  session change. Explicit `suspend`/`resume`/`rearm` works; automatic notification
  is not wired.
- macOS and Linux registration adapters. Both are reported unsupported; no native
  code exists.
- The GPUI shortcut recorder/rebinding view and end-to-end desktop persistence.
- Per-command shortcut routing through the command/permission pipeline (F07 scope,
  not this wave).
- Physical keyboard-layout variation for punctuation virtual keys (the adapter maps
  the documented US punctuation VKs; non-US layouts were not exercised).
- Win-key combinations reserved by the OS, secure-desktop behavior, and any
  elevation path. The implementation neither simulates input nor elevates.
- A real access-denied registration. Only the error-5 mapping is tested.

Because automatic sleep/lock handling and the rebinding UI are explicit acceptance
criteria, ANS-11/F07 is **not** marked done or closed here.

# ANS-9 / F04 — GPUI host lifecycle proof

Date: 2026-11-18
Ticket: ANS-9 `[F04] Prove GPUI host slice: window, monitors, theme, shutdown`
Branch: `ans-foundation-evidence`
Status: implementation and automated evidence complete for this Windows machine;
**F04 is not closed** because the real-hardware/lifecycle items in the list at the
end cannot be exercised here.

## Scope of this change

Files owned and changed:

- `crates/host/**` (new implementation of the existing empty `ansa-host` crate)
- `crates/platform-windows/src/lifecycle.rs` (new; all new Win32 unsafe is here)
- `crates/platform-windows/src/lib.rs` (module and export lines only)
- `crates/platform-windows/Cargo.toml` (`ansa-host` path dependency)
- `crates/ui-gpui/src/host_lifecycle.rs` (new)
- `crates/ui-gpui/src/lib.rs` (host/lifecycle integration only)
- `crates/ui-gpui/Cargo.toml` (`ansa-host` dependency)
- `scripts/lifecycle.ps1` (new; optional wrapper for the native smoke)
- this document

No root workspace file was edited by hand. No new external dependency was
added. `ansa-host` keeps `#![forbid(unsafe_code)]`; all new unsafe is confined
to `platform-windows/src/lifecycle.rs`. Existing `--smoke-test`,
`--interaction-smoke-test`, `--a11y-fixture` and the 30-stage smoke still pass.

## Implemented contract

### 1. Invocation identity

- `InvocationId` is opaque (private nonce + monotonic sequence). `Debug` prints
  `InvocationId(..)`; `Display` prints only `invocation#<n>`. It can only be
  created by `InvocationIssuer`, which `HostController` owns.
- Each successful show issues a new id. A dismiss/focus request carrying an
  older id returns `HostError::StaleInvocation` before any native effect runs.
  The injection test `an_old_invocation_cannot_dismiss_the_current_window`
  proves the newer window stays visible.
- Ids are unique across issuers in one process (nonce plus per-issuer counter),
  and across 10,000 issues in the uniqueness test.

### 2. Focus outcomes

`FocusOutcome` has exactly `Succeeded`, `Denied { reason }`,
`Skipped { reason }` and `Unsupported { reason }`. It provides `label()`,
`Display`, `reason()` and `is_success()` (true only for `Succeeded`).
`ShowOutcome { visible, focus }` keeps visibility separate, so a visible window
with denied focus is represented honestly. Every controller show records one
outcome in `ShowReport`; the GPUI status line reports Denied/Skipped/Unsupported
instead of claiming activation.

### 3. Monitor model and placement

- `MonitorId`, `MonitorInfo { id, work_area (physical px), scale_factor, primary }`,
  `Point`, `Size`, `Rect`.
- `select_monitor` priority: pointer monitor, current-window intersection,
  primary, remembered monitor only when the platform reports no primary, then
  first. A removed monitor is never returned.
- `place_on_monitor` centers the requested physical size in the work area and
  clamps it fully inside. `Size::scaled` converts logical to physical at
  100/150/200%. Tests cover tiny work areas, zero size and negative origins.
- `HostController::reapply_placement` re-places after a DPI or size change
  without showing, focusing or issuing a new invocation.

### 4. Host backend and state machine

`HostBackend` exposes `list_monitors`, `show`, `place`, `hide`,
`request_focus`, `observe_theme` and `release`. `HostController` states are
`Hidden`, `Showing`, `Visible`, `Hiding`, `ShuttingDown`; it keeps one active
invocation, idempotent show/hide, monitor-removal re-placement, appearance
observations and a fixed shutdown plan (`Hotkeys` → `NativeHandles` →
`WorkerChannels`). `shutdown()` runs the plan exactly once and is idempotent;
show/hide after shutdown return `HostError::ShutDown`. Dropping the controller
runs the same idempotent shutdown as a resource-leak backstop.

### 5. Main-thread dispatcher

`MainThreadDispatcher` issues a checked `MainThreadToken` and records accepted
`MainThreadCall`s. Dispatch from a non-owner thread returns
`DispatchError::WrongThread`; a token from another dispatcher returns
`ForeignToken`; nothing is recorded on refusal. `MainThreadOnly<T>` contains an
`Rc` marker, so it is `!Send`/`!Sync`; binding it checks the token's thread.
The real launcher HWND is stored in `MainThreadOnly`, so it cannot be moved into
async work at compile time.

### 6. Windows adapter

- `enumerate_monitors` uses `EnumDisplayMonitors` + `GetMonitorInfoW`
  (`MONITORINFOEXW`) and reports physical work areas. Ids are device name plus
  monitor rectangle. DPI comes from `GetDpiForMonitor` loaded dynamically from
  `shcore.dll`, with a 96 DPI (1.0) fallback. A real test enumerates the
  machine's monitors and asserts non-empty work areas inside monitor bounds.
- `read_apps_theme` reads
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`
  with `RegGetValueW`. Missing, inaccessible or unexpected values return
  `ThemeError::Unavailable`; the adapter never guesses. The test reads but does
  not mutate the registry.
- `place_launcher_window` validates a live, same-thread, same-process HWND,
  requires the rectangle inside the intended work area, calls `SetWindowPos`
  with `SWP_NOZORDER | SWP_NOACTIVATE`, then verifies with `GetWindowRect`.
- `show_launcher_checked` reports `visible` separately from focus. Wrong-thread
  calls on an owned window return `visible: false` with `Skipped`; stale and
  foreign-process handles are rejected before any state change; Win32
  foreground denial maps to `Denied`. `hide_launcher` verifies the window is no
  longer visible. The existing `set_launcher_visible` API is unchanged and
  remains available to callers.
- `LauncherBackend` implements `HostBackend` for the real HWND, owns the hotkey
  registration and summon sender for actual shutdown release, and uses the
  main-thread dispatcher check before native calls.

### 7. GPUI integration

- Startup adopts the window GPUI already created and observes the system theme
  (or reports it unavailable in the status line). Deterministic debug
  fixture/smoke modes keep the CLI/default theme so the existing 30-stage
  interaction smoke does not depend on the machine's current theme; the normal
  launcher observes the system value.
- Summon selects the pointer/current/primary monitor, places the window, then
  shows, using `HostController::show`. Dismiss is checked against the current
  invocation; the checked focus-restore path runs through `hide_with`.
- A low-frequency (2 second) watcher observes theme changes and monitor removal.
  This is a poll because GPUI 0.2.2 exposes no display-change/theme-change
  callback; it is documented as polling, not a native event subscription.
- Ctrl+Q calls `shutdown_host()` before quitting. Window close/app quit drops
  the launcher root view, and its `Drop` plus the controller's `Drop` backstop
  run the same idempotent plan; this avoids depending on a GPUI version-specific
  close callback. Dropping the hotkey joins its worker, then the summon sender
  is dropped so the receiver task can exit. No global singleton blocks a second
  process run.

### 8. Native lifecycle smoke

Debug-only flags:

- `--lifecycle-smoke-test`
- `--lifecycle-cycles=N` (default 1000, clamped to 1..=2000)

The smoke opens the real launcher window, runs N show/hide cycles through the
real monitor/placement/show/hide path, checks the HWND is still live after every
show and hide, checks the final controller state is `Hidden`, prints focus
counts, prints exactly `ANSA_LIFECYCLE_OK cycles=<n>`, and quits. It does not
register the global hotkey and does not launch external applications. A failed
cycle prints `ANSA_LIFECYCLE_FAILED ...` and returns a nonzero exit code.

## Verification commands and actual results

Run on Windows 11, Rust 1.99.1, branch `ans-foundation-evidence`.

| Command | Result |
| --- | --- |
| `cargo test -p ansa-host` | 44 passed, 0 failed |
| `cargo test -p ansa-platform-windows` | 61 passed, 0 failed |
| `cargo check -p ansa-ui-gpui` | exit 0 |
| `cargo clippy -p ansa-host -p ansa-platform-windows -p ansa-ui-gpui --all-targets -- -D warnings` | exit 0 |
| `cargo test --workspace` | 248 passed, 0 failed, 1 ignored doctest |
| `cargo fmt -p ansa-host -- --check` | exit 0 |
| `cargo fmt -p ansa-platform-windows -- --check` | exit 0 |
| `cargo fmt -p ansa-ui-gpui -- --check` | exit 0 |

Focused native tests worth reviewing:

- `lifecycle::tests::enumerates_real_monitors_with_nonempty_work_areas_inside_bounds`
- `lifecycle::tests::placement_verifies_the_window_is_inside_the_work_area`
- `lifecycle::tests::placement_rejects_stale_handles_and_the_wrong_thread`
- `lifecycle::tests::a_live_window_owned_by_another_process_is_rejected_before_any_change`
- `lifecycle::tests::checked_show_reports_visibility_and_focus_separately_then_hides`
- `controller::tests::one_thousand_cycles_issue_unique_invocations_and_end_hidden`
- `controller::tests::an_old_invocation_cannot_dismiss_the_current_window`
- `controller::tests::shutdown_releases_in_order_exactly_once_and_blocks_actions`
- `dispatcher::tests::a_different_thread_is_refused_without_recording`

## Native smoke result

The exact requested command initially could not link because an unrelated
`ansa-desktop.exe` (PID 9944, started before this change) held
`target\debug\ansa-desktop.exe`:

```text
error: failed to remove file `C:\Users\abhir\ansa\target\debug\ansa-desktop.exe`
Caused by: Access is denied. (os error 5)
```

The same command was then run against a separate target directory, with
identical arguments and the same source:

```text
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\Temp\ansa-ans9-target"
cargo run -p ansa-desktop -- --lifecycle-smoke-test --lifecycle-cycles=1000
```

Actual output:

```text
ANSA_LIFECYCLE_FOCUS succeeded=0 denied=1000 skipped=0 unsupported=0
ANSA_LIFECYCLE_OK cycles=1000
```

Exit code 0. A 10-cycle dry run produced the same shape:
`succeeded=0 denied=10 skipped=0 unsupported=0`.

Focus outcome distribution: **Succeeded 0, Denied 1000, Skipped 0,
Unsupported 0.** Windows denied foreground activation for this process launched
from the automation shell. The smoke still proved that the window became
visible and the HWND stayed live on every cycle, and that the final state was
hidden. This is an honest observation, not a claim that activation always
fails; the controller test covers the Succeeded classification and the
checked-show test accepts either an observed `Succeeded` or `Denied` on the
owning thread. No input was injected, `AttachThreadInput` was not called, and no
elevation was requested.

Existing smoke paths were re-run after the integration:

| Command | Result |
| --- | --- |
| `cargo run -q -p ansa-desktop -- --smoke-test` | exit 0 |
| `cargo run -q -p ansa-desktop -- --interaction-smoke-test` | 30 stages passed, exit 0 |
| `ansa-desktop.exe --a11y-fixture` | printed `ANSA_UIA_READY:<hwnd>`; process stopped after capture |

Independent rerun after stopping the stale process, in the normal target
directory and with the exact requested command:

```text
$ cargo run -q -p ansa-desktop -- --lifecycle-smoke-test --lifecycle-cycles=1000
ANSA_LIFECYCLE_FOCUS succeeded=0 denied=1000 skipped=0 unsupported=0
ANSA_LIFECYCLE_OK cycles=1000
exit 0
```

The earlier binary-lock failure was environmental (a leftover process from a
prior manual run), not a code failure. After the later navigation integration
the interaction smoke reports 46 stages; that count belongs to the ANS-8 change,
not this ticket.

A later rerun from the same automation session, after the workspace had been
rebuilt, observed real foreground activation rather than denial:

```text
$ cargo run -q -p ansa-desktop -- --lifecycle-smoke-test --lifecycle-cycles=1000
ANSA_LIFECYCLE_FOCUS succeeded=1000 denied=0 skipped=0 unsupported=0
ANSA_LIFECYCLE_OK cycles=1000
exit 0
```

This is the first native `Succeeded` observation on this machine. The earlier
`denied=1000` run remains recorded because it demonstrates the honest-denial
path; neither run proves focus succeeds on every desktop/foreground policy.

## Public API summary (additive)

`ansa_host`:

- `InvocationId`, `InvocationIssuer`
- `FocusOutcome`, `ShowOutcome`, `FocusOutcomeCounts`
- `Point`, `Size`, `Rect`
- `MonitorId`, `MonitorInfo`, `MonitorSelection`,
  `MonitorSelectionReason`, `select_monitor`, `place_on_monitor`
- `HostBackend`, `HostError`, `ThemeAppearance`
- `HostController`, `LifecycleState`, `ShowRequest`, `ShowReport`,
  `HideReport`, `MonitorChange`, `ThemeObservation`
- `ReleaseStep`, `RELEASE_PLAN`, `ReleaseFailure`, `ShutdownReport`
- `MainThreadDispatcher`, `MainThreadToken`, `MainThreadCall`,
  `MainThreadOnly`, `DispatchError`

`ansa_platform_windows` additions:

- `ThemeError`
- `enumerate_monitors`, `cursor_position`, `read_apps_theme`
- `launcher_window_live`, `launcher_window_rect`
- `place_launcher_window`, `place_and_show_launcher`
- `show_launcher_checked`, `hide_launcher`, `request_launcher_focus`

The existing `set_launcher_visible`, `make_launcher_frameless`,
`round_launcher_window`, `enable_launcher_alpha`, focus capture/dismiss and
hotkey APIs remain source-compatible.

## Not yet proven — do not claim these pass

- Fullscreen applications (exclusive and borderless fullscreen) and taskbar
  auto-hide/full-screen edge cases.
- Virtual desktops, Windows 11 snap groups and per-desktop window behavior.
- Sleep, lock/unlock, resume and RDP/session changes. No power/session observer
  is created here; hotkey rearm after such events is not wired.
- Physically unplugging/replugging a monitor. Monitor removal is covered by
  fake monitor lists in `ansa-host` plus a 2-second polling path, not by a real
  unplug.
- Mixed-DPI physical multi-monitor hardware. 100/150/200% placement is
  unit-tested with synthetic monitor data, not observed on real scaled displays.
- macOS and Linux adapters. Non-Windows stubs return typed unsupported errors.
- The real keyboard route after focus denial. The 1000-cycle run observed
  denied focus but did not send physical keystrokes to prove typing still
  reaches the launcher.
- A native `FocusOutcome::Succeeded` observation on this machine was missing in
  the first 1000-cycle run (all denied). A later 1000-cycle rerun observed
  1000/1000 `Succeeded`; the denial path is still real and remains part of the
  contract. Neither run proves success under every foreground-lock policy.
- OS theme-change events. The launcher polls the registry every 2 seconds; it
  does not subscribe to a Windows theme-change notification.
- Screen-reader/accessibility journeys; the framework gate remains open.

The 1,000 native cycles are evidence for this machine and this configuration
only. They do not establish behavior on other GPUs, DPI layouts, Windows
versions or foreground policies.

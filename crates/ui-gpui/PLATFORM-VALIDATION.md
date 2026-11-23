# Desktop composition validation

Scope: ANS-98 (macOS desktop composition), ANS-94 (Linux desktop
composition), and the opaque-fallback portion of ANS-93 (Linux glass). This is
the UI integration checklist for the shared worktree; it is **not** native
macOS/Linux acceptance evidence.

Host used for the validation below: Windows (x86_64-pc-windows-msvc).
No Apple or Linux desktop was available. Only Windows-hosted tests ran; they
exercise the unchanged Windows path plus platform-neutral appearance, storage,
identity and setup helpers. macOS/Linux native code is compiled only for those
targets and is listed under missing evidence.

## Delivered scope

- `crates/ui-gpui` now consumes `ansa-platform-api` for application
  discovery, identified application launch/switch, HTTP(S) URL opening, host
  lifecycle/focus, and global-shortcut registration. Direct
  `ansa_platform_windows` references in the launcher are inside
  `#[cfg(windows)]` fields, methods and startup blocks. `apps/desktop` is a
  thin `ansa_ui_gpui::run()` wrapper and contains no platform adapter.
- `crates/ui-gpui/Cargo.toml` target-gates `ansa-platform-windows`,
  `raw-window-handle` and `objc2-app-kit`. The non-Windows build does not link
  the Windows adapter.
- The non-Windows desktop opens `DailyDriver` during catalog refresh, composes
  built-ins plus discovered applications plus enabled static HTTP(S)
  quicklinks, exposes setup/onboarding, emoji search, persisted-setting
  diagnostics and actionable storage recovery. Application discovery failure
  keeps built-ins and saved quicklinks usable and reports the failure in the
  status line.
- A selected application cannot launch a path from a replaced catalog
  snapshot. The UI captures the catalog generation that was rendered and
  clears that rendered generation as soon as a refresh starts. On non-Windows
  hosts it also retains the discovered `Vec<AppEntry>` for that generation.
  Application execution (including the onboarding test launch) finds the exact
  retained entry and calls `launch_or_switch_selected(&AppEntry)` under the
  generation gate. The native adapter additionally requires the current
  snapshot's path, stable ID and per-discovery selection token to match before
  recheck/launch, so a rediscovered replacement at the same path and ID is not
  launchable from an old row. Stale or busy selections return an actionable
  `NoTarget`/"catalog changed" message and leave the launcher open. Windows
  keeps `launch_or_switch_identified(path, id)` and its existing behavior.
- macOS glass is a request, never a claimed observed effect:
  `Glass: Requested` for GPUI's blurred window background. The user's requested
  preference is stored separately from the effective native state. Reduce
  Transparency applies the opaque palette and reports `Glass: Unavailable`
  without erasing that request; clearing the OS setting restores
  `Glass: Requested` unless the user explicitly chose Off (`Glass: Off`).
  The state is re-evaluated when the window is activated and on each user glass
  toggle. GPUI exposes no macOS accessibility display-option change observer,
  so a live change while the launcher is already active is not notified by
  this build; it is picked up on the next activation or toggle. `--opaque`
  starts the request off. GPUI has no API to confirm the compositor produced
  vibrancy, so success is not claimed. The rendered states are explicit: `On`
  only for the Windows backdrop after native attachment, `Requested` for the
  macOS GPUI request, `Off`, and `Unavailable` for Linux, unsupported hosts, or
  a requested effect blocked by Reduce Transparency. No separate `Degraded`
  glass state is claimed because no component reports a partial native effect.
- Linux glass is always the opaque themed surface with
  `Glass: Unavailable`, on X11 and Wayland and with or without `--opaque`.
  There is no clear tint, wallpaper read, screenshot or other fake blur.
  `Ctrl+Shift+G`, `Ctrl+Shift+T` and the footer buttons remain present; the
  interaction smoke asserts the window bounds do not move during appearance
  toggles.
- macOS attempts a real Control+Space registration. The retained launcher is
  hidden only while that registration is live and the retained hide/show route
  exists. If registration is unavailable or later degraded, the regular Ansa
  window, Dock reopen and desktop task switcher remain the fallback; Escape
  reports that it cannot safely hide instead of stranding a hidden launcher.
  Focus and placement are separate observations; keyboard focus is reported
  only after a bounded native observation, and no previous-application focus
  restoration is claimed.
- Linux X11 attempts a real Ctrl+Space grab and requests focus, then checks
  observed activation. It never hides the retained window, so the desktop
  window remains usable. Wayland, including XWayland sessions that export
  `DISPLAY`, reports the global shortcut and programmatic activation as
  unsupported and requires selecting Ansa from the desktop.
- Emoji uses the embedded emoji dataset and GPUI's system clipboard write.
  On macOS/Linux GPUI does not acknowledge the write, so the status says
  `Copy requested` and asks the user to paste manually. Clipboard history,
  auto-paste and vault UI are not wired into this desktop slice.

## Exact local validation

Run from the worktree root, date 2026-11-23, Windows host:

| Command | Result |
| --- | --- |
| `cargo test -p ansa-platform-api -p ansa-ui-gpui -p ansa-desktop --locked --offline` | exit 0; `ansa-platform-api` 43 passed, 0 failed; `ansa-ui-gpui` 30 passed, 0 failed, 0 ignored; `ansa-desktop` 0 tests; 0 doc-tests |
| `cargo clippy -p ansa-platform-api -p ansa-ui-gpui -p ansa-desktop --all-targets --locked --offline -- -D warnings` | exit 0 |
| `cargo fmt -p ansa-platform-api -p ansa-ui-gpui -p ansa-desktop -- --check` | exit 0 |
| `cargo check -p ansa-platform-api --target x86_64-unknown-linux-gnu --all-targets --locked --offline` | exit 0 |
| `cargo check -p ansa-platform-api --target x86_64-apple-darwin --all-targets --locked --offline` | exit 0 |
| `cargo clippy -p ansa-platform-api --target x86_64-unknown-linux-gnu --all-targets --locked --offline -- -D warnings` | exit 0 |
| `cargo clippy -p ansa-platform-api --target x86_64-apple-darwin --all-targets --locked --offline -- -D warnings` | exit 0 |
| `cargo build -p ansa-desktop --locked --offline` | exit 0 |
| `cargo test -p ansa-ui-components --locked --offline` | exit 0; 28 passed; 1 doc-test ignored |
| `target\debug\ansa-desktop.exe --smoke-test` | exit 0; window created and two GPUI frame callbacks reached |
| `target\debug\ansa-desktop.exe --smoke-test --opaque --light` | exit 0; same two-frame success |
| `target\debug\ansa-desktop.exe --interaction-smoke-test` | exit 0; 40 synthetic widget/action stages passed |
| `target\debug\ansa-desktop.exe --interaction-smoke-test --opaque --light` | exit 0; 40 synthetic widget/action stages passed |

The interaction smoke reports `Ansa appearance smoke: native backdrop attached
and toggled` on Windows. It exercises synthetic widget/keyboard routing, not a
physical keyboard, IME, compositor or external application launch.

Cross-target type checks were attempted but are **not** validation of this
change:

- `cargo check -p ansa-ui-gpui --target x86_64-unknown-linux-gnu --all-targets --locked --offline`
  exited 1 because the `ring` build script could not find
  `x86_64-linux-gnu-gcc` on this Windows host.
- `cargo check -p ansa-ui-gpui --target x86_64-apple-darwin --all-targets --locked --offline`
  exited 1 because the `ring` build script could not find `cc` (no Apple
  toolchain/SDK) on this Windows host.
- `ansa-platform-api` itself did type-check and pass Clippy for both targets
  (`--all-targets ... -D warnings`), including the Linux-only opener fixture
  and selection-token tests. That is compile-time evidence, not UI or native
  runtime validation.

Both stopped in a dependency build script before compiling `ansa-ui-gpui`.
A later coordinator retry with the host's MinGW `gcc` set as the Linux target C
compiler got past `ring` but failed in `freetype-sys` because
`x86_64-linux-gnu-g++` and a Linux sysroot are not present. The macOS-only
`raw_window_handle` calls were reviewed against the pinned GPUI 0.2.2 API and
use fully qualified `HasWindowHandle::window_handle` to avoid GPUI's inherent
`Window::window_handle()` returning `AnyWindowHandle`. A real macOS/Linux
toolchain build remains required.

## Native acceptance still to run

| Native session | Hotkey/focus check | Glass check | Status |
| --- | --- | --- | --- |
| macOS on real Apple hardware | Native Control+Space, conflict fallback, Dock reopen, AppKit ordering and observed focus | Vibrancy over real text/color, Reduce Transparency, `--opaque`, bounds after toggles | Not run |
| GNOME X11 | Global grab, conflict fallback, observed X11 activation; desktop remains usable | Opaque, `Glass: Unavailable` | Not run |
| KDE Plasma X11 | Same X11 checks | Opaque even when KWin blur is enabled | Not run |
| GNOME Wayland | Unsupported summon; direct window selection; no XWayland grab | Opaque, `Glass: Unavailable` | Not run |
| KDE Plasma Wayland | Same Wayland checks | Opaque even when KWin blur is enabled | Not run |
| wlroots-based Wayland session | Unsupported summon and usable manual fallback | Opaque, `Glass: Unavailable` | Not run |

Record OS, desktop/compositor versions, session type, GPU, display scale and
the exact executable for every row. Xvfb or a software compositor is
startup/interaction smoke only; it is not Mutter, KWin, wlroots or native glass
acceptance.

1. Build with the native toolchain:
   `cargo build -p ansa-desktop --locked --offline`. On Linux use the
   X11/Wayland, font and Vulkan development/runtime packages from the desktop
   CI job. Run `ansa-desktop --smoke-test` and debug
   `ansa-desktop --interaction-smoke-test`, each with default appearance and
   `--opaque --light`. Expect exit 0 and the documented success markers.
2. Start normally with `ANSA_DATA_DIR` set to a new disposable directory.
   Open **Start Setup**, check Ctrl+Space against the live registration, skip
   an unavailable step and confirm its warning, run **Test launch** on a real
   installed application, complete setup, quit and reopen to verify
   persistence. Then press Enter on an application and compare the reported
   outcome with the actual foreground. Force a shortcut conflict and confirm
   Escape does not hide a launcher that has no working summon route. On macOS,
   also reopen from the Dock.
3. While Ansa is closed, import a static HTTP(S) quicklink into the same
   profile, reopen, search for it, and confirm Enter opens the expected
   browser target. Confirm an unsupported/local-file quicklink is explicitly
   omitted. Do not treat native file reveal through `ansa-platform-api` as
   desktop file-quicklink coverage; the desktop catalog does not author or
   reveal file quicklinks.
4. Search `emoji smile`, copy a result, and paste into a separate editor to
   confirm the exact grapheme and that nothing was inserted automatically.
5. Hold the same disposable profile open in a second process and check the
   lock warning; release it and use **Refresh Applications**. Repeat with a
   corrupt disposable database and verify the reported data directory and
   move-aside/retry instructions keep the original for recovery. Replace or
   move a discovered target during refresh and confirm the rendered old
   selection cannot launch the replacement. Also replace a macOS bundle or
   Linux desktop entry at the same path after rediscovery and confirm the old
   selection is rejected while the refreshed row launches.
6. In dark and light themes check primary, secondary, caret and selected text
   against light/dark extremes (minimum 4.5:1), and compare window bounds
   before/after Ctrl+Shift+G and Ctrl+Shift+T. On macOS inspect the requested
   vibrancy over another app showing fine text and broad colors, then request
   glass, enable Reduce Transparency and activate the window (expect
   `Glass: Unavailable`), clear it and reactivate (expect `Glass: Requested`),
   then turn glass Off and repeat (expect `Glass: Off`). A live OS setting
   change while the window stays active is not wired to a notification in this
   build; it is observed only on activation or a glass toggle.
   `Glass: Requested` must not be reported as a verified native effect. On
   Linux confirm no backdrop or wallpaper shows through, and report
   compositor-owned frame/shadow behavior separately.
7. macOS and Linux native acceptance also needs an actual physical hotkey,
   IME, screen reader, sleep/lock and multi-display pass; none of those are
   covered by this Windows-hosted validation.

## Missing native evidence

- No macOS build or run: no evidence for an actual macOS compile, native
  vibrancy, Reduce Transparency, `--opaque`, AppKit focus/placement, Dock
  reopen, Control+Space conflict handling, real application launch or
  quicklink opening. The requested-vs-effective glass state is unit-tested,
  but GPUI has no accessibility display-option observer: a live OS change
  while the window stays active is not delivered until the next activation or
  user toggle.
- No Linux build or run on GNOME, KDE or wlroots: no evidence for X11 grab and
  focus observation, Wayland unsupported-state behavior, opaque rendering,
  contrast or compositor-owned decorations.
- No physical keyboard, IME, clipboard, screen-reader or external-application
  check on macOS/Linux. The Windows interaction smoke is synthetic only.
- ANS-93's requirement for working native blur on at least one Linux
  combination is **not satisfied** by this slice. Only the honest opaque
  fallback (`Glass: Unavailable`, no transparency or fake blur) is delivered.

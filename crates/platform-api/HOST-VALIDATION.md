# Host and global shortcut validation

Scope: ANS-91 (F04a), ANS-86 (F04b), ANS-89 (F07a), ANS-83 (F07b).

**No native macOS or Linux acceptance is recorded here.** Windows-hosted unit
tests and cross-target type checks do not establish a minimum macOS version, an
Intel or Apple Silicon support declaration, or any Linux compositor support
claim. The 1,000-cycle test here runs an in-memory reservation model; it does
not exercise a native keyboard, window manager, display server, or power event.

## Delivered bounded behavior (2026-11-23)

- **Catalog identity and staleness.** Linux discovery keeps the XDG flattened
  desktop-file ID only for precedence/masking. The launcher's stable
  `app:xdg-...` ID is now derived losslessly from each relative path component
  (percent-encoded, joined with encoded `%2F`), so paths such as
  `a-b/c.desktop` and `a/b-c.desktop` retain distinct identities even though
  they intentionally flatten to the same XDG desktop ID. The first path in
  deterministic root/candidate order is exposed; the masked copy is not exposed
  with the winner's identity. `recheck()` now re-resolves the flattened ID
  across the current roots before launch, so a newly added higher-priority
  override or a Hidden/NoDisplay/TryExec mask makes the old selected row stale.
  The selected manifest and resolved executable stamps are then compared with
  the discovered snapshot. A replaced path fails before launch.
- **Unix selection tokens.** `AppEntry::selection` is a doc-hidden
  per-discovery token (zero on Windows and manually built entries).
  macOS/Linux discovery assigns a new token to each row, and recheck reuses the
  retained token so metadata equality still compares bundle/manifest content.
  `launch_or_switch_selected(&AppEntry)` requires the current native snapshot
  to contain the same path, stable ID and token before recheck/launch. An old
  row therefore cannot launch a replacement that rediscovery found at the same
  path and ID. `launch_or_switch_identified(path, id)` remains only for
  compatibility and is documented as current-snapshot-only; the GPUI launcher
  now retains the discovered entries for the rendered generation and uses the
  selected-entry API.
- **Linux deduplication.** The catalog dedup key now includes the invoked
  executable path (argv0 identity) as well as the canonical executable,
  remaining arguments, working directory and terminal flag. Two differently
  named symlinks to one multicall binary remain separate rows.
- **macOS bundle identity.** Duplicate CFBundleIdentifiers select one copy
  deterministically (healthy before broken, then watch-root order, non-alias
  before alias, then path). Launch uses the selected bundle URL/path, and
  recheck inspects only that path; a stale row cannot be redirected to another
  copy. Broken-path IDs use a namespace that an `app:mac-<bundle-id>` value
  cannot occupy. The plist state machine now rejects a non-string dictionary
  key at the top level while still skipping nested values under unrelated keys.
- **Openers.** `execute_open_request` re-runs policy classification and
  host-side path/type checks even for a directly constructed public
  `OpenRequest`. Local paths are canonicalized before extension/executable
  checks, extensionless files are not documents (extensionless directories may
  be opened/revealed as directories), package directories are rejected for
  default-app opening, and executable files can be revealed but not opened as
  documents. Handoff uses literal argv/typed APIs, never a shell string.
  Linux now keeps a `LocalTarget` metadata identity (original pathname, symlink
  target, canonical object, file type, mode, size, mtime and Unix dev/inode/
  ctime) from the policy decision. It revalidates that exact object after the
  `xdg-mime` file-type query and after the default-handler query. The handoff
  also revalidates after the shared child-process lock is acquired and
  immediately before `spawn`, closing the wait while another opener holds that
  lock for its observation window. The containing-directory fallback captures
  its folder before the FileManager1 `ShowItems` attempt, revalidates that same
  captured folder after a failed attempt, queries the handler, revalidates
  again, and hands that folder to the same checked opener. A replaced file,
  retargeted symlink or changed mode/type is rejected at the checked points
  before the helper is spawned. This is not an atomic file-descriptor handoff:
  a helper still resolves a pathname, so an ancestor-path swap during a MIME
  query or a replacement after the final post-lock check can affect what the
  helper observes. Inert helper-fixture tests cover replacement during the
  default-handler query, parent retargeting during `ShowItems`, handler-query
  retargeting, and validation under the child lock; they are compiled by the
  Linux target and not executed on this Windows host. There is no shell in the
  fix or the fixtures.
- **Host lifecycle and shortcuts.** Linux capability reporting now distinguishes
  the compiled X11 shortcut route (Degraded until registration succeeds) from
  Wayland (Unsupported; no GlobalShortcuts portal client) and reports X11
  window control as Degraded because only an advisory work-area hint exists.
  macOS shortcut registration is likewise reported as Degraded, not
  Unsupported or unconditionally registered. X11 monitor/work-area intersection
  is a pure function with a host-runnable unit test. The socket/worker deadlines
  bound normal X11 request/event work, but the pinned xauth reader re-opens and
  reads `XAUTHORITY` outside that deadline, and the hotkey initialization path
  still joins its worker on timeout; a swapped or slow authority path can
  therefore delay setup. macOS
  `PositionOutcome::Verified` means only an
  immediate AppKit frame read-back, not durable Spaces/fullscreen placement or
  on-screen visibility.
- **macOS rebind press fence.** A replacement registration now carries a
  monotonic nanosecond fence converted from Carbon `EventTime` seconds (a
  `double`, not an integer tick). `Lifecycle::replace` reads
  `GetCurrentEventTime` only at commit, after registration and the potentially
  slow save, then stores the fence before enabling delivery. `handle_hotkey`
  converts `GetEventTime(event)` to the same unit and rejects a queued press at
  or before the fence; equal is rejected because a pre-commit press can quantize
  to the same `EventTime`. The clock does not wrap. Non-finite or negative
  conversion becomes 0 and very large values saturate. A rejected press still
  latches the physical key-down so its release clears the route state. The early
  same-binding save path is unchanged, and Linux/Windows use the timestamp-free
  delivery path. Host tests model a slow save by supplying pre-commit event
  timestamps, and cover a failed save keeping the previous registration, the
  equal/equal+1 boundary, invalid conversion and the existing 1,000-cycle
  rebind/suspend/resume loop. No real AppKit queue/drain was exercised.

## Capability and fallback

| Backend | Implemented boundary | Limitation |
|---|---|---|
| macOS | Main-thread AppKit `NSWindow` order-in/order-out with visibility read-back; current pointer screen’s `NSScreen.visibleFrame` with fallback to window/main screen, work-area fit, full-frame read-back; AppKit key-window plus active-application focus observation. HIToolbox `RegisterEventHotKey` with exclusive registration and a single owned event handler. | Focus is a separate, bounded observation; an activation request does not prove focus. AppKit decides Spaces/fullscreen behavior. Global registration does not require an event tap, Input Monitoring or Accessibility permission; exclusive conflicts reported by Carbon are surfaced, but not all competing system or non-exclusive shortcuts are necessarily detected. Secure Event Input suppresses dispatch and reports degradation when queried. Letter/digit bindings denote ANSI physical positions; non-ANSI label translation is not implemented. |
| Linux, X11 | Checked passive grabs on a dedicated local Unix-socket X11 client; a conflicting lock-mask variant aborts and disconnects the entire candidate. XKB detectable repeat, first press to summon, real release to re-arm. Optional EWMH/RandR physical-pixel monitor/work-area hint. | X11 GPUI 0.2.2 does not expose the launcher XID. Absolute placement is not verified; the hint is advisory only. The retained Linux window is not hidden. GPUI requests focus; the UI must check the actual window’s focus event/state, not just request completion. XKB and `:N[.screen]`/`unix:N[.screen]` are required for the shortcut; SSH/TCP display forwarding is not implemented. |
| Linux, Wayland (including XWayland `DISPLAY`) | Detects Wayland before X11. No passive global grabs, no unsupported call that masquerades as success, and no hidden launcher dependent on this route. | The GlobalShortcuts portal client and availability negotiation are **not implemented**, even if a portal exists. No absolute positioning or independently verified, portal-mediated focus path. |
| Headless / other | Explicit unsupported shortcut/activation routes. | No attempt to simulate native events. |

`HostCapabilities.global_hotkey` reports the compiled macOS and X11 routes as
`Degraded` (a reservation can still conflict or be denied), Wayland as
`Unsupported` with the missing portal documented in the capability notes, and
never reports a route as available merely because it was compiled. Linux
`window_control` is `Degraded` on X11 because the module exposes only an
advisory work-area hint, not window enumeration/activation/positioning.
`HostCapabilities.clipboard_capture` remains `Unsupported` in this host
contract: clipboard capture is implemented and probed by the separate
`ansa-clipboard` crate, not exposed through this platform-api snapshot. That is
an explicit boundary, not a claim that macOS/Linux clipboard capture is absent.

If global summon is unavailable, use the visible launcher, the desktop task
switcher or the normal desktop launcher entry. To start a fresh desktop
instance, use the normal UI executable/entry after quitting Ansa. Binding a
desktop shortcut to an executable is not a promise of reliable single-instance
summon/activation: no such CLI-to-running-process protocol is supplied here.

Theme observation belongs to GPUI’s native `Window::observe_window_appearance`
subscription, owned by the UI. It is not a worker poll and this module does not
claim that any compositor emits a notification. Test it on each accepted host.
On X11 the server socket connect/setup/requests have a shared deadline; the
event worker caps each drain and joins on drop. This does not put a timeout on
the OS filesystem reading the current user's local Xauthority file, or on an
arbitrary blocking callback supplied by a caller. Use a normal local authority
file and nonblocking callbacks.

## UI integration

- Use `HostLifecycle::current()`. Keep visibility, `FocusOutcome` and
  `PositionOutcome` separate. AppKit operations take a fresh *borrowed*
  `WindowHandle` during GPUI updates, not a handle stored in async work.
  Place on each summon rather than cache screen geometry; a denied placement
  does not prevent showing. Observe focus after a bounded interval. Before
  hiding, make sure the live hotkey is usable; leave the desktop route reachable
  if a previously working hotkey becomes degraded. Dismissal does not prove
  focus restoration to the prior application.
- `HotkeyRegistration::register` is the existing default, intent-only callback.
  For a saved binding and later edits, prefer
  `register_saved(saved.as_deref(), |event| { let _ = sender.try_send(event); })`
  on the UI thread. Surface the optional invalid-setting warning. A valid
  conflicting stored chord returns a conflict rather than silently picking a
  different chord. On consumption, call `registration.accepts(event)` and
  separately enforce any time-to-live. A token from a previous registration
  cannot activate the launcher after rebind or resume.
- For an edit, parse `HotkeyBinding` and call `registration.rebind(binding,
  |binding| atomic_save(binding.canonical()))`; render
  `binding.display(lifecycle.backend)` separately. The candidate is acquired
  with delivery gated while the old binding remains active. A conflict does not
  call persistence; a save error drops the candidate. Storage entry points,
  recorder UI and settings diagnostics are UI responsibilities.
- On observed suspend/session lock, call `suspend()`. On resume/session unlock,
  call `resume()` and report its result. Drop the registration before ending the
  UI event loop; the callback sender is released with it. The lifecycle methods
  here are real reservation release/reacquisition, but OS sleep/lock/session
  notification subscriptions and automatic retry after X server/theme/seat
  changes are not wired by this module. X11 connection or keyboard-map failure
  drops the grab and requires explicit recovery.

## Validation run (Windows host, 2026-11-23)

These commands record host unit behavior and cross-target type checking only.
They are not native desktop acceptance.

| Command | Result |
|---|---|
| `cargo test -p ansa-platform-api --locked --offline` | pass: 43 passed, 0 failed (doc-tests 0) |
| `cargo test -p ansa-platform-api -p ansa-ui-gpui -p ansa-desktop --locked --offline` | pass: `ansa-platform-api` 43 passed, 0 failed; `ansa-ui-gpui` 30 passed, 0 failed; `ansa-desktop` 0 tests; doc-tests 0 |
| `cargo clippy -p ansa-platform-api -p ansa-ui-gpui -p ansa-desktop --all-targets --locked --offline -- -D warnings` | pass, exit 0 |
| `cargo fmt -p ansa-platform-api -p ansa-ui-gpui -p ansa-desktop -- --check` | pass, exit 0 |
| `cargo check -p ansa-platform-api --target x86_64-apple-darwin --locked --offline --all-targets` | pass, exit 0 |
| `cargo check -p ansa-platform-api --target x86_64-unknown-linux-gnu --locked --offline --all-targets` | pass, exit 0 |
| `cargo clippy -p ansa-platform-api --target x86_64-apple-darwin --all-targets --locked --offline -- -D warnings` | pass, exit 0 |
| `cargo clippy -p ansa-platform-api --target x86_64-unknown-linux-gnu --all-targets --locked --offline -- -D warnings` | pass, exit 0 |

The host test run includes the catalog/recheck, selection-token, flattened-ID
identity, desktop visibility/TryExec, multicall argv0 dedup (Unix compile only),
argv parsing, duplicate-bundle, plist-key, opener-policy, `LocalTarget`
replacement, EventTime conversion/fence boundary, monitor-intersection and
shortcut-capability tests. Tests marked `#[cfg(unix)]` or Linux-only (symlink
policy, executable-file reveal, default-handler replacement fixture,
`ShowItems` parent-retarget ordering, child-lock validation ordering and
expired-deadline X11 reads) are type-checked by the Linux target; their runtime
behavior was not exercised by this Windows host. The mac-only AppKit/Carbon and
Linux native X11 code is type-checked only. No macOS or Linux linker/native
harness was run.

Cross-target clippy was also run with `--target x86_64-apple-darwin` and
`--target x86_64-unknown-linux-gnu` (`--all-targets --locked --offline -- -D
warnings`); both passed. This remains compile-time evidence, not native runtime
evidence.

## Residual risks / not evidenced

- macOS duplicate-ID recheck is deliberately path-exact. It rejects a moved or
  replaced selected path, but it does not re-rank a session where a new
  higher-priority copy of the same bundle ID appeared; launch still uses the
  selected bundle URL, so it cannot silently launch the other copy. Native
  LaunchServices behavior is not exercised here.
- The macOS rebind fence is compile-checked only. `GetEventTime` and
  `GetCurrentEventTime` return `double` seconds according to
  `CarbonEventsCore.h`, but no macOS linker or Carbon event loop ran on this
  Windows host. Native build and a real queued-press-during-save reproduction
  are still required. Equal times are rejected conservatively, so a genuinely
  fresh press quantized to exactly the commit time is also suppressed. The
  event clock is monotonic seconds since boot and does not wrap.
- The Linux opener revalidation narrows the race between policy checks and
  `xdg-open`, but it is not an atomic file-descriptor handoff: a process that
  can replace the pathname in the interval between the final post-lock check
  and helper execution is still outside this crate's control. The inert
  fixture tests only prove rejection at their injected points and are not run
  on this host; a real `xdg-mime`/`xdg-open` desktop session remains unverified.
  Helper queries may still block until their bounded deadline.
- Linux selection tokens are process-local. They detect rediscovery within one
  process; they are not a cross-process or persistence identity. The selected
  API also only proves the snapshot entry matches before its bounded recheck.
- X11 authorization-file reading is outside the socket deadline. The earlier
  regular-file/size check is only a preflight; the pinned x11rb xauth reader
  re-resolves and opens the path with blocking I/O and no read cap, so a path
  swap, a growing file, or a slow filesystem can still block setup while the
  session mutex is held. Hotkey initialization also joins its worker on timeout.
  No real X server, XKB map, modifier conflict, or session teardown was run.
- AppKit positioning is verified only by an immediate frame read-back. Spaces,
  fullscreen, hot-plug and compositor adjustments are not evidenced.
- Wayland GlobalShortcuts portal availability, consent, revocation and
  compositor-specific placement/focus remain unimplemented and unverified.

## Remaining native acceptance

Run on *real desktop sessions*, with the same build/commit, per-machine logs
and raw artifacts. If a platform/combination cannot be validated, narrow the
support declaration rather than marking it passed.

1. **Identify hardware and software.** For macOS record exact OS build, app
   build/signing mode, Apple Silicon model, GPU and monitor arrangement. Test a
   real Intel Mac only if Intel is declared; establish the actual minimum OS
   from tested artifacts. For Linux record distribution/version, kernel, GPU,
   GNOME/KDE Plasma and compositor/version, real X11 vs Wayland session, portal
   implementation/version (or absence), and display scale/geometry. Test each
   declared GNOME and KDE X11/Wayland pair independently; add wlroots only if
   declared. Xvfb and software Vulkan are supporting evidence only.
2. **Window lifecycle on each declared host.** Wire and run a native harness for
   **1,000 actual hide/show and focus cycles**; log backend, per-cycle visible
   and keyboard-route observations, placement outcome and bounds, denial cases
   and elapsed time. Include an actual keyboard input after focus and an
   independent check of what received it. Force activation denial and verify it
   is not counted as success. Cover multiple displays, hot-plug during use,
   Retina/mixed/fractional scale changes, Spaces/virtual desktops, fullscreen
   applications, and light/dark native theme changes. Repeat launch/hide/quit
   while checking worker counts, file descriptors/native handles and ability to
   relaunch. Linux retained hide/show and native acceptance remain blocked until
   a verified restore/visibility path exists or the target is explicitly scoped
   as degraded.
3. **Registration lifecycle on each supported backend.** Run **1,000 actual
   reservation and suspend/resume cycles** with one observed summon per
   activation, no repeat toggling, and none from stale/unregistered chords.
   Include a second process holding the candidate, checking the old chord
   during rejection and after release, save failure, restart with stored
   rebinding, and invalid-config fallback. Exercise sleep/wake, lock/unlock and
   fast user switching/session changes; on Linux also restart X/compositor and
   change modifier mapping/layout. Record cleanup and successful re-acquisition
   after restart. These tests need host notification wiring; calling the
   adapter methods alone doesn't verify OS lifecycle delivery.
4. **Permission and policy cases.** macOS: I/O grants denied/revoked (the
   registered API should not present a monitoring prompt), app/system
   conflict matrix including IME shortcuts, Secure Event Input, ANSI versus
   other keyboard layouts, and desktop fallback. X11: X authorization failure,
   locked modifier variants, active keyboard remaps and duplicate registrations.
   Wayland: portal absent, present, denied, consent and revocation on each
   target compositor. Until a portal client exists, verify only the explicit
   unsupported path and fallback; do not count it as a successful summon.

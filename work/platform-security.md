# Platform integration and security design

Research date: 2026-11-11. These are public-document observations and proposed engineering choices, not observations of Raycast binaries or live execution. No REA evidence IDs exist for this research. No platform integration has been implemented or tested here.

## Executive decision

Build one Rust domain core with explicit platform adapters and runtime capability discovery. Promise a common keyboard-driven workflow, not identical OS powers. The largest parity risks are background clipboard monitoring, global text expansion, arbitrary window manipulation and activation on Wayland; macOS permission-sensitive automation; and Windows elevated-process boundaries. Extension compatibility and extension isolation are separate goals: a Node child process alone is not a sandbox.

## Evidence ledger

| ID | Public documentation observation | Consequence |
|---|---|---|
| P01 | Apple exposes `AXIsProcessTrustedWithOptions` to check Accessibility trust; optional prompting is asynchronous. [Apple](https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions) | Permission state must be queried and represented; requesting it does not mean it was granted. |
| P02 | `NSPasteboard.changeCount` changes when pasteboard ownership changes. [Apple](https://developer.apple.com/documentation/appkit/nspasteboard/changecount) | A bounded change-count poll is a possible macOS adapter, with explicit power/latency measurement. |
| P03 | Windows `RegisterHotKey` registers system-wide hotkeys and generally fails for existing registrations. [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey) | Detect conflict and offer an editable binding; never silently intercept a different shortcut. |
| P04 | Windows clipboard listeners receive `WM_CLIPBOARDUPDATE`. [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-addclipboardformatlistener) | Use a message-loop listener, not a busy poll. |
| P05 | Windows `SendInput` is subject to UIPI and can inject only into equal/lower-integrity applications. [Microsoft](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-sendinput) | Paste/expansion into elevated apps can fail. Do not run the entire launcher as administrator to hide this limitation. |
| P06 | Windows places conditions on `SetForegroundWindow` and may deny activation even if conditions appear satisfied. [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow) | Focus restoration is a checked operation with a fallback, not a guaranteed arbitrary focus steal. |
| P07 | `SetWindowPos` changes geometry and Z order; cross-session operations fail. [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos) | Window service must be per-user-session and preserve DPI/monitor/restore state. |
| P08 | GlobalShortcuts portal binds shortcut sessions and emits activation events regardless of focus. [XDG](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html) | Prefer this on Wayland when the installed portal backend actually implements it. |
| P09 | Clipboard portal extends other sessions, currently RemoteDesktop/InputCapture; it does not create an independent clipboard session. [XDG](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Clipboard.html) | Its existence does not establish universal silent clipboard-history support. |
| P10 | RemoteDesktop has a user-mediated session granting input devices, optionally clipboard. [XDG](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html) | Do not present a remote-desktop permission as an invisible general automation fallback. |
| P11 | X11 EWMH defines window move/resize requests interpreted by window managers. [freedesktop](https://specifications.freedesktop.org/wm/latest-single/) | X11 window management needs WM capability checks; an API request is not proof of a successful placement. |
| P12 | Node's permission model explicitly does not defend against malicious code. [Node](https://nodejs.org/api/permissions.html) | Node compatibility workers remain trusted-code execution unless independently confined and tested at OS level. |
| P13 | Wasmtime's sandbox isolates Wasm, and WASI uses capability-based file access; host/runtime defects remain possible. [Wasmtime](https://docs.wasmtime.dev/security.html) | A constrained Wasm API is a credible default boundary; every imported host call still requires validation. |
| P14 | Windows defines clipboard formats asking for exclusion from history and cloud synchronization. [Microsoft](https://learn.microsoft.com/en-gb/windows/desktop/dataxchg/clipboard-formats) | Honor those markers before capture, indexing or persistence. |
| P15 | TUF documents rollback, freeze, arbitrary-installation and metadata attacks. [TUF](https://theupdateframework.io/docs/security/) | A signed binary alone is insufficient for a robust update protocol. |
| P16 | Apple Developer ID distribution supports signing, hardened runtime, notarization and stapling. [Apple](https://developer.apple.com/developer-id/) | Release workflow and budget need signing/notarization; open-source/free software does not mean zero distribution expense. |

## Platform capability contract (proposed)

`CapabilityStatus = Available | PermissionRequired | Unsupported | TemporarilyUnavailable | Degraded { reason, alternative }`. Include adapter/version, scope, last successful probe, and user-facing recovery. Never cache permissions indefinitely. Re-probe on launch, wake, session transition and failed operation, with throttling. Commands declare their capability requirements; discoverable commands can explain missing capability without appearing broken.

| Feature | macOS adapter | Windows adapter | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| Launcher summon | Native global shortcut + AppKit panel; verify Spaces/fullscreen behavior | RegisterHotKey + native activation path | X11 passive grab; conflict handling | GlobalShortcuts portal if supported; desktop-configured command shortcut fallback |
| Application catalog | Application bundles/Launch Services metadata; file watchers | Start Menu shortcuts, packaged app catalog; dedupe identity | XDG desktop entries | XDG desktop entries; package confinement affects visibility |
| Launcher focus/restore | AppKit + captured previous app; respect user switching | Checked SetForegroundWindow and HWND validity | WM-aware activation request | Activation tokens/compositor policy; no universal focus guarantee |
| Clipboard capture | NSPasteboard; changeCount-based adapter | WM_CLIPBOARDUPDATE; retry clipboard contention | XFixes/selection owner changes | Explicitly supported compositor protocol or user-mediated portal; otherwise unavailable |
| Paste selected item | Restore target; permitted input/AX route | Restore HWND; SendInput under UIPI | Selection/clipboard + input injection | Portal/approved compositor integration; otherwise copy-only |
| Window positioning | Accessibility elements and mutable position/size attributes | EnumWindows + SetWindowPos, monitor work areas | EWMH supported-WM requests | Compositor-specific adapter; no promised universal arbitrary positioning |
| Global text expansion | Input observation + permission-aware injection; secure-input exclusions | Hooks + bounded trigger buffer; SendInput limits | X11 input observation/injection | Unsupported by default until a specific compositor integration passes tests |
| Selected text | AX when exposed; user-driven copy fallback | UI Automation when exposed; user-driven copy fallback | Application/toolkit dependent | Application integration or explicit copy; generic access not promised |
| File search | Own scoped index; optional platform search adapter | Own scoped index; optional Windows Search adapter | Own index + inotify | Same, subject to portal/package file grants |
| Screenshot/OCR | Permission-aware capture and local OCR | Supported capture APIs and local OCR | Desktop capture adapter | Screenshot/ScreenCast portal, with permission |
| System controls | Explicit native commands; per-command permissions | Explicit native commands; no blanket elevation | D-Bus/desktop services | D-Bus/desktop services, runtime capability tests |

The Wayland protocol source pages for ext-data-control and ext-foreign-toplevel-list could not be read due to website access denial in this research. Exact protocol versions and compositor implementation breadth remain unresolved; do not turn this matrix into a claim that all GNOME/KDE/wlroots versions behave identically. Ship and publish a tested compositor/version/package matrix.

Adapter details: keep unsafe FFI in small reviewed crates; own native thread-affine event loops; use typed OS window handles with generation checks; distinguish logical points, physical pixels and per-monitor scale; honor work area/taskbar/dock; retain restore rectangles; cancel if target changes while a command is preparing. Avoid native pointers crossing process IPC. A pure Rust core may still require thin Objective-C/Swift or C ABI glue where system APIs demand it.

## Focus and insertion state machine (proposed)

1. On summon, record target identity (process plus window plus generation), input locale, monitor, and clipboard version, without collecting document content.
2. While launcher is visible, freeze that invocation's target unless user explicitly picks another.
3. Before insertion, verify target still exists and operation is supported/authorized; construct text without shell interpolation.
4. Hide launcher, request focus restoration, observe success or time out. If the user independently switches targets, cancel automatic insertion.
5. Paste via a tested route. Clipboard-based insertion uses a transaction identifier and only considers restoring the previous clipboard if the launcher still owns the same value/version. Never overwrite a newer user copy.
6. Report `Copied — paste manually` if focus or injection cannot be established. Never claim insertion success only because `SendInput` returned a count.
7. Record metadata-only outcome and latency; exclude the inserted content.

Text expansion must keep a short in-memory trigger buffer, reset on focus changes, redact secure contexts where detectable, support immediate pause, and never persist a keystroke log. Password/secure-context detection is imperfect; default to conservative exclusions and document the residual risk. IME composition, dead keys, grapheme deletion, cursor relocation and selection replacement are explicit tests.

## Extension trust model (proposed)

Three distinct classes avoid a false promise of both arbitrary Node compatibility and strong isolation:

| Class | Runtime | Trust/permissions | Compatibility |
|---|---|---|---|
| Built-in features | Rust, inside trusted service boundaries | Reviewed first-party code; minimal internal interfaces | Full own-product feature surface |
| Portable extensions | Wasmtime worker process; versioned component/host API; optional JS-to-supported-runtime toolchain only after a spike | No ambient FS, sockets, environment, process spawn or clipboard; brokered capabilities | Own SDK, stable declarative views; not automatically Raycast/Node compatible |
| Compatibility/scripts | Separate Node or explicit shell process | User-approved trusted code; OS confinement where available and verified; clearly labeled when unconfined | Best-effort Raycast API subset/Node ecosystem and local scripts |

Do not call an isolated JS VM, worker thread, Node permission flag, or child process a security sandbox. A malicious compatibility extension can use native addons, process creation, filesystem or networking unless the OS boundary genuinely blocks it. Do not silently switch a portable extension into trusted mode when an API is missing.

Permissions should be fine-grained and enforceable at the Rust broker: `network.connect` scoped to hosts/ports, `filesystem.read` to picked roots, `filesystem.write` separately, `clipboard.read_current` distinct from `clipboard.read_history`, `clipboard.write`, `applications.open`, `windows.manage`, `selection.read`, `secrets.use`, `notifications.show`, `ai.request`, and explicit `process.spawn` only for the trusted class. Host calls bind extension identity, package hash, invocation, permissions and cancellation token. Permissions cannot be selected by a client-supplied extension ID.

Network broker: revalidate redirects, block loopback/private/link-local destinations unless explicitly granted, defend DNS rebinding, bound response size/time, strip credentials on cross-origin redirects, and distinguish outgoing upload from passive retrieval. File broker: resolve symlinks and prevent traversal/races using directory handles/capability APIs rather than string-prefix checks. Secrets broker should perform authorized requests when possible rather than handing raw tokens to extensions. A grant to an extension that can read a secret and access arbitrary network inevitably permits exfiltration; describe that honestly.

No extension JavaScript runs in the privileged launcher WebView. Host renders a validated declarative view tree (List/Grid/Detail/Form/Actions), rejects scripts and raw active HTML, sanitizes Markdown/links, brokers image fetches, caps depth/node counts, and virtualizes large lists. React-based compatibility requires a custom renderer/translation layer, not DOM injection into the trusted app. Version and validate every message; cap frame size, recursion, queued work and log output. Handle cancellation even when an extension ignores it. Use fuel/epoch interruption, memory limits and wall-clock worker termination; tear down associated tasks and handles.

Package lifecycle: content-addressed immutable package; signed provenance or publisher signature plus registry verification; lock dependencies; scan archives for traversal/symlinks/decompression bombs; no install-time arbitrary scripts in the safe registry; staged activation; per-extension crash quarantine; disable/revoke; pin/rollback; permission changes require explicit review. Pin compatibility runtime version, patch on a security SLA, and test old extensions against new host versions.

## Data and clipboard privacy (proposed)

Use SQLite for ordinary metadata with transactional migrations. Store secret payloads in an encrypted store and keep wrapping keys in OS-provided secret storage; on Linux, if no unlocked supported secret service exists, require a user passphrase or disable persistent sensitive history. Do not silently fall back to plaintext. Encryption does not defend against a compromised logged-in session with access to the app's keys.

Clipboard capture is an explicit onboarding choice. Exclude sensitive clipboard markers (P14), configurable apps and MIME types; reject oversize payloads; sanitize previews; deduplicate by keyed content hash; bounded retention and item/byte quotas; allow pause, private mode and delete-all. Automatic secret detection can reduce exposure but is not a security guarantee. Images/OCR are separate opt-ins and quotas. Never index plaintext sensitive history into an unencrypted FTS table. Options: encrypted database with measured FTS performance, or decrypt selected items into a bounded ephemeral search index. Include thumbnails, WAL/journals, backups and crash reports in the privacy design.

Deletion removes rows, blobs, thumbnails, search records and pending sync events. Do not promise forensic erasure on SSDs/backups; key destruction provides a more useful boundary when correctly implemented. Clipboard source attribution can be incomplete or misleading, so exclusion behavior must not assume the foreground app always authored the data.

## Optional sync and AI (proposed)

Core product works offline with no account. Sync is optional and server implementation is open-source/self-hostable. Begin with settings, snippets and quicklinks; omit clipboard and API keys by default. Add notes after conflict semantics are specified. Use a reviewed E2EE protocol/library design, not homemade cryptography: random workspace root key, per-device keys, authenticated device enrollment, versioned authenticated envelopes, collection key separation, unique nonces, key rotation, device revocation and an explicit recovery key. Server stores ciphertext and necessary routing/version metadata, which still leaks sizes/timing/device activity.

Define conflict rules per type: settings field-level revisions with deterministic tie-break/conflict UI; snippets/quicklinks preserve conflicting copies; notes use a proven CRDT or retain both revisions; deletions use tombstones with bounded compaction and device-watermark policy. A compromised/revoked device may retain previously decrypted data; revocation protects future access only after rotation. Client rollback/replay detection needs authenticated counters/heads and a recovery UX; an untrusted server can still deny service. Credentials should normally be re-authorized per device rather than synced.

AI providers are adapters: local model endpoint or bring-your-own-key remote provider. Free software cannot guarantee free third-party inference. Show destination and included context; no automatic clipboard/history upload; allow per-command context opt-in. Treat model outputs and tool results as untrusted data. Mutating tool actions require concrete preview/confirmation unless pre-authorized by a narrowly scoped automation rule. Keep prompt injection from escalating extension capabilities.

## Update, distribution and operations (proposed)

Keep launcher/service non-elevated. Narrow updater helper only where packaging requires it. Separate stable/beta channels. Sign platform artifacts; macOS hardened runtime, Developer ID, notarization and stapling; Windows signing; Linux package repository signatures. Pin release toolchains/dependencies, produce SBOM and provenance, scan dependencies/licenses, isolate CI signing keys from pull-request builds. Reproducible builds are a measured target (report which artifacts can be independently reproduced), not a claim that notarized/signature bytes match automatically.

Use TUF or an equivalently reviewed metadata design: offline root, delegated/rotatable signing roles, expiry, monotonically tracked versions and artifact hashes. Reject stale/tampered metadata and binaries. Download into staging, verify before replacing, atomic install, crash recovery, safe rollback to the last compatible version. Distinguish operator-authorized rollback from attacker-driven downgrade; database migrations need snapshots or backward-compatible reads before app rollback. The updater must not install unsigned dynamic extension code under a trusted core identity.

Publish SECURITY.md, supported versions, private vulnerability contact, threat model, key rotation/revocation procedure, and a documented incident path. Logs are bounded, local, redacted and exportable after user review. Telemetry is optional; measure performance without queries, clipboard text, paths, notes or extension secrets. Crash dumps can contain sensitive memory; opt-in and review/scrubbing are needed.

## Verification and release gates (proposed)

1. **Platform conformance:** on actual supported OS builds, test summon/hide/focus 1,000 cycles; conflicting shortcut; multiple monitors with mixed DPI; dock/taskbar changes; sleep/wake; lock/unlock; fullscreen/Spaces/virtual desktops; app exits during insertion; revoked permissions; remote desktop session; keyboard-layout switch. Publish failures by platform, not a blended pass rate.
2. **Accessibility/input:** complete core flows with VoiceOver, Narrator/NVDA and Orca; visible focus; high contrast, large text and reduced motion; CJK IME composition; dead keys; RTL; combining emoji; screen-reader announcement of streamed result changes without excessive chatter.
3. **Clipboard correctness:** rapid changes, clipboard lock/contention, large/invalid MIME payloads, rich-text/image fallbacks, sensitive flags, delayed rendering, excluded apps, concurrent user copy during paste, deletion across DB/WAL/cache, secure-context exclusions. Verify copy-only fallback preserves new clipboard values.
4. **Window correctness:** minimum-size apps, dialogs, app-owned child windows, minimized/maximized/fullscreen states, offscreen restored positions, monitor disconnect, DPI transition, elevated targets, window-handle reuse. Read resulting geometry and report clamping/failure.
5. **Extension abuse:** infinite loop, memory blowup, output flood, malformed protocol, process crash, decompression bomb, traversal/symlink race, DNS rebinding/redirect, unsolicited privileged calls, UI spoofing, cross-extension data access, permission revocation mid-call. Core remains usable and denied operations stay denied.
6. **Storage/sync:** crash/power loss at each migration boundary; low disk; corrupt DB; schema upgrade/downgrade; duplicate/reordered/replayed sync operations; clock skew; concurrent edits; expired device; recovery key loss; revoked device; server unavailable; delete and restore semantics. Restore from a real backup in CI scenarios.
7. **Update:** corrupted download, old metadata, expired key, rotation, compromised online key drill, interrupted replacement, insufficient disk, bad new build, staged rollback, offline installation and complete uninstall including explicit choice about retained data.
8. **Performance:** measured budgets per supported hardware class; service ready, hotkey-to-first-paint, keystroke-to-local-results, extension cold/warm start, memory for entire process tree, idle CPU/wakeups, power impact while clipboard monitoring, large file index. Network providers never block local search or typing.
9. **Release gate:** no known critical/high exploitable issues; all declared core capabilities pass per-platform contract; permissions fail closed; no sensitive data in sample logs/crash export; rollback and recovery drill passes; signed artifacts launch on clean machines; registry can revoke an extension without disabling offline core functionality. Any exception is documented and changes the supported feature matrix.

## Remaining research and proof work

- Concrete macOS API behavior for global shortcuts, secure input, permission revocation, UIA/AX coverage, and packaged-app identity requires on-device prototypes.
- Confirm Wayland compositor/portal versions and data-control/window-management adapters against actual distributions; specifications alone do not establish deployment coverage.
- Validate OS sandbox profiles for Node separately on all three OSes; until then classify compatibility mode as trusted and unconfined.
- Security review the extension broker, renderer, updater and E2EE design before calling them production-grade.
- Benchmark end-to-end launcher behavior with the selected UI framework and all background workers; runtime and protocol choices here are design proposals.

# Implementation status

Started 2026-11-11. Updated 2026-11-19 with the Phase 1 daily-launcher implementation. This file tracks this repository, not reference-product behavior. The supplied research remains a design baseline; no reference app was executed here.

## Scope

| Backlog | Slice | Boundary |
|---|---|---|
| F01 | Versioned 100-scenario planning ledger, checked schema, per-OS A-behavior coverage and design-evidence audit | Unknown-only reference identities; zero reference runtime captures or implementation-test passes |
| F03 | Exact toolchain selection, passing three-OS desktop build/test/startup CI, locked dependency/license inventory | Activated X11 startup only; inactive X11/Wayland/lifecycle not certified; no signed releases or third-party legal clearance |
| F04 | Retained GPUI window and checked best-effort Windows focus return | Foreground races handled conservatively; real focus/monitor/lifecycle gate open |
| F05 | Shared Unicode input, bounded undo/redo, one transaction per IME composition, oversized-edit rejection | Physical IME and assistive-technology testing open; not a Notes editor |
| F06 | Searchable snapshot-bound actions, stable selection, arrows/Enter/Escape and Tab focus | Synthetic native interaction test passes; full assistive-technology/control acceptance remains open |
| F07 | Event-driven Windows Ctrl+Space registration, chord recorder, probe/reconfigure/reapply lifecycle with old-binding release | Desktop still registers the default chord and does not expose a rebinding UI; no physical summon test; other OS adapters not implemented |
| F08 | UIA/semantic metadata for some candidate-mode controls and accessibility labels in the new desktop surfaces | Real screen-reader acceptance has not run; F08 cannot close without NVDA/Narrator evidence |
| L01 | Typed command/action registry with prepare/authorize/execute/reconcile, opaque refs, kind capabilities, pending-operation blocking, bounded tombstones | No platform `Host` implementation and no desktop execution path yet; in-process trust and TOCTOU remain |
| L02 | Bounded Shell Link parser, classic and packaged discovery, target/argument/working-directory identity, dedupe, broken entries, launch-or-switch, polling watcher API/example, safe ICO icon extraction, shell-API reveal/copy identity | Packaged AUMID switching is explicitly Unsupported; desktop doesn't render icons or run the watcher; real Explorer reveal and real switch/foreground remain manual; watcher desktop wiring is L11 |
| L03 | Deterministic provider coordinator with revisions, deadlines, quotas, bounded queues, terminal states and selection stability | No real async/IPC/remote transport; caller-driven logical clock |
| L04 | Tiered ranking, capped favorite/usage/recency, private mode, reset, golden tuning/held-out corpus and metrics | Held-out top-1 is 0.897; transposition/diacritic gaps are documented, not hidden |
| L05 | SQLite single-writer actor, WAL/FULL, transactional migrations and snapshots, revisioned settings, records, secret refs, separate cache, verified backup/restore, real SQLITE_FULL test, identity-keyed single-instance lock, recovery reports | Generic records/cache are unencrypted; physical power-loss/ENOSPC and rollback-failure branch remain uninduced; lock is advisory and per-user |
| L06 | Structured quicklinks with RFC 3986 encoding, scheme/path/opener policy, preview/commit fingerprint, versioned import/export, desktop catalog consumption and `--data-dir` persistence | Desktop authoring/preview/import/export form and real browser/file-manager probes remain L06a; external import requires Ansa closed (single-instance lock) |
| L07 | Bounded arithmetic, percentages, exponentiation, length/mass/time/temperature units, typed errors, versioned corpus | `f64` approximate; currencies, dates, time zones and natural language are explicitly unsupported |
| L08 | 139-entry licensed Unicode 16.0 emoji/symbol dataset with pinned checksum, grapheme integrity, accessible variants, search, bounded recents/favorites, activation model, desktop `emoji <query>` copy mode | No real insertion host, persisted collections or full variant-picker UI; screen-reader audit not run |
| L09 | First-run onboarding model with recoverable shortcut conflicts, launch verification, permission explanations, JSON persistence and reset; keyboard-only `Start Setup` desktop surface | Full Welcome/Shortcut/TryLaunch/Integrations/Done screens, real hotkey rebinding probe and real launch verification remain L09a |
| L10 | `ansa-alpha` composition root, CLI surfaces, desktop `DailyDriver` consumption and deterministic 1,000-cycle harness | Two-week human dogfooding, clean-machine install/uninstall, physical keyboard/screen-reader and crash/restart drill remain open |
| L11 | Desktop consumes `DailyDriver` for catalog/onboarding/quicklinks; `emoji <query>` copy mode; keyboard-only setup; smoke isolation | Bounded first slice: no quicklink authoring, no full emoji variant picker, no full onboarding screens; F08/manual acceptance open |
| P03 | Windows DPAPI plus macOS Keychain and Linux Secret Service vault backends, XChaCha20-Poly1305 sealed payloads, Argon2id passphrase and memory-only fallbacks, exact-byte header binding, secret aliases, rotation, redacted errors and no silent plaintext downgrade | Windows DPAPI is user-bound; macOS/Linux providers are compile-checked only; no cross-user/machine backup; rotation isn't transactional; generic records/cache encryption integration remains |
| P04 | Clipboard capture model with Windows, macOS AppKit and Linux X11/Wayland text + file-list readers, privacy markers, sensitive filtering, quotas/pins, sealed history, crash-safe store locks and legacy migration | No live clipboard listener/UI; image capture unsupported; macOS/Linux readers have no native acceptance yet; heuristics are not proof; session-only pause/private policy |
| P05 | Target-aware insertion transaction with copy-only default, identity/generation/permission/IME/secure-input checks, clipboard ownership, no restore-over-newer-copy | No Windows automatic insertion host; `Inserted` evidence path is fake-host tested only; real-app insertion matrix absent |

These are implemented slices within the reviewed boundaries, not a claim that Phase 1 or the release gates are closed. L08, L09 and L10 still require host/UI/human evidence.

### macOS/Linux follow-up batch (21 November)

ANS-82 through ANS-99 are tracked in the
[platform batch implementation record](platform-batch-implementation.md).
The historical Windows slice descriptions above are not claims that the new
platform adapters have native acceptance. Real macOS/Linux lifecycle,
screen-reader and visual evidence remains required before a support claim.

### Foundation evidence/tooling follow-up (17 November)

ANS-5 adds the [reference planning ledger](reference/README.md): 100 retained
scenarios across Windows/macOS with explicit unknown target identities. Its
validator checks schema, coverage and evidence boundaries; a planning-ledger pass
is not a runtime capture or a passing implementation test.

ANS-6 expands [ADR 0003](adr/0003-reuse-evaluation.md) to six pinned candidates
and confirms a greenfield, zero-source-reuse scope decision. Four bounded build
probes failed before tests ran; Vicinae's follow-up configure failed at missing
Qt6 6.9 after detecting MSVC, and Flare remains historical. The
[closure assessment](reuse/closure.md) and [static dependency scan](reuse/dependency-scan.md)
record the bounded investigation's final decision and unresolved per-package
licences. No candidate tests passed, no source was adopted, and no legal or
runtime clearance is claimed.

ANS-7 adds [build and inventory tooling](build/README.md). The lockfile currently
contains 762 packages, including 744 third-party packages whose review remains open.
The exact named Rust 1.99.1 toolchain subsequently passed local Windows validation:
138 unit tests, one doctest, Clippy, desktop build, startup smoke and 30 synthetic
interaction stages. One UI example doctest remains ignored. Post-merge main
[CI run 44085915233](https://github.com/ansahq/ansa/actions/runs/44085915233)
passed all eight jobs on commit `7b22edc`, including Windows/macOS/Linux desktop
builds and tests. That run did not execute native startup smokes on macOS/Linux.
Follow-up [PR #2 run 44250538706](https://github.com/ansahq/ansa/actions/runs/44250538706)
passed all eight jobs on `c261854`, including native startup on every host;
Linux uses explicit activation on Xvfb/software Vulkan. It does not certify
unattended X11 first presentation or Wayland. See the
[18 November acceptance record](foundation-acceptance-2026-11-18.md).
See the [CI follow-up](ci-follow-up-2026-11-17.md) for the compatibility fix and
actual exact-pin validation, superseding the initial toolchain failure in the
[integration record](foundation-validation-2026-11-17.md).

## Test record

The 2026-11-20 follow-up record is in [`validation-2026-11-20.md`](validation-2026-11-20.md) with the Phase 2 additions. On Windows with Rust 1.99.1:

- `cargo fmt --all -- --check` — clean.
- `cargo test --workspace --offline --no-fail-fast` — **695 distinct passed, 0 failed, 2 ignored**; 563 unit + 128 integration + 4 doctests. Cargo reports 697 pass events because two tests relaunch themselves in child processes. Ignored: the `text_input` doctest and the manual real-clipboard probe.
- `cargo test --locked --offline --workspace` — passes against the updated lockfile.
- `cargo clippy --workspace --all-targets --offline -- -D warnings` — clean; the only note is the pre-existing `proc-macro-error2` future-incompatibility warning from a dependency.
- Desktop: startup smoke exits 0; **40-stage** interaction smoke passes, including keyboard-only setup traversal and bounded emoji navigation; `--smoke-test` does not touch the real profile.
- CLI checks: `doctor` reports 156 applications (108 classic, 48 packaged, 3 genuinely broken); `calc "10 km to mi"` prints `6.21371192237 mi`; `calc "0.1+0.2"` prints `0.3`; `emoji rocket` returns licensed dataset metadata; `alpha-smoke --cycles 1000` passes 15,003 assertions with 3,000 stale batches rejected; `storage --demo` verifies a backup; `onboard --demo` completes a fake-probe journey with explicit warnings; `quicklink ... --data-dir` persists to the desktop's SQLite record.
- Real catalog example: 156 total, 108 classic, 48 packaged, 3 broken, 29 entries with arguments, 6 argument-distinct groups, watcher not wired.
- Platform icon example: valid 32×32 ICO, 4,286 bytes from notepad.exe. Watch example: 6 bounded polls. Vault: DPAPI round-trip, tamper/wrong-context/lock tests. Clipboard: DPAPI-sealed history, exclusion markers, cross-process lock and legacy migration tests.
- Independent Max-variant reviews are recorded in [`reviews/`](reviews/). The final scoped verdicts have no open BLOCKER/HIGH/MEDIUM on the accepted slices; residual risks and manual checks are documented in those reports and below.

### macOS/Linux platform batch (23 November)

The ANS-82–ANS-99 follow-up adds macOS/Linux catalog, opener, host, hotkey,
Keychain/Secret Service vault, clipboard and desktop paths; see the
[platform batch record](platform-batch-implementation.md). On Windows with Rust
1.99.1 and the locked/offline resolution:

- `cargo fmt --all -- --check` — clean.
- `cargo test --workspace --locked --offline --no-fail-fast` — **786 passing
  test events, 0 failed, 2 ignored** across 50 Cargo result lines (not 50
  distinct test binaries). Two pass events are child-process relaunches used by
  the process-boundary tests; ignored: the UI text-input doctest and the manual
  real-Windows-clipboard probe.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` —
  exit 0; only the existing `proc-macro-error2` future-incompatibility note.
- `cargo build -p ansa-desktop --locked --offline` — exit 0; default and
  `--opaque --light` startup smokes exit 0; the **40-stage** interaction smoke
  exits 0 in both appearances.
- Focused package runs: platform-api **43**, vault **41**, clipboard **110**
  passed plus one ignored, UI **30** passed.
- Cross-target compile and Clippy checks passed for platform-api, vault and
  clipboard on macOS/Linux. The desktop cross-target attempt stopped in `ring`
  because this host lacks the Linux cross C compiler and Apple SDK; it is not
  native evidence.
- Independent GPT-6.2 Sol Max reviews recorded the original 4 HIGH/7 MEDIUM/LOW
  findings, three fix-verification rounds and a final micro-confirmation under
  [`reviews/`](reviews/). The
  passphrase-header, selection-token, X11 owner/generation, Linux-opener,
  Carbon-ABI and transactional-eviction defects are closed in the reviewed
  paths; Xauthority blocking I/O, Linux provider-prompt consent and live macOS
  Reduce Transparency notification remain documented residuals, not native
  evidence.
- Native acceptance remains **0/8** in the gate ledger. VoiceOver, Orca/AT-SPI,
  macOS visual glass, Linux blur, real X11/Wayland and macOS host lifecycle,
  real Keychain/Secret Service and real clipboard/browser/file-manager checks
  are still blocked on hardware and human testers.

Linux blur remains unimplemented: the desktop uses an opaque surface with
`Glass: Unavailable`. No native platform support claim is made from the
Windows-hosted results above.

CI configuration is not evidence of a completed remote CI run. Physical-input, screen-reader, lifecycle, accessibility and cross-platform gates remain open.

## Required next acceptance work

Latest appearance follow-up: monochrome light/dark palettes and translucency default on.
The native correction removed the invisible shadow gutter and fixed old GPUI's unwanted non-client
insets. On Windows 11, a separate compositor layer now blurs the live backdrop beneath GPUI's themed
tint. The older geometry observations in the [12 November validation](validation-2026-11-12.md)
do not verify the current blur. See the [ANS-75 validation notes](validation-2026-11-18.md).

1. Complete L06a: GPUI quicklink authoring/preview/import/export and real browser/file-manager availability probes. External `--data-dir` import intentionally requires Ansa to be closed because the desktop holds the single-instance storage lock.
2. Complete L08a: real clipboard/insertion host for emoji (P05), persisted recents/favorites, full variant-picker UI and a screen-reader name audit.
3. Complete L09a: full Welcome/Shortcut/TryLaunch/Integrations/Done screens, hotkey rebinding using the F07 manager/recorder, and real launch verification.
4. Close F08 with NVDA/Narrator and keyboard-only journeys; add UIA semantics for setup focus movement beyond the current candidate-mode labels.
5. Wire icon rendering and `CatalogWatcher` into the desktop; verify real switch/foreground behavior; keep packaged switching explicitly unsupported.
6. Persist or deliberately reset ranking signals; define the private-mode contract in the desktop.
7. Add generic-record/cache encryption integration on top of P03, induce physical ENOSPC/power-loss around SQLite commitments, and test the rollback-failure branch.
8. Run the L10 two-week daily-driver dogfood with regression triage, a clean-profile install/upgrade/uninstall by a new tester, a crash/restart drill and a published known-limitations list.
9. Complete the reuse/dependency/license review and the extension/ecosystem track. No extension runtime exists.

## Security boundaries

- Native application launch passes one validated `.lnk`/`.exe` path or a validated `shell:AppsFolder\<AUMID>` to the shell; it never builds a `cmd.exe` or PowerShell expression from metadata.
- Packaged enumeration uses a compile-time constant `Get-StartApps` command, an absolute PowerShell path, strict size/time limits, strict JSON and a strict AUMID allowlist.
- Quicklinks never invoke a shell. They resolve inert target data under an HTTP(S)/local-path policy; local paths are opt-in, executable/unknown extensions are denied by default for `DefaultApp`, and the caller must confirm replace imports.
- Storage keeps only credential references; generic user records and cache values remain unencrypted and must not contain secrets. Logs and error messages do not include stored payloads.
- P03 secrets and P04 clipboard payloads are sealed with Windows DPAPI on Windows and with a 32-byte OS-keyring key plus XChaCha20-Poly1305 on macOS Keychain and Linux Secret Service. Explicit passphrase (Argon2id-wrapped data key) and memory-only modes are available; there is no plaintext fallback. Vault/clipboard files contain only ciphertext plus non-sensitive metadata. `lock()` is a session gate rather than cryptographic re-authentication, and same-user processes allowed by the OS provider can access the key or decrypt DPAPI data.
- P05 defaults to copy-only. Automatic insertion has no Windows host in this slice; it must not report `Inserted` without destination evidence, and it never restores an older clipboard value over a newer user copy.
- The emoji dataset is embedded, offline and pinned by version and checksum. The checksum is an integrity pin, not a cryptographic signature.
- `ansa doctor` probes capability only; it does not register a global hotkey. No account, network service, automatic paste, clipboard history or extension execution is enabled in this slice.
- No user query text is logged by the desktop. CLI stdout deliberately contains the results the user requested.

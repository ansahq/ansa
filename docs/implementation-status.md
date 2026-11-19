# Implementation status

Started 2026-11-11. This file tracks this repository, not reference-product behavior. The supplied research remains a design baseline; no reference app was executed here.

## Scope

| Backlog | Slice | Boundary |
|---|---|---|
| F01 | Versioned 100-scenario planning ledger, checked schema, per-OS A-behavior coverage and design-evidence audit | Unknown-only reference identities; zero reference runtime captures or implementation-test passes |
| F03 | Exact toolchain selection, passing three-OS desktop build/test/startup CI, locked dependency/license inventory | Activated X11 startup only; inactive X11/Wayland/lifecycle not certified; no signed releases or third-party legal clearance |
| F04 | Retained GPUI window and checked best-effort Windows focus return | Foreground races handled conservatively; real focus/monitor/lifecycle gate open |
| F05 | Shared Unicode input, bounded undo/redo, one transaction per IME composition, oversized-edit rejection | Physical IME and assistive-technology testing open; not a Notes editor |
| F06 | Searchable snapshot-bound actions, stable selection, arrows/Enter/Escape and Tab focus | Synthetic native interaction test passes; full assistive-technology/control acceptance remains open |
| F07 | Event-driven Windows Ctrl+Space registration | No rebinding/shortcut recorder; other OS adapters not implemented |
| L01/L03/L04 | Typed catalog, deterministic ranking, session pins/usage, action revision guards, coalesced catalog refresh | No persistent preferences or general asynchronous provider/deadline runtime yet |
| L02 | Windows Start Menu `.lnk` discovery/native launch | No packaged apps, icon extraction, live watcher, dedupe by target or switch-to-existing |
| L07 | Bounded arithmetic, explicit `=` routing and user-visible error states | Approximate f64 only; no units, dates, currencies, or arbitrary natural language |

These are partial implementations, **not closed backlog tickets**. No claim of screen-reader accessibility, cross-platform native support, security sandboxing or Raycast compatibility is made.

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

ANS-7 adds [build and inventory tooling](build/README.md). The lockfile contains
717 packages, including 708 third-party packages whose review remains open.
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

See [`validation-2026-11-12.md`](validation-2026-11-12.md) for the latest commands/results: the default workspace again passed 133 unit tests, one doctest, Clippy, startup smoke and its 26-stage native synthetic interaction smoke. The immutable newer-GPUI candidate now runs the actual launcher shell, passes that same 26-stage smoke, candidate Clippy and 38 shared-widget tests. Real owned-process Windows UIA probes pass Unicode text-range/selection/geometry checks, atomic oversized-edit rejection, independent action search, pin/unpin with stable row identity, details/Back and calculator errors. The small 10k fixture additionally passes offscreen end/start realization, polite live-region events and an eight-stage navigation smoke. The main dependency pin remains unchanged. CI configuration is not evidence of a completed remote CI run; physical-input, screen-reader and lifecycle release gates remain open.

## Required next acceptance work

Latest appearance follow-up: monochrome light/dark palettes and translucency default on.
The native correction removed the invisible shadow gutter and fixed old GPUI's unwanted non-client
insets. On Windows 11, a separate compositor layer now blurs the live backdrop beneath GPUI's themed
tint. The older geometry observations in the [12 November validation](validation-2026-11-12.md)
do not verify the current blur. See the [ANS-75 validation notes](validation-2026-11-18.md).

1. Run the desktop on the Windows test machine; inspect GPU startup, keyboard routing, selection, action panel and error states.
2. Validate CJK composition confirmation/cancellation, emoji grapheme editing, Hindi, RTL, AltGr/dead keys and caret placement at 100/150/200% scaling.
3. Resolve the blocking [framework accessibility gate](framework-gate.md): the main pin has no bridge; the newer revision now runs the actual shell with bounded automated UIA coverage. Complete NVDA/Narrator journeys, RTL/mixed-bidi and scaling checks, offscreen-provider behavior and the remaining lifecycle/dependency review before promoting the new pin.
4. Capture shortcut conflicts and 1,000 hide/show/focus cycles across monitors, sleep and locked sessions. Check the new conservative focus-return path with real apps; no automatic insertion is implemented.
5. Freeze a 10k-command benchmark corpus and capture release-mode query/frame/open traces. No latency/memory budget is marked met yet.
6. Complete third-party release licence review, any future [source-reuse audit](adr/0003-reuse-evaluation.md), reference-build capture and unattended X11/Wayland lifecycle validation. The zero-reuse scope decision and exact-toolchain three-OS build/test/activated-startup checks passed; they do not replace those other acceptance checks.
7. Introduce a storage actor only with transactional migration/recovery fixtures. Do not persist clipboard text or notes in an unencrypted prototype shortcut.

## Security boundaries

- Native ShellExecute opens an explicitly selected local shortcut, not a constructed `cmd.exe` or PowerShell expression.
- HTTP/HTTPS opening is allowlisted. Future quicklinks require structured argument encoding and validation.
- No extensions, scripts, background capture, remote AI, or automatic insertion exist in this slice.
- Copy result/target writes the current clipboard only on explicit action. It does not capture history or claim paste success.
- No user query text is logged by the desktop. CLI stdout deliberately contains the results the user requested.

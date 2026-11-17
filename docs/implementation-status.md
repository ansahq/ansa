# Implementation status

Started 2026-11-11. This file tracks this repository, not reference-product behavior. The supplied research remains a design baseline; no reference app was executed here.

## Scope

| Backlog | Slice | Boundary |
|---|---|---|
| F03 | Workspace, CLI, CI configuration | Not signed artifacts or a three-OS desktop release pipeline |
| F04 | Retained GPUI window and checked best-effort Windows focus return | Foreground races handled conservatively; real focus/monitor/lifecycle gate open |
| F05 | Shared Unicode input, bounded undo/redo, one transaction per IME composition, oversized-edit rejection | Physical IME and assistive-technology testing open; not a Notes editor |
| F06 | Searchable snapshot-bound actions, stable selection, arrows/Enter/Escape and Tab focus | Synthetic native interaction test passes; full assistive-technology/control acceptance remains open |
| F07 | Event-driven Windows Ctrl+Space registration | No rebinding/shortcut recorder; other OS adapters not implemented |
| L01/L03/L04 | Typed catalog, deterministic ranking, session pins/usage, action revision guards, coalesced catalog refresh | No persistent preferences or general asynchronous provider/deadline runtime yet |
| L02 | Windows Start Menu `.lnk` discovery/native launch | No packaged apps, icon extraction, live watcher, dedupe by target or switch-to-existing |
| L07 | Bounded arithmetic, explicit `=` routing and user-visible error states | Approximate f64 only; no units, dates, currencies, or arbitrary natural language |

These are partial implementations, **not closed backlog tickets**. No claim of screen-reader accessibility, cross-platform native support, security sandboxing or Raycast compatibility is made.

## Test record

See [`validation-2026-11-12.md`](validation-2026-11-12.md) for the latest commands/results: the default workspace again passed 133 unit tests, one doctest, Clippy, startup smoke and its 26-stage native synthetic interaction smoke. The immutable newer-GPUI candidate now runs the actual launcher shell, passes that same 26-stage smoke, candidate Clippy and 38 shared-widget tests. Real owned-process Windows UIA probes pass Unicode text-range/selection/geometry checks, atomic oversized-edit rejection, independent action search, pin/unpin with stable row identity, details/Back and calculator errors. The small 10k fixture additionally passes offscreen end/start realization, polite live-region events and an eight-stage navigation smoke. The main dependency pin remains unchanged. CI configuration is not evidence of a completed remote CI run; physical-input, screen-reader and lifecycle release gates remain open.

## Required next acceptance work

Latest appearance follow-up: monochrome light/dark palettes and translucency default on.
The native correction removed the invisible shadow gutter, disabled the accent
backdrop, added full-window rounded clipping and fixed old GPUI's unwanted non-client insets.
Default and candidate native geometry observations now pass; the final default validation passes
138 tests, one doctest, Clippy and startup/30-stage interaction smokes. Visible desktop pixel
transparency and shadow quality remain unverified, explicitly separate from those geometry checks.
See the final section of the [12 November validation](validation-2026-11-12.md); saved earlier
full-shell UIA observations do not validate these last appearance changes.

1. Run the desktop on the Windows test machine; inspect GPU startup, keyboard routing, selection, action panel and error states.
2. Validate CJK composition confirmation/cancellation, emoji grapheme editing, Hindi, RTL, AltGr/dead keys and caret placement at 100/150/200% scaling.
3. Resolve the blocking [framework accessibility gate](framework-gate.md): the main pin has no bridge; the newer revision now runs the actual shell with bounded automated UIA coverage. Complete NVDA/Narrator journeys, RTL/mixed-bidi and scaling checks, offscreen-provider behavior and the remaining lifecycle/dependency review before promoting the new pin.
4. Capture shortcut conflicts and 1,000 hide/show/focus cycles across monitors, sleep and locked sessions. Check the new conservative focus-return path with real apps; no automatic insertion is implemented.
5. Freeze a 10k-command benchmark corpus and capture release-mode query/frame/open traces. No latency/memory budget is marked met yet.
6. Complete the reuse evaluation beyond the bounded [Gauntlet inspection](adr/0003-reuse-evaluation.md), dependency/license inventory, reference-build capture and 100-scenario ledger.
7. Introduce a storage actor only with transactional migration/recovery fixtures. Do not persist clipboard text or notes in an unencrypted prototype shortcut.

## Security boundaries

- Native ShellExecute opens an explicitly selected local shortcut, not a constructed `cmd.exe` or PowerShell expression.
- HTTP/HTTPS opening is allowlisted. Future quicklinks require structured argument encoding and validation.
- No extensions, scripts, background capture, remote AI, or automatic insertion exist in this slice.
- Copy result/target writes the current clipboard only on explicit action. It does not capture history or claim paste success.
- No user query text is logged by the desktop. CLI stdout deliberately contains the results the user requested.

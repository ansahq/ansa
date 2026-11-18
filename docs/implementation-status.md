# Implementation status

Started 2026-11-11. This file tracks this repository, not reference-product behavior. The
supplied research remains a design baseline; no reference app was executed here.

**Current status: Windows 11 build 26200 x86_64 keyboard-first development foundation. The
release framework accessibility gate is open and blocking.** The decision and support matrix
are in [ADR 0004](adr/0004-framework-gate-decision.md) and [the gate](framework-gate.md). The
shipping pin is still `gpui 0.2.2`; the newer-GPUI candidate is still isolated and
unpromoted.

## Scope

| Backlog | Slice | Boundary |
|---|---|---|
| F01 | Reference scope survey | Documentation-level feature map only; no reference build/hash, no 100-scenario ledger, no observed reference execution |
| F02 | Reuse evaluation | Bounded Gauntlet source inspection in [ADR 0003](adr/0003-reuse-evaluation.md); greenfield still **provisional**; F02 not closed |
| F03 | Workspace, CLI, CI configuration | Local clean build/test/Clippy pass; CI workflow file exists, but no remote CI run, no three-OS desktop build, no SBOM/full licence inventory |
| F04 | Retained GPUI window, Windows monitors/theme/placement/show/hide, ordered shutdown | One machine/GPU/topology; 1000-cycle smoke passes here but focus outcome varied between denied and succeeded across runs; sleep/lock, fullscreen, virtual desktop, hot-unplug and mixed-DPI hardware unverified |
| F05 | Shared Unicode input, bounded undo/redo, one transaction per IME composition, oversized-edit rejection, logical RTL, UTF-16 host conversion | Physical IME, real AT, scaled RTL and OS clipboard integration open; not a Notes editor |
| F06 | Typed view stack, restore-once parent state, invocation/view-scoped action references, Escape hierarchy | 46 synthetic stages and model/property fuzz pass; physical keyboard and real screen reader unverified |
| F07 | Platform-neutral chord/config/store/manager/recorder, Windows arbitrary-chord worker, CLI fallback | Desktop still registers fixed `Ctrl+Space`; no rebind UI, no store/manager wiring, no automatic sleep/lock observer, no macOS/Linux adapter; ANS-11 observed the chord owned, the reviewer probe saw it available (session-dependent) |
| F08 | Accessibility spike | Shipping pin exposes 0 UIA descendants; candidate has automated UIA only; **no NVDA/Narrator/VoiceOver/Orca journey** |
| F09 | Performance/resource harness | `quick`, `cancel-flood`, `delayed-provider` run; `full` and 72-hour `soak` unrun; no budget defined or met |
| F10 | Framework gate | Narrow Windows development decision in [ADR 0004](adr/0004-framework-gate-decision.md); release gate intentionally open |
| L01/L03/L04 | Typed catalog, deterministic ranking, session pins/usage, action revision guards, coalesced catalog refresh | No persistent preferences or general asynchronous provider/deadline runtime yet |
| L02 | Windows Start Menu `.lnk` discovery/native launch | No packaged apps, icon extraction, live watcher, dedupe by target, or switch-to-existing |
| L07 | Bounded arithmetic, explicit `=` routing and user-visible error states | Approximate `f64` only; no units, dates, currencies, or arbitrary natural language |

These are partial implementations, **not closed backlog tickets**. No claim of screen-reader
accessibility, physical IME behavior, cross-platform native support, production readiness,
security sandboxing, or Raycast compatibility is made.

Appearance is part of this slice: monochrome light/dark palettes and glass default to on;
native window geometry/clipping checks pass on the reviewed machine. Visible desktop pixel
transparency, shadow quality, remote-desktop behavior and disabled-composition fallbacks
remain unverified. The app reports `Glass: Unavailable` and uses an opaque surface rather than
claiming glass succeeded.

## Test record (2026-11-18, clean build)

The target directory had stale artifacts from an earlier source state: an incremental
`cargo test --workspace` reported 292 passing tests while `target/debug/ansa-desktop.exe` did
not contain the current `--perf-smoke-test` fixture. The hazard recurred later: an incremental
`cargo test --workspace --locked` failed with `E0599 glass_background` until
`cargo clean -p ansa-ui-components` was run. A full `cargo clean` would also delete the
hash-verified candidate archive under `target/framework-candidates`, so the final validation
instead cleaned every workspace package and rebuilt from source while preserving that archive.
The clean source produced the authoritative results below. See
[`docs/evidence/ans-13-framework-gate.md`](evidence/ans-13-framework-gate.md) for the full
incident.

| Command | Result |
|---|---|
| clean all workspace packages, then `cargo test --workspace --locked` | **292 passed, 0 failed**; 1 doctest passed; 1 doctest ignored |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0; only the pre-existing `proc-macro-error2 0.2.1` future-incompat note |
| `cargo run -q -p ansa-desktop -- --interaction-smoke-test` | 46 synthetic widget/action stages passed; exit 0; includes root Escape dismissal with no hotkey registered |
| `cargo run -q -p ansa-desktop -- --lifecycle-smoke-test --lifecycle-cycles=1000` | clean-run rerun `succeeded=0 denied=1000 skipped=0 unsupported=0`, `ANSA_LIFECYCLE_OK cycles=1000`, exit 0; an earlier rerun observed `succeeded=1000`. Both are valid host-policy outcomes |
| `cargo run -q -p ansa-bench -- quick --output-dir=<temp>` | exit 0; 3 launch samples, 0 launch failures; p50 query-to-results 96,213.15 us, p50 keystroke-to-list 87,201.0 us, p50 frame interval 16,641.5 us, working set 12,697,600 bytes, idle CPU 0.0 % of one core; `budget: not defined` |
| `cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --features launcher-shell --bins` | exit 0 after `prepare.ps1` materialized the hash-verified archive |
| `powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -LauncherShell -OutputPath <temp>` | **failed twice** at owned-window foreground activation (`Windows denied foreground activation of the owned fixture`); no JSON was written; not a pass |
| `powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-pinned-uia.ps1 -OutputPath <temp>` | exit 0; shipping fixture root `ControlType.Pane`, **0 descendants**, `SemanticTreePresent=false` |
| `cargo build --release -p ansa-ui-gpui` | exit 0; only the pre-existing `proc-macro-error2` future-incompat note |

The 292-test total is the sum of the workspace binaries and integration tests; the ignored
doctest is the intentional `TextInput` module example. The earlier 133-test and 266-test
figures in dated validation records are historical and are not current counts.

Candidate UIA observations from earlier runs remain in
`experiments/gpui-accessibility/` (search/list/text-range/live-region/action/details), but
they were not independently reproduced in this session. They are automated UIA evidence, not
screen-reader evidence.

## Required next acceptance work

Ordered by the release gate, not by feature appeal:

1. **Clean-build CI and gate evidence.** Add a clean-runner job that runs the workspace tests,
   Clippy, the 46-stage smoke, the 1000-cycle lifecycle smoke and the candidate `prepare.ps1`
   + build. Gate evidence from an incremental `target/` is rejected (GATE-01, GATE-04).
2. **Promote the candidate or keep it isolated, explicitly.** Record the candidate hash,
   dependency/licence inventory, compatibility inventory, rollback plan, and the promotion
   decision. Do not close F08 on experiment evidence (A11Y-01/02).
3. **Wire F07 end-to-end.** Load `HotkeyStore` at desktop startup, use `HotkeyManager` for
   activation/rebind, add the GPUI rebinding surface, and add a message-only
   `WM_POWERBROADCAST`/`WM_WTSSESSION_CHANGE` rearm path. Test a real conflict and
   sleep/unlock on two machines (GATE-02).
4. **Fix the two known semantic gaps.** Route the calculator error into the polite live
   region (A11Y-21) and make stale-action rejection reachable from UIA or another owned
   automation path (A11Y-11). Then run AT J5-J7.
5. **Run real NVDA and Narrator J1-J10** on the promoted build with two testers and
   speech-viewer logs (A11Y-03/04). Only then run J8-J10 (IME, RTL, scaling).
6. **Build macOS and Linux** and run VoiceOver/Orca. The declared platforms currently have no
   build (A11Y-05/06/20).
7. **Define performance budgets and run `full` plus 72-hour `soak`** in release mode on named
   hardware. The debug `quick` fixture is not a budget result (GATE-03).
8. **Complete F02** beyond the bounded Gauntlet inspection: maintainer/test/licence review and
   the 100-scenario ledger. Greenfield is still provisional (GATE-06/F02).
9. **Root dismissal is mitigated; add the physical failure run.** Root Escape now hides the
   launcher even when no global hotkey is registered, and the 46-stage smoke asserts
   `LifecycleState::Hidden` in that path. A physical keyboard run on a machine where
   `Ctrl+Space` is owned is still required to close GATE-05.

## Security boundaries

- Native ShellExecute opens an explicitly selected local shortcut, not a constructed
  `cmd.exe` or PowerShell expression.
- HTTP/HTTPS opening is allowlisted. Future quicklinks require structured argument encoding
  and validation.
- No extensions, scripts, background capture, remote AI, or automatic insertion exist in this
  slice.
- Copy result/target writes the current clipboard only on explicit action. It does not
  capture history or claim paste success.
- No user query text is logged by the desktop. CLI stdout deliberately contains the results
  the user requested.

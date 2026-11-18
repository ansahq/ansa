# ANS-15 / F08 — accessibility acceptance evidence

Date: 2026-11-18
Ticket: ANS-15 `[F08] Complete real screen-reader and semantic acceptance`
Branch: `ans-foundation-evidence`
Status: **real screen-reader acceptance NOT RUN. F08 is open and blocking. No accessibility
support claim is made.**

> **Blocking statement.** This environment cannot run NVDA, Narrator, VoiceOver or Orca and cannot
> capture screen-reader speech. No assistive-technology journey exists for this repository. The
> ticket and the [acceptance protocol](../accessibility/screen-reader-acceptance.md) reject keyboard
> automation, UIA tree dumps and screenshots as screen-reader evidence, so none of the results below
> pass F08. Everything here is supporting structural evidence at best.

Scope of this change: `experiments/gpui-accessibility/**`, `docs/accessibility/**`,
`docs/risk-register.md`, and this file. The framework gate, implementation status, ADRs and crate
sources were not edited. The main pin remains `gpui 0.2.2`; the newer candidate remains an
unpromoted experiment. Running workspace cargo commands refreshed the root `Cargo.lock` entry for the
concurrently added `apps/bench` member; the root `Cargo.toml` was not edited by this work.

## 1. What was inspected

- The isolated candidate workspace, its README, `UiaProbe.cs`, `inspect-uia.ps1`, `prepare.ps1`,
  the 12 November observation JSONs and the spike/launcher sources.
- `docs/framework-gate.md` and ADR 0002 (read only) for the recorded source-inspection finding.
- `crates/ui-gpui` and `crates/ui-components` (read only) to know which semantic surfaces exist and
  which extractions the probe can assert.
- No NVDA, Narrator, VoiceOver, Orca, Accessibility Insights, Accerciser or IME was installed or
  started. No system display setting was changed.

## 2. Commands and actual results

Environment: Windows 11 `Microsoft Windows NT 10.0.26200.0`, Rust 1.99.1, branch
`ans-foundation-evidence`, no network. Windows PowerShell 5.1 and `dotnet` 8.0.2225 are present;
`csc` is not on `PATH`, and the probe compiles through `powershell.exe` `Add-Type` (which worked).

| # | Command | Result |
| --- | --- | --- |
| 1 | `cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc` | **Failed**: `cannot update the lock file … because --locked was passed`. The experiment lockfile predated new path-crate dependencies. |
| 2 | `cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --offline --target x86_64-pc-windows-msvc` | Passed, exit 0. `Locking 1 package`; added `ansa-host` to the experiment lockfile (path crate only, no registry change). Finished in 10.05 s. |
| 3 | `& experiments/gpui-accessibility/target/x86_64-pc-windows-msvc/debug/ansa-gpui-accessibility-spike.exe --interaction-smoke-test` | Passed, exit 0: `Candidate navigation smoke: 8 stages passed, including 10k-list end/start realization and IME-owned arrow deferral.` Synthetic keys; not AT evidence. |
| 4 | `powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -OutputPath experiments/gpui-accessibility/uia-observation-2026-11-18.json` | Passed, exit 0, observation written. First invocation earlier failed with `First row label/identity differs from the fixture`; see §3. |
| 4b | Same command with `-OutputPath experiments/gpui-accessibility/uia-observation-2026-11-18-rerun.json` | Passed, exit 0. Key fields identical to run 4 (roles, focus, 9 rows, position 1/10,000, size 10,000, filtered size 1, identity stability, 100 % scale, physical-pixel bounds, all text-range checks). |
| 5 | `powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-pinned-uia.ps1 -OutputPath experiments/gpui-accessibility/default-pin-uia-negative-observation-2026-11-18.json` | Passed, exit 0. Shipping fixture exposes **0 descendants** with an active UIA client. |
| 6 | `cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc --features launcher-shell --bins` (first attempt) | **Failed**: `error[E0432]: unresolved import gpui::Timer` at `crates/ui-gpui/src/host_lifecycle.rs:18`. Historical; fixed later the same day. |
| 7 | `cargo test -p ansa-ui-components --offline` | Passed: 45 library tests, 4 integration tests, 0 failed, 1 doctest ignored. No change from the ANS-10 record. |
| 8 | `cargo build -q -p ansa-desktop --offline` | Passed, exit 0. |
| 9 | `cargo run -q -p ansa-desktop -- --a11y-fixture` (stdout read once, then the started process tree terminated) | Handshake received: `ANSA_UIA_READY:2230312`. The started `cargo`/`ansa-desktop` tree was killed; 0 `ansa-desktop` processes remained afterwards. An earlier direct launch of the same binary also returned `ANSA_UIA_READY:7538776`. |
| 10 | `cargo test --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc -p ansa-ui-components` | Passed: 61 library tests (includes the `upstream-candidate` AccessKit path), 4 integration tests, 0 failed, 1 doctest ignored. |
| 11 | `cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc` (repeat) | Passed in 3.48 s; the refreshed lockfile is stable under `--locked`. |
| 12 | `cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc --features launcher-shell --bins` (after the renderer fix) | Passed, exit 0, `Finished dev profile` in 39.45 s. |
| 13 | `powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -LauncherShell -OutputPath experiments/gpui-accessibility/launcher-uia-observation-2026-11-18.json` and the same with `…-rerun.json` | Both passed, exit 0, identical key fields; no fixture process remained. Facts in §4b. |

## 3. Probe robustness incident

On the first 18 November run, `inspect-uia.ps1` stopped at:

```text
First row label/identity differs from the fixture.
```

A manual owned-process dump immediately afterwards showed the expected tree (`Edit`, `List`, nine
`ListItem`s starting at `Project 00000 Terminal`). A rerun of the unchanged script passed. The
assertion had used `FindAll(...)[0]`, assuming UIA descendant enumeration is logical order; it is
not guaranteed. The script now:

- finds the first row by name and reports the realized labels in the failure message;
- validates that realized logical positions form a consecutive set starting at 1;
- reads set size from the items (the list container reports `-1`);
- records the observation timestamp, executable path/hash-time and OS version.

This is a probe defect that was corrected, not an application fix. The flake remains recorded
because an unreproducible first result must not be presented as a pass.

### Focus activation incident

While another application held the foreground, the probe failed with
`Search UIA focus does not match the native input focus: focusable=True; focused=False`. The
fixture window was focusable but Windows' foreground lock kept keyboard focus elsewhere. The probe
now explicitly activates **only the fixture window it started** (the same operation the shell probe
already used) and then waits for the focus property through a UIA event. If activation is denied it
fails with the foreground window's identity instead of reporting success. After that change it
passed and the second observation was saved; a later invocation was denied again (while another
application held the foreground, named in the error) and the following run passed. Focus-dependent
evidence therefore requires a session where the owned fixture may take foreground; denial is
recorded as a failure, not filtered out. This is an evidence-tooling fix, not a change to the app's
focus behaviour.

## 4. Automated UIA evidence that passed (supporting only)

Source: [`uia-observation-2026-11-18.json`](../../experiments/gpui-accessibility/uia-observation-2026-11-18.json),
owned process only, 10,000-command synthetic fixture, 100 % scale. These are UIA properties and
events, **not** speech.

| Check | Observed |
| --- | --- |
| Search role | `ControlType.Edit` (AccessKit `SearchInput`; UIA has no separate SearchInput control type) |
| Search name | `Search applications and commands` |
| Search focus | keyboard-focusable `true`, has keyboard focus `true` |
| Search value | ValuePattern available, `IsReadOnly = false` |
| Search text | TextPattern available; document text matches Unicode fixture |
| List role/name | `ControlType.List` / `Search results` |
| Realized rows | 9, labels `Project 00000…00008 Terminal`, positions `1…9` |
| First row | position 1 of 10,000; selected; `IsOffscreen = false`; SelectionItemPattern present |
| Set size | every realized item reports 10,000; list container reports `-1` |
| List selection pattern | not exposed (`false`) |
| Row selection | `SelectionItem.Select` selects row 2 without executing it; focus stays in search; polite announcement `Project 00001 Terminal, result 2 of 10000` |
| End/start navigation | `Project 09999 Terminal, result 10000 of 10000` and `Project 00000 Terminal, result 1 of 10000`; each produced one live event; focus and query retained |
| Query update | `ValuePattern.SetValue("Project 09999")` filters to one row; set size 1 |
| Result identity | the filtered row keeps the RuntimeId observed before filtering |
| Text ranges | document read, emoji selection round-trip, positive selection bounds inside the field, point-to-text range inside the selected scalar |
| Caret boundaries | collapsed caret at document start and end round-trips; character movement before start / past end clamps; text unchanged |
| Rejection | oversized `SetValue` leaves text and selection unchanged and is announced through the polite live region (`Input not changed:`) |
| Scale | `WindowScalePercent = 100`; UIA bounds equal native physical pixels |
| Explicit non-claims | `ScreenReaderAcceptance = false`, `TextRangeAndSelectionAcceptance = false`, `ScalingMatrixVerified = "current monitor only; 150% and 200% unrun"` |

`inspect-pinned-uia.ps1` additionally records the shipping pin's structural absence:

| Check | Observed |
| --- | --- |
| Root | `ControlType.Pane`, owned by the fixture process |
| Descendants | 0 |
| Edit / List / labelled search | 0 / 0 / 0 |
| Semantic tree present | `false` |

This is negative structural evidence that `gpui 0.2.2` has no semantic tree; it is not a
screen-reader result (there is nothing for a screen reader to read).

## 4b. Launcher-shell UIA refresh (18 November, after the build fix)

The renderer owner replaced the 0.2.2-only `Timer` with
`cx.background_executor().timer(Duration::from_secs(2)).await`; the shell feature then built
(exit 0). The owned-process probe ran twice against the real `--a11y-fixture` launcher with
identical key fields, saved as
[`launcher-uia-observation-2026-11-18.json`](../../experiments/gpui-accessibility/launcher-uia-observation-2026-11-18.json)
and
[`…-rerun.json`](../../experiments/gpui-accessibility/launcher-uia-observation-2026-11-18-rerun.json).
This replaces the earlier blocked status for the shell; it is still UIA, not speech.

| Check | Observed |
| --- | --- |
| Fixture | 3 built-in commands; no discovery or hotkey registration |
| Results list | `ControlType.List` named `Search results`; realized positions `1,2,3`; item set size `3`; all rows onscreen; container set size `-1` |
| Selected row | `Integration Status. See what this development build supports` (title + subtitle) |
| Search field | labelled/focused Edit with ValuePattern; Unicode TextPattern document/selection/geometry checks pass |
| Text ranges | document read, emoji round-trip, positive bounds, point hit-test, caret at start/end, clamped boundary movement, oversized edit rejected atomically and announced through the polite live region |
| Actions | menu opens with independent filter; Pin/Unpin invoke; selected row keeps RuntimeId across reordering; menu items expose InvokePattern |
| Stale action | **not reachable**: `SelectionItem.Select` on an underlying row did not move selection while the menu was open (`StaleActionSelectionBlockedByMenu=true`); `StaleActionSurfaceExercised=false`, so no stale rejection is claimed |
| Details/Back | document label `Ansa`; Back closes details and returns search focus |
| Calculator error | exposed as `ControlType.Text` named `Cannot calculate: division by zero`; **no live event** carried the error (`CalculatorErrorLiveEventObserved=false`) |
| Live region at error time | status label `Unpinned for this session`; provider rejects UIA `LiveSetting` (`Unsupported Property` 30135) |
| Appearance | native chrome absent; glass default; glass/theme toggles preserve bounds and focus |
| Scale | `WindowScalePercent=100`; UIA bounds equal native physical pixels |
| Non-claims | `ScreenReaderAcceptance=false`, no clipboard or external launch invoked |

Two findings, both recorded as risks rather than passes:

1. **A11Y-21 (new):** the calculator error is a labelled static `ControlType.Text`; no polite or
   assertive live event observed, and the status live region carried unrelated text at that moment.
   Whether an AT speaks the static Alert on appearance is unverified.
2. **Stale-action rejection** cannot be invalidated through this read-only UIA flow; it remains
   unverified and is included in A11Y-11.

The shell probe activates only the window it starts and fails if Windows denies foreground. Neither
saved run was denied, but the probe failed intermittently during desktop use: several invocations
stopped at `Launcher search could not receive UIA focus; owned window is foreground: True` (or
`Action search did not receive focus`) and passed on retry. Two passing observations were saved;
failed attempts are not presented as passes. See A11Y-19.

## 5. What was not run, and why

| Area | Status | Reason |
| --- | --- | --- |
| NVDA journey (J1–J10) | **not run** | No AT available in this environment; speech cannot be captured. |
| Narrator journey | **not run** | Same. |
| VoiceOver journey | **not run** | No macOS host. |
| Orca / AT-SPI journey | **not run** | No Linux host; no Linux build; `accesskit_unix` is an optional feature. |
| Real IME composition | **not run** | No physical IME session was possible. |
| 150 % / 200 % scaling, high contrast | **not run** | Only the current 100 % monitor; changing system scaling is out of scope and was not done. |
| Launcher-shell UIA refresh (action panel, stale action, details, calculator error) | **refreshed** | Build fixed; see §4b. Two shell surfaces still fail to provide evidence (next two rows). |
| Calculator error live announcement | **not observed** | Static `ControlType.Text` only; no live event; status live region showed unrelated text. A11Y-21. |
| Stale-action rejection via UIA | **not reachable** | Underlying selection could not move while the menu was open; no synthetic keys/pointer were injected. |
| macOS/Linux builds and geometry | **not run** | No platform hosts/toolchains in scope. |
| Settings/form-like surface | **not applicable** | The launcher has no settings surface yet; nothing exists to inspect or announce. |
| Full offscreen provider enumeration | **not run** | Probe covers end/start and 9 realized rows only. |

### Launcher-shell compatibility break (fixed 18 November)

`crates/ui-gpui/src/host_lifecycle.rs` (commit `5b25e7b`) called `Timer::after`. In crates.io
`gpui 0.2.2`, `Timer` is `pub use smol::Timer`; in the pinned candidate revision `a19e1944` there
was no `Timer` export, so the experiment could not compile the launcher shell. The renderer owner
replaced it with `cx.background_executor().timer(Duration::from_secs(2)).await`; the build then
passed (item 12) and the shell probe was refreshed (item 13, §4b). This was not a change to the
probe or to screen-reader behaviour. The 12 November shell artifacts remain historical.

## 6. Narrowed-support question

**Can anything be declared supported today? No accessibility support claim.** Specifically:

- The shipping build has no accessibility tree, so "supported" would be false by construction.
- The candidate spike has a labelled UIA tree and passes structural checks. That is a developer
  prototype, not a supported product surface.
- Keyboard-only operation is not assistive-technology support. It may be described as
  "keyboard-operable" only, with no AT implication.
- No platform (Windows, macOS, Linux) can be declared supported; Windows is the only platform with
  any structural evidence, and it has no AT journey.

If a public statement is required, use: *"No assistive-technology support is claimed. The shipping
build exposes no accessibility tree; a newer framework candidate is under evaluation and has not
completed screen-reader acceptance."*

## 7. Risks and next actions

Missing semantics are recorded with owners and stop conditions in the
[risk register](../risk-register.md). The blocking set is A11Y-01/02 (unpromoted bridge), A11Y-03/04
(no Windows AT), A11Y-05 (no VoiceOver), A11Y-06 (no Orca), plus the per-semantic gaps
A11Y-08…A11Y-21 (logical position, live regions, text ranges, actions/errors, RTL, IME, scaling,
offscreen navigation, selection-focus model, settings surface, probe reliability, cross-platform,
calculator-error announcement). A11Y-07 (shell build) is fixed but the shell's AT evidence remains
open. No risk is closed.

Next actions, in order:

1. Desktop-shell owner wires the calculator error into the live region (or confirms the Alert role
   is announced by real AT) and adds a reachable stale-action journey (A11Y-21, A11Y-11).
2. Framework/release owners decide the pin promotion on the candidate with the licence and
   compatibility record (A11Y-01/02).
3. Accessibility verification owner schedules two-tester NVDA and Narrator runs on the resulting
   build, then VoiceOver and Orca (A11Y-03…06).
4. Only then are J8–J10 (IME, RTL, scaling) meaningful; schedule them with the same builds.

## 8. Explicit non-claims

- F08 is **not** passed, **not** closed, and **not** partially accepted by this document.
- No screen-reader speech transcript exists here, and none was simulated.
- UIA events, tree dumps, geometry and keyboard smokes are supporting evidence only.
- The framework gate and implementation status were not changed by this work.

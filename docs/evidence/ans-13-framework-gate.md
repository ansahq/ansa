# ANS-13 / F10 - framework gate review and decision evidence

Date: 2026-11-18
Branch: `ans-foundation-evidence` (working tree `a275972c1ee21bfca0773585d4954db67fedd7d8`
plus uncommitted review documents; no commit or branch switch)
Reviewer role: independent reviewer and decision author
Decision: **narrow Windows development support; release accessibility gate remains open**
Decision record: [`docs/adr/0004-framework-gate-decision.md`](../adr/0004-framework-gate-decision.md)
Gate: [`docs/framework-gate.md`](../framework-gate.md)
Risks: [`docs/risk-register.md`](../risk-register.md)

## 1. Scope and method

I re-read the ANS-13 ticket, blueprint sections 3/16/19, the implementation backlog sections
1/2/8, `docs/framework-gate.md`, ADRs 0001-0003, all six F04-F09 evidence records, the ANS-15
accessibility record, the risk register, and the relevant host/platform-api/platform-windows/
core/ui-gpui/ui-components/bench code. I then reran the gate commands independently on this
machine. I did not edit crates, Cargo files, ADRs 0001-0003, benchmark data, accessibility
documents, or another agent's evidence file.

No NVDA, Narrator, VoiceOver, Orca, Accessibility Insights, Accerciser, or physical IME session
was available. The shell UIA probe was attempted twice and failed before writing a JSON file;
that is recorded as a non-reproduction, not a pass.

## 2. Environment

- Windows 11 `Microsoft Windows NT 10.0.26200.0`, x86_64.
- Rust 1.99.1 (`58cf8f9c3 2026-10-13`), Cargo 1.99.1.
- GPUI pin: crates.io `gpui 0.2.2`; candidate: immutable Zed revision
  `a19e19440d4102668830a9da579c3ee71d2fbf1a`.
- No network was needed for the workspace tests; `prepare.ps1` did download the 24.54 MiB
  candidate archive after `cargo clean` removed its cached copy.

## 3. Exact commands and results

### 3.1 The stale-artifact incident (important)

The first `cargo test --workspace` on the existing target directory passed **292 tests** and
`cargo clippy --workspace --all-targets -- -D warnings` exited 0, but the built
`target/debug/ansa-desktop.exe` did **not** contain the working tree's
`ANSA_PERF_FIRST_FRAME_US` string and its `.d` dependency file omitted `perf.rs`. A manual
`--perf-smoke-test` run hung with no marker, and `ansa-bench quick` reported three launch
failures. This was an artifact/source mismatch, not a passing performance fixture.

Commands and observations before the clean rebuild:

```text
$ cargo test --workspace
# 292 passed, 0 failed; 1 doctest passed; 1 doctest ignored
$ cargo run -q -p ansa-desktop -- --interaction-smoke-test
# 46 stages passed
$ cargo run -q -p ansa-desktop -- --lifecycle-smoke-test --lifecycle-cycles=1000
# succeeded=1000 denied=0 skipped=0 unsupported=0
$ cargo run -q -p ansa-bench -- quick --output-dir=<temp>
# launch_failures: p50=3.0; all launch markers unavailable
```

A forced `cargo clean` (5.4 GiB, 27,386 files) followed by a clean build produced the
authoritative results below. `cargo clean` also removed
`target/framework-candidates/pinned-zed`, so the candidate build initially failed with
`failed to read .../crates/gpui/Cargo.toml` until `prepare.ps1` re-materialized it.

The hazard recurred later in the same review: an incremental
`cargo test --workspace --locked` failed with
`error[E0599]: no method named glass_background found for struct Theme`, even though
`crates/ui-components/src/theme.rs` defines that method. `cargo clean -p ansa-ui-components`
followed by `cargo test --workspace --locked` passed 291 tests. This is the same
artifact/source mismatch, not a source regression; the shared/incremental target directory is
not trustworthy for gate evidence.

### 3.2 Clean workspace tests

```text
$ cargo clean
# Removed 27386 files, 5.4GiB
$ cargo test --workspace
# exit 0
# 291 passed, 0 failed; 1 doctest passed; 1 doctest ignored
```

Per binary: bench 25, CLI 6, core 44 unit + 10 fuzz + 3 navigation property, domain 6, host
44, platform-api 28, platform-windows 61, search 14, ui-components 45 + 4 integration,
ui-gpui 1. Total 291.

The stale run's 292nd test was an old `visibility` test name that no longer exists in the
working tree (`glass_...` vs the current `alpha_...`), another sign the artifact set did not
match the source.

### 3.3 Clippy

```text
$ cargo clippy --workspace --all-targets -- -D warnings
# exit 0
# warning: proc-macro-error2 v2.0.1 future incompatibility (pre-existing)
```

No new warning was introduced by the reviewed code.

### 3.4 Synthetic interaction smoke

```text
$ cargo run -q -p ansa-desktop -- --interaction-smoke-test
Ansa interaction smoke: 46 synthetic widget/action stages passed, including typed view
restore, Escape ordering, stale action refs, Tab/Shift+Tab cycles, glass and theme toggling.
No clipboard writes or external app launches.
# exit 0
```

### 3.5 Native lifecycle smoke

```text
$ cargo run -q -p ansa-desktop -- --lifecycle-smoke-test --lifecycle-cycles=1000
ANSA_LIFECYCLE_FOCUS succeeded=1000 denied=0 skipped=0 unsupported=0
ANSA_LIFECYCLE_OK cycles=1000
# exit 0
```

This is a real native show/hide cycle count on this machine. It does **not** prove focus
always succeeds: the ANS-9 record also contains a `denied=1000` run, and no physical keystroke
was sent after denial.

### 3.6 Debug performance quick profile

```text
$ cargo run -q -p ansa-bench -- quick --output-dir=<temp>\ansa-review-bench-clean-facb61bcf6504fe7bcfcffc5c6fb5ae2
# exit 0; harness duration 78.448 s; profile debug
# launch_failures: 0 (3 launch samples)
# cold_launch_first_frame_internal p50: 224,468 us
# cold_launch_first_frame_wall p50: 564,693 us
# warm_launch_first_frame_internal p50: 224,877.5 us
# warm_launch_first_frame_wall p50: 558,234 us
# frame_interval p50: 16,641.5 us (n=180)
# query_to_results p50: 96,213.15 us, p95 101,636.78 us, p99 109,884.345 us (n=160)
# keystroke_to_list p50: 87,201.0 us, p95 109,926.52 us, p99 113,265.496 us (n=340)
# memory_working_set: 12,697,600 bytes
# memory_private_bytes: 8,081,408 bytes
# memory_pss: 7,399,936 bytes
# idle_cpu_percent_one_core: 0.0 over 5.001 s
# budget: not defined
```

The saved release-harness run remains
[`docs/benchmarks/runs/20261118T080933968Z`](../benchmarks/runs/20261118T080933968Z/summary.md)
(query p50 11,945.65 us). The reviewer debug run is not comparable and is not a release
measurement. Fresh raw output was written to the temp path above; it is not committed because
this review owns only the five documents named in the ticket.

### 3.7 Candidate shell build

```text
$ cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml \
    --features launcher-shell --bins
# first attempt: failed to read target/framework-candidates/pinned-zed/crates/gpui/Cargo.toml
$ pwsh -NoProfile -File experiments/gpui-accessibility/prepare.ps1
# downloaded 24.54 MiB; archive SHA-256 verified
# 35D3D5C2AF07BB4F7FDCF7DDA594990730DF8C8B94E994DF7729CB19F55C5E2F
$ cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml \
    --features launcher-shell --bins
# exit 0; Finished dev profile in 21.60 s
```

The candidate build is not hermetic from a clean checkout unless `prepare.ps1` can download
the archive; that is recorded as GATE-04.

### 3.8 Owned-process UIA shell probe (failed twice)

```text
$ powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 \
    -LauncherShell -OutputPath <temp>\ansa-review-shell-uia-2e847d62...json
Windows denied foreground activation of the owned fixture: hwnd=460986; pid=26864; title=
# exit 1; no JSON written

$ powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 \
    -LauncherShell -OutputPath <temp>\ansa-review-shell-uia-retry-259ee7ac...json
Windows denied foreground activation of the owned fixture: hwnd=460986; pid=26864; title=
# exit 1; no JSON written
```

The candidate process started and printed `ANSA_UIA_READY` in a separate manual check, so this
is a foreground-activation denial in the automation session, not a build/startup failure. Per
A11Y-19, two identical failures are recorded as a **non-reproduction**, not a pass. The saved
prior observations in `experiments/gpui-accessibility/launcher-uia-observation-2026-11-18*.json`
were not independently reproduced here.

### 3.9 Shipping-pin negative UIA probe (passed)

```text
$ powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-pinned-uia.ps1 \
    -OutputPath <temp>\ansa-review-pinned-uia-409aeb267e754661b698ff6327952246.json
# exit 0; owned fixture process only
{
  "RootControlType": "ControlType.Pane",
  "DescendantCount": 0,
  "ControlTypes": {},
  "NamedDescendants": [],
  "LabelSearchElements": 0,
  "EditElements": 0,
  "ListElements": 0,
  "SemanticTreePresent": false,
  "ScreenReaderAcceptance": false
}
```

This independently confirms the main pin has no semantic tree. It is negative structural
evidence, not a screen-reader result.

### 3.10 Release build of the renderer

```text
$ cargo build --release -p ansa-ui-gpui
# exit 0; Finished release profile in 3m 57s
# only the pre-existing proc-macro-error2 future-incompat warning
```

### 3.11 Hotkey availability probe

```text
$ cargo run -q -p ansa-cli -- hotkey status
capability: available
  global summon can be registered
# exit 0
```

This contradicts the earlier ANS-11 session's `capability: degraded` observation for the same
chord. The chord is process/session-dependent; neither observation is a promise. The desktop
still registers the fixed default and does not use the store/manager (GATE-02).

### 3.12 Process cleanup

After each run I checked for `ansa-desktop`, `ansa-bench`,
`ansa-launcher-accessibility-candidate`, and `ansa-gpui-accessibility-spike`. No matching
process remained at the end of the review. One earlier `ansa-desktop` process (PID 16232,
started by a timed-out manual perf probe) held the debug executable; I stopped it before the
clean rebuild. No process was left intentionally running.

## 4. Code review findings

This was a bounded correctness/security review, not a full audit. Severity is about impact if
the condition is exercised.

| # | Severity | Finding | Evidence / file | Disposition |
|---|---|---|---|---|
| 1 | **High (evidence integrity)** | Incremental artifacts passed tests while not matching the working tree. The binary lacked `ANSA_PERF_FIRST_FRAME_US`; the `.d` file omitted `perf.rs`; the test count differed by one. A clean rebuild was required. | `target/debug/ansa-desktop.exe`, `.d` files; `crates/ui-gpui/src/lib.rs:1466-1481`; `crates/ui-gpui/src/perf.rs` | Not an app-code defect. Gate evidence must use clean builds; GATE-01. |
| 2 | **Low (design fragility)** | `execute_panel` resolves an `ActionRef` and validates the target, but `run_resolved` then ignores `ResolvedAction::target` and re-reads `engine.selected()` / `engine.query()`. The current path is synchronous, so resolve and execution cannot diverge today. Any future async/yield between resolve and execution would act on current state instead of the validated target. | `crates/ui-gpui/src/lib.rs:722-756`, especially `:741-747`; `execute_primary` at `:374-418` | Current behavior is safe because resolve checks invocation/revision/view/generation/target immediately before the synchronous call. Recorded as a hardening note, not a present exploit. |
| 3 | **Medium (documented limitation, untested)** | `dismiss_from_host` refuses to hide when no hotkey is registered. When `Ctrl+Space` is unavailable (ANS-11 observed that state; the reviewer's later probe reported available), root Escape is inert and only `Ctrl+Q` quits. The README documents "root Escape hides only when a global shortcut was successfully registered"; the 46-stage smoke does not cover the failure path. | `crates/ui-gpui/src/host_lifecycle.rs:290-293`; `README.md` boundaries | Not a hidden defect, but a support limitation and an untested branch. GATE-05. |
| 4 | **High (scope gap, documented)** | Hotkey persistence/rebinding is not wired into desktop startup. `HotkeyStore`/`HotkeyManager` are tested in `ansa-platform-api`, but `crates/ui-gpui/src/lib.rs:1506-1519` still calls `HotkeyRegistration::register` for the fixed default. No GPUI rebinding UI; no automatic sleep/lock/session rearm. | `crates/ui-gpui/src/lib.rs:1506`; ANS-11 "Persistence/restart"; no reference to `HotkeyStore`/`HotkeyManager` in `crates/ui-gpui` or `apps/desktop` | Honest scope gap; F07 remains open. GATE-02. |
| 5 | **Medium (evidence reliability)** | The clean candidate shell probe failed foreground activation twice. The fixture starts and prints `ANSA_UIA_READY`, but the probe's `SetForegroundWindow` is denied in this automation session, so no shell JSON was produced. | `experiments/gpui-accessibility/inspect-uia.ps1:51`; `UiaProbe.cs:244-253` | Saved prior observations remain historical; not independently reproduced. A11Y-19. |
| 6 | **Medium (build reproducibility)** | The candidate source cache lives under `target/framework-candidates`; `cargo clean` deletes it, and `prepare.ps1` must re-download the 24.54 MiB archive. The archive is hash-verified, but clean/offline CI needs an explicit prepare/cache step. | `experiments/gpui-accessibility/prepare.ps1`; `Cargo.toml` path deps | GATE-04. |
| 7 | **Low (doc drift)** | `docs/implementation-status.md` and `docs/framework-gate.md` still described older test counts/stage counts and did not record the ANS-13 decision. `README.md` still says 30 smoke stages (current 46). | `docs/implementation-status.md`, `docs/framework-gate.md`, `README.md` | Corrected in the two owned docs; README was not edited (not owned). |
| 8 | **Informational (no defect found)** | `ActionRef` resolution rejects `UnknownAction`, `StaleInvocation`, `CrossView`, `ExpiredView`, and `StaleTarget` with fixed precedence. A delayed callback from a popped frame is `CrossView`; a same-view new-generation ref is `ExpiredView`; a query/catalog change is `StaleTarget`/`StaleInvocation`. The model fuzz and 46-stage smoke cover these. | `crates/core/src/action_state.rs:322-371`; `crates/core/tests/action_state_fuzz.rs`; `crates/ui-gpui/src/smoke.rs:331-405` | No current path found for a delayed/cross-view ref to execute. Finding 2 is the hardening note. |
| 9 | **Informational (no defect found)** | Input size checking runs before mutation; UTF-16 range conversion rounds down inside surrogate pairs and clamps/normalises reversed ranges; composition cancel restores text/selection/direction and keeps redo; oversized paste/marked updates reject atomically. | `crates/ui-components/src/text_input/model.rs:219-274, 358-386, 510-626`; `crates/ui-components/tests/input_contracts.rs` | Tests pass. Physical IME/AT/scaled RTL remain unverified; no claim. |
| 10 | **Informational (no critical defect found)** | Host lifecycle uses opaque invocation ids, checks stale ids before native effects, separates visible/focus outcomes, and runs the ordered release plan once. `Drop` plus controller shutdown is idempotent. | `crates/host/src/controller.rs:224-362, 472-532`; `crates/ui-gpui/src/host_lifecycle.rs` | One-machine evidence only. Focus outcome varied across runs; no sleep/lock/fullscreen coverage. |
| 11 | **Low (shutdown retry)** | `HostController::shutdown` marks `shutdown = true` before running release steps, so a failed step is reported once and never retried; a later `Drop`/`shutdown` returns `already_shut_down`. `LauncherBackend::release(NativeHandles)` also ignores the hide error. | `crates/host/src/controller.rs:472-499`; `crates/ui-gpui/src/host_lifecycle.rs:176-199` | Bounded because this is a process-exit path and OS hotkey registrations die with the process; recorded as a residual lifecycle note, not a gate blocker. |
| 12 | **Informational (known limitation)** | Stale-HWND checks use `IsWindow` plus owner thread/process IDs. A same-process/same-thread recycled HWND cannot be fully distinguished from the original. | `crates/platform-windows/src/lifecycle.rs:295-325`; ADR 0001 | Already documented in ADR 0001; no new gate action beyond the existing lifecycle limitation. |

## 5. Decision rationale

The evidence supports **using** the Windows GPUI slice for development, not **accepting** it
for release:

- The shipping pin provably has no semantic tree (0 descendants). The candidate has a
  semantic tree and bounded automated UIA, but no real AT journey, no promotion, and the shell
  probe did not reproduce in this session.
- Physical IME, RTL display, 150/200 % scaling, macOS and Linux are unverified or unbuilt.
- The performance harness is honest but has no budget, no `full`, and no `soak`.
- Hotkey rebinding/lifecycle is not wired end-to-end.
- F02 is still provisional, so greenfield is not final.

A blanket accept would overstate all of those. A pivot to Slint would be premature because the
candidate bridge exists and the remaining work is bounded acceptance/tooling, not a
demonstrated framework impossibility. The honest decision is to **narrow** current support to
Windows 11 x86_64 keyboard-first development and keep the release gate open. The Slint trigger
is written down and has not been run.

## 6. Gate status before and after

- **Before:** `docs/framework-gate.md` said "open/blocking before broad feature expansion"
  with no F10 decision, no support matrix, and no statement of which Windows development
  configuration (if any) was usable.
- **After:** the gate is still **open and release-blocking**, but the support set is explicit:
  Windows 11 x86_64 keyboard-first development on `gpui 0.2.2`, automated-only; candidate
  unpromoted; no accessibility, IME, scaling, cross-platform, or performance claim. The
  candidate/Slint trigger is documented in ADR 0004.

## 7. Explicit non-claims

- F08 is **not** passed, closed, or partially accepted by this document.
- No NVDA, Narrator, VoiceOver, or Orca speech transcript exists here.
- No physical IME, RTL display, dead-key/AltGr, or scaled-display acceptance was performed.
- macOS and Linux were not built; no cross-platform support is claimed.
- No performance budget is defined or met; the `quick` debug numbers are not release numbers.
- The candidate is not the shipping product and is not promoted.
- Keyboard-only operation and UIA tree dumps are not assistive-technology support.
- `gpui 0.2.2` remains the shipping pin; the root `Cargo.lock` is unchanged.

## 8. Artefacts

- Saved release `quick` run:
  [`docs/benchmarks/runs/20261118T080933968Z/raw.jsonl`](../benchmarks/runs/20261118T080933968Z/raw.jsonl)
  and [`summary.md`](../benchmarks/runs/20261118T080933968Z/summary.md).
- Reviewer debug `quick` raw output (temp, not committed):
  `C:\Users\abhir\AppData\Local\Temp\ansa-review-bench-clean-facb61bcf6504fe7bcfcffc5c6fb5ae2\raw.jsonl`.
- Shipping-pin negative observation (existing):
  `experiments/gpui-accessibility/default-pin-uia-negative-observation-2026-11-18.json`.
- Reviewer negative observation (temp):
  `C:\Users\abhir\AppData\Local\Temp\ansa-review-pinned-uia-409aeb267e754661b698ff6327952246.json`.
- Candidate shell observations (historical, not reproduced here):
  `experiments/gpui-accessibility/launcher-uia-observation-2026-11-18.json` and
  `...-rerun.json`.
- Candidate source archive hash: `35D3D5C2AF07BB4F7FDCF7DDA594990730DF8C8B94E994DF7729CB19F55C5E2F`.

## 9. What would falsify this decision

This decision should be revisited if any of the following occurs:

1. A promoted candidate passes two-tester NVDA and Narrator J1-J10 on Windows, then VoiceOver
   and Orca on built macOS/Linux configurations.
2. Physical IME, RTL, and 100/150/200 % scaling pass on real hardware.
3. Release-mode `full` and 72-hour `soak` meet a named budget.
4. A clean-build CI run fails, or repeated gate runs cannot reproduce the counts above.
5. The candidate cannot pass the AT journeys after the bounded two-week fix effort; then run
   the same-slice Slint comparison described in ADR 0004.

Until then, the support matrix in ADR 0004 and the gate in `docs/framework-gate.md` are the
authoritative limits on what may be claimed.

## 10. Follow-up verification after review

Two follow-up actions were taken after this review, without changing the decision:

1. **Clean rebuild (GATE-01).** Every workspace package was cleaned with `cargo clean -p`
   (the external candidate archive under `target/framework-candidates` was preserved), then
   `cargo test --workspace --locked` rebuilt from source and passed **292 tests, 0 failed**,
   with one doctest passed and one ignored. `cargo clippy --workspace --all-targets --locked
   -- -D warnings`, `cargo fmt --all -- --check`, the 46-stage interaction smoke and the
   1000-cycle lifecycle smoke all passed. This is the authoritative clean-build count; the
   review's 291 was from an earlier clean state before the benchmark crate's final test count.
   The latest 1000-cycle lifecycle rerun observed `succeeded=0 denied=1000`, while an earlier
   run observed `succeeded=1000`; the variance is recorded, not resolved.
2. **Root dismissal (GATE-05).** `dismiss_from_host` no longer refuses to hide when no global
   hotkey is registered. It hides, records the fallback notice, and the 46-stage interaction
   smoke now asserts `LifecycleState::Hidden` after root Escape with no hotkey registered.
   This is synthetic evidence; a physical run with the hotkey owned by another application
   remains the next verification.

These changes reduce GATE-01 evidence uncertainty and mitigate GATE-05. They do not promote
the candidate, run real AT, close F08, or change the narrowed support matrix.

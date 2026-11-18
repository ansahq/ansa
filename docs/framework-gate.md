# Framework accessibility gate - open, release-blocking, development narrowed

Status: **OPEN and blocking for release**, updated 2026-11-18. This is not a generic
"blocking" one-liner: the gate decision is recorded in
[ADR 0004](adr/0004-framework-gate-decision.md). It narrows what may be built and claimed
today to a **Windows 11 build 26200 x86_64, keyboard-first development foundation on the pinned
`gpui 0.2.2` renderer**. It does not accept GPUI for release, does not promote the newer
candidate, and does not close F08 or mark the release gate closed.

The short version:

- The shipping pin has **no accessibility tree**. An independent owned-process UIA probe on
  2026-11-18 saw a `ControlType.Pane` root and **0 descendants**.
- The isolated candidate at `a19e1944d4102668830a9da579c3ee71d2fbf1a` builds and has saved
  automated UIA observations, but it is **unpromoted** and **no real screen reader was run**.
  Its shell probe was denied foreground activation twice in the independent review session;
  that is recorded as a non-reproduction, not a pass.
- Physical IME, scaled RTL, 150/200 % scaling, macOS/Linux, and performance budgets remain
  unverified or unbuilt. Automated UIA and keyboard smokes are not assistive-technology
  evidence.
- The bounded Slint comparison is **not triggered now**; its trigger is in ADR 0004 and the
  comparison has not been run.

## Gate matrix (F01-F09)

| Item | Evidence | Tested result | Limitation | Blocking? |
|---|---|---|---|---|
| F01 reference scope | [`outputs/research-and-sources.md`](../outputs/research-and-sources.md) | Documentation-level reference map | No reference binary/build hash, no 100-scenario ledger, no reference app executed | Compatibility/parity claims only |
| F02 reuse evaluation | [`ADR 0003`](adr/0003-reuse-evaluation.md) | Bounded Gauntlet source inspection; greenfield retained provisionally | F02 is **not closed**; no full maintainer/test/licence/100-scenario ledger | Production investment |
| F03 workspace/CI | `.github/workflows/ci.yml`, `Cargo.lock`, `rust-toolchain.toml` | Local clean build/test/Clippy pass; CI workflow exists | No remote CI evidence; no three-OS desktop builds; no SBOM/full licence inventory | Release |
| F04 host lifecycle | [`ANS-9`](evidence/ans-9-host-lifecycle.md) | Windows monitor/theme/placement/show/hide, invocation identity, ordered shutdown; reviewer 1000 cycles ended hidden with `succeeded=1000`; a later clean-run rerun was `denied=1000` | One machine/GPU/topology; focus outcome varied between denied and succeeded; no sleep/lock, fullscreen, virtual desktop, unplug, mixed-DPI hardware, or physical key after denial | Production lifecycle |
| F05 shared input | [`ANS-10`](evidence/ans-10-input-contracts.md) | 45 library + 4 integration tests for graphemes, undo/redo, IME transaction/cancel, UTF-16, oversize rejection, logical RTL, a11y ranges | No physical IME, no AT, no scaled RTL, no OS clipboard integration | F08/release |
| F06 navigation/actions | [`ANS-8`](evidence/ans-8-navigation-actions.md) | Typed view stack, restore-once parent state, `ActionRef`, 46-stage synthetic smoke, model/property fuzz | Synthetic keystrokes only; no physical keyboard/AT; no pixel-exact pointer scroll | Release UX claim |
| F07 hotkeys | [`ANS-11`](evidence/ans-11-hotkeys.md) | Neutral chord/config/store/manager/recorder; Windows arbitrary-chord worker; typed conflict/denial; 1000-cycle worker and manager tests; CLI fallback | Desktop still registers fixed `Ctrl+Space`; no store/manager/rebind UI wiring; no automatic sleep/lock observer; no macOS/Linux adapter; ANS-11 saw the chord owned, the reviewer probe saw it available (session-dependent) | F07 and any reliable-shortcut claim |
| F08 accessibility | [`ANS-15`](evidence/ans-15-accessibility-acceptance.md), [protocol](accessibility/screen-reader-acceptance.md), [risk register](risk-register.md) | Shipping pin: 0 UIA descendants. Candidate: automated UIA roles/labels/positions/text ranges from saved 18 Nov observations | **No NVDA/Narrator/VoiceOver/Orca journey.** Candidate shell probe not reproduced in review (two foreground denials). Candidate unpromoted. | Yes, critically |
| F09 performance | [`ANS-12`](evidence/ans-12-performance-harness.md), [saved run](benchmarks/runs/20261118T080933968Z/summary.md) | `quick`, `cancel-flood`, `delayed-provider` run; reviewer `quick` had 0 launch failures, p50 query 96,213.15 µs (debug harness) | No budget; `full`/`soak` unrun; debug-only launcher fixture; no physical input/paint/display timing; GPU unavailable | Any latency/memory/power claim |
| F10 gate | [`ADR 0004`](adr/0004-framework-gate-decision.md), [`ANS-13`](evidence/ans-13-framework-gate.md) | Narrow Windows development decision with owned stop conditions | Release gate intentionally remains open | Yes by design |

## Support matrix

| Configuration | Evidence level | Scaling | Screen reader | Known limitations |
|---|---|---|---|---|
| Windows 11 build 26200 x86_64, single monitor, `gpui 0.2.2`, debug developer build | **Tested (automated only)** | 100 % on this machine; 100/150/200 % placement only unit-tested with synthetic monitor data | **None.** Pin has no semantic tree; zero UIA descendants | One machine/GPU; synthetic keyboard/IME; focus varied between denied and succeeded; hotkey may be owned; no product support |
| Windows 11 build 26200 x86_64, candidate `a19e1944` launcher shell, debug build | **Automated UIA only** (saved observations); reviewer rerun denied | 100 % observed; 150/200 % unrun | **None.** UIA is not AT speech; no NVDA/Narrator journey | Candidate unpromoted; not in shipping build; stale-action surface unreachable via UIA; calculator error had no live event; focus contention can deny the probe |
| Windows 11 build 26200 x86_64, 150 %/200 %, mixed DPI, multi-monitor, high contrast | **Compilable/unit-tested only** | Not observed on hardware | None | No hardware scaling matrix, no mixed-DPI geometry, no high-contrast journey |
| Windows 11 DWM/DirectComposition glass path | **Compilable/geometry-checked only** | 100 % | None | Native rounded region/alpha setup is checked; visible desktop transparency, shadow quality, remote-desktop/disabled-composition behavior, and native blur are unverified; falls back to opaque with "Glass: Unavailable" |
| Windows 10 or earlier | **Unsupported/unverified** | None | None | No build or test evidence; Windows 11 findings are not assumed to transfer |
| macOS 14/15 | **Unsupported/unbuilt** | None | None | No macOS build, no VoiceOver, no hotkey adapter |
| Linux Wayland/X11 | **Unsupported/unbuilt** | None | None | No Linux build, no Orca/AT-SPI, no hotkey adapter |
| Windows 11 ARM64 / other GPUs/compositors | **Unsupported/unverified** | None | None | No build or driver evidence |

A compile is not a runtime result. "Automated UIA only" is not "screen-reader tested".
Windows 100 % automated UIA is not a Windows support claim.

## Why the gate stays open

The evidence is useful but narrower than the ticket checklist:

1. It is one Windows machine, one GPU, one monitor topology, and one 100 % scale. macOS and
   Linux were not built.
2. UIA properties and events are not screen-reader speech or a completed user journey.
3. The input, navigation, and IME results are synthetic. No physical keyboard, physical IME,
   dead-key/AltGr, RTL display, or scaled caret was exercised.
4. The shipping pin has no accessibility bridge; the candidate is an isolated experiment
   that depends on a downloaded Zed archive under `target/framework-candidates`.
5. No performance budget exists, and `full`/`soak` were not run. The debug fixture's numbers
   are not release numbers.
6. Hotkey persistence/rebinding is tested at contract level but not wired into desktop
   startup; there is no automatic sleep/lock/session rearm.
7. Incremental build artifacts can pass tests while not matching the source. The review had
   to force a clean rebuild before the performance fixture appeared in the binary. Gate
   evidence must come from a clean build. A follow-up clean rebuild of every workspace package
   passed 292 tests, but the hazard remains a CI process risk.

## Local source inspection (unchanged finding)

The design's current-main GPUI accessibility research is not evidence about the published
version used here. Ansa pins `gpui = 0.2.2`, whose crates.io archive checksum is
`979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707` (also recorded in
`Cargo.lock`).

A case-insensitive search of that archive's `src/` and `Cargo.toml` for `accesskit`,
`accessibility`, `UIAutomation`, and `IAccessible` found no accessibility bridge or semantic
element API. Broader `accessible` matches were unrelated documentation/clipboard comments.
There is no AccessKit dependency in this GPUI manifest. Ansa's custom input supports the
pinned native text-input/IME contract, but that is not a screen-reader semantic
implementation.

This is a bounded **source-inspection finding**, not a live NVDA/Narrator test and not a claim
about all GPUI versions. Native two-frame and synthetic action tests do not validate
accessibility trees, announcements, text navigation, or IME behavior.

The independent negative probe on 2026-11-18 confirms the runtime consequence: an active UIA
client attached to the owned `--a11y-fixture` process sees a root pane with 0 descendants
(`SemanticTreePresent=false`, `EditElements=0`, `ListElements=0`,
`LabelSearchElements=0`). This is structural absence, not a screen-reader result.

## Bounded newer-revision experiment

The independent [candidate workspace](../experiments/gpui-accessibility/README.md) builds
against upstream revision `a19e19440d4102668830a9da579c3ee71d2fbf1a` after
`prepare.ps1` materializes the hash-verified archive. Its saved Windows UIA observations cover
search role/label/value/focus, list/item roles, selected state, logical position 1 of 10,000,
text ranges/selection/geometry, live-region events, and an action/details/calculator-error
shell refresh. Details are in [ADR 0002](adr/0002-accessibility-investigation.md) and
[ANS-15](evidence/ans-15-accessibility-acceptance.md).

These are structural checks, **not** screen-reader acceptance. The candidate shell probe also
failed twice in the independent review session at owned-window foreground activation, so its
saved observations were not independently reproduced here. The candidate remains unpromoted;
the shipping pin and root `Cargo.lock` are unchanged.

## What must happen before the gate can close

1. Promote a renderer with a real semantic bridge, with a recorded hash, licence/dependency
   inventory, and rollback plan.
2. Run the protocol J1-J10 with NVDA and Narrator on Windows, then VoiceOver and Orca on built
   macOS/Linux configurations. Attach speech-viewer/log artefacts and a second tester's
   agreement.
3. Run physical IME (CJK, Hindi, emoji, dead keys/AltGr), RTL/mixed-bidi, and 100/150/200 %
   scaling on real hardware. Confirm caret placement and candidate-window alignment.
4. Fix the known missing semantics: calculator-error live announcement, reachable
   stale-action rejection, selection/focus ownership, offscreen list navigation, and form
   semantics when a form exists.
5. Define and meet performance budgets with release-mode `full` and 72-hour `soak` traces on
   named hardware.
6. Close F02 or explicitly record the greenfield decision as production-final. Complete the
   licence/SBOM and clean-build CI evidence.

## Release and marketing rule

Until every item above is recorded, do not claim accessibility, screen-reader support, IME
support, RTL support, scaled-display support, macOS/Linux support, cross-platform behavior,
performance-budget compliance, production readiness, or Raycast compatibility. Do not use the
candidate's UIA results as a product claim. Do not close F08 or treat this F10 decision as
release acceptance; the release gate remains open. Suitable wording:

> "A Windows-only keyboard-first foundation is under development. The shipping build exposes
> no accessibility tree. A newer framework candidate is under evaluation with automated UIA
> evidence only; no screen-reader, physical-IME, scaled-display, cross-platform, or
> performance-budget acceptance has been completed."

## Other pinned-build maintenance finding

`cargo report future-incompatibilities --id 1` identifies `proc-macro-error2 2.0.1`
re-exporting a private `extern crate proc_macro` (future error E0365, Rust issue
[127909](https://github.com/rust-lang/rust/issues/127909)). `cargo tree -p ansa-desktop -i
proc-macro-error2 --locked --offline` resolves the chain as GPUI -> `stacksafe 0.1.4` ->
`stacksafe-macro 0.1.4` -> `proc-macro-error2 2.0.1`. Current Rust 1.99.1 builds pass; this is
not a claim of indefinite future compiler compatibility. No cached dependency was patched or
silently vendored.

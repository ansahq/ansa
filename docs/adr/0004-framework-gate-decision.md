# ADR 0004: framework gate - narrow Windows development support, release gate open

Date: 2026-11-18 · Status: **narrowed; release accessibility gate remains open and blocking**

Decision owner: release gate owner, with the framework/renderer owner and accessibility
verification owner. This ADR records the F10 decision requested by ANS-13. It does not close
F08 or the release gate, and it does not close F02.

## Decision

Retain `gpui = 0.2.2` as the **Windows-only development renderer** for a keyboard-first
foundation slice. Do **not** accept GPUI for release, and do **not** declare any platform or
accessibility support.

Specifically:

1. The supported development configuration is Windows 11 build 26200 x86_64 (the reviewed
   machine), a single 100 %-scale monitor, the crates.io `gpui 0.2.2` pin, and a local
   developer build. The evidence for that configuration is automated only.
2. The immutable candidate at `a19e1944d4102668830a9da579c3ee71d2fbf1a` is the only
   runtime-probed path to a semantic tree. It remains **unpromoted** and is not supported.
3. The release accessibility gate stays open. No screen-reader, physical-IME, scaled-RTL,
   cross-platform, or performance-budget claim is permitted.
4. The bounded Slint comparison is **not triggered now** because a candidate bridge exists,
   builds, and has bounded UIA evidence. The trigger is stated below and remains available.
5. Greenfield remains provisional until F02 closes. This ADR narrows what may be built on the
   current foundation; it does not make the reuse decision final.

## What "narrowed" means

| Term | Meaning in this ADR |
|---|---|
| Development-supported | A developer may build and test this slice on the named Windows configuration. It is not a product support statement. |
| Automated-only | Tests or probes passed without a human screen reader, physical keyboard/IME, or second machine. |
| Compilable-untested | The code has a build path but no runtime evidence on that configuration. |
| Unsupported | No build, adapter, test, or acceptance evidence exists. |

## Gate evidence (F01-F09)

| Item | Evidence | Tested result | Limitation | Blocks release? |
|---|---|---|---|---|
| F01 reference scope | [`outputs/research-and-sources.md`](../../outputs/research-and-sources.md), product reference map | Documentation-level feature map; no reference binary/build hash, no 100-scenario ledger, no reference app executed | Compatibility scope is not frozen; docs are mutable web sources | Yes for a compatibility or parity claim |
| F02 reuse evaluation | [`ADR 0003`](0003-reuse-evaluation.md), bounded Gauntlet inspection | Greenfield retained provisionally; MPL/licence and runtime delta reviewed at source level | F02 is **not closed**: no full maintainer/test/licence/100-scenario ledger, no Gauntlet build | Yes for production investment; no for this Windows development slice |
| F03 workspace and CI | `.github/workflows/ci.yml`, `Cargo.lock`, `rust-toolchain.toml`, `crates/ui-components/THIRD_PARTY.md` | Local clean `cargo test --workspace` and Clippy pass; CI workflow file exists | No evidence of a remote CI run; no all-three-OS desktop builds; no SBOM/full licence inventory | Yes for release |
| F04 host lifecycle | [`ANS-9 evidence`](../evidence/ans-9-host-lifecycle.md) | Windows monitor/theme/placement/show/hide, invocation identity, ordered shutdown; reviewer run 1000/1000 cycles visible and hidden with `succeeded=1000` | One machine, one GPU, one monitor topology; earlier run was `denied=1000`; no sleep/lock, fullscreen, virtual desktop, hot-unplug, mixed-DPI hardware, or physical keystroke after denial | Yes for a production lifecycle claim |
| F05 shared input | [`ANS-10 evidence`](../evidence/ans-10-input-contracts.md) | 45 library tests + 4 integration tests for graphemes, undo/redo selection, IME transaction/cancel, UTF-16, oversize rejection, logical RTL, a11y ranges | No physical IME, no screen reader, no scaled RTL, no OS clipboard integration, no hardware caret placement | Yes for F08/release |
| F06 navigation and actions | [`ANS-8 evidence`](../evidence/ans-8-navigation-actions.md) | Typed view stack, restore-once parent state, invocation/view-scoped `ActionRef`, 46-stage synthetic smoke, model/property fuzz | Synthetic keystrokes only; no physical keyboard, no screen reader, no pixel-exact pointer scroll | Yes for a release UX claim |
| F07 hotkeys | [`ANS-11 evidence`](../evidence/ans-11-hotkeys.md) | Platform-neutral chord/config/store/manager/recorder; Windows arbitrary-chord worker; typed conflict/denial; 1000-cycle worker and manager tests; CLI fallback | Desktop still registers a fixed `Ctrl+Space`; no store/manager/rebind UI wiring; no automatic sleep/lock observer; no macOS/Linux adapter. ANS-11 observed `Ctrl+Space` owned by another app/IME; the reviewer's later probe reported it available, so availability is session-dependent | Yes for F07 and any "reliable global shortcut" claim |
| F08 accessibility | [`ANS-15 evidence`](../evidence/ans-15-accessibility-acceptance.md), [`screen-reader protocol`](../accessibility/screen-reader-acceptance.md), [`risk register`](../risk-register.md) | Shipping pin exposes **0 UIA descendants** (reviewer probe, 2026-11-18); candidate shell has automated UIA roles/labels/positions/live regions/text ranges from saved observations | **No NVDA/Narrator/VoiceOver/Orca journey was run.** UIA is not AT speech. The candidate shell probe could not be reproduced in the reviewer session (two foreground-activation denials). Candidate is unpromoted. | Yes, critically |
| F09 performance | [`ANS-12 evidence`](../evidence/ans-12-performance-harness.md), [`benchmark artifacts`](../benchmarks/runs/20261118T080933968Z/summary.md) | Deterministic 10k/1M corpora; `quick`, `cancel-flood`, `delayed-provider` ran; reviewer `quick` run had zero launch failures and p50 query-to-results 96,213.15 µs (debug harness) | No budget defined; `full` (30 launches / 1M index / 30 min idle) and 72-hour `soak` unrun; launcher fixture is debug-only; no physical input/paint/display timing; GPU unavailable | Yes for any latency/memory/power claim |
| F10 gate | This ADR and [`ANS-13 evidence`](../evidence/ans-13-framework-gate.md) | Explicit narrow decision; support matrix; owned stop conditions | Release gate intentionally still open | Yes by design |

## Support matrix

| Configuration | Evidence level | Scaling | Screen reader | Known limitations |
|---|---|---|---|---|
| Windows 11 build 26200 x86_64, single monitor, `gpui 0.2.2`, debug developer build | **Tested (automated only)** | 100 % only on this machine; 100/150/200 % placement is unit-tested with synthetic monitor data | **None.** No UIA tree on the pin; zero descendants | One machine/GPU; synthetic keyboard/IME; focus outcome varied between denied and succeeded across runs; hotkey availability is session-dependent (ANS-11 owned, reviewer available); no product support |
| Windows 11 build 26200 x86_64, candidate `a19e1944` launcher shell, debug build | **Automated UIA only** (prior saved observations); reviewer rerun denied foreground activation | 100 % observed; 150/200 % unrun | **None.** UIA roles/events are not AT speech; no NVDA/Narrator journey | Candidate unpromoted; no candidate in the shipping build; stale-action surface unreachable via UIA; calculator error had no live event; focus contention can deny the probe |
| Windows 11 build 26200 x86_64 at 150 %/200 %, mixed DPI, multi-monitor, high contrast | **Compilable/unit-tested only** | Not observed on hardware | None | No hardware scaling matrix, no mixed-DPI geometry, no high-contrast journey |
| Windows 11 DWM/DirectComposition glass path | **Compilable/geometry-checked only** | 100 % | None | Native rounded region/alpha setup is checked; visible desktop transparency, shadow quality, remote-desktop/disabled-composition behavior, and native blur are unverified; falls back to opaque with "Glass: Unavailable" |
| Windows 10 or earlier | **Unsupported/unverified** | None | None | No build or test evidence; Windows 11 findings are not assumed to transfer |
| macOS 14/15 | **Unsupported/unbuilt** | None | None | No macOS build, no `accesskit_macos` wiring, no VoiceOver, no hotkey adapter |
| Linux Wayland/X11 | **Unsupported/unbuilt** | None | None | No Linux build, no `accesskit_unix` verification, no Orca/AT-SPI, no hotkey adapter |
| Windows 11 ARM64 or other GPUs/compositors | **Unsupported/unverified** | None | None | No build or driver evidence; GPUI 0.2.2 renderer support is not established |

"Compilable-untested" is deliberately not a support level. A successful compile is not a
runtime result.

## Why the evidence is weaker than the ticket list suggests

1. **One machine, one platform.** Every runtime result was produced on this Windows 11
   install with one GPU and one monitor topology. macOS and Linux have no build at all.
2. **UIA is not assistive technology.** The candidate probes assert roles, properties,
   events, text ranges, and geometry. None of them captures what NVDA, Narrator, VoiceOver,
   or Orca speaks, or whether a user can complete a journey.
3. **Input is synthetic.** The 46-stage smoke dispatches GPUI keystrokes and actions. There
   is no physical keyboard run, no physical CJK/Hindi/emoji IME session, no dead-key/AltGr
   layout test, and no real RTL display. The model tests are necessary but not sufficient.
4. **The candidate is not the product.** The shipping pin has no accessibility bridge and
   exposes zero descendants. The candidate is an isolated experiment depending on a
   downloaded Zed archive under `target/framework-candidates`, not a promoted dependency.
5. **Performance has no target.** The harness is honest, but `full` and `soak` were not run,
   the launcher fixture is debug-only, and no budget exists. The reviewer debug query p50
   (96,213.15 us) is 8.05 times the saved release-harness p50 (11,945.65 us), so the two are
   not interchangeable.
6. **Hotkey lifecycle is incomplete.** Binding persistence and the manager/recorder are
   tested at contract level but are not wired into desktop startup. ANS-11 observed `Ctrl+Space`
   owned by another application/IME; the reviewer's later `ansa hotkey status` probe reported it
   available. Availability is session-dependent. There is no automatic sleep/lock/session
   rearm.
7. **Build artifacts can lie.** During this review, an incremental target directory passed
   tests while its `ansa-desktop` binary did not contain the current performance fixture; a
   forced clean rebuild was required. A later incremental test also failed with a missing
   `Theme::glass_background` until the `ansa-ui-components` package was cleaned. Gate evidence
   must come from a clean build, not an incremental tree.

## Rejected alternatives

| Alternative | Why rejected |
|---|---|
| **Blanket accept GPUI** | The shipping pin has no semantic tree; no real AT journey exists; macOS/Linux are unbuilt; no performance budget is met. A blanket accept would be a false support claim. |
| **Keep the gate open with no narrowing** | Accurate but not actionable. Engineers need to know that Windows keyboard-first development may continue while release accessibility remains blocked. |
| **Trigger the Slint comparison now** | A candidate bridge exists and builds; the remaining gaps are acceptance, tooling, and promotion work, not a demonstrated framework impossibility. The comparison has **not** been run and is not evidence. |
| **Promote the candidate immediately** | It has automated UIA evidence only, no real AT, no cross-platform build, no licence/dependency promotion record, and the shell probe was not reproducible in this session. |
| **Replace the UI with a webview/Tauri** | Outside the current decision and would discard the Rust/GPUI core evidence without a comparison. |

## Trigger for the Slint comparison

Run the same slice in Slint only if the candidate cannot pass real NVDA and Narrator
J1-J10 journeys on a promoted Windows build after a **bounded two-week fix effort** following
promotion. The comparison must cover licensing, backends, accessibility, text editing,
input/IME, and packaging. Until it is run and recorded, it is a contingency, not evidence.

## Unresolved risks, owners, and stop conditions

The detailed matrix is in [`docs/risk-register.md`](../risk-register.md). The gate-level
owners are roles, not named individuals; each must be assigned to a person before the next
verification starts.

| Risk | Owner role | Mitigation | Stop condition | Next verification |
|---|---|---|---|---|
| A11Y-01 shipping pin has no accessibility tree | Framework / renderer owner | Promote only a renderer with a real semantic bridge | No accessibility or production claim on the pin | Repeat the zero-descendant probe on any proposed pin |
| A11Y-02 candidate is an unpromoted experiment | Release gate owner + framework owner | Licence/dependency audit, compatibility inventory, rollback plan | Do not close F08 on experiment evidence | Gate review with candidate hash and promotion record |
| A11Y-03/04 no real Windows AT journey | Accessibility verification owner | Two-tester NVDA and Narrator J1-J10 with speech-viewer logs | F08 stays open; no "Windows supported" claim | Schedule on the promoted build |
| A11Y-05/06/20 no macOS/Linux AT or builds | Platform integration owners (macOS/Linux) + verification owner | Build each platform, run the protocol, record adapters | No cross-platform support claim | Platform build matrix with hashes |
| A11Y-08...21 per-semantic gaps | Desktop shell, shared input, UX, QA owners as listed | Fix semantics, then run the specific AT journey | Any silent/jumbled/missing semantic keeps F08 open | Real AT on two builds |
| GATE-01 stale build artifacts can mask source | Build/CI owner | Clean CI builds; verify binary markers; record commit | Any gate claim from an incremental tree is rejected | Clean-runner CI job |
| GATE-02 hotkey lifecycle not wired end-to-end | Platform integration owner + desktop shell owner | Wire store/manager/recorder, add power/session rearm, test | No "reliable global shortcut" or rebinding claim | Two-machine conflict/sleep/unlock test |
| GATE-03 no performance budget; full/soak unrun | Performance owner | Define Section 14 budgets; run release `full` and 72-hour `soak` | No latency/memory/power claim | Budget ADR plus raw traces |
| GATE-04 candidate build depends on `target/` cache | Build/CI owner + framework owner | Run `prepare.ps1` in clean CI, verify archive hash, cache or vendor deliberately | No candidate evidence from an unprepared tree | Clean CI candidate build |
| GATE-05 root Escape when hotkey registration failed | Desktop shell owner | **Mitigated in code and the 46-stage smoke:** root Escape now hides even with no registered summon, and the smoke asserts `LifecycleState::Hidden`. A physical keyboard run on a machine where `Ctrl+Space` is owned is still needed | No release "keyboard-only dismiss" claim until the physical failure run | Run the desktop with the hotkey unavailable and dismiss by keyboard |
| F02 greenfield provisional (GATE-06) | Architecture/reuse owner | Complete the reuse ledger and maintainer/licence review | No production commitment on an open F02 | F02 close-out review |

## Revised schedule

| Window | Work | Exit criterion |
|---|---|---|
| Now (Phase 0 close) | Keep Windows keyboard-first development on `gpui 0.2.2`; keep candidate isolated; add clean-build CI; record this ADR | This ADR and the gate/status docs agree; no release claim |
| Next 2 weeks | Candidate promotion checklist; wire `HotkeyStore`/`HotkeyManager`; fix calculator-error live announcement; make stale-action rejection reachable; run clean CI | Candidate promotion decision record; F07 plumbing tests; A11Y-11/21 evidence |
| Weeks 3-4 | Two-tester NVDA and Narrator J1-J10 on the promoted build; physical IME J8; 150/200 % J10 | Real AT artefacts; no critical A11Y row closed without a second tester |
| Weeks 5-6 | macOS/Linux builds; VoiceOver/Orca; F02 close-out | Platform build matrix; F02 decision |
| Weeks 7-8 | Release `full` and 72-hour `soak`; define and test budgets | Budget report; no unowned critical risk |
| Contingency | Same-slice Slint comparison only if the candidate fails the bounded two-week AT fix | Written comparison ADR; not run before that |

## What would change this decision

The decision moves to **accept for release** only when all of the following are recorded:

1. A promoted renderer with a semantic tree, tested with NVDA and Narrator on Windows, and
   VoiceOver and Orca on built macOS/Linux configurations, using the protocol journeys.
2. Physical IME (CJK, Hindi, dead keys/AltGr), RTL/mixed-bidi, and 100/150/200 % scaling on
   real hardware.
3. Clean-build CI on all advertised platforms, a licence/SBOM record, and a rollback plan.
4. Named performance budgets with release-mode `full` and `soak` traces on named hardware.
5. No open critical/high risk without an owner, mitigation, stop condition, and next check.

The decision moves to **pivot/Slint** only under the trigger above. A failure of one probe
run, without a bounded fix attempt, is not enough to pivot.

## Release and marketing constraints

Until the gate closes, do not say or imply:

- "Accessible", "screen-reader supported", "NVDA/Narrator/VoiceOver/Orca tested".
- "macOS supported", "Linux supported", "cross-platform", or "works on Wayland".
- "IME tested", "RTL supported", "scaled-display tested".
- "Meets a latency/memory/power budget" or "fast" based on the debug fixture.
- "Production ready", "release", "Raycast compatible", or "Raycast parity".
- That the candidate or its UIA results are the shipping product.

Accurate wording: *"A Windows-only keyboard-first foundation is under development. The
shipping build exposes no accessibility tree. A newer framework candidate is under evaluation
and has automated UIA evidence only; no screen-reader, physical-IME, scaled-display,
cross-platform, or performance-budget acceptance has been completed."*

## Review record

The independent reruns, findings, and exact counts are in
[`docs/evidence/ans-13-framework-gate.md`](../evidence/ans-13-framework-gate.md). The short
version: clean workspace tests and Clippy pass; the 46-stage synthetic smoke and 1000-cycle
lifecycle smoke pass; a debug `quick` benchmark passes with no budget; the candidate shell
builds; the shipping pin has zero UIA descendants; the candidate shell UIA probe was denied
foreground activation twice in this session and is therefore **not** independently reproduced.
A follow-up clean rebuild of every workspace package (preserving the candidate archive) passed
292 tests. That rebuild's 1000-cycle lifecycle run observed `denied=1000`, while an earlier run
observed `succeeded=1000`; focus remains host-policy dependent and both outcomes are recorded.

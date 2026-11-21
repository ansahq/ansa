# Ansa working-tree validation — 2026-11-19

**Automated implementation/readiness: GREEN for the tested Windows working tree. Release/human gates: OPEN.** Every requested command passed. This is not release sign-off, a parity or accessibility claim, a benchmark, or evidence of two weeks of dogfooding.

## Environment and worktree

- Repository: `C:\Users\abhir\.local\share\opencode\worktree\1bbf61\playful-eagle`, base `a5b3be0`.
- Windows (`Microsoft Windows NT 10.0.26200.0`), PowerShell 7.7.0.
- Cargo networking was disabled with `CARGO_NET_OFFLINE=true`, including for `cargo fmt`, which does not take `--offline`. All test, Clippy, and run commands also explicitly used `--offline`.
- This was an **uncommitted working tree**, not a clean checkout of `a5b3be0`. Only this report was written by the validator. The final pre-report status check also showed ` M README.md` and ` M docs/implementation-status.md`, which were not in the initial status below.

<details>
<summary>Initial <code>git status --short</code> output</summary>

```text
 M Cargo.lock
 M Cargo.toml
 M apps/cli/Cargo.toml
 M apps/cli/src/main.rs
 M crates/core/Cargo.toml
 M crates/core/src/calculator.rs
 M crates/core/src/query.rs
 M crates/platform-api/src/lib.rs
 M crates/platform-windows/src/lib.rs
 M crates/platform-windows/src/start_menu.rs
 M crates/platform-windows/src/types.rs
 M crates/platform-windows/src/visibility.rs
 M crates/platform-windows/src/win32.rs
 M crates/search/Cargo.toml
 M crates/search/src/lib.rs
 M crates/ui-gpui/src/lib.rs
?? apps/cli/tests/
?? crates/alpha/
?? crates/coordinator/
?? crates/core/data/
?? crates/core/tests/
?? crates/emoji/
?? crates/onboarding/
?? crates/platform-windows/examples/
?? crates/platform-windows/src/launch.rs
?? crates/platform-windows/src/packaged.rs
?? crates/platform-windows/src/shell_link.rs
?? crates/platform-windows/src/switch.rs
?? crates/platform-windows/src/watch.rs
?? crates/quicklinks/
?? crates/registry/
?? crates/search/data/
?? crates/search/tests/
?? crates/storage/
?? docs/alpha/
?? docs/ranking/
?? docs/reviews/
```

</details>

## Commands and results

| Command | Result and key output |
| --- | --- |
| `rustc --version` | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` |
| `cargo --version` | `cargo 1.99.1 (5f94df478 2026-08-27)` |
| `git status --short` | Dirty; exact initial output above. |
| `cargo fmt --all -- --check` | PASS, exit 0, no formatter output. |
| `cargo test --workspace --offline --no-fail-fast` | PASS, exit 0; **535 passed, 0 failed, 1 ignored** across 38 result lines. |
| `cargo test --locked --offline --workspace` | PASS, exit 0; **535 passed, 0 failed, 1 ignored** across 38 result lines; same per-suite counts. |
| `cargo clippy --workspace --all-targets --offline -- -D warnings` | PASS, exit 0. No lint failures. Cargo emitted the dependency future-incompatibility notice below. |
| `cargo run --offline -p ansa-cli -- doctor` | PASS, exit 0. `platform: windows (adapter windows-start-menu-packaged-launch-switch-v4)`; `applications: 156 total · 108 classic · 48 packaged · 3 broken`. Catalog, packaged apps, application launch, URL open, and global hotkey reported available; doctor did not register a shortcut. Clipboard capture and window control reported unsupported. Emoji `emoji-1.0.0`, Unicode 16.0, 139 entries, Unicode License v3, checksum `e80345152b0e31eb`; ranking corpus `v2026-11-19.3`, calculator corpus `v1.2.0`. `storage: skipped (pass --data-dir <dir>)`; `No network access was used.` |
| `cargo run --offline -p ansa-cli -- calc "10 km to mi"` | PASS, exit 0; `6.21371192237 mi`. |
| `cargo run --offline -p ansa-cli -- calc "0.1+0.2"` | PASS, exit 0; `0.3`. |
| `cargo run --offline -p ansa-cli -- emoji rocket --limit 3` | PASS, exit 0; `🚀  Rocket  [Travel & Places]` (`accessible: Rocket`), then `🎾  Tennis  [Activities]` (`accessible: Tennis`); `2 result(s)`. Dataset `emoji-1.0.0`, Unicode License v3, checksum `e80345152b0e31eb`. |
| `cargo run --offline -p ansa-cli -- alpha-smoke --cycles 1000` | PASS, exit 0; exact output below. |
| `cargo run --offline -p ansa-cli -- storage --demo` | PASS, exit 0. `schema version: 3`; `pragmas: foreign_keys=1 journal_mode=wal synchronous=2 busy_timeout=5000ms`; `test setting: maximum_results=42 (revision 1)`; `compare-and-swap: stale revision rejected as expected`; backup `integrity ok · schema v3 · 1 settings · 0 records`. Demo left files under `C:\Users\abhir\AppData\Local\Temp\ansa-storage-demo-8272-1795106654437120300`. |
| `cargo run --offline -p ansa-cli -- onboard --state <unique temp path> --demo` | PASS, exit 0. Welcome → Choose a shortcut → Try a launch → Optional integrations → Ready; `Ctrl+Space · probe available (fake) · registered true`; `Launched for app:demo (fake probe; no process started)`; `status completed · completions 1`. The file `C:\Users\abhir\AppData\Local\Temp\opencode\ansa-onboarding-8eaeb67b-5f7f-45d8-a6bb-8f35a7def901.json` was created, removed, and confirmed absent. No real hotkey registration or application launch. |
| `cargo run --offline -p ansa-platform-windows --example catalog` | PASS, exit 0. `total=156`, `classic=108`, `packaged=48`, `broken=3`, `duplicate_titles=2`, `with_arguments=29`, `argument_distinct_targets=6`, `watcher_wired=false`; `packaged_capability=available catalog=available`. Broken entries: Discord ×2 and Steam ×1. Duplicate title groups: `discord` and `uninstall`, two each. This is a local discovery snapshot, not a launch/switch test. |

Cargo notice during workspace validation (not a Clippy lint failure):

```text
warning: the following packages contain code that will be rejected by a future version of Rust: proc-macro-error2 v2.0.1
note: to see what the problems were, use the option `--future-incompat-report`, or run `cargo report future-incompatibilities --id 1`
```

Exact `alpha-smoke` output (elapsed is one observation, not a benchmark):

```text
Alpha smoke: PASS
cycles: 1000
accepted batches: 1000
rejected batches: 3000
stable selection checks: 1000
max pending: 1
max results: 401
catalog size: 404
assertions: 15003
elapsed: 4773.2 ms
fingerprint: 0x02807f025f6a8ad7
```

### Every `test result:` line — `--no-fail-fast` run

**Total = sum of the 38 lines below:** 24 unit/integration suites with **532 passed, 0 failed, 0 ignored**, plus 14 doctest suites with **3 passed, 0 failed, 1 ignored**. Overall: **535 passed, 0 failed, 1 ignored**. The ignored test is the explicitly `ignore`-marked GPUI entity usage example, `crates\ui-components\src\text_input.rs - text_input (line 11)`, not an ignored UI acceptance test. The two workspace runs are separate confirmations, not additive totals.

| Suite (in run order) | Exact result |
| --- | --- |
| `ansa_alpha` unit | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_alpha` integration `alpha_integration` | `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.90s` |
| `ansa_alpha` integration `catalog_composition` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa-cli` binary `ansa` unit | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa-cli` integration `cli` | `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s` |
| `ansa_coordinator` unit | `test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s` |
| `ansa_core` unit | `test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `ansa_core` integration `calculator_corpus` | `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s` |
| `ansa_desktop` binary unit | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_domain` unit | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_emoji` unit | `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_emoji` integration `emoji_search` | `test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `ansa_onboarding` unit | `test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `ansa_onboarding` integration `onboarding_flow` | `test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_platform_api` unit | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.87s` |
| `ansa_platform_windows` unit | `test result: ok. 110 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.85s` |
| `ansa_quicklinks` unit | `test result: ok. 77 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.41s` |
| `ansa_registry` unit | `test result: ok. 70 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `ansa_search` unit | `test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_search` integration `relevance_corpus` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `ansa_storage` unit | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_storage` integration `storage` | `test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.68s` |
| `ansa_ui_components` unit | `test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_ui_gpui` unit | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_alpha` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_coordinator` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_core` doctests | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `ansa_domain` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_emoji` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_onboarding` doctests | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `ansa_platform_api` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_platform_windows` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_quicklinks` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_registry` doctests | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `ansa_search` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_storage` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_ui_components` doctests | `test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `ansa_ui_gpui` doctests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |

## What the checks cover

| Area | Evidence from the run |
| --- | --- |
| Registry/lifecycle | Typed binding, registration rejection, grant/confirmation and target-revision checks, stale/foreign references, replay guards, exact uncertain-operation attribution, multiple pending operations and bounded terminal retention; alpha exercises a fake host. |
| Platform discovery/switch | Real Start Menu and packaged discovery; shortcut parsing and bounds, identity/dedupe by target, arguments and working directory, broken-target reporting, AUMID checks, watcher debounce, switch candidate and conservative outcome tests. Some native guard/hotkey tests ran, not a real user switch or focus-restoration session. |
| Coordinator/ranking corpus | Stale and foreign batches, cancellation, logical deadlines, partial/final states, budget and queue handling, stable selection under updates, six ranking tiers and scoped ranking signals. Corpus `.3` tests ordered prefixes, pinned metric floors and detection of a synthetic order regression. Review baseline: 31 commands, 23 tuning and 39 validation queries; validation top-1/recall@10/MRR `0.897/0.923/0.897`. Known failures `calender`, `termianl`, `cafe`, `uber` still count in those metrics. |
| Storage/migrations/backup | Revisioned settings/records, compare-and-swap, cache isolation, migration rollback and prefix checks, subprocess kills during writes and after migration DDL before commit, backup/restore verification, failed refresh/checkpoint paths, corruption and disk-full error mapping. |
| Quicklinks policy/import | Typed single-pass templates, encoding, URL/path policy and opener compatibility, preview fingerprint/staleness, import caps, per-item validation, dry run and all-or-nothing replace. Alpha catalog composition excludes unresolved placeholders and local paths from HTTP commands. |
| Calculator/units | Bounded arithmetic, typed errors, supported unit conversions, significant-digit and scientific round trips, malformed-input regressions; live CLI confirms retained units and `0.3` display. |
| Emoji dataset/graphemes | 139 pinned entries, checksum rejection (including a recomputed checksum), single extended graphemes including multi-codepoint variants, distinguishable accessible-name strings, bounded search and in-memory collection separation. Activation tests verify policy outcomes, not the clipboard or insertion. |
| Onboarding | Renderer-independent state transitions, restart/resume, fallback and optional-step handling, shortcut conflict recovery, launch outcome checks and persistence. Demo is deliberately fake. |
| Alpha/CLI/UI | Alpha integration and catalog composition, CLI diagnostics and smoke execution, and 28 UI-component tests for editing, simulated composition, Unicode selection and theme contrast. Desktop and GPUI targets built but each ran zero unit tests; no interactive UI was exercised. |

## Final review verdicts and residual risks

These are the final scoped verdicts from all four review files, not their superseded initial findings. Prior reviewers' independent probes were not rerun separately for this report.

| Ticket | Final verdict |
| --- | --- |
| ANS-14 / L01 registry | **Accepted at crate/API level**; final R2-X delta closes the multiple-pending-operation HIGH. Fake-host alpha coverage is present, but no production action-panel → registry → real-host execution gate. [Review](reviews/2026-11-19-registry.md) |
| ANS-16 / L02 Windows discovery and launch | **Accepted with documented gaps**; final delta closes the wrong shortcut-target precedence HIGH and working-directory identity MEDIUM. [Review](reviews/2026-11-19-platform-calc-emoji.md) |
| ANS-17 / L03 coordinator | **Accepted**; final delta closes custom-budget adapter and rejected-submit clock MEDIUMs. [Review](reviews/2026-11-19-coordinator-ranking.md) |
| ANS-18 / L04 ranking | **Accepted**; ordered prefixes and metric floors are enforced for the pinned corpus; known failures and scope limits remain. [Review](reviews/2026-11-19-coordinator-ranking.md) |
| ANS-19 / L05 storage | **Accepted with residual risks**; reported path-alias and failed-checkpoint data-loss paths are closed for the reviewed cases. [Review](reviews/2026-11-19-storage-quicklinks.md) |
| ANS-20 / L06 quicklinks | **Accepted with residual risks**; final delta closes the earlier executable-policy MEDIUM. Not end-to-end desktop acceptance. [Review](reviews/2026-11-19-storage-quicklinks.md) |
| ANS-21 / L07 calculator | **Accepted for the crate and verified consumer paths**; display and unit routing defects are fixed. [Review](reviews/2026-11-19-platform-calc-emoji.md) |
| ANS-24 / L08 emoji | **Accepted at crate level; ticket remains partial** without end-to-end copy/insertion, desktop browsing and persisted collections. [Review](reviews/2026-11-19-platform-calc-emoji.md) |

**Severity:** No unresolved implementation BLOCKER/HIGH/MEDIUM was carried forward in the final scoped verdicts. This does not downgrade open product/release gates. The registry's earlier MEDIUM provenance finding remains a documented **host trust contract**, not proof of authentication or permission isolation.

- **LOW — Storage:** NTFS alternate-stream and hard-link aliases are not fully identified; rollback failure can leave the store closed without identifying the recovery copy, and a late sidecar check can leave staged files. No parent-directory fsync for backup rename. Restore verifies required schema objects by name and accepts extra triggers/views; use trusted backups. Listing defaults to a 1,000-row page. No cross-process lock; generic payload secrecy is a caller responsibility. Process-kill tests are not physical power-loss tests.
- **LOW — Quicklinks:** Import checks samples, not all possible argument values; pass the host's actual policy for untrusted imports and recheck the resolved target at preview. Some active/legacy types are allowed, and extensionless targets need a host filesystem check. IPv6 validation still has shape edges, validators are separate, and display text is not as strict as resolved targets. Schema v1 revalidation can reject formerly accepted items without a version change. Replace confirmation is caller-owned and opener probes are synthetic.
- **LOW — Coordinator/ranking:** `ProviderQueueFull` is defensive-only in the present dispatch model; coordinator IDs are process-local, and the synchronous logical-tick model has no asynchronous provider runtime or serialized-byte budget. Rank limiting caps returned rows, not the full scan/sort or signal-map growth. Recency uses logical ticks. Short golden prefixes and known-failure exceptions limit what the corpus measures; held-out authorship history is not independently established.
- **LOW / operational — Windows:** Real focus behavior and native shell failure mapping are not demonstrated by these checks. Packaged switching is not implemented; the watcher is unwired. A PowerShell child that cannot be terminated can leave pipe-reader threads until it exits. The final review flagged cache and resolution-order doc nits; the current Windows adapter source reflects the cache and actual order.
- **LOW / policy — Calculator and emoji:** The calculator's seeded property coverage is bounded and postfix `%` deliberately makes `200 + 10%` display `200.1`. The desktop's unit/scientific query route requires `=`. The emoji FNV-1a checksum is not cryptographic; the review independently probed the entry cap, while the checked-in cap test covers bytes. Accessible-name strings are not screen-reader verification.
- **Documented registry integration risks (not severity-rated in the final delta):** Confirmation, grants and provider namespace/target ownership are trusted-host responsibilities; checks before `perform` do not eliminate time-of-check/time-of-use gaps. No durable operation journal protects against a crash between the effect and recording its outcome.

## Not verified / not implemented

- **Desktop integration:** No GPUI quicklink authoring, emoji browsing, or onboarding screens. Desktop does not consume `DailyDriver`. The built desktop was not launched for interactive acceptance; real application switching, foreground restoration/denial and UI behavior remain unverified.
- **Discovery and actions:** `CatalogWatcher` is not wired into desktop or CLI; packaged install/uninstall changes require manual refresh. Icon source metadata is present, but icon pixels, reveal-in-file-manager, and structured copy identity are absent.
- **Human release gates:** No evidence of two consecutive weeks of human dogfooding, clean-machine install/uninstall by a new tester, or screen-reader/physical-IME acceptance. Model tests and simulated composition are not substitutes.
- **Storage:** No encryption for generic records/cache; do not put secrets there. No cross-process storage lock. No physical power-loss or real disk-full induction, and no broad Windows file-lock/antivirus restore acceptance.
- **Calculator:** Bounded approximate `f64`, not decimal-exact arithmetic; no currencies, dates or time zones.
- **Emoji:** Real clipboard copy/insertion and persisted recents/favorites are unverified; policy and in-memory collection tests do not establish those behaviors.
- **Quicklinks:** Replace confirmation remains a caller contract. No real browser/file-manager availability probe or end-to-end opener exercise was performed.

## Verdict

Automated implementation and readiness are **green for this Windows working tree**: formatting, Clippy with `-D warnings`, the full offline workspace suite (535 passed, 0 failed, 1 ignored), the locked suite, the CLI diagnostics, and the 1,000-cycle alpha harness all pass. The scoped reviews found no unresolved implementation BLOCKER/HIGH/MEDIUM after the fix loops.

This is not a release sign-off. Desktop integration for quicklinks/emoji/onboarding, real clipboard/insertion, interactive GPUI acceptance, real switch/focus behavior, accessibility and physical-IME testing, low-disk/power-loss recovery, cross-platform adapters, and the two-week human dogfooding gate remain open. No parity, benchmark, screen-reader or production-readiness claim is made.

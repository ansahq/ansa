# Platform batch implementation — 23 November 2026

## Acceptance boundary

This batch covers ANS-82 through ANS-99. Work was implemented and tested from
Windows; this is **not** a macOS or Linux support certification. Native
hardware, compositor, screen-reader, physical-input and visual acceptance
requirements remain release blockers until their required evidence exists.
Cross-compilation and simulated lifecycle tests do not satisfy native acceptance.

## Exclusive implementation ownership

The batch was split by writable path to avoid concurrent edits. Initial
GPT-6.2 Sol agents produced partial adapters, then hit provider usage limits.
Completion and security hardening moved to DeepSeek V4.1 Flash 1017 Max
subagents; the final independent review is reserved for GPT-6.2 Sol Max. The
coordinator performed integration, lockfile/inventory work and the checks below.
Agents shared one worktree but not editable files.

| Owner | Tickets | Exclusive files |
|---|---|---|
| Catalogs/openers | 84, 85, 92, 96 | `crates/platform-api/**`, excluding the host/hotkey modules below |
| Host/hotkeys | 83, 86, 89, 91 | `crates/platform-api/src/host_lifecycle.rs`, `src/host_lifecycle/**`, `src/hotkey.rs`, `src/hotkey/**`, `HOST-VALIDATION.md` |
| Vault | 82, 90 | `crates/vault/**` |
| Clipboard | 95, 99 | `crates/clipboard/**` |
| Desktop/glass | 93, 94, 98 | `crates/ui-gpui/**`, `apps/desktop/**` |
| Acceptance gates | 87, 88, 97 | `docs/platform-batch-acceptance.md`, `docs/platform-acceptance-evidence.json`, `scripts/platform_acceptance_gate.py`, `scripts/test_platform_acceptance_gate.py` |
| Independent review | All risky changes | `docs/reviews/platform-batch-final-review.md` only; implementation is read-only |
| Coordinator | Integration and validation | Root lockfile/manifests if needed, CI/local validation wiring, inventory outputs, this record and status links |

The host/hotkey owner finished its first pass before the platform-api integration
owner made the final capability-truthfulness and parser fixes; those edits were
sequential, not concurrent. No agent wrote another owner's files while it was
active.

No agent may mark native acceptance complete merely because portable tests pass.
The coordinator records final commands and residual gaps below after integration.

## Initial validation

- Clean initial `git status --short`.
- `cargo test --locked --offline`: exit 0, default workspace members;
  648 pass events, including two child-process duplicate passes, one ignored
  real-clipboard probe. This baseline excludes the GPUI desktop packages.
- Python build-tool tests: 14 passed.
- Python reference validator tests: 41 passed.
- Python dependency-tool tests after CI contract addition: 31 passed.
- Python on `PATH` is an unusable Windows Store shim. Tests use the official
  Python 3.13.0 embedded runtime unpacked under
  `C:\Users\abhir\AppData\Local\Temp\opencode\python-3.13.0-platform-batch`.
  No system installation or PATH change was made.

Final integration results will supersede the initial validation for changed code.

## Acceptance-gate validation

The gate is wired into the three-OS reference CI job and local
`scripts/validate.ps1`; workflow configuration is not evidence of a hosted run.
The initial independent review found positive-acceptance validation gaps.
The acceptance owner added external approval/binary-manifest binding,
structured non-smoke launch arguments, capture/build-artifact separation,
three labeled toggle round trips and escaped diagnostic output.

- Python gate tests after the second hardening pass: **56 run, 55 passed, 1
  skipped** (the skipped symlink-escape test cannot create a symlink on this
  Windows host), exit 0.
- `scripts/platform_acceptance_gate.py --check`: exit 0; the ledger is internally
  consistent with **0/8 accepted native profiles**.
- `scripts/platform_acceptance_gate.py --require-accepted`: exit 1 as expected
  for the blocked ledger, not a passing release gate.
- VoiceOver, Orca/AT-SPI and macOS glass tickets remain In Progress, with the
  missing physical-native evidence recorded in Kaneo.

See [the native protocols](platform-batch-acceptance.md) and
[the evidence ledger](platform-acceptance-evidence.json). The gate validates
records and release-owner decisions; it does not authenticate human speech,
hardware observations or media provenance.

## Final integration validation (Windows host)

All build/test/check commands used Rust 1.99.1 with `--locked --offline`;
`cargo fmt --check` does not accept those flags. The lockfile was refreshed once
by the coordinator, then every package check used the locked resolution. These
are host tests and compile/type checks, **not** native macOS/Linux acceptance.

| Scope | Command | Result |
|---|---|---|
| Workspace format | `cargo fmt --all -- --check` | exit 0 |
| Workspace Clippy | `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | exit 0; only the pre-existing `proc-macro-error2` future-incompatibility note |
| Workspace tests | `cargo test --workspace --locked --offline --no-fail-fast` | **786 pass events, 0 failed, 2 ignored** across 50 Cargo result lines (not 50 distinct test binaries); two events are child-process relaunches |
| Workspace doctests | `cargo test --workspace --doc --locked --offline` | **4 passed**, 1 ignored |
| Platform adapters | `cargo test -p ansa-platform-api --locked --offline` | **43 passed**, 0 failed |
| Vault | `cargo test -p ansa-vault --locked --offline` | **41 passed**, 0 failed |
| Clipboard | `cargo test -p ansa-clipboard --locked --offline` | **110 passed**, 1 ignored (existing real-Windows-clipboard probe) |
| Desktop | `cargo test -p ansa-ui-gpui -p ansa-desktop --locked --offline` | **30 passed** in `ansa-ui-gpui`; desktop 0 tests |
| Desktop build | `cargo build -p ansa-desktop --locked --offline` | exit 0 |
| Startup smoke | `target\debug\ansa-desktop.exe --smoke-test` and `--opaque --light` | exit 0; window created; two GPUI frame callbacks |
| Interaction smoke | `target\debug\ansa-desktop.exe --interaction-smoke-test` and `--opaque --light` | exit 0; 40 synthetic stages |
| Acceptance gate | `python scripts/platform_acceptance_gate.py --check` | exit 0; ledger valid, **0/8** native profiles accepted |
| Gate tests | `python -m unittest discover -s scripts -p test_platform_acceptance_gate.py -v` | **56 run, 55 passed, 1 skipped** (symlink creation unavailable) |
| Dependency inventory | `python scripts/dependencies/inventory.py --check` | exit 0; 762 packages, 65 not observed for declared CI targets |

Cross-target compile/type checks passed for `ansa-platform-api`,
`ansa-vault` and `ansa-clipboard` (macOS and Linux targets, plus Clippy with
`-D warnings`). Desktop cross-target attempts stopped in native dependency
build scripts: the default run failed at `ring` (no `x86_64-linux-gnu-gcc` or
Apple SDK); a coordinator retry with the available MinGW `gcc` reached
`freetype-sys` and failed because `x86_64-linux-gnu-g++` and a Linux sysroot are
absent. Both are environment failures before `ansa-ui-gpui` compilation, not
native evidence.

## Independent review and fix rounds

The original GPT-6.2 Sol Max review is
[platform-batch-final-review.md](reviews/platform-batch-final-review.md): no
BLOCKER, four HIGH, seven MEDIUM and one LOW finding, with a do-not-merge
verdict. Three scoped DeepSeek V4.1 Flash Max fix rounds followed, checked by
[platform-batch-fix-review.md](reviews/platform-batch-fix-review.md),
[platform-batch-fix2-review.md](reviews/platform-batch-fix2-review.md) and
[platform-batch-fix3-review.md](reviews/platform-batch-fix3-review.md), with a
final [micro-confirmation](reviews/platform-batch-fix4-review.md). They closed
the passphrase-header, selection-token, Carbon-ABI, Linux-test and
transactional-eviction defects and identified narrower residuals that were then
fixed or documented:

- The X11 ownership drain stops at the first reassert the server processed
  before the grab, treats an event-bound overflow as `ClipboardChanged`, and
  sends zero conversions against the unscreened offer. A peer request merely
  queued before the grab is not covered by the barrier.
- Volatile-budget backends are non-persistent, and a persistent store rejects a
  budget that appears after open before any metadata or payload mutation.
- The final Linux opener check runs under the child lock; the fallback folder is
  captured before `ShowItems`. Pathname handoff is still not an atomic
  file-descriptor handoff.
- The macOS hotkey fence uses Carbon `EventTime` (`double` seconds), not ticks.

The [third-pass verification](reviews/platform-batch-fix3-review.md) confirmed
the X11 fix and the in-tree mixed-store fix. Its only remaining source-trace
was an embedding backend that enables a budget after open; `capture` now rejects
that configuration with `InvalidArgument` before mutation, covered by a
regression test. The [fix4 micro-confirmation](reviews/platform-batch-fix4-review.md)
cleared that HOLD and found no new regression in the reviewed paths. These are
Windows-hosted tests and type checks, not native execution.

## Remaining native blockers

The implementation is bounded and host-tested, but these tickets cannot be
closed from this environment:

- **ANS-82 / ANS-90 / ANS-95 / ANS-99:** real macOS Keychain, Linux Secret
  Service, AppKit pasteboard, X11 selection and Wayland data-control execution;
  Keychain/Secret Service denial, revoke and locked-provider behavior; real
  clipboard fixtures and compositor coverage. Cross-compilation is not evidence.
- **ANS-83 / ANS-86 / ANS-89 / ANS-91:** real 1,000-cycle hide/show, focus,
  suspend/resume, registration, conflict, X11/Compositor, Spaces/full-screen,
  hot-plug and scale runs. Simulated lifecycle tests are not acceptance.
- **ANS-84 / ANS-85 / ANS-92 / ANS-96:** real LaunchServices/Finder and
  xdg-open/file-manager opening; browser/default-handler absence and permission
  cases; launch/switch behavior against installed applications.
- **ANS-87 / ANS-88 / ANS-97:** VoiceOver and Orca/AT-SPI J1–J10 journeys with
  two independent testers and native speech artifacts; macOS frosted-glass
  visual matrix, contrast sampling, Reduce Transparency and bounds checks.
  The acceptance ledger remains blocked at **0/8**.
- **ANS-93:** Linux native blur is **not implemented**. Linux uses the opaque
  themed surface with `Glass: Unavailable`; the ticket's blur acceptance
  requirement is unmet and must not be marked complete.
- **ANS-94 / ANS-98:** native X11/Wayland and macOS startup/interaction smokes
  must still run on declared hardware; the Windows smoke is synthetic.

## Ticket disposition

Kaneo was updated with per-ticket implementation and validation comments:

- **In Review:** ANS-82, ANS-83, ANS-84, ANS-85, ANS-86, ANS-89, ANS-90, ANS-91,
  ANS-92, ANS-94, ANS-95, ANS-96, ANS-98 and ANS-99. Their scoped host
  implementation and validation are complete, but real
  hardware/provider/compositor checks remain required before Done. Documented
  implementation residuals are not hidden by that status: the Xauthority read
  is still outside the socket deadline, Linux `CreateItem` can still surface a
  provider prompt, and a live macOS Reduce Transparency change still requires
  activation or a glass toggle.
- **In Progress / blocked:** ANS-87, ANS-88 and ANS-97 (native assistive
  technology and visual acceptance) and ANS-93 (native Linux blur not
  implemented; only the honest opaque fallback exists).

No status was promoted to Done on the strength of portable tests. See the
per-crate validation files for exact evidence boundaries and residual risks:
[platform-api](../crates/platform-api/HOST-VALIDATION.md),
[vault](../crates/vault/PLATFORM-VALIDATION.md),
[clipboard](../crates/clipboard/PLATFORM-VALIDATION.md),
[desktop](../crates/ui-gpui/PLATFORM-VALIDATION.md), and the
[native acceptance protocols](platform-batch-acceptance.md). No hosted CI run is
claimed for this local change.

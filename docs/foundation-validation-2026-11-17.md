# ANS-5 / ANS-6 / ANS-7 integration validation

Date: 2026-11-17. This record concerns repository tooling and planning evidence,
not reference-product parity or release acceptance.

Host: Windows NT 10.0.26200.0, x64; Python 3.12.10. Starting repository commit:
`a5b3be03bcf60d98d9274c514b233841accc65bd`. This is the tooling host identity,
**not** the missing reference application capture identity.

## Architecture and ownership

Sol retains architecture, integration, and final verification. DeepSeek V4.1
Flash 1017 High implements bounded, non-overlapping slices:

| Task | Owned files |
|---|---|
| ANS-5 | `docs/reference/**`, `scripts/reference/**`, `docs/ans-5-validation.md` |
| ANS-6 | `docs/adr/0003-reuse-evaluation.md`, `docs/reuse/**`, `scripts/reuse/**` |
| ANS-7 | `rust-toolchain.toml`, `.github/workflows/**`, `scripts/validate.ps1`, `scripts/dependencies/**`, `docs/build/**` |

Sol owns shared README/status/ADR integration and this validation record.
After ANS-7's completed handoff, Sol owns final CI/local-runner wiring and its
repository-contract test adjustments; the High implementer no longer edits those
files. Max review is read-only and has no write ownership.
The default OpenCode provider rejected the initial delegated requests for
insufficient balance; implementation uses the same requested model and High
variant through OpenCode Go instead. Failed requests did not implement changes.

The launcher architecture and runtime feature scope are unchanged. A scenario
specification is not an observation; a license declaration is not legal approval;
a CI definition is not a completed remote build. No candidate source reuse is
authorized by this work.

Sol's architecture decision: retain the renderer-independent Rust foundation and
current GPUI feasibility gate, with zero candidate source adoption. The inspected
candidates do not demonstrate the required Rust-UI/cross-platform foundation
without substantial language, integration, maintenance, or distribution changes.
ADR 0003's reopen conditions govern reconsideration; the decision does not close
its build, runtime, license-audit, or framework acceptance gaps.

## Validation

Final command results and remaining acceptance gaps are recorded below after
integration. Windows is the available host; macOS/Linux execution and reference
application runtime capture cannot be inferred from checks on this machine.

Initial integration checks:

| Command | Actual result |
|---|---|
| `rustc --version` / `cargo --version` | 1.99.1 / 1.99.1 through the existing stable toolchain |
| `cargo +1.99.1 test --workspace --locked --offline` | Failed before compilation: missing manifest in the installed named toolchain |
| `cargo +stable fmt --all -- --check` | Passed |
| Initial fallback workspace test and Clippy attempts | Each hit the 120-second command timeout during initial compilation; rerun without that limit, results below |
| `cargo +stable test --workspace --locked --offline` (rerun) | Passed: 138 unit tests, 1 doctest; 1 UI example doctest explicitly ignored |
| `cargo +stable clippy --workspace --all-targets --locked --offline -- -D warnings` (rerun) | Passed |
| `cargo +stable build --workspace --locked --offline` | Passed, including the Windows development desktop executable |
| `cargo +stable run --locked --offline -p ansa-desktop -- --smoke-test` | Passed (exit 0) |
| `cargo +stable run --locked --offline -p ansa-desktop -- --interaction-smoke-test` | Passed: 30 synthetic stages; no clipboard writes or external app launches |

The broken named installation was not repaired or replaced. A `+stable`
fallback, even when its compiler version matches, does not prove reproducibility
with the checked-in exact toolchain selection. Python validation uses the
existing embedded Python 3.12 in the approved temporary tools directory because
the host's `python` executable alias is unavailable.

Cargo emitted a dependency future-incompatibility notice for
`proc-macro-error2 2.0.1`; the commands above nevertheless exited successfully.
No native smoke establishes screen-reader, physical keyboard/IME, focus-return,
or cross-platform acceptance. Cargo.lock SHA-256 at the start of verification:
`036226b0f97f06a855d2ad07787f62ea0e002e3c279150033924594709d4a160`.
Independent `tomllib` parsing of that lockfile counted 717 packages: 708
registry packages with checksums and 9 local workspace packages. An inventory
must account for all 717, not only dependencies compiled on Windows.

Sol reran the dependency tooling after its review fixes:

- `python -m unittest discover -s scripts/dependencies/tests -v` with the explicit
  stable fallback environment: **21 passed**, including the stale-lock rejection.
- `python scripts/dependencies/inventory.py --check --toolchain stable`:
  **passed**, 717 packages checked; 65 outside the declared CI target filters.
- Inventory review status: 9 first-party manifest declarations and 708 unreviewed
  third-party packages; all concluded licenses remain `NOASSERTION`.

SPDX parsing now rejects malformed parentheses/operators and misused exceptions;
document namespaces include content identity, not just lockfile identity. These
were Sol findings corrected by High within its original file ownership.

After Sol's CI/local-runner integration, the dependency/repository contract suite
was rerun: **22 passed**. CI now wires reference validator/tests on all three
OSes and isolated reuse-script safety tests on Windows. The local runner's
PowerShell syntax check passed. No hosted job was triggered or reported as run.
An independent `tomllib`/JSON comparison also matched every generated SPDX
package identity, Cargo source and registry checksum against the lockfile;
Cargo.lock's SHA-256 remains unchanged.

## Acceptance boundaries

- ANS-5: synthetic scenario specifications and design-evidence review must not
  be counted as reference runtime captures. Unknown reference identities remain
  unknown; no personal profile or clipboard history is needed for tooling tests.
- ANS-6: choosing a direction does not prove an upstream application builds or
  passes its test suite. Candidate-specific commands, source identities, audit
  limits, and remaining failures belong in the reuse evidence record.
- ANS-7: the available Windows fallback build does not establish a clean-clone
  pass with the exact pin, a completed hosted CI matrix, or macOS/Linux startup.
  Development artifacts are not signed releases. Dependency license metadata
  and review status are separate from permission to redistribute a release.

These boundaries apply even when all repository validator tests pass.

## Sol reuse-evidence sample

Independently verified all six checkout HEADs against the ADR pins with
`git rev-parse HEAD`. Ten direct pinned-source assertions passed: Gauntlet's
deprecation statement, root MPL text and Deno pin; Vicinae's GPL text and Node
pin; pop-launcher's root MPL and plugin GPL declarations; anyrun's GPL text;
cosmic-launcher's GPL text; and Flare's MIT grant. This is a targeted factual
sample, not a complete per-file/dependency audit or a candidate runtime test.

The implementation agent's recorded candidate probes failed before tests ran:
Gauntlet (missing libclang), pop-launcher (Unix-only dependency), anyrun (GTK
prerequisites), cosmic-launcher (locked dependency compile error). Vicinae was
not built and Flare remained historical evidence. Zero passing candidate tests
are claimed. Sol did not repeat these expensive build probes.
Sol inspected the retained stderr files in the approved temporary `f02/logs`
directory and confirmed all four reported failure signatures. Commit timestamps
were also checked with `git show -s --format='%H %cI %s' HEAD`; cosmic-launcher's
committer timestamp crosses a UTC/local-date boundary and must be labelled
accordingly in dated activity claims.

Sol reran `pwsh -NoProfile -File scripts/reuse/tests/run-tests.ps1` after all
probe hardening: **14 passed, 0 failed**. This includes pin/dirty refusal,
all-candidate preflight, timeout handling, environment restoration, native Git
errors, and preservation of earlier JSONL records and per-run logs. The final
pre-origin-hardening fetch verifier independently accepted all six existing
pinned, clean checkouts. It checked commit/cleanliness, not the stricter origin
policy added after Max review; the hardened rerun is recorded below. Neither run
launched candidate builds.

## Sol reference-source spot check

On 17 November Sol fetched the public [Quickstart](https://manual.raycast.com/quickstart),
[File Search](https://manual.raycast.com/file-search) and
[Quicklinks](https://manual.raycast.com/quicklinks) manual pages. They support the category-level claims: root file/app search,
selected-item actions, configured scopes/optional content search, URL/path
quicklinks, tags and preferred openers. They do not establish a particular
binary's focus restoration, cancellation, cycle handling, missing-opener fallback
or Windows/macOS parity. No reference executable was opened. These three fresh
documentation reads are separate from the inherited 11 November research
baseline and are not artifact-bound runtime evidence.

## Integrated local run before the final Max follow-ups

Sol executed the complete integrated runner, with no skip switches:

```powershell
pwsh -NoProfile -File scripts/validate.ps1 -Toolchain stable `
  -PythonExecutable 'C:\Users\abhir\AppData\Local\Temp\opencode\python-embed\python.exe' `
  -Offline -NativeSmoke
```

Exit 0. Actual results:

| Gate | Result |
|---|---|
| Format, Clippy, desktop development build | Passed |
| Rust workspace unit tests | 138 passed |
| Doctests | 1 passed, 1 explicitly ignored |
| Dependency/repository tooling | 22 passed |
| Lockfile inventory regeneration check | 717 packages matched |
| Reference schema/coverage/evidence validator | Passed: 100 rows, all 48 required A behaviors on each OS |
| Reference validator/subset tests | 34 passed |
| Reuse probe safety tests | 14 passed |
| Windows native startup / interaction | Passed / 30 synthetic stages passed |

The ledger retains 72 specified/documented, 26 deferred and 2 unsupported rows.
All 100 implementation-test procedures remain manual and planned; zero reference
observations or scenario implementation-test passes are claimed. The ten-row
audit is a design/source-baseline consistency audit. The catalog distinguishes
17 inherited entries, 5 proposed/unfetched citations and 5 local design entries.
Unknown-only schemas intentionally require a versioned migration before verified
reference captures can be added.

The named-toolchain failure, absent hosted CI/macOS/Linux execution, candidate
build/test failures, and open third-party review remain acceptance gaps despite
this successful fallback run. No task is represented as fully accepted solely
because its repository tooling passes.

Repository hygiene: `git diff --check` passed for tracked changes; a separate
relative-file-link check passed for 72 Markdown links across 17 documents.
This is not external-link, anchor, YAML-schema, or hosted workflow validation.

## Independent Max review

DeepSeek V4.1 Flash 1017 **Max**, through OpenCode Go, independently reviewed the
actual tooling and documentation read-only and reported P2/P3 findings. Sol
triaged them and assigned bounded corrections to High in the original file
boundaries. Sol retains CI/runner integration and final verification.

The inventory's name/version-only mapping is an explicit accepted limitation for
this slice: the actual lockfile has 717 distinct name/version pairs, while a
source-disambiguated lockfile is rejected with an actionable error. It is not
silently merged or partially inventoried. General multisource identity support
is deferred, not represented as implemented.

The suggested `tested`/passed-state inconsistency is unreachable in this frozen
planning schema, which rejects `tested`, `implemented` and `captured` statuses and
all runtime observations. It does not justify expanding this ticket into verified
capture support. Empty audits, duplicate catalog IDs and invalid calendar dates
do require rejection within the current planning contract.

Sol also found a checkout-portability issue outside Max's list: the Windows
working-tree lockfile uses CRLF while the Git blob uses LF. Their physical hashes
differ (`036226b0…` versus `08af1e2b…`) despite identical lock semantics. The
inventory identity must use explicitly labelled LF-normalized lock bytes; this
normalization must never alter registry artifact checksums or reference binary
hashes. The original physical host hash above remains an accurate historical
measurement, not the portable inventory identity.

### Review disposition

| Finding | Disposition |
|---|---|
| Empty audit, duplicate evidence IDs, calendar-invalid dates | Rejected; regression tests added |
| Observation/test status gates | Runtime-reference defense added; verified/tested states remain forbidden in this planning schema |
| Overbroad zero-upstream-code wording | Scoped to candidate launcher source; existing GPUI attribution explicitly retained |
| Checkout origin and in-repository destinations | Origin verification and resolved-path/junction guard added before writes/builds |
| Permissive synthetic fetch server | Removed; local advertised-HEAD test does not claim GitHub fetch-policy coverage |
| Stale CI documentation | Updated to configured, not remotely executed |
| Missing valid SPDX identifiers | NCSA, MIT-0 and GPL-2.0-only added; 671 declared expressions emitted, 46 outside the allowlist/legacy forms retained verbatim with NOASSERTION |
| Source-disambiguated Cargo identities | Deferred with fail-closed diagnostic and regression; current lock has no duplicate name/version pairs |
| Windows PowerShell OS gate | Uses Windows_NT environment check; Windows PowerShell parser check passed |
| Test-count drift | Historical and current counts separated; latest combined inventory suite is 25 |
| Custom wrapper root | Passed to both validator and test harness; Sol independently confirmed missing-audit failure in a custom temporary tree |
| Sol's CRLF/LF inventory portability finding | Canonicalizes CRLF only; regression proves equivalent rendered outputs |

### Final Sol run after review corrections

Sol reran the same complete `scripts/validate.ps1` command shown above after the
review corrections, with `-Toolchain stable -Offline -NativeSmoke`, the explicit
Python executable, and **no skip switches**. Exit **0**:

- Rust: **138 unit tests**, **1 doctest passed**, **1 doctest ignored**.
- Dependency/repository tooling: **25 passed**.
- Reference validator/subset suite: **41 passed**; ledger/coverage check passed.
- Reuse safety suite: **22 passed**.
- Format, Clippy, development desktop build, and the 717-package inventory check:
  passed.
- Windows startup smoke and **30-stage** synthetic interaction smoke: passed.

Thus **88 tooling tests** passed; none is a claim that the 100 planned manual
scenario tests or upstream candidate suites passed. The dependency
future-incompatibility notice remains as described above.

Sol independently compared all 717 generated package identities/sources and all
708 raw registry checksums against Cargo.lock again. The canonical lock bytes
equal the committed Git blob, SHA-256
`08af1e2b90c05370590482b9f0e5237b2227f3eb7f12f3ced049c09ab70db565`.
The physical Windows lockfile is unchanged.

The hardened cached fetch verifier exited **1**, refusing AnyRun's historical
`https://github.com/Kirottu/anyrun` origin instead of the declared
`https://github.com/anyrun-org/anyrun`. Sol did not rewrite that origin or discard
the checkout. A separate read-only verification accepted the other five cached
checkouts' pin/origin/cleanliness and confirmed this one expected refusal. All
six commit hashes still match the source evidence. A fresh canonical checkout,
not automatic relabelling of cached provenance, is required to use that cache
with the hardened probe.

Timeout cleanup confirmation is tested on the normal stop path; an unconfirmed
kill failure is handled defensively but was not synthetically induced. No remote
CI, macOS/Linux execution, reference runtime capture, complete candidate build,
or third-party legal review is inferred from these results.

The final relative Markdown file-link check passed **74 links across 17
documents** after the attribution/doc fixes; external URLs and anchors were not
part of that check.

### Max targeted follow-up

The independent Max follow-up confirmed the prior findings resolved within the
declared contract, independently reran **41 reference tests** and **22 reuse
safety tests** successfully, and verified the LF-normalized inventory identity
and all **708** registry checksums. No other prior findings remained.

Two disclosed P3 caveats remain:

- Before any future verified-capture migration, bind `observation.evidence_id`
  itself to a catalog runtime entry, not merely a runtime entry in the row's
  `evidence_refs`. Current unknown-only validation rejects every observation,
  so this cannot accept an observation in the current schema.
- The unconfirmed-kill failure path records cleanup failure and exits nonzero,
  but only the confirmed-stop timeout path was test-induced. This is an explicit
  coverage caveat, not a reported code failure.

Sol accepts the bounded repository deliverables with these limitations and the
external acceptance gaps above. Max did not approve an unexecuted hosted CI run,
exact-pin build, candidate runtime, release, or reference parity claim. At the
time of this validation the changes were uncommitted in the task worktree;
subsequent PR publication does not change these results or assert Kaneo completion.

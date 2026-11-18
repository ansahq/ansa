# ANS-6 closure and ownership handoff (2026-11-18)

Ticket: F02, "Complete build-versus-contribute reuse evaluation". This file closes the
bounded evidence slice; it does not replace ADR 0003 or claim that any candidate
application builds, passes its tests or is runtime-validated.

## Decision status

ADR 0003 is **accepted as the final F02 scope decision**: build Ansa greenfield, with
zero reuse of candidate launcher source. The provisional greenfield choice is closed.
The decision does not authorise copying code later; any future component reuse needs a
new ADR and the licence/provenance work in
[`license-audit.md`](license-audit.md).

## Acceptance criteria assessment

| Criterion | Status | Where recorded |
|---|---|---|
| ADR records contribute/fork/greenfield with explicit criteria, evidence and unresolved unknowns | Met | [ADR 0003](../adr/0003-reuse-evaluation.md), decisions 1-5, addendum, acceptance gaps |
| Licence obligations and notice requirements are listed for every candidate considered | Met at repository and manifest level | [`license-audit.md`](license-audit.md), [`dependency-scan.md`](dependency-scan.md) |
| Any proposed reuse names exact commits, files and dependencies; no source copied before audit | Not triggered: zero reuse. No candidate source, test, asset, schema or documentation text was copied | [ADR 0003](../adr/0003-reuse-evaluation.md) decision 2; [`license-audit.md`](license-audit.md) "What zero reuse means" |
| ADR 0003 is superseded or explicitly confirmed as the final scope decision | Met: explicitly confirmed as final | [ADR 0003](../adr/0003-reuse-evaluation.md) header and confirmation section |

The ticket's test plan is disposed of as follows:

- **Clean build or documented build failure:** four cargo probes failed at host
  prerequisites or dependency/toolchain limits (Gauntlet `libclang`; pop-launcher Unix-only
  `wayland-sys`; anyrun missing GTK `gio-2.0`; cosmic-launcher locked
  `accesskit_winit` error). Vicinae's CMake configure identified MSVC 19.44 and failed
  at missing Qt6 6.9. Flare remains documented non-feasible historical evidence
  (Tauri/pnpm/Swift/SoulverCore/AppImage). No full compile completed.
- **Run existing tests where feasible:** not feasible after those failures. Zero
  candidate tests executed. Test-quality statements are repository/CI inspection only.
- **Dependency and licence scan:** a static, no-network scan counted 3,466 Cargo lock
  packages, 704 npm lock entries and 836 pnpm package entries. All Cargo and pnpm
  entries remain licence-unknown from their lockfiles; 120 npm entries have no licence
  field. No registry/SPDX resolution was attempted.
- **Dated maintainer activity:** the 2026-11-18 re-check found Gauntlet and Flare
  inactive, Vicinae active through that day, and pop-launcher, anyrun and
  cosmic-launcher active within 90 days. Only Vicinae's default-branch HEAD had moved
  past its pin. No GitHub archive banner was present for any of the six.

## Evidence executed

- The five scanned cached checkouts matched their pins and were clean; the cached
  anyrun origin (`https://github.com/Kirottu/anyrun`) was **not** rewritten. Anyrun's
  static scan used a fresh canonical checkout at the same pin in
  `%TEMP%\opencode\f02-scan-anyrun`.
- `scripts/reuse/scan-dependencies.ps1` ran on the five cached checkouts and the fresh
  anyrun checkout. Per-candidate counts and unknown classes are in
  [`dependency-scan.md`](dependency-scan.md).
- The Vicinae configure probe is recorded in
  [`build-verification.md`](build-verification.md); it used an external build
  directory and disabled downloads and Node module installation.
- `pwsh -NoProfile -File scripts/reuse/tests/run-tests.ps1` passed **27 tests, 0
  failures**. The suite covers pin/origin/dirty refusal, repository-boundary guards,
  timeout handling, environment restoration, append-only records, and five static-scan
  cases (counts/unknowns/no-network, dirty refusal, wrong-pin refusal, in-repository
  output refusal and candidate-source overwrite refusal).
- Sol reports an independent run of the updated 27-case suite and the full exact-pin
  offline validation passing. That independent run is not duplicated or reinterpreted
  here.

## Remaining gaps (not blockers to the scope decision)

- No clean candidate build on Linux, macOS or Windows; no packaged artifact, no
  installed runtime, and no candidate test suite executed.
- No per-package licence resolution or SBOM, no C++/Qt/FetchContent transitive notice
  inventory, no per-file MPL audit, and no legal review or clearance.
- No GUI, accessibility, IME, focus, performance or extension-corpus validation.
- Flare was not built; it remains historical design evidence only.
- GitHub REST data was rate-limited; archival/activity statements rest on public Atom
  feeds, the HTML archive banner and `git ls-remote`, not star counts or API-only
  fields.
- CI Linux execution is independent work and remains open; this handoff does not claim
  a hosted CI run.

These gaps are acceptance limitations, not evidence that the greenfield decision is
wrong. Reopen conditions are listed in ADR 0003.

## Ownership and publication handoff

Owned files changed in this slice:

- `docs/adr/0003-reuse-evaluation.md`
- `docs/reuse/README.md`
- `docs/reuse/candidate-evidence.md`
- `docs/reuse/license-audit.md`
- `docs/reuse/build-verification.md`
- `docs/reuse/dependency-scan.md` (new)
- `docs/reuse/closure.md` (new)
- `scripts/reuse/scan-dependencies.ps1` (new)
- `scripts/reuse/bounded-build.ps1` (comma-separated candidate normalisation)
- `scripts/reuse/tests/run-tests.ps1` (five scanner cases and a real multi-candidate
  preflight assertion)

No candidate source was copied into this repository. The implementation subagent
made no commits or pushes; Sol committed the handed-off work for publication in PR #2.
CI workflow files, the shared implementation-status document and the reference ledger
were not changed by this slice. The work is ready for review and publication by the
owning session.

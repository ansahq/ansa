# Reuse evaluation records (F02)

Status: bounded evaluation performed 2026-11-17, addenda 2026-11-18. This directory is
evidence for [ADR 0003](../adr/0003-reuse-evaluation.md). It is not a licence opinion
and not a verification that any candidate launcher works. The final acceptance
assessment and ownership handoff are in [`closure.md`](closure.md).

The three-day investigation proposed in blueprint section 18 was compressed into a
single bounded session (network inspection plus host-feasibility probes), with a
documented 18 November addendum for the static dependency scan, dated activity re-check
and vicinae configure probe. Nothing here is a substitute for the build, runtime,
accessibility, extension-corpus and legal work listed as gaps in
[`build-verification.md`](build-verification.md) and
[`license-audit.md`](license-audit.md).

## Files

| File | Contents |
|---|---|
| [`candidate-evidence.md`](candidate-evidence.md) | Pinned identity, activity, architecture, build/test indicators, extension model and fit/delta per candidate |
| [`license-audit.md`](license-audit.md) | Repository-level licence findings, obligations if source were reused, mixed-licence traps and audit gaps |
| [`dependency-scan.md`](dependency-scan.md) | Static manifest/lockfile counts for Cargo, npm and pnpm, Vicinae CMake declarations, SPDX header counts and explicit unknowns |
| [`build-verification.md`](build-verification.md) | Exact commands, host, durations, exit codes, first failures and what could not be attempted |
| [`closure.md`](closure.md) | ANS-6 acceptance assessment, remaining gaps and ownership handoff |

Reproduction assets live in [`scripts/reuse`](../../scripts/reuse):

- `candidates.json` is the single pin table; the scripts validate its 40-character
  commit values and reject duplicates or incomplete entries.
- `fetch-candidates.ps1` fetches the six pinned commits into an external scratch
  directory (default `%TEMP%\opencode\ansa-reuse-candidates`). It refuses dirty,
  pin-mismatched or wrong-origin existing checkouts and never resets or discards local
  work. It never copies candidate source into this repository.
- `bounded-build.ps1` verifies all selected checkouts against the pins, their
  `remote.origin.url` and tracked/untracked cleanliness before running any command;
  refuses destinations inside this Ansa repository; restores `CARGO_TARGET_DIR`;
  enforces a `1..86400` second timeout with truthful process-stop records; appends
  JSONL records to `<Destination>/results.jsonl`; and writes each run's logs under
  `<Destination>/logs/<runId>`. Confirmed probe failures still exit 0; refusals and an
  unconfirmed process stop exit non-zero.
- `scan-dependencies.ps1` performs the same pin/origin/cleanliness and
  repository-boundary checks, then reads only manifests, lockfiles, CMake
  declarations and licence/SPDX text already present in the checkouts. It makes no
  network request, consults no registry metadata, executes no candidate script and
  writes a static JSON record. It reports unresolved licence classes rather than
  inferring them.
- `tests/run-tests.ps1` uses temporary miniature repositories and a fake cargo to test
  refusal, origin validation, repository-boundary guards, environment restoration,
  timeout validation, result records and the static scanner without a candidate build
  or network access.

## Method and evidence classes

1. Pin each candidate to a concrete commit returned by `git ls-remote HEAD`, then
   shallow-clone that commit. Record commit date, subject and release feed.
2. Read repository files at the pin (README, manifests, licence, CI workflows) with
   raw-content URLs that include the commit SHA.
3. Attempt only builds that fit the host and a fixed time limit. Record the command,
   elapsed time, exit code and first failure. A timeout or missing host tool is a
   result, not a defect claim about upstream. On 2026-11-18 a bounded Vicinae CMake
   configure was added; it reached the Qt6 requirement and stopped because Qt6 6.9 is
   not installed.
4. Count manifests and lockfile entries statically, without network or registry
   metadata, and report every class of unresolved licence rather than inferring one.
5. Treat repository/CI statements as claims, not as our test results. No candidate
   application was installed, launched, screen-reader-tested or benchmarked.

Evidence used here is **source inspection, a static manifest/lockfile scan and local
build probes**. Product behaviour, latency, accessibility and compatibility are **not**
evidenced.

## What was deliberately not done

- No candidate source, asset or text was copied into Ansa.
- No registry/SPDX lookup or per-package licence resolution was performed for the
  3,466 Cargo, 704 npm and 836 pnpm entries. The 2026-11-18 scan counts them and names
  the unknowns; see the audit gap statement.
- No Linux or macOS build was attempted and no full compile of any candidate was
  completed. The probe shell had no WSL, no configured Qt6/Vulkan CMake environment,
  and no GTK development libraries, `protoc` or `just`; `cl` was not on `PATH`. The
  Vicinae configure probe then confirmed MSVC is discoverable through the Visual
  Studio generator, and failed at missing Qt6 6.9.
- No Raycast/Vicinae extension was run; no extension-store account or service was
  contacted.
- No upstream issue, pull request or contribution was opened.
- No benchmark of candidate startup, search latency or memory.

## Scope note

This evaluation answers "should the Ansa foundation be contributed to, forked from,
or built greenfield?" It does not authorise copying code later. Any future component
adoption needs its own licence/provenance review and an ADR update.

# Reuse evaluation records (F02)

Status: bounded evaluation performed 2026-11-17. This directory is evidence for
[ADR 0003](../adr/0003-reuse-evaluation.md). It is not a licence opinion and not a
verification that any candidate launcher works.

The three-day investigation proposed in blueprint section 18 was compressed into a
single bounded session (network inspection plus host-feasibility probes). Nothing
here is a substitute for the build, runtime, accessibility, extension-corpus and
legal work listed as gaps in [`build-verification.md`](build-verification.md) and
[`license-audit.md`](license-audit.md).

## Files

| File | Contents |
|---|---|
| [`candidate-evidence.md`](candidate-evidence.md) | Pinned identity, activity, architecture, build/test indicators, extension model and fit/delta per candidate |
| [`license-audit.md`](license-audit.md) | Repository-level licence findings, obligations if source were reused, mixed-licence traps and audit gaps |
| [`build-verification.md`](build-verification.md) | Exact commands, host, durations, exit codes, first failures and what could not be attempted |

Reproduction assets live in [`scripts/reuse`](../../scripts/reuse):

- `candidates.json` is the single pin table; both scripts validate its 40-character
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
- `tests/run-tests.ps1` uses temporary miniature repositories and a fake cargo to test
  refusal, origin validation, repository-boundary guards, environment restoration,
  timeout validation and result records without a candidate build or network access.

## Method and evidence classes

1. Pin each candidate to a concrete commit returned by `git ls-remote HEAD`, then
   shallow-clone that commit. Record commit date, subject and release feed.
2. Read repository files at the pin (README, manifests, licence, CI workflows) with
   raw-content URLs that include the commit SHA.
3. Attempt only builds that fit the host and a fixed time limit. Record the command,
   elapsed time, exit code and first failure. A timeout or missing host tool is a
   result, not a defect claim about upstream.
4. Treat repository/CI statements as claims, not as our test results. No candidate
   application was installed, launched, screen-reader-tested or benchmarked.

Evidence used here is **source inspection plus local build probes**. Product
behaviour, latency, accessibility and compatibility are **not** evidenced.

## What was deliberately not done

- No candidate source, asset or text was copied into Ansa.
- No repository-wide dependency licence scan for the ~3,466 lockfile packages was
  performed; see the audit gap statement.
- No Linux or macOS build was attempted. The probe shell had no WSL, no configured
  MSVC/Qt6/Vulkan build environment, and no GTK development libraries, `protoc` or
  `just`; `cl` not being on PATH does not establish that MSVC is absent.
- No Raycast/Vicinae extension was run; no extension-store account or service was
  contacted.
- No upstream issue, pull request or contribution was opened.
- No benchmark of candidate startup, search latency or memory.

## Scope note

This evaluation answers "should the Ansa foundation be contributed to, forked from,
or built greenfield?" It does not authorise copying code later. Any future component
adoption needs its own licence/provenance review and an ADR update.

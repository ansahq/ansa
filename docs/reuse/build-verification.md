# Build and test verification record (2026-11-17)

This records what was actually executed on this host for the F02 reuse evaluation,
including failures and time limits. It is not a candidate compatibility report.

## Host and timebox

| Item | Value |
|---|---|
| OS | Windows 11 Home Single Language 10.0.26200 (build 26200), 64-bit |
| CPU / RAM | Intel Core i5-12450H / 15.6 GB |
| Rust | rustc 1.99.1 (58cf8f9c3, 2026-10-13), cargo 1.99.1 |
| Other tools | git 2.56.0.windows.2, Node 24.22.1, CMake 4.4.3 (MinGW distribution), Ninja |
| Probe-shell availability | `cl` and Qt6 on PATH were not available; Vulkan SDK, GTK4 development libraries, `protoc`, `just`, `clang`/`libclang` and WSL were not found. MSVC installation is **not disproven**: another workspace built with MSVC on this host, and `cc` can locate Visual Studio without `cl` on PATH. |
| Local time | IST (UTC+05:30); probes ran 15:11:54–15:19:40 |
| Timebox | One working session, under one hour wall-clock including reading and documentation. About 10 minutes were clone plus build probes; no multi-day investigation and no unbounded retries. |

The three-day activity from blueprint section 18 was intentionally compressed. That
compression is a limitation, not a claim that a day-per-candidate review would find
nothing new.

## Checkouts

Shallow clones were made into
`C:\Users\abhir\AppData\Local\Temp\opencode\f02` (approved external temp area), one
per candidate:

```powershell
git clone --depth 1 --quiet https://github.com/<repo> <temp>\<candidate>
```

`HEAD` of each clone was verified with `git rev-parse HEAD`; all six matched the pins
in [`candidate-evidence.md`](candidate-evidence.md). The same check can be repeated
with:

```powershell
pwsh -File scripts/reuse/fetch-candidates.ps1 -Destination "$env:TEMP\opencode\ansa-reuse-candidates"
```

That script was run against the recorded checkouts and verified all six pins
successfully. No candidate source was copied into this repository.

## Build probes

All probes used `CARGO_TARGET_DIR=<temp>\f02\target` (one shared target directory) and
the candidate's own `Cargo.lock` through `--locked`. Each ran under a hard
`Start-Process.WaitForExit` limit; on timeout the process tree would be killed. No
probe timed out. The registry cache and shared target directory mean these are
feasibility probes, not cold-build benchmarks.

| Candidate | Command | Result | Duration | First failure |
|---|---|---|---|---|
| Gauntlet | `cargo check -p gauntlet --locked` | exit 101 | 222.0 s | `libsqlite3-sys` build script: `Unable to find libclang` (`bindgen`, `clang.dll`/`libclang.dll`) |
| pop-launcher | `cargo test --workspace --locked` | exit 101 | 30.0 s | `wayland-sys 0.31.11`: `cannot find unix in os` (`std::os::unix::io::RawFd`) |
| anyrun | `cargo check --workspace --locked` | exit 101 | 30.3 s | `gio-sys`: system library `gio-2.0` not found (`PKG_CONFIG_PATH` unset) |
| cosmic-launcher | `cargo check --locked` | exit 101 | 182.7 s | `accesskit_winit` from a locked git checkout under Rust 1.95: `E0782 expected a type, found a trait` on `&Window` |
| Vicinae | — | not attempted | — | No configured MSVC/Qt6/Vulkan CMake environment in the probe shell; toolchain discovery was not attempted |
| Flare | — | not attempted | — | Historical only; needs pnpm, Tauri Linux prerequisites, Swift and SoulverCore |

Notes on interpretation:

- The Gauntlet failure is a **host prerequisite**, not an upstream defect: its
  `libsqlite3-sys` dependency uses `bindgen`, and this host has no LLVM. Installing
  LLVM was out of scope. The full Windows MSI was not attempted; upstream CI allows
  60 minutes and installs Node 22, `protoc`, WiX, then runs `npm ci`, so a clean
  package build is materially larger than this probe.
- pop-launcher, anyrun and cosmic-launcher failures are consistent with their
  documented Linux/Wayland system dependencies. They are **not evidence that those
  projects fail on their supported platforms**; no Linux machine or WSL was available.
- The cosmic-launcher failure is inside the locked git dependency checkout
  (`accesskit_winit`, rev `6c20249`), under the toolchain selected by its
  `rust-toolchain.toml` (Rust 1.95) on a host it does not target. Because
  `Cargo.lock` pins the git revisions, this is **not observed dependency drift**.
  If the lockfile is regenerated, manifests that reference git dependencies without
  a `rev` (libcosmic and others) could resolve different revisions; that remains a
  future update risk, not the cause recorded here.
- The bound `cargo check -p gauntlet` probe was repeated through the pre-hardening
  `scripts/reuse/bounded-build.ps1` and failed identically in 5.3 s with warm caches,
  confirming the script reproduced the recorded result.

### Script used

```powershell
pwsh -File scripts/reuse/bounded-build.ps1 `
  -Destination "$env:TEMP\opencode\ansa-reuse-candidates" `
  -TimeoutSeconds 600
```

The script writes stdout/stderr logs beside the checkouts, prints exit code or
timeout, and never installs packages or elevates.

### Script safety contract and result records

The scripts were hardened after the 17 November probes, so the table above came from
the ad-hoc session runner rather than from `results.jsonl`. The checkouts were pinned
and verified manually at the time; treat the table as a recorded probe log, not as
script output.

Current behaviour:

- `fetch-candidates.ps1` verifies every existing checkout against
  `scripts/reuse/candidates.json`: commit, `remote.origin.url` and tracked/untracked
  cleanliness. HTTPS origin variants are normalized for case, trailing slash and
  `.git`; credentialed URLs, alternate ports and different owner/repository origins
  are refused. It never resets, cleans, stashes or deletes local work. Every native
  git call is exit-code checked.
- `bounded-build.ps1` verifies **all** selected checkouts (commit, origin and
  cleanliness) before starting any candidate command, then re-checks immediately
  before each spawn. A refusal exits non-zero and nothing is executed.
- Both scripts refuse a destination (or candidate path) that resolves inside this
  Ansa repository, including paths reached through junction or symlink ancestors.
  The guard runs before directories, git state or records are created.
- `TimeoutSeconds` is validated to `1..86400`; zero, negative and overflowing values
  are rejected before a probe starts. The process tree is killed on timeout; the
  record truthfully carries `processStopConfirmed` and `cleanupError`. If the process
  cannot be confirmed stopped, a probe record is still written and the script then
  exits non-zero rather than silently continuing.
- `CARGO_TARGET_DIR` is set for probes and restored to the caller's exact value, or
  removed if it was previously unset, in a `finally` block.
- Confirmed probe failures and timeouts produce exit code **0** from the collector,
  because they are evidence. Refusals, invalid arguments and an unconfirmed process
  stop exit non-zero.
- Records are appended as JSON Lines to `<Destination>/results.jsonl`; an existing file
  is never truncated. Each run gets `<Destination>/logs/<runId>/name.out` and
  `name.err`, so earlier logs are not overwritten.
  - `kind: "probe"` records carry `runId`, UTC timestamp, candidate, repository,
    `expectedCommit`, `actualCommit`, executable, the semantic `command`, the full
    actual `arguments` vector (including injected prefixes), `exitCode` (null on
    timeout), `timedOut`, `processStopConfirmed`, `cleanupError`, `timeoutSeconds`,
    `durationSeconds`, log paths and the first stderr line containing `error`;
  - `kind: "refusal"` records carry the same schema with per-candidate problems and
    a message, and no command is recorded as executed.

### Script tests

```powershell
pwsh -NoProfile -File scripts/reuse/tests/run-tests.ps1
```

The suite uses temporary miniature git repositories and a fake cargo executable. It
does not contact the network, touch the candidate checkouts, or run a candidate build.
Recorded result on 2026-11-17: **22 passed, 0 failed**. Cases cover:

- wrong pin refusal before cargo, with no invocation marker;
- tracked modification and untracked file refusal, preserving local content;
- preflight of all selected checkouts (a dirty second candidate stops the first);
- HTTPS origin normalization (case, trailing slash, `.git`) accepted; different
  owner/repository and credentialed origins refused before cargo;
- repository-root, nested-in-repository and junction-routed destinations refused
  before directories or records are written, for both fetch and build;
- `CARGO_TARGET_DIR` restoration for a set value and for an originally unset value;
- timeout arguments `0`, `-1` and `86401` rejected before execution;
- a 1-second timeout that kills the fake probe, records `timedOut: true`, a null exit
  code, `processStopConfirmed: true` and no cleanup error;
- two consecutive probes in one destination, verifying that prior result bytes and
  prior log bytes are retained, runIds and log paths are distinct, and the record
  carries both the semantic command and the full injected argument vector;
- a refusal in the same outside-repository destination, verifying that earlier probe
  records are retained and the refusal is appended;
- fetch refusal for dirty and clean-but-mismatched checkouts without mutation;
- a new fetch at the pin, and non-zero failure with the native git exit code surfaced.

The fetch test uses a local mini repository and its advertised HEAD, with no
`uploadpack.allowAnySHA1InWant` override. It proves the script's local fetch path, not
GitHub's arbitrary-SHA fetch policy. For each real pin, the exact fetch policy remains
a remote boundary until the commit is fetched and initially verified; later checks are
local. The unconfirmed-kill failure path is implemented defensively but was not
induced in the synthetic suite.

The real `fetch-candidates.ps1` was also re-run against the six recorded checkouts:
all six verified at their pins and were clean. No candidate build was rerun.

## Tests actually executed

| Suite | Executed? | Evidence |
|---|---|---|
| Gauntlet Rust/JS tests | No | `cargo check` failed before test build; textual test scan found none |
| Vicinae Catch2 tests | No | No configured MSVC/Qt6/Vulkan build environment; not built |
| pop-launcher tests | No | Workspace test build failed on Unix-only `wayland-sys` before tests ran |
| anyrun tests | No | Workspace check failed on missing GTK4 development libraries |
| cosmic-launcher tests | No | Check failed in an `accesskit_winit` git dependency |
| Flare Vitest/Rust tests | No | Historical only; build intentionally not attempted |

**Zero candidate tests executed successfully.** Any statement about candidate test
quality in this evaluation is based on repository/CI inspection, not execution.

## What this does not validate

- No candidate was installed or launched; no GUI behaviour, summon/focus, IME,
  screen-reader, clipboard, launch or extension-corpus check was performed.
- No performance, memory, power or startup measurement.
- No build on Linux or macOS; no Windows build of Vicinae; no full artifact build of
  any candidate.
- No dependency-level licence inventory or vulnerability scan.
- No legal review of licences, notices, bundled binaries or store terms.
- GitHub's REST API returned 403 for this unauthenticated host, so star counts and
  the archived flag were not verified programmatically.

These are acceptance gaps in the evidence, not reasons to treat the greenfield
decision as unverified. The decision in [ADR 0003](../adr/0003-reuse-evaluation.md)
rests on project identity, licence, maintenance and architecture evidence, all of
which were inspected directly at pinned commits.

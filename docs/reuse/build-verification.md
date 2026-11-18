# Build and test verification record (2026-11-17, addendum 2026-11-18)

This records what was actually executed on this host for the F02 reuse evaluation,
including failures and time limits. It is not a candidate compatibility report.

## Host and timebox

| Item | Value |
|---|---|
| OS | Windows 11 Home Single Language 10.0.26200 (build 26200), 64-bit |
| CPU / RAM | Intel Core i5-12450H / 15.6 GB |
| Rust | rustc 1.99.1 (58cf8f9c3, 2026-10-13), cargo 1.99.1 |
| Other tools | git 2.56.0.windows.2, Node 24.22.1, CMake 4.4.3 (MinGW distribution), Ninja |
| Probe-shell availability | `cl` and Qt6 were not on `PATH`; Vulkan SDK, GTK4 development libraries, `protoc`, `just`, `clang`/`libclang` and WSL were not found. On 18 November `vswhere` located Visual Studio 2022 Build Tools and the Vicinae configure probe used MSVC 19.44.35229.0; Qt6 6.9 was still absent. |
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

Before origin-verifier hardening, that script was run against the recorded
checkouts and verified all six commit pins. The current verifier also checks
origin and refuses the cached anyrun checkout; the 18 November scan used a fresh
canonical checkout instead. No candidate source was copied into this repository.

## Build probes

The four cargo probes used `CARGO_TARGET_DIR=<temp>\f02\target` (one shared target
directory) and the candidate's own `Cargo.lock` through `--locked`. Each ran under a
hard `Start-Process.WaitForExit` limit; on timeout the process tree would be killed. No
probe timed out. The registry cache and shared target directory mean these are
feasibility probes, not cold-build benchmarks. The Vicinae CMake configure used a
separate external build directory and disabled downloads.

| Candidate | Command | Result | Duration | First failure |
|---|---|---|---|---|
| Gauntlet | `cargo check -p gauntlet --locked` | exit 101 | 222.0 s | `libsqlite3-sys` build script: `Unable to find libclang` (`bindgen`, `clang.dll`/`libclang.dll`) |
| pop-launcher | `cargo test --workspace --locked` | exit 101 | 30.0 s | `wayland-sys 0.31.11`: `cannot find unix in os` (`std::os::unix::io::RawFd`) |
| anyrun | `cargo check --workspace --locked` | exit 101 | 30.3 s | `gio-sys`: system library `gio-2.0` not found (`PKG_CONFIG_PATH` unset) |
| cosmic-launcher | `cargo check --locked` | exit 101 | 182.7 s | `accesskit_winit` from a locked git checkout under Rust 1.95: `E0782 expected a type, found a trait` on `&Window` |
| Vicinae | `cmake -S . -B <temp>\vicinae-configure-build -G "Visual Studio 17 2022" -A x64 -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTS=OFF -DINSTALL_NODE_MODULES=OFF -DVICINAE_NODE_RUNTIME_DOWNLOAD=OFF -DFETCHCONTENT_FULLY_DISCONNECTED=ON -DLOCAL_AI=OFF` | exit 1 | 3.6 s | `find_package(Qt6 6.9 REQUIRED COMPONENTS Core Qml)` at `CMakeLists.txt:152`: no Qt6 package configuration found. MSVC 19.44.35229.0 was identified first. |
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
- The Vicinae configure probe used an **external build directory** so the pinned
  checkout stayed clean. Downloads and Node module installation were disabled
  (`FETCHCONTENT_FULLY_DISCONNECTED=ON`, `INSTALL_NODE_MODULES=OFF`,
  `VICINAE_NODE_RUNTIME_DOWNLOAD=OFF`). CMake identified MSVC 19.44.35229.0 through
  the Visual Studio 17 2022 generator, then stopped at missing Qt6 6.9. This is a
  **host prerequisite failure**, not evidence that Vicinae fails on its documented
  Windows CI image. The project's own Windows preset uses Ninja with `cl` on `PATH`;
  the probe used the Visual Studio generator because `cl` was not on this PATH. No
  compile, link or Catch2 test ran, so the Windows build and test claims remain
  upstream CI statements rather than results reproduced here.
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

### Static dependency scan (2026-11-18)

The scanner was run on the five cached checkouts whose origin matched the pin table,
and separately on a fresh canonical anyrun checkout at the same pin:

```powershell
& scripts/reuse/scan-dependencies.ps1 -Destination "$env:TEMP\opencode\f02" `
  -Candidate @('gauntlet','vicinae','pop-launcher','cosmic-launcher','flare') `
  -OutputPath "$env:TEMP\opencode\f02\dependency-scan-cached-five.json"

pwsh -NoProfile -File scripts/reuse/fetch-candidates.ps1 `
  -Destination "$env:TEMP\opencode\f02-scan-anyrun" -PinFile "$env:TEMP\opencode\anyrun-pin.json"

& scripts/reuse/scan-dependencies.ps1 -Destination "$env:TEMP\opencode\f02-scan-anyrun" `
  -Candidate anyrun -OutputPath "$env:TEMP\opencode\f02\dependency-scan-anyrun.json"
```

Actual output totals: 3,466 Cargo lock packages, 704 npm lock entries (584 with a
licence field, 120 without) and 836 pnpm package entries. The per-candidate table and
all explicit unknowns are in [`dependency-scan.md`](dependency-scan.md). The scan made
no network request and executed no candidate command. The cached anyrun checkout was
not modified or relabelled; the fresh checkout was required only because its cached
`remote.origin.url` is the historical `Kirottu/anyrun` path and the hardened verifier
refuses it.

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
Recorded result on 2026-11-18: **26 passed, 0 failed** (22 earlier safety cases plus
four static-scanner cases). Cases cover:

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
- a new fetch at the pin, and non-zero failure with the native git exit code surfaced;
- static scan of a miniature checkout reporting Cargo/npm/CMake counts, licence
  signatures, SPDX header count, explicit unknowns and `networkUsed: false`;
- static scan refusal for a dirty checkout and for a wrong pin, with no evidence file
  written;
- static scan refusal for an output path inside this Ansa repository, before the file
  is created.

The fetch test uses a local mini repository and its advertised HEAD, with no
`uploadpack.allowAnySHA1InWant` override. It proves the script's local fetch path, not
GitHub's arbitrary-SHA fetch policy. For each real pin, the exact fetch policy remains
a remote boundary until the commit is fetched and initially verified; later checks are
local. The unconfirmed-kill failure path is implemented defensively but was not
induced in the synthetic suite.

The historical pre-hardening `fetch-candidates.ps1` re-run confirmed six clean
checkouts at their commit pins. This is not a claim that today's stricter
origin check accepts cached anyrun; it deliberately does not. No candidate build
was rerun in that historical fetch check.

## Tests actually executed

| Suite | Executed? | Evidence |
|---|---|---|
| Gauntlet Rust/JS tests | No | `cargo check` failed before test build; textual test scan found none |
| Vicinae Catch2 tests | No | Configure failed at missing Qt6 6.9 before any build; no test binary was produced |
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
- No build on Linux or macOS; no completed Windows build of Vicinae (its configure
  stopped at the missing Qt6 6.9 requirement); no full artifact build of any candidate.
- No per-package licence resolution, registry/SPDX lookup, vulnerability scan or
  transitive C++/Qt notice inventory. Static manifest and lockfile counts are in
  [`dependency-scan.md`](dependency-scan.md).
- No legal review of licences, notices, bundled binaries or store terms.
- GitHub's REST API was rate-limited on 17 November and again after one lookup on
  18 November, so star counts and API-only fields were not used. Archival state was
  checked through the public HTML banner and commit/release Atom feeds on 18 November.

These are acceptance gaps in the evidence, not reasons to treat the greenfield
decision as unverified. The decision in [ADR 0003](../adr/0003-reuse-evaluation.md)
rests on project identity, licence, maintenance and architecture evidence, all of
which were inspected directly at pinned commits.

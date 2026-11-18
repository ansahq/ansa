# ADR 0003: bounded reuse evaluation and foundation decision

Date: 2026-11-11, revised 2026-11-17 and 2026-11-18 · Status: **accepted as the
final F02 contribute/fork/greenfield scope decision**. This record confirms the
provisional greenfield choice, closes it, and supersedes the "broader F02 remains
uninvestigated" caveat in the 11 November text. Evidence and acceptance gaps are
separated below; the global implementation-status document is not updated by this
record. The decision is **greenfield with zero candidate source reuse**.

Inspected at pinned commits on 2026-11-17:

| Candidate | Pinned commit | Licence | Role in this evaluation |
|---|---|---|---|
| Gauntlet | `ca066da5c9d56bd53a739c0dda1572fccc5019a8` | MPL-2.0 (root) | Closest Rust/React-launcher reference |
| Vicinae | `23fec1e1da14c422fba01db5c286589b6e946d06` | GPL-3.0 | Maintained launcher, Raycast-compatible extensions, Windows CI |
| pop-launcher | `4111617b7345134abe67332a7baa5ef12538f412` | MPL-2.0 + GPL-3.0-only parts | Maintained Rust IPC launcher service |
| anyrun | `7997bc470cafc1d201ad4d8a8f0432d83fa6c161` | GPL-3.0 | Maintained Rust/GTK4 Wayland runner |
| cosmic-launcher | `29c1bac5e8f9f83b9e2cef03bb0259a04ffd7f2a` | GPL-3.0 | Maintained Rust/libcosmic frontend |
| Flare | `09e126fc77f7716d912c85ce8f308ce708bc2530` | MIT | Historical only |

No application binary was installed or run, no candidate test suite executed
successfully, and no candidate source, asset, test or schema was copied into Ansa. A
static manifest/lockfile inventory was added on 2026-11-18; it reads only files already
in the pinned checkouts and resolves no package licences.

## Context

F02 requires inspecting Gauntlet and relevant open projects, comparing architecture,
current tests, licence, maintainers and delta, then recording a contribute/fork/
greenfield decision. The 11 November record inspected only Gauntlet and explicitly
left maintained forks, other launchers, build/test quality and migration cost
uninvestigated.

Blueprint section 18 proposes a three-day build-versus-contribute investigation and
names Gauntlet, Vicinae and Flare, treating Flare as historical. That three-day budget
was compressed into one bounded session: shallow clones at `git ls-remote HEAD` pins,
direct file inspection at those commits, release/commit feeds, textual test
indicators, and time-limited `cargo check`/`cargo test` probes on a Windows host.
Method and limits are in [`docs/reuse`](../reuse/README.md).

## Comparison matrix

| Candidate | Fit | Architecture and build/test evidence | Licence | Dated activity / releases | Extension model | Delta to Ansa | Outcome |
|---|---|---|---|---|---|---|---|
| Gauntlet | Rust launcher with React plugins; Windows best-effort | 12-crate Rust workspace; 3-OS build CI, no test job; local check blocked by host `libclang` | MPL-2.0 root, undeclared crates | Last commit 2025-10-02; last release v21, 2025-08-16; declares development stopped | Custom React reconciler + Deno | Own Iced/winit forks, Deno host, accessibility/IME unproven | Do not fork |
| Vicinae | Closest product scope; maintained | C++/Qt6 CMake; Linux/macOS/Windows builds; Catch2 tests run on Linux/macOS, not shown on Windows; local configure stops at missing Qt6 6.9 after identifying MSVC 19.44 | GPL-3.0 | Commits through 2026-11-18; v0.30.1 2026-11-15; weekly/monthly releases | TypeScript/React, Raycast API compat, pinned Node 22 | Different language/UI, GPL-3.0 combined-work obligations, upstream roadmap | Do not adopt as base; contribution only under a changed product/licence decision |
| pop-launcher | Launcher service only, no UI | Rust IPC service; no build/test CI; 8 test attributes; Linux-only deps | Mixed MPL-2.0 / GPL-3.0-only | Commit 2026-11-06; epoch-1.11.0 2026-10-20; ~biweekly epochs | Out-of-process executable plugins, JSON IPC | All UI, cross-platform integration and JS runtime remain | Design reference only |
| anyrun | Wayland launcher | Rust/GTK4; Ubuntu-only build CI; no tests; local check missing GTK dev libs | GPL-3.0 | Commit 2026-11-08; v26.10.0 2026-10-20; irregular releases | cdylib ABI plugins | Wayland-only, GTK, no Windows/macOS | Reject |
| cosmic-launcher | COSMIC desktop component | Thin Rust/libcosmic frontend; Ubuntu CI fmt/clippy/test; no in-tree tests; Unix-only | GPL-3.0 | Commit 2026-11-07 (local); epoch-1.11.0 2026-10-20 | pop-launcher plugins | Wayland/COSMIC only; locked failure on this host/toolchain, manifest git deps lack `rev` | Reject as base |
| Flare | Historical Raycast-style launcher | Tauri/Svelte/Node/Swift; textual tests only; not built | MIT | Last commit 2025-12-30; only release v0.1.0 2025-07-29; author-declared unmaintained | Raycast-compatible (historical) | Tauri/web UI, Linux AppImage, vendor SoulverCore | Historical only |

## Decision

1. **Build the Ansa foundation greenfield.** Do not fork a candidate, do not adopt one
   as the upstream base, and do not attempt to contribute the foundation into one.
   This is the final F02 scope decision; it closes the provisional greenfield choice
   rather than leaving it open for another candidate round.
2. **Zero reuse of candidate launcher source.** No code, test, asset, manifest schema
   or documentation text from the six evaluated candidates is incorporated. This is
   **not** a claim that the Ansa tree contains no upstream-derived code:
   `crates/ui-components/src/text_input.rs` adapts the pinned GPUI 0.2.2 example, and
   its attribution/notice remains in
   [`crates/ui-components/THIRD_PARTY.md`](../../crates/ui-components/THIRD_PARTY.md).
   This evaluation changes neither that adapted code nor its notice.
3. **Design-level reuse only, independently implemented.** Process-isolated extension
   workers, a manifest/permission negotiation model, cancellable plugin IPC and a
   layered view/action model remain valid architectural references. They are ideas to
   re-specify from Ansa's requirements, not copied interfaces, and they do not by
   themselves establish Raycast compatibility.
4. **Later component adoption needs a new ADR.** If a specific library or protocol
   implementation looks reusable, it must pass a per-file provenance, dependency and
   notice audit. It must also be compatible with the desired Apache-2.0-only
   distribution direction; this is not a finding that Apache-2.0 or MPL-2.0 are
   inherently incompatible with GPL-3.0, and MPL secondary-license and provenance
   questions would need legal review.
5. **No relicensing in this record.** Ansa remains Apache-2.0 as committed. The
   blueprint's MPL-2.0 proposal for new application/server code is still an open
   owner decision; this ADR does not resolve or silently apply it.

## Why this outcome

- **Gauntlet** has the intended Rust + React-extension shape, but its own README says
  development stopped, its last commit is 2025-10-02, Windows is best-effort, and the
  UI depends on a project-owned Iced fork (and its winit fork) plus a Deno runtime. Its
  1,247 lockfile packages and 60-minute packaged Windows build are a different
  maintenance surface, not a shortcut. No tests were found in a textual scan; the
  scenario-runner crate is not evidence of a passing conformance suite. A fork would
  revive an abandoned codebase.
- **Vicinae** is the strongest maintained candidate: active weekly/monthly releases,
  Windows/macOS/Linux CI, a pinned Node 22 extension runtime and a Raycast API
  compatibility layer. But it is C++/Qt6 under GPL-3.0. Distributing a combined work
  with it would have to satisfy GPL-3.0 while Apache-2.0 file notices and permissions
  remain in place; that is a different distribution direction, not a finding that the
  licences are inherently incompatible. Contributing would also replace Ansa's
  Rust/GPUI premise and accept upstream's roadmap; its CONTRIBUTING file discourages
  large architectural changes. Windows CI builds are evidenced, but the inspected
  Windows workflow does not show tests.
- **pop-launcher** is a Linux-shaped Rust IPC service with no UI. Its licence is mixed
  (MPL-2.0 library/service, GPL-3.0-only binary/plugins), it has no build/test CI, and
  its test suite is thin. It is a useful isolation reference, not an application base.
- **anyrun** is Wayland-only GTK4 with a cdylib plugin ABI, GPL-3.0, no tests and
  Ubuntu-only CI. **cosmic-launcher** is a thin COSMIC/libcosmic frontend over
  pop-launcher, GPL-3.0, Unix-only, with manifest git dependencies that lack `rev`
  pins. Neither is cross-platform.
- **Flare** is unmaintained, Tauri/Svelte with a Node sidecar and a vendor-supplied
  SoulverCore calculator path; it is historical design evidence only, as the
  blueprint already states.

The prior Gauntlet-only reasoning still holds after adding candidates: no maintained
project satisfies Ansa's Rust-UI + Windows/macOS/Linux + accessibility + extension-
platform combination without a larger identity change than greenfield development.

## 2026-11-18 evidence addendum

Three bounded follow-ups were completed without installing prerequisites, executing
candidate repository scripts or copying source:

1. A static dependency/licence inventory counted 3,466 Cargo lock packages, 704 npm
   lock entries and 836 pnpm package entries. All Cargo and pnpm entries remain
   licence-unknown from their lockfiles; 120 npm entries lack a licence field. Vicinae's
   seven CMake `FetchContent` declarations and eleven vendor directories were named,
   including an untagged `layer-shell-qt` dependency, but not licence-resolved. See
   [`dependency-scan.md`](../reuse/dependency-scan.md).
2. A dated activity re-check on 2026-11-18 found Gauntlet and Flare still inactive,
   Vicinae active through that day, and pop-launcher, anyrun and cosmic-launcher active
   within the preceding 90 days. Only Vicinae's default-branch HEAD had moved past its
   pin. None of the six showed a GitHub archive banner.
3. A bounded Vicinae CMake configure found MSVC 19.44.35229.0 and then failed at
   missing Qt6 6.9 after 3.6 s. No compile, link or Catch2 test ran. Flare remains
   intentionally unbuilt historical evidence.

These addenda close the inventory-counting, dated-activity and "Vicinae was not even
configured" gaps. They do **not** prove that any candidate builds on its supported
platforms, that any candidate test suite passes, or that any dependency licence is
compatible with Ansa. The greenfield, zero-source-reuse decision is unchanged.

## Explicit confirmation and supersession of the 11 November record

This revision:

- **confirms as final** the provisional greenfield choice for the foundation, with
  zero reuse of candidate launcher source, and closes the provisional stage;
- **supersedes** the Gauntlet-only evidence base, the statement that broader F02
  remains uninvestigated, and any reading of "MPL-2.0" that treats pop-launcher as a
  uniformly MPL project;
- **does not** confirm the blueprint's MPL-2.0 licensing direction, the GPUI framework
  gate, universal Raycast compatibility, or candidate quality beyond the inspections
  recorded here.

## Consequences

Accepted costs: Ansa owns the UI, input/IME, accessibility, native integration,
extension host and protocol work; there is no ecosystem or upstream maintenance
shortcut. The decision also forgoes the option of influencing Vicinae's roadmap
directly.

Avoided costs: no Iced/Relm4/libcosmic fork maintenance, no dead-upstream revival, no
shift to GPL-3.0 combined-work distribution obligations, and no dependency on a
candidate's release cadence or store service.

Residual risk: greenfield effort is underestimated, or a competitor's compatibility
layer outpaces Ansa. The blueprint's framework gate, extension-corpus plan and
release gates remain the mitigations.

## Reopen conditions

Re-evaluate this ADR, with a fresh pinned inspection and a new decision record, if any
of the following becomes true:

1. A maintained candidate exposes a Rust-embeddable host/UI library that supports
   Windows, macOS and Linux and provides credible evidence for the framework gate's
   accessibility/input slice (not merely a release claim).
2. Ansa's licensing direction changes to a GPL-3.0 combined-work distribution; Vicinae,
   anyrun or cosmic-launcher then become plausible bases and the delta changes
   materially.
3. A candidate relicenses to Apache-2.0/MIT or offers a stable extension-host
   abstraction usable without abandoning the Rust UI and platform goals.
4. Ansa fails the bounded framework gate or misses the planned extension-host
   milestone by enough to threaten the delivery plan, **and** a candidate demonstrably
   covers the mandatory slice; build-versus-contribute is then reopened rather than
   continued by inertia.
5. Gauntlet or a maintained fork resumes releases, moves Windows to a supported
   target and provides test evidence; fork cost is then re-compared with greenfield.
6. A new maintained Rust launcher meets the screen (Rust, cross-platform, extension
   ecosystem or extensible host, visible maintenance). Scheduled check no later than
   2027-05-17.
7. Any proposal to copy candidate source: this triggers a separate reuse ADR and the
   licence/provenance work in [`docs/reuse/license-audit.md`](../reuse/license-audit.md);
   the zero-reuse scope here must not be assumed to extend to that proposal.

## Evidence and verification

- Candidate identities, activity, architecture, test/build indicators and deltas:
  [`docs/reuse/candidate-evidence.md`](../reuse/candidate-evidence.md). A dated
  activity re-check on 2026-11-18 used public commit/release Atom feeds and an HTML
  archive-banner check; only Vicinae's default-branch HEAD had moved past its pin.
- Licence findings and obligations: [`docs/reuse/license-audit.md`](../reuse/license-audit.md)
- Static manifest/lockfile counts and explicit unknowns:
  [`docs/reuse/dependency-scan.md`](../reuse/dependency-scan.md). The scan reproduced
  3,466 Cargo lock packages and additionally counted 704 npm lock entries (584 with a
  licence field, 120 without) and 836 pnpm package entries. All Cargo and pnpm entries
  remain licence-unknown from their lockfiles; no registry/SPDX lookup was made.
- Exact commands, host, durations and failures:
  [`docs/reuse/build-verification.md`](../reuse/build-verification.md)
- Reproduction scripts: [`scripts/reuse/fetch-candidates.ps1`](../../scripts/reuse/fetch-candidates.ps1),
  [`scripts/reuse/bounded-build.ps1`](../../scripts/reuse/bounded-build.ps1),
  [`scripts/reuse/scan-dependencies.ps1`](../../scripts/reuse/scan-dependencies.ps1) and
  [`scripts/reuse/tests/run-tests.ps1`](../../scripts/reuse/tests/run-tests.ps1)

Pinned build probes (Windows host, 2026-11-17; Vicinae addendum 2026-11-18):
Gauntlet `cargo check -p gauntlet --locked` failed at a missing host `libclang` after
222 s; pop-launcher `cargo test --workspace --locked` failed on Unix-only `wayland-sys`
after 30 s; anyrun `cargo check --workspace --locked` failed on missing GTK `gio-2.0`
after 30 s; cosmic-launcher `cargo check --locked` failed in a locked
`accesskit_winit` git dependency under Rust 1.95 after 183 s. Vicinae's CMake configure
identified MSVC 19.44.35229.0 and then failed at missing Qt6 6.9 after 3.6 s; no
compile or Catch2 test ran. Flare was not built. No candidate tests executed.

The reproduction scripts refuse dirty, pin-mismatched or wrong-origin checkouts before
any candidate command, refuse destinations and output paths inside this Ansa
repository, restore `CARGO_TARGET_DIR`, append JSONL result records with per-run log
directories, and record timeout process-stop status truthfully. The build table above
predates those guards and came from the session runner; hardening was validated with
26 isolated fake-tool tests (22 earlier cases plus four static-scanner cases), and no
candidate build was rerun. The cached anyrun checkout was not relabelled or modified;
its static scan used a fresh canonical checkout at the same pin.

## Acceptance gaps

The decision above is supported by direct inspection of pinned sources, licences,
manifests, CI and activity. It is **not** supported by the following, which remain
open and must be stated whenever F02 is reported:

- no clean build on Linux or macOS, no WSL; no complete compile or packaged Windows
  artifact for any candidate. The Vicinae configure probe found MSVC 19.44 through the
  Visual Studio generator but stopped at missing Qt6 6.9; it did not use the project's
  Ninja/`cl` preset and does not establish Windows CI status;
- zero candidate tests executed; test-quality statements come from repository and CI
  inspection only;
- no installed runtime, GUI, accessibility, IME, focus, performance or extension-
  corpus validation of any candidate;
- no per-package licence resolution or SBOM for the 3,466 Cargo, 704 npm and 836 pnpm
  entries; the 2026-11-18 scan is a static count with named unknowns. No C++/Qt
  dependency graph, per-file MPL audit, or legal review and clearance;
- GitHub's REST API was rate-limited on this host, so star counts and API-only fields
  were not used; archival state was checked through the HTML banner and commit/release
  Atom feeds on 2026-11-18, and "unmaintained/archived" claims still rely on README
  statements plus commit/release dates;
- the three-day blueprint budget was compressed into one session, so absence of a
  finding is not proof of absence.

These gaps do not invalidate the contribute/fork/greenfield decision, which turns on
maintenance, licence and architecture deltas that were inspected directly. They do
mean F02 evidence is not equivalent to a validated candidate build or a legal
clearance.

## ANS-6 acceptance assessment

For the **scope decision**, the ticket's acceptance criteria are satisfied:

- the ADR records the greenfield choice with criteria, evidence and unresolved
  unknowns;
- licence obligations and notice requirements are listed for every candidate;
- no reuse is proposed, so the "name exact commits, files and dependencies" condition
  is not triggered, and no source was copied;
- ADR 0003 is explicitly confirmed as the final scope decision.

The test-plan items are addressed to the extent the host and timebox allow: documented
build failures for four Rust candidates, a documented Vicinae configure failure at
Qt6, a documented non-feasibility note for Flare, a reproducible static dependency
inventory with explicit unknowns, and dated maintainer activity. No candidate test
suite or full build executed. The remaining gaps above are acceptance limitations, not
blockers to the scope decision. The handoff record is
[`docs/reuse/closure.md`](../reuse/closure.md).

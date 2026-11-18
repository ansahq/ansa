# Candidate evidence (pinned 2026-11-17)

All links below point at the exact commit inspected. Repository statements are
labelled as claims; only the commands in
[`build-verification.md`](build-verification.md) were executed locally.

Pins were obtained from `git ls-remote <repo> HEAD` on 2026-11-17 and verified after
shallow clone with `git rev-parse HEAD`. Commit timestamps below are committer-local
ISO 8601 values from `git log -1 --format='%H %cI %s'` on the pinned checkout, with
the UTC equivalent shown separately. Release dates come from each repository's
`releases.atom` feed.

## Pinned commit records

| Candidate | Commit | Committer-local (`%cI`) | UTC equivalent |
|---|---|---|---|
| Gauntlet | `ca066da5c9d56bd53a739c0dda1572fccc5019a8` | 2025-10-02T18:55:18+02:00 | 2025-10-02T16:55:18Z |
| Vicinae | `23fec1e1da14c422fba01db5c286589b6e946d06` | 2026-11-15T21:58:24+01:00 | 2026-11-15T20:58:24Z |
| pop-launcher | `4111617b7345134abe67332a7baa5ef12538f412` | 2026-11-06T21:00:11+01:00 | 2026-11-06T20:00:11Z |
| anyrun | `7997bc470cafc1d201ad4d8a8f0432d83fa6c161` | 2026-11-08T21:13:14+02:00 | 2026-11-08T19:13:14Z |
| cosmic-launcher | `29c1bac5e8f9f83b9e2cef03bb0259a04ffd7f2a` | 2026-11-07T00:38:04+01:00 | 2026-11-06T23:38:04Z |
| Flare | `09e126fc77f7716d912c85ce8f308ce708bc2530` | 2025-12-30T11:04:42-08:00 | 2025-12-30T19:04:42Z |

## Activity re-check (2026-11-18)

The pinned dates above are unchanged. On 18 November the public commit and release
Atom feeds were read again, without cloning or logging in, to put the pins in a current
maintenance context. Feeds return at most 20 commit entries, so "≥20 in 90 days" means
the feed was full and is a lower bound.

| Candidate | Default branch | Latest commit in feed (UTC) | Commits in feed within 90 days | Latest release in feed (UTC) | GitHub archive banner |
|---|---|---|---|---|---|
| Gauntlet | `main` | 2025-10-02T16:55:18Z | 0 | v21, 2025-08-16T16:28:39Z | No |
| Vicinae | `main` | 2026-11-18T03:03:20Z | ≥20 | v0.30.1, 2026-11-15T23:57:15Z | No |
| pop-launcher | `master` | 2026-11-06T20:00:11Z | 9 | Epoch 1.11.0, 2026-10-20T22:41:16Z | No |
| anyrun | `master` | 2026-11-08T19:13:14Z | 4 | v26.10.0, 2026-10-20T20:03:53Z | No |
| cosmic-launcher | `master` | 2026-11-06T23:38:04Z | ≥20 | epoch-1.11.0, 2026-10-20T21:52:31Z | No |
| Flare | `main` | 2025-12-30T19:04:42Z | 0 | v0.1.0, 2025-07-29T18:33:12Z | No |

A `git ls-remote HEAD` check on 2026-11-18 found five default-branch HEADs still equal
to their pins. Vicinae had moved from the pinned `23fec1e1…` to `f9554985…`, consistent
with its feed; the evidence in this record remains the pinned commit, not the newer
HEAD. Gauntlet and Flare still show no commits or releases since their final
author-declared maintenance dates, and neither shows a GitHub archive banner; the
"unmaintained" label therefore remains their authors' statement plus the absence of
activity, not a GitHub state.

The unauthenticated REST API had been rate-limited on 17 November. One repo lookup
succeeded on 18 November before the limit was reached again; limited REST results
are not used for an all-six archival claim. The HTML banner and Atom feeds above
cover all six.
Star counts and API-only fields are not used as evidence here.

## Static dependency inventory

[`dependency-scan.md`](dependency-scan.md) records the reproducible manifest/lockfile
scan of 18 November. It counts 3,466 Cargo lock packages, 704 npm lock entries and 836
pnpm package entries across the six pins. All 3,466 Cargo entries and all 836 pnpm
entries remain licence-unknown from their lockfiles; 120 of 704 npm entries have no
licence field. See that file for the per-candidate table and the explicit limits.

## Summary

| Candidate | Stack | Licence | Pin | Last commit | Latest release | Bounded local result |
|---|---|---|---|---|---|---|
| Gauntlet | Rust + Iced forks + React/Deno | MPL-2.0 (repo-level) | `ca066da5…` | 2025-10-02 | v21, 2025-08-16 | `cargo check` exit 101 after 222 s (host `libclang` missing) |
| Vicinae | C++/Qt6 + TypeScript/Node extensions | GPL-3.0 | `23fec1e1…` | 2026-11-15 | v0.30.1, 2026-11-15 | CMake configure exit 1 after 3.6 s (Qt6 6.9 not found; MSVC 19.44 detected) |
| pop-launcher | Rust IPC service + plugins | Mixed MPL-2.0 / GPL-3.0-only | `4111617b…` | 2026-11-06 | epoch-1.11.0, 2026-10-20 | `cargo test` exit 101 after 30 s (`wayland-sys` is Unix-only) |
| anyrun | Rust + GTK4 layer-shell | GPL-3.0 | `7997bc47…` | 2026-11-08 | v26.10.0, 2026-10-20 | `cargo check` exit 101 after 30 s (`gio-2.0` missing) |
| cosmic-launcher | Rust + libcosmic/Iced | GPL-3.0 | `29c1bac5…` | 2026-11-07 (local; UTC 11-06) | epoch-1.11.0, 2026-10-20 | `cargo check` exit 101 after 183 s (`accesskit_winit` git dependency fails) |
| Flare | Tauri 2 + Svelte + Node sidecar + Swift | MIT | `09e126fc…` | 2025-12-30 | v0.1.0, 2025-07-29 | Not attempted (historical only; Swift/SoulverCore, Linux AppImage) |

None of the candidate test suites ran to completion on this host. The "latest
release" dates are upstream publication timestamps, not evidence that a release
artifact was downloaded or validated. The static dependency inventory in
[`dependency-scan.md`](dependency-scan.md) is a file-level count, not a candidate
test or build result.

## Gauntlet

Identity: `project-gauntlet/gauntlet` @ `ca066da5c9d56bd53a739c0dda1572fccc5019a8`
(2025-10-02, "Update README.md"). Previous commit 2025-08-24; latest release v21
2025-08-16. The [README at the pin](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/README.md)
carries a "no longer being developed" warning. The GitHub repository page did not
show an archived banner in the HTML fetched by this host; the unauthenticated REST API
was unavailable on 17 November and rate-limited again on 18 November after a single
lookup, so **this evaluation does not claim GitHub-level archival** — only the upstream
author's statement and the absence of commits since 2025-10-02.

Architecture: a 12-member Rust workspace ([Cargo.toml](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/Cargo.toml)),
1,247 packages in `Cargo.lock`, a custom React reconciler with Deno plugins. The UI
depends on a project-owned Iced fork (the `gauntlet-0.13.1` branch, which itself pulls
a project-owned winit fork), and the plugin runtime pins `deno_core =0.347.0`,
`deno_runtime =0.213.0` and Numbat
([plugin-runtime manifest](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/rust/plugin_runtime/Cargo.toml)).
Windows, macOS Intel and Wayland are listed as best-effort; Linux X11 and macOS M1
are the documented official targets.

Build/test indicators: build CI runs Linux, macOS and Windows jobs
([build.yaml](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/.github/workflows/build.yaml));
the Windows job has a 60-minute timeout, installs Node 22, `protoc`, WiX and runs
`npm ci` before packaging an MSI
([setup-windows.yaml](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/.github/workflows/setup-windows.yaml)).
There is no test job. A textual scan found no `#[test]`, `#[cfg(test)]` or JS test
files in 155 Rust files; the `scenario_runner` crate exists, but its presence is not
evidence of a passing conformance suite. Our own bounded `cargo check` stopped at a
host prerequisite (see build record), so even the compile claim is unverified here.

Licence: [MPL-2.0](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/LICENSE).
Crate manifests inspected (`client`, `server`, `common`, `plugin_runtime`) declare no
`license` field and source files carry no SPDX headers, so a future reuse audit would
have to establish per-file coverage, not just the root licence file.

Delta to the Ansa plan: adopting it means owning Iced/winit forks, a Deno extension
host and a custom reconciler; the Windows target is best-effort upstream; there is no
maintained contribution route. Reviving it as a fork is a maintenance decision, not a
shortcut. Accessibility, IME and Windows behaviour are untested by us.

## Vicinae

Identity: `vicinaehq/vicinae` @ `23fec1e1da14c422fba01db5c286589b6e946d06`
(2026-11-15, "chore: bump to v0.30.1"), released as v0.30.1 the same day. Release
cadence is roughly weekly-to-monthly (v0.29.0 2026-09-18, v0.30.0 2026-11-07), and
the commit feed shows several changes per week including Windows fixes.

Architecture: C++/Qt6 application built with CMake; separate `src/typescript`,
`src/file-indexer`, `src/server`, `src/cli`, `src/mcp`, `src/inference` components.
Extensions are React/TypeScript with a Raycast API compatibility layer
([`src/typescript/raycast-api-compat`](https://github.com/vicinaehq/vicinae/tree/23fec1e1da14c422fba01db5c286589b6e946d06/src/typescript/raycast-api-compat)),
and the build pins a Node runtime, `v22.23.1`, with per-platform SHA-256 values
([CMakeLists.txt line 267](https://github.com/vicinaehq/vicinae/blob/23fec1e1da14c422fba01db5c286589b6e946d06/CMakeLists.txt#L267)).
It advertises in-app integration with both the Vicinae store and the Raycast store.

Build/test indicators: Linux, macOS and Windows CI workflows exist; the Windows job
uses MSVC, Qt, the Vulkan SDK and Azure signing
([build-windows.yaml](https://github.com/vicinaehq/vicinae/blob/23fec1e1da14c422fba01db5c286589b6e946d06/.github/workflows/build-windows.yaml)).
Catch2 tests are configured and run in the Linux and macOS workflows; the inspected
Windows workflow builds and checks for `vicinae-inference.exe` but does not show a
test invocation. A textual scan found 314 `TEST_CASE(` occurrences. This is the only
candidate here with a current Windows build pipeline and an actively developed
Raycast-compatibility layer.

A bounded local configure was attempted on 18 November with CMake's Visual Studio 17
2022 generator, an external build directory and downloads disabled. CMake identified
MSVC 19.44.35229.0, then failed at
`find_package(Qt6 6.9 REQUIRED COMPONENTS Core Qml)` because no Qt6 6.9 package
configuration is installed. Exit 1 after 3.6 s; no compile or test ran. The project's
own Windows preset uses Ninja with `cl` on `PATH`; the probe used the Visual Studio
generator specifically because `cl` was not on `PATH`, and that does not establish the
preset would fail on a correctly prepared Windows machine.

Licence: [GPL-3.0](https://github.com/vicinaehq/vicinae/blob/23fec1e1da14c422fba01db5c286589b6e946d06/LICENSE).
macOS builds may additionally bundle the third-party SoulverCore framework
(`BUNDLE_SOULVER_CORE`, CMakeLists line 76); the repository licence does not grant
redistribution rights to that binary. See the licence audit.

Delta: it is not Rust, not GPUI and not a library we can embed; contributing to it
means working in C++/Qt and accepting GPL-3.0 plus the upstream roadmap. Its
[CONTRIBUTING.md](https://github.com/vicinaehq/vicinae/blob/23fec1e1da14c422fba01db5c286589b6e946d06/CONTRIBUTING.md)
explicitly discourages large architectural PRs. On the other hand, its product scope
(launcher, clipboard, snippets, file search, AI, extensions) overlaps Ansa's later
tracks more than any other maintained candidate.

## pop-launcher

Identity: repository now `pop-os/launcher` (the README still says Pop Launcher;
`pop-os/pop-launcher` no longer resolves) @ `4111617b7345134abe67332a7baa5ef12538f412`
(2026-11-06, "chore: update deps and address lints"). Latest release epoch-1.11.0
2026-10-20; epoch releases appear roughly every two weeks.

Architecture: modular IPC launcher service in Rust: a `service` process spawns
plugins on demand and speaks line-delimited JSON over stdin/stdout; plugin metadata
is RON; plugins and frontends can be written in any language. It deliberately has no
UI. It is Linux-shaped: `zbus`, `freedesktop-desktop-entry`, cosmic protocols and a
`wayland-sys` transitive dependency
([workspace](https://github.com/pop-os/launcher/blob/4111617b7345134abe67332a7baa5ef12538f412/Cargo.toml),
[plugins](https://github.com/pop-os/launcher/blob/4111617b7345134abe67332a7baa5ef12538f412/plugins/Cargo.toml)).

Build/test indicators: 32 Rust files, three `#[cfg(test)]` modules and eight test
attributes (calculator, web plugin, one service test). The only workflow is a
checklist gate; there is no build/test CI workflow
([workflows](https://github.com/pop-os/launcher/tree/4111617b7345134abe67332a7baa5ef12538f412/.github/workflows)),
and the `justfile` has a clippy target but no test target. Our bounded test run failed
on a Unix-only dependency after 30 s.

Licence: mixed. Root library and `service` are MPL-2.0; `bin` and `plugins` are
`GPL-3.0-only`
([root](https://github.com/pop-os/launcher/blob/4111617b7345134abe67332a7baa5ef12538f412/Cargo.toml),
[plugins](https://github.com/pop-os/launcher/blob/4111617b7345134abe67332a7baa5ef12538f412/plugins/Cargo.toml),
[bin](https://github.com/pop-os/launcher/blob/4111617b7345134abe67332a7baa5ef12538f412/bin/Cargo.toml)).
"MPL-2.0 repo" is therefore an incomplete description.

Delta: not a launcher application and not cross-platform; useful as a process-IPC
design reference for extension isolation. A fork would still leave all UI, ranking,
accessibility, Windows/macOS integration and the Node/React runtime to build.

## anyrun

Identity: `anyrun-org/anyrun` (the former `Kirottu/anyrun` path redirects) @
`7997bc470cafc1d201ad4d8a8f0432d83fa6c161` (2026-11-08, "nix, lock: Fix lockfile and
fix Nix builds"). Latest release v26.10.0 2026-10-20; releases are irregular.

Architecture: GTK4 layer-shell, Wayland-native runner with `abi_stable` cdylib
plugins and RON configuration
([crate manifest](https://github.com/anyrun-org/anyrun/blob/7997bc470cafc1d201ad4d8a8f0432d83fa6c161/anyrun/Cargo.toml)).
It depends on git branches, including a Relm4 fork (`branch = "service"`) and
separate `anyrun-interface`/`anyrun-provider` repositories, so lockfile revisions are
load-bearing. 370 lockfile packages.

Build/test indicators: 25 Rust files and no tests; CI builds only on Ubuntu through a
Nix development shell
([cargo-build.yml](https://github.com/anyrun-org/anyrun/blob/7997bc470cafc1d201ad4d8a8f0432d83fa6c161/.github/workflows/cargo-build.yml)).
Our bounded check stopped at missing `gio-2.0` after 30 s, consistent with the
documented GTK4 system dependency.

Licence: [GPL-3.0](https://github.com/anyrun-org/anyrun/blob/7997bc470cafc1d201ad4d8a8f0432d83fa6c161/LICENSE);
the `anyrun` crate itself declares no `license` field. Plugin `.so` files are built
from the same GPL-covered tree.

Delta: Wayland-only and GTK-based; no Windows or macOS path; no evidence of
accessibility or Raycast compatibility. Not a foundation candidate.

## cosmic-launcher

Identity: `pop-os/cosmic-launcher` @
`29c1bac5e8f9f83b9e2cef03bb0259a04ffd7f2a` (2026-11-07T00:38:04+01:00, or
2026-11-06T23:38:04Z, "chore: update deps"). Latest release epoch-1.11.0
2026-10-20, aligned with COSMIC epochs.

Architecture: a thin libcosmic/Iced layer-shell frontend over the pop-launcher
service; eight source files
([src](https://github.com/pop-os/cosmic-launcher/tree/29c1bac5e8f9f83b9e2cef03bb0259a04ffd7f2a/src)).
`libcosmic`, `cosmic-launch` and `pop-launcher` reference git dependencies without a
`rev` in the manifests, so `Cargo.lock` is what pins the actual revisions
([Cargo.toml](https://github.com/pop-os/cosmic-launcher/blob/29c1bac5e8f9f83b9e2cef03bb0259a04ffd7f2a/Cargo.toml)),
and `nix` is an unconditional dependency, so the workspace is Unix-only. 732 lockfile
packages.

Build/test indicators: no Rust tests in tree; CI runs `cargo fmt`, `cargo clippy`
and `cargo test --all-features` on Ubuntu
([ci.yml](https://github.com/pop-os/cosmic-launcher/blob/29c1bac5e8f9f83b9e2cef03bb0259a04ffd7f2a/.github/workflows/ci.yml)).
Our bounded check compiled for ~183 s, then failed in a git dependency checkout
(`accesskit_winit`, rev `6c20249`) with `E0782` (`&Window` used as a bare trait), under
the Rust 1.95 toolchain selected by `rust-toolchain.toml`, on a host this frontend does
not target. The lockfile pins that revision, so this is not observed drift. If the lock
is regenerated, manifests that reference git dependencies without a `rev` (libcosmic
and others) could resolve different revisions; that is a future update risk, not the
cause recorded here, and it does not establish Linux CI status.

Licence: [GPL-3.0](https://github.com/pop-os/cosmic-launcher/blob/29c1bac5e8f9f83b9e2cef03bb0259a04ffd7f2a/LICENSE.md);
no `license` field in `Cargo.toml`. Redistribution must satisfy GPL-3.0 for the
combined frontend, and the libcosmic/Iced forks carry their own notices.

Delta: a COSMIC desktop component, not a cross-platform application. Its split between
frontend and pop-launcher service is architecturally interesting but already reflected
in the blueprint's process boundaries.

## Flare

Identity: `ByteAtATime/flare` @
`09e126fc77f7716d912c85ce8f308ce708bc2530` (2025-12-30, "docs: add deprecation
notice to readme"). Only release v0.1.0 (2025-07-29). The README states the project is
no longer actively maintained and points users to Vicinae; the GitHub page did not
show an archived banner in fetched HTML, so "archived" should be read as
"author-declared unmaintained", not a GitHub state.

Architecture: Tauri 2 host, Svelte frontend, Node sidecar packaged as a single binary,
Swift wrapper around the third-party SoulverCore calculator, AppImage distribution,
Raycast-compatible extension/store claims
([README](https://github.com/ByteAtATime/flare/blob/09e126fc77f7716d912c85ce8f308ce708bc2530/README.md)).
757 packages in `src-tauri/Cargo.lock`.

Build/test indicators: 41 test attributes, 11 `#[cfg(test)]` modules and two Vitest
files; a nightly workflow exists. We did not attempt a build: it needs Node/pnpm, the
Tauri Linux prerequisites, a Swift toolchain and SoulverCore, and the project is
historical evidence per blueprint section 18. A textual test count is not a passing
suite.

Licence: [MIT](https://github.com/ByteAtATime/flare/blob/09e126fc77f7716d912c85ce8f308ce708bc2530/LICENSE)
for the repository, but the Swift/SoulverCore path and any Raycast store integration
need separate terms. The sidecar package declares ISC.

Delta: Tauri/web UI contradicts the Rust-UI premise; Linux/AppImage only; the
maintained successor recommended by its own author is Vicinae. Historical only.

## Candidates deliberately excluded

- Raycast itself: proprietary, not a reuse candidate; its store/API terms are
  separate from the public extension repository.
- Albert (C++/Qt), Ulauncher (Python), Wox (C#/Flutter), Flow Launcher (C#), Walker
  (Go): outside the Rust/maintained-launcher screen or not launcher foundations for
  this decision. They were not inspected at a pinned commit and are not counted as
  evidence.
- Generic Rust UI frameworks (GPUI, Slint, Iced, egui): already covered by the
  framework decision, not launcher reuse.

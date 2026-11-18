# Static dependency and licence inventory (2026-11-18)

This is the bounded dependency/licence scan required by F02. It reads manifests and
lockfiles at the six pinned commits and reports what those files declare. It is a
**static inventory**, not a licence clearance, SBOM or per-file audit.

Reproduce against a destination that holds canonical checkouts for all six candidates:

```powershell
pwsh -NoProfile -File scripts/reuse/scan-dependencies.ps1 `
  -Destination "$env:TEMP\opencode\ansa-reuse-candidates" `
  -Candidate gauntlet,vicinae,pop-launcher,anyrun,cosmic-launcher,flare
```

The scanner verifies each selected checkout against `scripts/reuse/candidates.json`
(commit, `remote.origin.url`, cleanliness) before reading it, refuses destinations or
output paths inside this repository, makes no network request, consults no registry or
store metadata, installs nothing and executes no candidate script.

The cached checkouts under `%TEMP%\opencode\f02` were used for five candidates. The
cached anyrun checkout still has its historical `https://github.com/Kirottu/anyrun`
origin. It was not modified or relabelled; anyrun was scanned from a fresh canonical
checkout at the same pin in `%TEMP%\opencode\f02-scan-anyrun`, created with
`fetch-candidates.ps1`:

```powershell
$pin = @([ordered]@{
  name = 'anyrun'
  repository = 'https://github.com/anyrun-org/anyrun'
  commit = '7997bc470cafc1d201ad4d8a8f0432d83fa6c161'
})
$pin | ConvertTo-Json -Depth 3 | Set-Content "$env:TEMP\opencode\anyrun-pin.json"

pwsh -NoProfile -File scripts/reuse/fetch-candidates.ps1 `
  -Destination "$env:TEMP\opencode\f02-scan-anyrun" `
  -PinFile "$env:TEMP\opencode\anyrun-pin.json"

pwsh -NoProfile -File scripts/reuse/scan-dependencies.ps1 `
  -Destination "$env:TEMP\opencode\f02-scan-anyrun" `
  -Candidate anyrun `
  -OutputPath "$env:TEMP\opencode\f02\dependency-scan-anyrun.json"
```

## Results at the pins

| Candidate | Root licence text | Cargo lock packages (registry / git / local) | Cargo manifests (declared / no declaration) | npm lock entries (with licence field / without) | pnpm entries | CMake FetchContent | SPDX headers |
|---|---|---|---|---|---|---|---|
| Gauntlet | MPL-2.0 | 1,247 (1,217 / 17 / 13) | 13 (0 / 13) | 322 (207 / 115) | — | — | 0 of 296 |
| Vicinae | GPL-3.0 | — | — | 382 (377 / 5) | — | 7 | 6 of 1,710 |
| pop-launcher | MPL-2.0 | 360 (352 / 3 / 5) | 5 (4 / 1) | — | — | — | 27 of 32 |
| anyrun | GPL-3.0 | 370 (349 / 5 / 16) | 17 (0 / 16) | — | — | — | 0 of 25 |
| cosmic-launcher | GPL-3.0 | 732 (675 / 56 / 1) | 1 (0 / 1) | — | — | — | 1 of 9 |
| Flare | MIT | 757 (756 / 0 / 1) | 1 (0 / 1) | — | 836 | — | 0 of 276 |
| **Total** | — | **3,466** (3,349 / 81 / 36) | **37 (4 / 32)** | **704 (584 / 120)** | **836** | **7** | — |

"Root licence text" is a keyword signature of the file named at the repository root.
It is not legal identification. Gauntlet and pop-launcher both carry MPL-2.0 at the
root, but pop-launcher's `bin` and `plugins` manifests separately declare
`GPL-3.0-only`; the mixed-licence finding is unchanged.

Counts in the table are not additive across ecosystems. The 3,466 Cargo entries are the
same total already recorded in the earlier audit; npm and pnpm entries are separate
Node dependency graphs, and some workspaces have more than one lockfile.

Other static facts:

- **Vicinae** has seven `FetchContent_Declare` targets: glaze v7.2.0, KDE
  extra-cmake-modules and syntax-highlighting (`v${KF6_VERSION}`, KF6 6.20.0),
  jkqtmathtext (commit-pinned archive with SHA-256), mediaremoteadapter
  (commit-pinned archive with SHA-256), qt-keychain v0.14.0 and `layer-shell-qt`.
  The last declaration has **no `GIT_TAG`**, so it follows the repository's default
  branch unless the build overrides it. Four `find_package` calls are marked
  `REQUIRED` in the scanned files: Qt6 unconditionally; whisper and parakeet on the
  local-AI/system-engine path; and PkgConfig on the Wayland path. Eleven vendor
  directories are present, and the CMake configuration pins Node runtime `v22.23.1`
  with five per-platform SHA-256 values.
- **Gauntlet** has 41 `package.json` files and none declares a `license` field. Its
  `package-lock.json` has 322 entries; 207 carry a licence field in the lock, 115 do
  not. All 13 Cargo manifests lack a `license` field.
- **pop-launcher** is the only Rust candidate whose workspace declares licences in
  manifests: `MPL-2.0` for the root and `service`, `GPL-3.0-only` for `bin` and
  `plugins`. `toolkit/Cargo.toml` has a `[package]` section but no `license` field.
- **Flare** has 757 Cargo packages in `src-tauri/Cargo.lock`, three `package.json`
  files (root MIT, sidecar ISC, private protocol package with no licence), a root
  `pnpm-lock.yaml` with 683 package entries and a `sidecar/pnpm-lock.yaml` with 153.
  pnpm counts are line heuristics over the YAML section keys, not a resolved graph.
- **cosmic-launcher's** 732-package lock has 56 git sources and one root workspace
  entry; its single manifest declares no licence. **Anyrun's** 370-package lock has
  17 manifests, none with a `license` field.

## Unknowns this scan does not resolve

- **All 3,466 Cargo lock entries remain licence-unknown here.** `Cargo.lock` records
  name, version, source and checksum, not a licence. The counts are an inventory
  boundary, not a licence conclusion for any package.
- **120 npm entries have no licence field in the lock.** The other 584 entries carry a
  field that was read as text; it was not checked against the npm registry, the
  package tarball or any SPDX database.
- **All 836 pnpm entries are unresolved.** pnpm lockfiles carry no per-package licence
  field, and the YAML was not parsed with a YAML library.
- **32 of the 36 Cargo manifests with a `[package]` section declare no licence.** One
  additional manifest is a virtual workspace manifest with no `[package]` section.
  A declaration, where present, does not prove that every file in the crate carries
  that licence.
- **7 of 51 `package.json` files declare a licence.** Several are private or
  workspace-only packages; absence is not by itself a redistribution finding.
- **Licences of CMake FetchContent targets, the 11 Vicinae vendor directories, Qt,
  GTK4, libcosmic/Iced, Relm4, Deno/V8, the pinned Node runtime and the SoulverCore
  binary were not resolved.** Their notices still need separate review before any
  distribution that includes them.
- **SPDX header counts are presence checks only.** They do not establish per-file
  coverage, and the absence of a header does not make a file unlicensed.

No registry metadata, store terms, dependency vulnerability feed or legal review was
consulted. This inventory does not authorise reuse of any candidate code or asset.

## What changed in the audit record

The earlier audit correctly said no dependency-level licence inventory had been run.
This scan closes the **inventory-counting** part of that gap only: the files and their
counts are now reproducible, and every unresolved class is named. It does **not** close
the per-package licence, transitive-notice, C++/Qt or legal-review gaps. The zero-reuse
decision means no per-file audit is required for this evaluation; a future proposal to
copy or link candidate source would still require one before use.

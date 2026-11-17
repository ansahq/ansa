# Licence and notice audit (repository-level)

Date: 2026-11-17. This is an engineering audit of repository licence files and
manifests, not legal advice, and no legal clearance has been obtained. It records
obligations that would apply **if** source were reused. No candidate source was copied
into Ansa, so none of these obligations is currently triggered by this evaluation.

Ansa's own [LICENSE](../../LICENSE) is Apache-2.0 today. Blueprint section 18 proposes
MPL-2.0 for new application/server code; that proposal has not been implemented. The
difference matters below and is left as an explicit open decision, not silently
changed by this audit.

## Findings

| Candidate | Declared licence at pin | Mixed/undeclared? | Notable bundled components |
|---|---|---|---|
| Gauntlet | MPL-2.0 (root `LICENSE`) | Crates inspected declare no `license`; no SPDX headers | Deno/V8 runtime dependencies, Iced/winit forks, Numbat |
| Vicinae | GPL-3.0 | No per-crate manifest system (CMake) | Pinned Node 22 runtime, optional vendor-supplied SoulverCore (macOS), whisper.cpp/ggml, Qt |
| pop-launcher | MPL-2.0 root/service; **GPL-3.0-only** `bin` and `plugins` | Yes, mixed within one repository | zbus/freedesktop, cosmic protocol libs |
| anyrun | GPL-3.0 | `anyrun` crate declares no `license` | GTK4/gtk4-layer-shell, Relm4 fork, abi_stable plugins |
| cosmic-launcher | GPL-3.0 (`LICENSE.md`) | `Cargo.toml` declares no `license` | libcosmic/Pop Iced fork, pop-launcher, `nix` |
| Flare | MIT | Sidecar package declares ISC | Tauri 2, Node sidecar, third-party SoulverCore wrapper, Raycast store integration |

Licence files inspected at the pins:

- Gauntlet: [`LICENSE`](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/LICENSE)
- Vicinae: [`LICENSE`](https://github.com/vicinaehq/vicinae/blob/23fec1e1da14c422fba01db5c286589b6e946d06/LICENSE)
- pop-launcher: [`Cargo.toml`](https://github.com/pop-os/launcher/blob/4111617b7345134abe67332a7baa5ef12538f412/Cargo.toml),
  [`plugins/Cargo.toml`](https://github.com/pop-os/launcher/blob/4111617b7345134abe67332a7baa5ef12538f412/plugins/Cargo.toml),
  [`bin/Cargo.toml`](https://github.com/pop-os/launcher/blob/4111617b7345134abe67332a7baa5ef12538f412/bin/Cargo.toml)
- anyrun: [`LICENSE`](https://github.com/anyrun-org/anyrun/blob/7997bc470cafc1d201ad4d8a8f0432d83fa6c161/LICENSE)
- cosmic-launcher: [`LICENSE.md`](https://github.com/pop-os/cosmic-launcher/blob/29c1bac5e8f9f83b9e2cef03bb0259a04ffd7f2a/LICENSE.md)
- Flare: [`LICENSE`](https://github.com/ByteAtATime/flare/blob/09e126fc77f7716d912c85ce8f308ce708bc2530/LICENSE)

## Obligations by licence family

### MPL-2.0 (Gauntlet; pop-launcher library/service)

- Copyleft is **per file**, not per application: modified MPL-covered files must be
  made available in source form under MPL-2.0, and recipients must be told how to
  obtain them.
- Larger works may carry other terms, provided the MPL files and their notices remain
  intact and no additional restrictions are placed on them.
- Copyright and licence notices must be retained; patent rights are granted, trademark
  rights are not.
- Because Gauntlet's crates do not declare licences and files have no SPDX headers, a
  future reuse would first need a per-file provenance audit and explicit NOTICE/THIRD-
  PARTY updates. A root `LICENSE` file alone is not a sufficient inventory.
- Copying MPL files into the current Apache-2.0 Ansa tree is *possible* with file-level
  compliance, but it creates a mixed-licence distribution that must be documented.
  MPL-2.0 section 3.3 allows a Larger Work to be distributed under other terms while
  the MPL conditions remain satisfied; secondary-license elections (MPL Exhibit B) and
  provenance/notice recording still need legal review rather than an engineering
  assumption.

### GPL-3.0 (Vicinae; anyrun; cosmic-launcher; pop-launcher `bin`/`plugins`)

- If GPL-3.0 code is combined into a covered work and that work is distributed,
  distribution of the combined work must satisfy GPL-3.0: complete corresponding
  source, licence texts and notices, no additional restrictions, and the applicable
  installation-information and patent terms.
- This does **not** retroactively relicense every original file. Apache-2.0 files keep
  their notices and permissions; the combined distribution as a whole is what must
  meet GPL-3.0. The practical conflict for Ansa is with its desired Apache-2.0-only
  distribution direction, not a conclusion that Apache-2.0 or MPL-2.0 are inherently
  legally incompatible with GPL-3.0. Whether a specific integration forms a covered or
  combined work, and how MPL secondary-license elections and provenance apply, needs
  legal review.
- Anyrun's `abi_stable` plugin ABI and pop-launcher's process IPC make the
  derivative-work boundary a **legal question, not an established technical fact**.
  A process boundary is generally safer than in-process linking, but this audit does
  not conclude that GPL obligations disappear.
- Vicinae's Catch2 tests and C++ sources are covered by GPL-3.0; copying tests is
  copying source.

### MIT (Flare)

- Copies or substantial portions must retain the copyright and permission notice.
- The MIT grant covers the repository as published; it does not automatically cover
  separately downloaded proprietary binaries or store services.

### Non-code and third-party components

- **SoulverCore** (Vicinae macOS option, Flare calculator) is a third-party framework
  not covered by the repository licence. Vicinae's CMake option downloads and bundles
  it; the README thanks the vendor for permission to ship it, which does not extend to
  Ansa. Do not copy or bundle it.
- **Node runtime** (Vicinae pins v22.23.1 with per-platform SHA-256) and **Deno/V8**
  (Gauntlet) ship their own licence and third-party notice sets. Redistributing a
  bundled runtime requires those notices; they were not inventoried here.
- **Qt** (Vicinae) is offered under LGPLv3/GPL/commercial terms depending on module and
  linking. **GTK4** (anyrun) is LGPL-2.1-or-later; static linking and relinking
  obligations need review before distribution. **libcosmic/Iced forks** and
  **Relm4 fork** carry their own notices. None was dependency-scanned.
- **Store/service terms** (Raycast store, Vicinae store) are separate from source
  licences. Open extension repositories do not authorise use of proprietary services,
  accounts or private APIs; blueprint section 18's caution stands.

## What zero reuse means here

This evaluation copies **no** candidate source, tests, assets, schemas or
documentation text into Ansa. The ADR and evidence files contain short factual
references (names, versions, commit hashes, file paths) and paraphrase upstream
claims. Those are not acts that trigger MPL/GPL/MIT notice obligations.

This is not a claim that the whole Ansa tree contains no upstream-derived code.
`crates/ui-components/src/text_input.rs` contains code adapted from the pinned GPUI
0.2.2 example, with its attribution and licence notice preserved in
[`crates/ui-components/THIRD_PARTY.md`](../../crates/ui-components/THIRD_PARTY.md).
This evaluation does not modify or remove that notice.

Consequently:

- No NOTICE or THIRD-PARTY file is added in this slice.
- Ansa remains Apache-2.0 as committed; the MPL-2.0 proposal in the blueprint is still
  an open licensing decision for the owner.
- Architectural patterns (process-isolated extension host, manifest/permission model,
  line-delimited IPC) are ideas, not copied expression. Any implementation must be
  written from Ansa's requirements, not by transcribing candidate files or schemas.

## Audit gaps (explicit)

- 3,466 lockfile packages were counted across the candidates (Gauntlet 1,247,
  pop-launcher 360, anyrun 370, cosmic-launcher 732, Flare 757), but **no dependency
  licence inventory or SPDX/SBOM scan was run**.
- Vicinae's CMake `FetchContent` dependencies and bundled binaries were not scanned;
  no C++ dependency graph was produced.
- No per-file audit of Gauntlet's unmarked MPL sources and no check of whether every
  crate is actually MPL-2.0.
- No trademark review of names/logos; no store-terms review; no export-control review.
- No legal counsel review of derivative-work boundaries (IPC, plugin ABI, static
  linking).
- Licence texts were read only at the pinned commits; no later relicensing was
  checked.

A future decision to copy or link any candidate code must close these gaps first and
record the result in a new ADR.

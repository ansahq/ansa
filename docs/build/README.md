# Build and release tooling

This directory records the F03 build contract: the exact Rust toolchain, the
blocking CI matrix, the lockfile dependency/license inventory, and the local
validation evidence. It is tooling documentation, not a release approval.

| File | Purpose |
|---|---|
| [`toolchain.md`](toolchain.md) | Exact compiler pin, update procedure and local toolchain caveat |
| [`ci.md`](ci.md) | Blocking macOS/Windows/Linux jobs, caching, offline resolution and secret policy |
| [`native-smoke.md`](native-smoke.md) | Bounded native startup gate, success marker and hosted logs |
| [`linux-compatibility.md`](linux-compatibility.md) | Temporary libc/xattr compatibility constraint, failure evidence and upgrade procedure |
| [`dependency-inventory.md`](dependency-inventory.md) | Generated human-readable Cargo.lock inventory |
| [`dependency-inventory.spdx.json`](dependency-inventory.spdx.json) | Generated SPDX 2.3-style machine inventory |
| [`validation-2026-11-17.md`](validation-2026-11-17.md) | Exact commands/results from the local F03 implementation run |

## Dependency inventory

The inventory is generated from `Cargo.lock` plus:

- `cargo metadata --locked --offline` (all packages), and
- `cargo metadata --locked --offline --filter-platform ...` for the host triples
  declared by CI.

Every locked package is emitted with its version, Cargo source, registry
checksum (when present), declared license expression, platform observation and
review status. License metadata is copied verbatim. A malformed or legacy form
such as `MIT/Apache-2.0` is **not** silently normalised; the SPDX document uses
`NOASSERTION` and keeps the original string in `licenseComments`.

The SPDX expression allowlist is deliberately conservative. An expression that
uses a valid SPDX identifier outside the allowlist also falls back to
`NOASSERTION`; that is an inventory limitation, **not** a claim that the license
is invalid. The allowlist currently includes `NCSA`, `MIT-0` and
`GPL-2.0-only` alongside the earlier identifiers. Being on the allowlist means
the string can be represented as SPDX; it is not a license-compatibility review.

Lock identity uses SHA-256 over a canonical LF view: only CRLF pairs are
normalized to LF. Lone carriage returns, UTF-8 bytes and every other byte are
preserved. This makes a Windows CRLF checkout and a Linux LF checkout generate
the same inventory. The generator never rewrites `Cargo.lock`, and Cargo's
registry artifact checksums remain raw checksums from the lock entries. A
physical worktree file hash may therefore differ from the labeled
LF-normalized identity hash.

### Scope and limits

- This is a **Cargo.lock inventory**. It includes every locked package,
  including target-specific packages, but only packages observed for the four
  target triples used by the platform filters are listed with a platform. The
  remaining entries are marked `none (locked only)` rather than presented as
  cross-platform build evidence. The generated file records the exact counts
  for the current lockfile.
- It does **not** inventory system/native libraries (X11, Wayland, Vulkan,
  fontconfig, Windows/macOS frameworks, the C/C++ linker), Rust toolchain
  components, GitHub Actions, or the excluded `experiments/` workspace.
- It is **not a legal review**. `licenseConcluded` is `NOASSERTION` for every
  package. First-party workspace members with a declared license are marked
  reviewed for that manifest declaration only; the generated detail records the
  current license instead of assuming Apache-2.0. All third-party packages are
  unreviewed unless an evidence-backed entry is added to
  [`scripts/dependencies/review-status.toml`](../../scripts/dependencies/review-status.toml).
- **Identity limitation:** the inventory keys packages by `name`/`version`.
  Cargo.lock files that contain the same name/version from multiple sources are
  not supported in this slice; the loader fails closed with a source-
  disambiguated-identity diagnostic instead of merging or misattributing them.
  The current lock has 717 unique name/version pairs: 708 registry packages and
  9 workspace/path packages. Expanding the identity model is a later change.
- CI will not run without a current generated inventory; the job is blocking.

### Regenerate and check

Python 3.11+ is required for `tomllib`. No third-party Python packages are used.

```sh
# One-time fetch so --offline metadata works for all target-specific packages.
cargo fetch --locked
cargo fetch --locked --target x86_64-pc-windows-msvc
cargo fetch --locked --target x86_64-apple-darwin
cargo fetch --locked --target aarch64-apple-darwin
cargo fetch --locked --target x86_64-unknown-linux-gnu

python scripts/dependencies/inventory.py
python scripts/dependencies/inventory.py --check

# Tooling tests (uses temp directories; deliberately does not build the workspace).
python -m unittest discover -s scripts/dependencies/tests -v
```

On systems where `python` is not on `PATH`, pass an interpreter explicitly, for
example `py -3.12 scripts/dependencies/inventory.py --check`.

`--check` regenerates in memory and exits non-zero on any difference. It never
edits generated files. A changed `Cargo.lock`, license declaration or review
entry requires regenerating both inventory files and reviewing the diff.

### Deliberately unlocked dependency test

`scripts/dependencies/tests/test_inventory.py` creates a temporary Cargo project,
generates a lockfile, then adds a dependency without updating the lockfile. The
inventory's `cargo metadata --locked --offline` call must fail; the test asserts
that failure. This checks that a stale lock cannot be mistaken for a complete
inventory.

## Review workflow

1. Add an entry under `[reviewed]` in
   `scripts/dependencies/review-status.toml`, keyed as `"name@version"`, with a
   non-empty evidence/reason string.
2. Regenerate the inventory and review the changed review status in both files.
3. Remove the entry if the package version changes; review status is versioned.
4. Do not describe the inventory as a license approval. It records what was
   reviewed, by version, and what remains unreviewed.

## CI status

The workflow in `.github/workflows/ci.yml` is configured to block on formatting,
Clippy, unit/integration tests, doctests, the generated inventory, and a native
desktop build on Windows, macOS and Linux. Following the documented libc
compatibility fix, post-merge main [run 44085915233](https://github.com/ansahq/ansa/actions/runs/44085915233)
passed all eight jobs at `7b22edc`. That run did not include native startup smokes.
The follow-up [native startup gate](native-smoke.md) now runs each built artifact
with a timeout and requires an in-application success marker plus zero exit;
hosted [run 44250538706](https://github.com/ansahq/ansa/actions/runs/44250538706)
passed all eight jobs including native startup on every host. Linux evidence is
activated X11 startup under Xvfb/software Vulkan, not inactive X11 or Wayland.
See [`ci.md`](ci.md), the [follow-up record](../ci-follow-up-2026-11-17.md), and
the PR's commit-specific checks for newer results. Hosted compile/test success
alone does not establish native startup, accessibility or reference-product parity.

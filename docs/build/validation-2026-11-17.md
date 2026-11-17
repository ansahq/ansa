# F03 build/tooling validation - 2026-11-17

Environment: Windows 11 Home Single Language build 26200, x86-64; Intel Core
i5-12450H; Rust `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, Cargo `1.99.1
(5f94df478 2026-08-27)`, rustup 1.29.1. The active `stable` toolchain reports
1.99.1. Python 3.12.10 was used from the local embedded interpreter because no
`python` executable is on `PATH`; the inventory uses only the standard library.
Node 24.22.1 was present but was not needed. The working tree was based on
commit `a5b3be0` with uncommitted changes; no commit was made. In the command
table, `python` denotes the embedded interpreter at
`C:\Users\abhir\AppData\Local\Temp\opencode\python-embed\python.exe`, and
`cargo +stable` was used because the named `+1.99.1` installation is broken.

## Exact toolchain pin is blocked locally

```text
$ cargo +1.99.1 --version
error: missing manifest in toolchain '1.99.1-x86_64-pc-windows-msvc'
help: this may happen if the toolchain installation was interrupted
help: try reinstalling or updating the toolchain

$ rustc +1.99.1 --version
error: missing manifest in toolchain '1.99.1-x86_64-pc-windows-msvc'

$ cargo +1.99.1 test --workspace --locked --offline
error: missing manifest in toolchain '1.99.1-x86_64-pc-windows-msvc'
```

The named installation was not repaired or mutated. Local commands therefore use
the explicit fallback `RUSTUP_TOOLCHAIN=stable` / `cargo +stable`, which is
actually rustc/cargo 1.99.1. This is not evidence that the immutable `+1.99.1`
pin works; a fresh CI installation or a repaired local toolchain is still
required.

## Commands and outcomes

| Command | Outcome |
|---|---|
| `RUSTUP_TOOLCHAIN=stable cargo metadata --locked --offline --format-version 1` | 717 packages, 9 workspace members, 717 resolve nodes |
| Same, `--filter-platform x86_64-pc-windows-msvc` | 485 packages |
| Same, `--filter-platform x86_64-apple-darwin` | 498 packages |
| Same, `--filter-platform aarch64-apple-darwin` | 494 packages |
| Same, `--filter-platform x86_64-unknown-linux-gnu` | 557 packages |
| Merged macOS filters | 498 packages |
| Union of platform filters | 652 packages; 65 lock entries not observed |
| `python -m unittest discover -s scripts/dependencies/tests -v` (historical run before Sol integration) | **21 tests passed**, including stale-lock, SPDX parser misuse, namespace and SPDX self-consistency checks |
| Same command, final combined run with `RUSTUP_TOOLCHAIN=stable` | **25 tests passed**, including Sol's repository contract test, the allowlist regression and two canonical-lock tests |
| `python scripts/dependencies/inventory.py --check --toolchain stable` (final bounded review) | exit 0; 717 packages checked; 65 not observed; LF-normalized lock identity `08af1e2b90c05370590482b9f0e5237b2227f3eb7f12f3ced049c09ab70db565` |
| Repeat inventory regeneration | deterministic output; `--check` passes |
| `cargo +stable fmt --all -- --check` | exit 0 |
| `cargo +stable test -p ansa-domain -p ansa-search --lib --locked --offline` | 6 + 14 = **20 tests passed** |
| PowerShell parser over `scripts/validate.ps1` | no syntax errors |
| `.github/workflows/ci.yml` text contract tests | passed (toolchain, matrix, locked/offline, inventory, no secrets, reference/reuse integration) |

Test-count reconciliation: the 21-test row is the historical local run from this
tooling slice. Sol's integration added one repository contract test, making the
integrated suite 22; this agent did not author that test. The allowlist/identity
review added one test, making 23. The final canonical-lock review added two tests
(CRLF/LF equivalence and version-sensitive identity), so the combined dependency
and repository suites now contain **25 tests**. The final combined run executed
all 25 and passed; the Sol-authored contract test is included as integrated
coverage, not claimed as authored by this agent.

The negative test creates a temporary path-dependency project, generates a
lockfile, then adds a dependency without updating the lock. `cargo metadata
--locked --offline` fails, the inventory exits non-zero, and no output directory
is written. This is a deliberate unlocked-dependency failure, not a skipped test.

## Lock identity vs physical checkout bytes

The generated inventory identity is SHA-256 over a canonical LF view: only CRLF
pairs are converted to LF; lone CR, UTF-8 and all other bytes are preserved. The
generator does not rewrite `Cargo.lock`. Observed during this review:

| Identity | Bytes | CRLF pairs | SHA-256 |
|---|---:|---:|---|
| Physical Windows worktree `Cargo.lock` | 186,508 | 7,269 | `036226b0f97f06a855d2ad07787f62ea0e002e3c279150033924594709d4a160` |
| LF-normalized identity used by inventory | 179,239 | 0 | `08af1e2b90c05370590482b9f0e5237b2227f3eb7f12f3ced049c09ab70db565` |
| Git HEAD blob (LF, as reported) | 179,239 | 0 | `08af1e2b90c05370590482b9f0e5237b2227f3eb7f12f3ced049c09ab70db565` |

The physical worktree hash is a correct hash of those physical bytes, not the
inventory identity. Without normalization, Linux CI `--check` would drift purely
because of checkout line endings. Full physical-vs-Git proof is owned by Sol's
consolidated record. Cargo registry artifact checksums in `Cargo.lock` are raw
and are not line-ending normalized; all 708 registry checksums in the generated
SPDX artifact match the lock entries.

Review correction: the SPDX expression check is now a small fail-closed parser
with balanced parentheses, explicit AND/OR operands, and `WITH` limited to a
known license plus a known exception. Standalone exceptions, `MIT WITH MIT`,
trailing/mismatched parentheses and trailing operators are negative tests, while
valid nested forms are accepted. First-party review detail now derives the
declared license and states that the manifest declaration is not a legal review.
The SPDX namespace includes a canonical content digest in addition to the
LF-normalized Cargo.lock hash, so metadata/review changes produce a different
document namespace even when the lock is unchanged. The final bounded review also
added `NCSA`, `MIT-0` and `GPL-2.0-only`, changed the multisource-identity
failure to an explicit fail-closed diagnostic, and canonicalized only CRLF->LF
for lock identity so Windows and Linux checkouts generate identical output. Both
artifacts were regenerated. `--check` passed again. No full Rust build was run
for these documentation/tooling corrections.

## Generated artifact scope

`docs/build/dependency-inventory.md` and
`docs/build/dependency-inventory.spdx.json` contain all 717 lock entries with
version, Cargo source, checksum (708 registry packages), declared license,
platform observation and review status. The other 9 entries are first-party
workspace packages with no registry checksum.

**No third-party package is marked reviewed.** First-party packages are marked
reviewed for their current declared manifest license (all currently declare
Apache-2.0), and the generated detail says explicitly that this is not a legal
review. `licenseConcluded` is `NOASSERTION` throughout; the SPDX artifact is an
inventory, not legal advice or an approval. **46** declared strings use
`NOASSERTION` because they are outside the conservative allowlist or are legacy
slash-separated forms. That is an inventory limitation, not a statement that
those licenses are intrinsically invalid. The verbatim strings remain in
`licenseComments` and the Markdown table.

The final bounded review added the valid SPDX identifiers `NCSA`, `MIT-0` and
`GPL-2.0-only` to the allowlist. Tests accept `(MIT OR Apache-2.0) AND NCSA`,
`CC0-1.0 OR MIT-0 OR Apache-2.0` and `Apache-2.0 OR GPL-2.0-only`; adding these
reduced the `NOASSERTION` count from 49 to 46.

The 65 lock entries without a platform are target-specific or transitive
alternate-target packages outside the four declared filter triples. They are
explicitly marked `none (locked only)`. The inventory does not cover system
libraries, Rust toolchain components, GitHub Actions or the excluded
`experiments/` workspace.

**Identity limitation:** the inventory keys by `name`/`version` only. A lock
containing the same name/version from multiple sources is not supported in this
slice; it fails closed with an explicit source-disambiguated-identity diagnostic
rather than merging entries. The current `Cargo.lock` was checked as **717
unique name/version pairs**: 708 registry packages and 9 workspace/path
packages. Expanding identity handling is deliberately out of scope.

## Not run here

- The full workspace build, full test suite, native startup smoke and 30-stage
  interaction smoke were not repeated in this bounded tooling run. Sol reported a
  full fallback `+stable` Windows run passing 138 unit tests, one doctest (one
  ignored), workspace Clippy, workspace offline build and both native smokes; see
  the Sol-owned `docs/foundation-validation-2026-11-17.md`. That report is not
  restated as evidence for the immutable named pin.
- The GitHub Actions matrix has **not** executed. The workflow is configuration,
  not a passed remote run.
- No YAML schema validator was available locally; the CI file was checked by the
  repository text-contract tests and manual inspection, not by GitHub.
- Screen-reader, physical IME, mixed-DPI, real desktop transparency and
  cross-platform native acceptance remain open as recorded elsewhere.

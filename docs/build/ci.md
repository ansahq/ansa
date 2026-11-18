# CI contract and offline/cache notes

The workflow is [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml).
It is the blocking F03 build contract; the current local machine cannot execute
the remote macOS/Linux matrix.

## Triggers

- `push` is limited to `branches: [main]`. A feature-branch push is validated
  once by the `pull_request` event instead of twice (branch push plus pull
  request synchronisation), which avoids duplicate runs and the wasted runner
  time and queue contention they cause.
- `pull_request` is retained without a branch filter, so a change is validated
  against its actual base before merge, including pull requests whose base is
  not `main`.
- Merged work is still validated: once a pull request lands, the push to `main`
  runs the same jobs on the merged commit, which is the commit that can be
  released. A green pull request therefore does not by itself stand in for a
  green `main`.
- `pull_request_target`, tags, schedules and manual or repository-dispatch
  triggers are not configured. Adding a privileged trigger would be a contract
  change, not a routine edit; the repository tests already reject
  `pull_request_target`, secret references and any `push` without a branch
  filter.

The repository contract test parses the `on:` block structurally with a narrow
stdlib parser (a two-space-indented trigger map with optional four-space
`branches: [ ... ]` flow sequences) rather than matching substrings, so a bare
`push:` or an unexpected shape fails the dependency tooling tests.

## Jobs

### Format and locked inventory (Ubuntu)

- Exact Rust `1.99.1` plus `rustfmt` and `clippy`.
- `cargo fmt --all -- --check`.
- `cargo fetch --locked` (all target-specific packages, once per job).
- `python -m unittest discover -s scripts/dependencies/tests -v`.
- `python scripts/dependencies/inventory.py --check`.

The tooling tests include the deliberately stale-lock negative test: a temporary
manifest is changed without updating its lockfile, and the inventory must fail
instead of emitting a partial inventory.

### Test matrix (Windows, macOS, Linux)

All commands run after `cargo fetch --locked` and use both `--locked` and
`--offline`. The matrix is `fail-fast: false`, so one host failure does not hide
another.

| Gate | Command |
|---|---|
| Format | `cargo fmt --all -- --check` |
| Clippy | `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` |
| Unit/integration | `cargo test --workspace --lib --bins --tests --locked --offline` |
| Doctests | `cargo test --workspace --doc --locked --offline` |
| Native desktop build | `cargo build -p ansa-desktop --locked --offline` |
| Startup helper tests | `python -m unittest discover -s scripts/build/tests -v` |
| Native startup | `python scripts/build/native_smoke.py --timeout 120` (Linux uses Xvfb/software Vulkan) |

The [startup gate](native-smoke.md) requires a genuine in-app marker and zero
exit, retains logs on failure, and does not establish visual or accessibility
acceptance. Local `scripts/validate.ps1` also runs the helper's rejection tests.

`windows-latest`-style runner images are moving targets; the pinned compiler is
not. The Linux job installs the native X11/Wayland/fontconfig/freetype/Vulkan
development packages required by GPUI before fetching or building. macOS uses
the runner's Xcode command-line tools; Windows uses the runner's Visual Studio
Build Tools and Windows SDK.

## Cache

`Swatinem/rust-cache@v2` caches Cargo's downloaded registry/git data and the
`target/` cache with the key `${{ runner.os }}-cargo-locked`. The cache is an
optimisation only:

- no cache miss may change resolution (all Cargo commands are `--locked`);
- no build command depends on network access after `cargo fetch --locked`;
- `cache-on-failure: false` avoids retaining failed build trees as a normal
  starting point.

For a local offline run, populate the cache once and then use the same flags:

```sh
cargo fetch --locked
cargo test --workspace --locked --offline
scripts/validate.ps1 -Offline -Toolchain stable
```

The inventory tool itself runs `cargo metadata --locked --offline` for the
default graph and for each declared platform filter. It therefore needs the
target-specific crates in the local Cargo cache. The inventory job runs
`cargo fetch --locked` and then explicitly fetches each filter target
(`x86_64-pc-windows-msvc`, `x86_64-apple-darwin`, `aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu`) before checking the inventory.

## Secrets and untrusted pull requests

- The workflow uses `on: pull_request`, not `pull_request_target`.
- Top-level permissions are `contents: read`; there is no write token or
  artifact/comment permission.
- No secret is referenced, and checkout uses `persist-credentials: false`.
- The jobs do not publish, sign, deploy or comment on pull requests.

## Reference validation matrix

Sol integrated a separate Python-only Windows/macOS/Linux matrix for
`python scripts/reference/validate_reference.py --root .` and
`python scripts/reference/test_validate_reference.py`. These check the planning
ledger, schema and evidence boundaries; they do not capture reference behavior.
This is configuration, not evidence that the remote matrix has passed.

The local `scripts/validate.ps1` also runs both reference commands and the
dependency tooling tests. Use `-PythonExecutable` if Python is not on PATH.
`-SkipReferenceValidation` and `-SkipDependencyInventory` explicitly omit those
local checks; CI does not skip them. `-Toolchain` applies to child Cargo commands
and is restored on exit; a stable fallback is not an exact-pin verification.

A separate Windows job runs `pwsh -NoProfile -File scripts/reuse/tests/run-tests.ps1`.
It uses miniature local repositories and fake processes: it neither fetches nor
builds upstream candidates. The local runner invokes it on Windows unless
`-SkipReuseToolingTests` is passed; on other hosts it explicitly reports it as
unexercised rather than claiming a cross-platform pass.

## Remote status

The original foundation record described CI as "configured, not executed remotely".
That is a historical boundary, not the current hosted status. The first PR run
passed Windows/macOS Rust builds and all reference/inventory/reuse jobs, but
failed Linux dependency compilation. See the [CI follow-up record](../ci-follow-up-2026-11-17.md)
and [PR #1 checks](https://github.com/ansahq/ansa/pull/1/checks) for subsequent
commit-specific results.

The local [tooling record](validation-2026-11-17.md) and
[full Windows validation](../foundation-validation-2026-11-17.md) remain evidence
for their stated commands, not substitutes for hosted jobs on another OS or a
claim that native startup/reference behavior was tested there.

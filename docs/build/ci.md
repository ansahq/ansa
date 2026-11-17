# CI contract and offline/cache notes

The workflow is [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml).
It is the blocking F03 build contract; the current local machine cannot execute
the remote macOS/Linux matrix.

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

Configured, not executed remotely. The local record in
[`validation-2026-11-17.md`](validation-2026-11-17.md) covers bounded tooling
checks; the full fallback Windows run is recorded in the Sol-owned
`docs/foundation-validation-2026-11-17.md`. Neither record is a substitute for
an actual three-OS CI run.

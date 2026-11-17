# Rust toolchain pin

Ansa pins the exact compiler and component set in
[`rust-toolchain.toml`](../../rust-toolchain.toml):

```toml
[toolchain]
channel = "1.99.1"
profile = "minimal"
components = ["rustfmt", "clippy"]
```

`1.99.1` is the installed and released compiler observed while implementing
F03 (`rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478
2026-08-27)`). It is not a guessed future version. The workspace
`rust-version = "1.99"` in `Cargo.toml` is the minimum-version declaration;
`Cargo.toml`/`Cargo.lock` are owned by Sol, so this change does not edit them.
The exact pin and CI's `RUST_VERSION` must remain equal.

## Why exact

A floating `stable` channel makes Clippy, rustfmt and compiler diagnostics move
without a reviewable repository change. The lockfile and generated inventory
would then have an unrecorded toolchain variable. CI installs `1.99.1`
explicitly; it does not rely on the runner's default Rust.

## Update procedure

Do this only with a reviewed, already-released compiler. Do not invent a future
version.

1. Install the complete named toolchain without changing unrelated toolchains:

   ```sh
   rustup toolchain install <VERSION> --profile minimal --component rustfmt,clippy
   rustc +<VERSION> --version
   cargo +<VERSION> --version
   ```

2. Update `rust-toolchain.toml` and `RUST_VERSION` in
   `.github/workflows/ci.yml` to the same `<VERSION>`.
3. If the minimum supported Rust version changes, ask the `Cargo.toml` owner to
   update `workspace.package.rust-version`. Do not edit that file from this
   tooling slice.
4. Fetch all locked packages, regenerate the inventory and check the diff:

   ```sh
   cargo +<VERSION> fetch --locked
   python scripts/dependencies/inventory.py --toolchain <VERSION>
   python scripts/dependencies/inventory.py --check --toolchain <VERSION>
   python -m unittest discover -s scripts/dependencies/tests -v
   ```

5. Run the local fallback/full validation in
   [`validation-2026-11-17.md`](validation-2026-11-17.md) or the final
   repository record. A toolchain update is not validated by this documentation
   change alone.
6. Add a dated validation entry that names the new compiler, the exact commands
   and the outcomes. Remote CI remains pending until it actually runs.

## Local named-toolchain caveat (2026-11-17)

On this development machine the named `1.99.1-x86_64-pc-windows-msvc`
installation is incomplete. Invoking it fails immediately with:

```text
missing manifest in toolchain '1.99.1-x86_64-pc-windows-msvc'
```

The active `stable` toolchain is itself `rustc 1.99.1`/`cargo 1.99.1`, so local
fallback commands use the explicit form `cargo +stable ...` (or
`scripts/validate.ps1 -Toolchain stable`). Plain `cargo` in this checkout selects
the broken named toolchain until it is repaired. This fallback is useful for
local evidence, but it is **not** proof that the immutable `+1.99.1` pin works.
The named installation was not repaired or mutated during this task. A developer
can repair it explicitly with `rustup toolchain install 1.99.1 --profile minimal
--component rustfmt,clippy`; fresh CI installs the exact pin independently.

# Linux compatibility pin: libc 0.2.189 / xattr 0.2.3

This record explains the bounded, lockfile-only change addressing the observed
Linux Clippy compilation failure. It is a compatibility constraint, not a license or security review,
and it does not claim that the Linux matrix has run locally. The run that proved
the failure is linked below; the run that proves the fix is expected after Sol
pushes.

## Status

| Item | Value |
|---|---|
| Failing run | [Rust workflow run 44072419286](https://github.com/ansahq/ansa/actions/runs/44072419286), head `71415b0efca0ac0385fe889ba9caba24139e3e19` |
| Failing job | [`ubuntu-latest` Clippy](https://github.com/ansahq/ansa/actions/runs/44072419286/job/134001696138), 2026-11-17 |
| Passing jobs in that run | [windows-latest](https://github.com/ansahq/ansa/actions/runs/44072419286/job/134001695775), [macos-latest](https://github.com/ansahq/ansa/actions/runs/44072419286/job/134001695927) |
| Lock change | `libc` `0.2.191` → `0.2.189` (two lines: `version`, `checksum`) |
| Source/manifest change | none |
| Linux proof | pending the next remote `ubuntu-latest` job after Sol pushes; **not executed locally** |

## Observed failure

`ubuntu-latest`, `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`:

```text
    Checking xattr v0.2.3
error[E0425]: cannot find value `ENOATTR` in crate `::libc`
  --> .../xattr-0.2.3/src/sys/mod.rs:12:56
   |
12 |             pub const ENOATTR: ::libc::c_int = ::libc::ENOATTR;
   |                                                ^^^^^^^^^^^^^^^
...
   ::: .../libc-0.2.191/src/unix/linux_like/linux/gnu/b64/x86_64/mod.rs:424:1
help: a constant with a similar name exists
   |
12 |             pub const ENOATTR: ::libc::c_int = ::libc::ENOSTR;
error: could not compile `xattr` (lib) due to 1 previous error
```

The Windows and macOS jobs in the same run passed with the same lockfile, so the
failure is Linux-specific in practice, not a general build break.

## Exact dependency chain

`cargo +stable tree --locked --target x86_64-unknown-linux-gnu -i xattr --workspace`
(workspace paths abbreviated):

```text
xattr v0.2.3
└── zed-async-tar v0.5.0-zed
    └── gpui_http_client v0.2.2
        └── gpui v0.2.2
            ├── ansa-ui-components v0.1.0 (crates/ui-components)
            │   └── ansa-ui-gpui v0.1.0 (crates/ui-gpui)
            │       └── ansa-desktop v0.1.0 (apps/desktop)
            └── ansa-ui-gpui v0.1.0 (crates/ui-gpui) (*)
```

- `gpui = "=0.2.2"` is the workspace pin; `gpui_http_client 0.2.2` is its
  dependency, and `zed-async-tar 0.5.0-zed` is the only version of that fork on
  crates.io.
- `zed-async-tar 0.5.0-zed` declares `[target."cfg(unix)".dependencies] xattr =
  "0.2"`, has `default = ["xattr"]`, and has no newer release; the optional
  dependency is active in this lock (it appears under the Linux filter).
- `xattr 0.2.3` (published 2022-05-03) is the newest `0.2.x`. Its `src/sys/mod.rs`
  macro emits `pub const ENOATTR: ::libc::c_int = ::libc::ENOATTR;` for each
  supported unix platform, Linux included, and `src/util.rs` maps that constant
  to "attribute not found" (`Ok(None)`).

## Root cause: where Linux `ENOATTR` went away

- `libc` 0.2.189 still defines it, already deprecated:

  ```rust
  // libc-0.2.189/src/unix/linux_like/linux/mod.rs:1350
  #[deprecated(
      since = "0.2.55",
      note = "ENOATTR is not available on Linux; use ENODATA instead"
  )]
  pub const ENOATTR: c_int = crate::ENODATA;
  ```

- `libc` 0.2.190 (released 2026-10-02) removed it as part of
  [rust-lang/libc#5401, "Remove most items that have been deprecated since
  before 0.2.100"](https://github.com/rust-lang/libc/pull/5401) (merged
  2026-08-12), listed under **Removed** in the
  [0.2.190 release notes](https://github.com/rust-lang/libc/releases/tag/0.2.190).
  The PR touches `src/unix/linux_like/linux/mod.rs` and
  `libc-test/semver/linux.txt` for `ENOATTR`.
- Verified against the published artifacts, not guessed: downloading
  `libc-0.2.189.crate` and `libc-0.2.190.crate` from
  `https://static.crates.io/crates/libc/` and grepping `src/` shows `ENOATTR` in
  0.2.189 (Linux and Android) and **no** `ENOATTR` anywhere in the Linux-like
  tree in 0.2.190. 0.2.190 is therefore the first incompatible release and
  0.2.189 is the newest compatible one. `libc` 0.2.191 keeps the removal.

## Why a transitive update was not available

The preferred fix — a semver-compatible `xattr` patch — does not exist:

- `xattr` `0.2.3` is the final `0.2.x` release. The fix lives only in the
  semver-major `1.x` line, which resolves the constant through `rustix`
  (`Errno::NODATA` on Linux, `Errno::NOATTR` on macOS) instead of
  `libc::ENOATTR`.
- `zed-async-tar 0.5.0-zed` requires `xattr = "0.2"`, and there is no newer
  `zed-async-tar` release.
- Moving to `xattr 1.x` would require changing `gpui`/`gpui_http_client` (the
  workspace pins `gpui = "=0.2.2"`), i.e. a manifest/source change outside this
  bounded lockfile fix. That needs an explicit decision on the GPUI dependency,
  not a silent patch.

Vendoring `xattr`, patching it in-tree, or adding a `[patch.crates-io]` override
was rejected for this bounded fix: each adds source provenance, license and
maintenance work outside the approved lockfile-only scope. Such a replacement
would need its own reviewed source inventory and an explicit guard update;
vendoring is not inherently invisible to dependency inventory tooling.

## The locked fix

```sh
cargo +stable update -p libc --precise 0.2.189
```

`cargo` accepted the downgrade with no manifest edit, i.e. every locked
dependent's `libc` requirement is satisfied by 0.2.189. The resulting
`Cargo.lock` diff is exactly:

```diff
 [[package]]
 name = "libc"
-version = "0.2.191"
+version = "0.2.189"
 source = "registry+https://github.com/rust-lang/crates.io-index"
-checksum = "65cd87c9a6d60536580b316ad985726104730a5151c99845f2490428ef91240a"
+checksum = "3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2"
```

The `0.2.189` checksum matches the crates.io artifact metadata
(`https://crates.io/api/v1/crates/libc/0.2.189`, `checksum =
3eaf3ede…12f2`, `rust_version = 1.65`, `MIT OR Apache-2.0`). No other package,
source, feature or toolchain entry changed. `libc` is a single version for every
target, so the pin also applies to Windows and macOS; both already passed with
the newer 0.2.191 before this change.

The regenerated inventory diff is correspondingly bounded: one `libc` row plus
the LF-normalized `Cargo.lock` SHA-256 line in `dependency-inventory.md`, and in
`dependency-inventory.spdx.json` the `libc` package/relationship entries plus the
document namespace/comment that embed that lock hash — all 158 changed SPDX
lines are those, no unrelated package changed.

## Verification performed

Local host is Windows; the Linux toolchain target is not installed and no
system/target packages were added. The Linux proof is therefore the remote CI
job, not a local claim.

| Check | Result |
|---|---|
| `cargo +stable update -p libc --precise 0.2.189` | downgraded `libc v0.2.191 -> v0.2.189`; only the two lock lines above changed |
| `cargo +stable fetch --locked` and `--target x86_64-unknown-linux-gnu` | exit 0 (cache populated for the offline inventory) |
| `cargo +stable tree --locked --target x86_64-unknown-linux-gnu -i xattr --workspace` | reproduces the chain above from the checked-in lock; `xattr 0.2.3` is still on the Linux graph |
| `cargo +stable clippy --workspace --all-targets --locked --offline -- -D warnings` (Windows) | exit 0, `Finished dev profile ... in 19.73s`; local Windows-host evidence, not a hosted matrix result |
| `cargo +stable test --workspace --lib --bins --tests --locked --offline` (Windows) | exit 0; every test binary reported `test result: ok` |
| `python scripts/dependencies/inventory.py --toolchain stable` | wrote 717 packages, 65 not observed for declared CI targets |
| `python scripts/dependencies/inventory.py --check --toolchain stable` | exit 0: `inventory: 717 packages checked; 65 not observed for declared CI targets` |
| `python -m unittest discover -s scripts/dependencies/tests -v` | `Ran 30 tests ... OK`, including the 4 new guard tests |
| Remote `ubuntu-latest` | **not run locally**; awaiting corrected-commit hosted execution |

Windows was recompiled here, so the Windows gate has local evidence. macOS
cannot be built on this host; it passed with 0.2.191 in the failing run above and
remains a remote proof point for the downgrade, alongside Linux.

Toolchain note: the implementation commands above used `+stable` because the
earlier foundation record reported a broken named `1.99.1` installation. Sol's
later checks found the named toolchain available; see the
[CI follow-up record](../ci-follow-up-2026-11-17.md). These observations do not
attribute a repair to this change; no toolchain configuration was edited.

Post-change builds and tests ran with `--locked --offline`; no new root
dependency, feature or framework upgrade was introduced.

## Regression guard

`scripts/dependencies/tests/test_linux_compat.py` (stdlib only, no Cargo, no new
dependencies) locks the constraint to the checked-in evidence:

- exactly one `libc` package, pinned to `0.2.189`, below the first incompatible
  release `0.2.190`, from `registry+https://github.com/rust-lang/crates.io-index`,
  with the reviewed crates.io SHA-256;
- the `gpui 0.2.2 → gpui_http_client 0.2.2 → zed-async-tar 0.5.0-zed →
  xattr 0.2.3 → libc` edges still exist in `Cargo.lock`, so the guard cannot
  silently become irrelevant;
- no `[patch]`, path replacement or Cargo source override masks the resolution
  (no vendoring);
- this record still contains the pinned version, removal release, failure run,
  upstream evidence links, exact `cargo`/inventory commands and the libc
  checksum.

A future `cargo update` that moves `libc` past 0.2.189 fails this test with the
remediation command instead of failing later on Linux with `E0425`.

Run it with:

```sh
# just this guard
python -m unittest discover -s scripts/dependencies/tests -p "test_linux_compat.py" -v
# or the whole dependency tooling suite
python -m unittest discover -s scripts/dependencies/tests -v
```

## Upgrade / reopen procedure

The pin is intentionally unattractive to keep once a maintained path exists.
Reopen it when **any** of these is true:

1. `xattr` publishes a `0.2.x` patch that stops referencing `libc::ENOATTR`; or
2. `zed-async-tar` (or `gpui_http_client`) is updated to depend on `xattr 1.x`;
   or
3. the GPUI dependency is upgraded or replaced and the old `xattr 0.2` chain
   leaves the graph; or
4. upstream evidence shows `ENOATTR` restored (not expected — it was deprecated
   since 0.2.55 and already wrong for Linux).

Procedure:

1. Make the dependent update in its own reviewed change (manifest/source
   change needed for 1–3; the lockfile alone cannot move `xattr` past its
   `"0.2"` requirement).
2. Confirm the chain no longer reaches `xattr 0.2.3`:
   `cargo +stable tree --locked --target x86_64-unknown-linux-gnu -i xattr --workspace`
   must fail to match (or show only a fixed version).
3. Relax the pin only after that: `cargo +stable update -p libc` (or
   `--precise <newest>`), then `cargo +stable fetch --locked` and
   `cargo +stable fetch --locked --target x86_64-unknown-linux-gnu`.
4. Regenerate both generated artifacts:
   `python scripts/dependencies/inventory.py --toolchain stable`, then
   `python scripts/dependencies/inventory.py --check --toolchain stable`.
5. Update `test_linux_compat.py` (`PINNED_LIBC`,
   `FIRST_LIBC_WITHOUT_LINUX_ENOATTR`, checksum, chain assertions) and this
   record in the same change; never change one without the others.
6. Run the full local Windows gates, then let the remote matrix — especially
   `ubuntu-latest` Clippy — provide the Linux proof and link that run here.

Do not silently add `[patch.crates-io]`, a vendored `xattr`, a forked `libc`, or
edit `target/` to bypass this guard. A different resolution needs explicit
review, corresponding source/notice inventory and updated regression tests.

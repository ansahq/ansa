# PR #1 CI follow-up

This supplements the historical [foundation validation record](foundation-validation-2026-11-17.md).
It does not replace earlier command results or close the reference/runtime/license gates.

## Observed hosted failure

The first PR run, [44072419286](https://github.com/ansahq/ansa/actions/runs/44072419286),
tested commit `71415b0efca0ac0385fe889ba9caba24139e3e19`. Its matching
[push run](https://github.com/ansahq/ansa/actions/runs/44072382359) duplicated the jobs.

| PR job | Actual result |
|---|---|
| Reference ledger: Windows, macOS, Ubuntu | All three passed |
| Reuse probe safety tests: Windows | Passed |
| Format and locked inventory: Ubuntu | Passed |
| Rust checks/tests/doctests/desktop build: Windows, macOS | Both passed with the selected exact Rust toolchain |
| Rust checks/tests/doctests/desktop build: Ubuntu | Compiler error during Clippy; later test/doctest/build steps were skipped |

The Linux error was `E0425` in `xattr 0.2.3`, which references `libc::ENOATTR`;
that constant is absent from the locked Rust crate `libc 0.2.191`. This is a
dependency compilation failure, not a Clippy lint to suppress. No Linux pass is
inferred from the other jobs.

## Change ownership

DeepSeek V4.1 Flash 1017 High through OpenCode Go implements two disjoint slices:

- Triggers/docs/contract tests: `.github/workflows/ci.yml`,
  `scripts/dependencies/tests/test_repository.py`, `docs/build/ci.md`.
- Dependency compatibility: `Cargo.lock`, the two generated dependency inventory
  files, `scripts/dependencies/tests/test_linux_compat.py`, and
  `docs/build/linux-compatibility.md`.

Sol retains integration, this record, final verification and PR publication.
After the trigger implementer's completed handoff, Sol updates the CI guide's
hosted-evidence status; there is no overlapping write ownership.
The intended event policy is PR checks plus pushes to `main` only. The Linux fix
must retain every existing CI gate and use a bounded, documented dependency
compatibility change, not a disabled Linux job or weakened warnings.

## Evidence boundary

The hosted Windows/macOS successes above are new evidence beyond the earlier
local fallback run. They do not repair the local incomplete named toolchain,
prove native startup on those runners, or validate reference-product behavior.
Results for the corrected commit must be read from its own hosted run rather
than inferred from either the old jobs or Windows-only local validation.

## Current local toolchain observation

Unlike the original foundation run, the named toolchain now responds locally:
`rustup show active-toolchain` reports `1.99.1-x86_64-pc-windows-msvc`, selected
by this checkout's `rust-toolchain.toml`. `rustc +1.99.1 --version` reports
`1.99.1 (58cf8f9c3 2026-10-13)`, and `cargo +1.99.1 fmt --all -- --check`
passed. This records the current observation, not a claim that this work repaired
the installation or that formatter success proves the remaining build/tests.

## Bounded correction and Sol validation

The trigger change adds only `branches: [main]` under `push`; PR triggers,
permissions, concurrency and all eight jobs remain enabled. Its structural
contract test rejects the old unfiltered push configuration.

The dependency correction changes only the `libc` version/checksum in Cargo.lock:
`0.2.191` to `0.2.189`. `0.2.190` removed Linux's deprecated `ENOATTR` alias;
the old `xattr 0.2.3` dependency still needs it. The
[compatibility record](build/linux-compatibility.md) documents the source
evidence, regression guard and required upgrade/unpin procedure. This changes a
Rust crate, not the runner's system C library, and introduces no vendored patch,
manifest edit or application-code change.

Sol independently verified that every other lockfile package is unchanged,
that the cached published `libc` source defines `ENOATTR = ENODATA`, and that the
regenerated inventory exactly matches all 717 package identities/sources and
708 raw registry checksums. Canonical lock SHA-256:
`c0c6e1bd925291244b07f2388d22321ba5d0c3fee8a7925b4b2318262bc4397c`.

Sol then unset the process-local `RUSTUP_TOOLCHAIN` override and ran the complete
runner against the checkout's exact toolchain selection, with no skip switches:

```powershell
pwsh -NoProfile -File scripts/validate.ps1 `
  -PythonExecutable 'C:\Users\abhir\AppData\Local\Temp\opencode\python-embed\python.exe' `
  -Offline -NativeSmoke
```

Exit **0**:

- 138 Rust unit tests; 1 doctest passed and 1 explicitly ignored.
- Formatting, Clippy and development desktop build passed with Rust 1.99.1.
- 30 dependency/repository/compatibility tests, 41 reference-validator tests,
  and 22 reuse-safety tests passed (**93 tooling tests**).
- Inventory/ledger checks passed; Windows startup and 30-stage synthetic
  interaction smokes passed without clipboard writes or external app launches.

The existing `proc-macro-error2 2.0.1` future-incompatibility notice remains.
No Linux target was installed or Linux application executed locally. The
corrected commit's hosted checks remain the authority for Linux/macOS results;
this local run alone is not a Linux-fix claim.

## Independent review and hosted follow-up

DeepSeek V4.1 Flash 1017 Max through OpenCode Go independently reviewed the
scoped correction and found **no blocking finding**. It confirmed the artifact
checksum, non-yanked version, `ENOATTR = ENODATA` semantics, bounded inventory
diff, unchanged gates and requested event policy. The pin remains temporary;
the guard's coupling to historical command wording is a non-blocking
maintainability note, not a functional CI result.

This record captures pre-push verification. The corrected commit's actual hosted
run URL, head SHA and outcomes are reported on [PR #1](https://github.com/ansahq/ansa/pull/1)
after execution. A successful Windows local run or static review must not be
substituted for the new Linux and macOS jobs.

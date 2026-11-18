# Native startup smoke

This document describes the blocking hosted-runner startup check added to the
Rust test matrix in [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml).
It is configuration and local-tooling documentation; a configured job is not
evidence that the macOS or Linux jobs have run.

## What the gate proves

The helper [`scripts/build/native_smoke.py`](../../scripts/build/native_smoke.py)
launches the actual `ansa-desktop` executable built in the preceding CI step.
It passes `--smoke-test`, which starts the real GPUI application, opens the
launcher window, and exits after two frame callbacks. The application emits this
line from the second frame callback:

```text
Ansa native startup smoke: window opened and two GPUI frames rendered, exiting with success.
```

The helper requires **both** conditions:

1. the exact marker line is present on the process's stdout, not only stderr; and
2. the process exits with status `0` inside the bounded timeout.

The marker is emitted by the application in
[`crates/ui-gpui/src/lib.rs`](../../crates/ui-gpui/src/lib.rs); the helper never
prints it into the child's captured output. A build that silently exits, exits
successfully without reaching the frame callbacks, crashes, or hangs therefore
fails the gate. `scripts/build/tests/test_native_smoke.py` also checks that the
helper constant and the application source stay identical.

This is a startup smoke, not an accessibility or input acceptance test. It does
not exercise screen readers, physical keyboards, IME composition, focus
restoration, clipboard behavior, screenshots, or visual correctness. The
existing debug-only `--interaction-smoke-test` remains available through
`scripts/validate.ps1 -NativeSmoke` for local 30-stage synthetic coverage; the
hosted startup gate deliberately does not depend on it.

## Timeout and failure behavior

Each run is started with `subprocess.Popen` and given a hard default timeout of
120 seconds. On timeout the child is killed, partial stdout/stderr are still
written, and the helper exits non-zero. A non-zero child exit or a missing marker
also fails. The timeout is intentionally larger than the two-frame smoke needs
so a cold software-rendered Linux start is not reported as a hang.

Captured output and diagnostics are written to `target/native-smoke/`:

| File | Contents |
|---|---|
| `startup.stdout.log` | Child stdout, including the required marker on success |
| `startup.stderr.log` | Child stderr and startup diagnostics |
| `summary.txt` | Binary path, platform, command, timeout, exit code, duration and result |

The workflow uploads this directory with `actions/upload-artifact@v4` as
`native-smoke-${runner.os}` even when the smoke step fails (`if: always()`), and
ignores the artifact when the build failed before logs existed.

## CI placement

The smoke runs in the existing Windows/macOS/Linux `test` matrix, after:

```text
cargo build -p ansa-desktop --locked --offline
```

and before the log upload. Hosted jobs use:

```text
python scripts/build/native_smoke.py --timeout 120
```

On Linux the command is wrapped in `xvfb-run` with a 1280x800x24 virtual X11
screen. The Linux dependency step installs `xvfb`, `xauth`, `libvulkan1`,
`mesa-vulkan-drivers` (Mesa lavapipe software Vulkan), `libgl1-mesa-dri` and
`vulkan-tools` alongside the existing X11/Wayland/fontconfig development
packages. The smoke step selects the installed `lvp_icd*.json` driver through
`VK_ICD_FILENAMES` and `VK_DRIVER_FILES` when present, and sets
`LIBGL_ALWAYS_SOFTWARE=1`; Mesa's normal device selection remains the fallback.
No GPU is assumed.

Windows and macOS use the hosted runner's native desktop session directly.

## Local usage

Build the same development executable, then run:

```sh
cargo build -p ansa-desktop --locked --offline
python scripts/build/native_smoke.py --timeout 120
```

On Windows the default binary path `target/debug/ansa-desktop` resolves to
`target/debug/ansa-desktop.exe`. Use `--binary` for another path and
`--artifact-dir` to keep logs elsewhere. The helper runs from the repository
root regardless of the caller's working directory.

Tooling tests use fake child processes and do not open a window:

```sh
python -m unittest discover -s scripts/build/tests -v
```

On a host without a usable `python` command, pass an explicit interpreter; this
is a tooling-environment issue, not a native smoke result.

## Evidence boundary

The hosted Windows/macOS/Linux startup gate has **not** been executed remotely as
part of the change that introduced this document. The local Windows check
(`cargo build -p ansa-desktop --locked --offline`, the helper, and its 11 tests)
passed on 2026-11-18. That local result does not establish macOS or Linux
behavior, and the configured workflow must not be described as a passed remote
matrix until the corresponding hosted jobs have actually run.

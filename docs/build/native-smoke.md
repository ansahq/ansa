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
Ansa native startup smoke: window opened and two GPUI frame callbacks reached, exiting with success.
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

The application also writes three progress lines to stderr while the flag is
active (`starting GPUI application`, `GPUI window created`, `first frame
callback reached`). They exist so a hosted failure says how far startup got
instead of producing an empty log. They are diagnostics only: the helper still
requires the exact success line on stdout and does not treat any stderr text as
a pass.

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
No GPU is assumed. It also clears `WAYLAND_DISPLAY`, because GPUI prefers
Wayland whenever that variable is non-empty and this job starts only an X11
server.

Windows and macOS use the hosted runner's native desktop session directly.

## Linux X11 frame scheduling with the pinned GPUI

[Run 44247153074](https://github.com/ansahq/ansa/actions/runs/44247153074)
included the HWND guard correction, but its Linux job then timed out after
120 seconds with empty stdout and stderr. Source inspection of the pinned
`gpui 0.2.2` gives a diagnosis consistent with that silence:

- GPUI's X11 backend starts a window's periodic frame loop only while handling
  `MapNotify` or `VisibilityNotify`; without that loop it never calls the
  request-frame callback that drives `Window::on_next_frame`.
- The `calloop` event source watches the X11 socket, while x11rb keeps its own
  internal event queue. GPUI synchronizes the map request immediately after
  asking the server to map the window (`VoidCookie::check`), and while waiting
  for that reply libxcb reads the freshly generated mapping events off the
  socket and leaves them in that internal queue.
- Once the socket is empty the event source does not wake, so the queued
  `MapNotify` is never dispatched, the refresh loop never starts, and the
  two-frame marker is never reached. On a desktop, later pointer or keyboard
  traffic wakes the connection, which is why the same pinned GPUI usually
  renders. Under `Xvfb` with no input devices and no window manager nothing
  may do so, leaving the observed completely silent stall.

GPUI fixed this class of defect after the 0.2.2 snapshot by draining buffered
X11 events after foreground work ([upstream PR #62081](https://github.com/zed-industries/zed/pull/62081),
merged 20 August 2026; independently checked by Sol on 18 November).
This repository keeps the exact `=0.2.2`
pin, so when the X11 backend is selected the smoke path in
[`crates/ui-gpui/src/lib.rs`](../../crates/ui-gpui/src/lib.rs) asks the window
to activate itself after registering its callbacks. On X11 that method issues
`SetInputFocus`, the same platform call a real launcher summon already performs
through `window.activate_window()`. The resulting `FocusIn` event makes the
connection readable again, which drains the queued mapping events and starts
frame delivery. It is a real platform request, not a simulated frame: the
marker still comes only from the second genuine frame callback, and the helper
still requires that line on stdout together with exit 0. Windows, macOS and
the Wayland backend do not take this path. Hosted run 44250538706 subsequently
passed this activated-startup check, as recorded below.

This checks **activated startup** under Xvfb. It does not fix or certify
unattended/inactive X11 first presentation, nor does it change normal startup's
window-manager focus policy. The upstream event-queue fix is not backported here.
Keep that GPUI limitation open for future framework/lifecycle work.

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

Initial hosted [run 44246643923](https://github.com/ansahq/ansa/actions/runs/44246643923)
passed Windows and macOS startup on 2026-11-18, but failed Linux startup.
The Linux backtrace showed the HWND lookup helper calling GPUI 0.2.2's
unimplemented X11 `HasWindowHandle` method while looking for a Windows HWND.
That HWND adapter is now compiled only on Windows; other platforms return
`None` without asking GPUI for a native handle. The Python smoke helper itself
is cross-platform and unchanged by that guard. This corrects normal startup
too, not just the smoke. The failure was not suppressed or converted into a
passing smoke.

The follow-up [run 44247153074](https://github.com/ansahq/ansa/actions/runs/44247153074)
confirmed Windows and macOS again. Linux no longer panicked, but it timed out
with no stdout, no stderr and no marker. The uploaded artifact contained two
empty logs and a summary showing exit code `None` after 120.103 seconds, so the
helper behaved correctly. The empty streams were not conclusive on their own;
the X11 scheduling path described above is the reasoned diagnosis from the
pinned GPUI source and the matching upstream root cause, and the progress
diagnostics added with that correction make the next failure attributable.
That historical run is not a Linux pass.

Sol's local exact-pin/offline validation passed 138 unit tests, one doctest
(one ignored), formatting, Clippy, the desktop build, both native smokes and
the tooling suites before the platform-guard correction, then repeated the full
validation on `c261854`: 138 Rust tests, one doctest (one ignored), 112 tooling
tests, format, Clippy, desktop build and both native smokes passed.

### Confirmed three-OS startup run

[Run 44250538706](https://github.com/ansahq/ansa/actions/runs/44250538706) passed
all eight jobs on `c2618547be8bc03f9d82cbbb55ac336474c09b92`.
Sol downloaded the three `native-smoke-*` artifacts and checked their actual logs:

| Artifact | Exit | Timeout | Duration | Result |
|---|---:|---|---:|---|
| `native-smoke-Windows` | 0 | false | 0.781 s | PASS |
| `native-smoke-macOS` | 0 | false | 1.197 s | PASS |
| `native-smoke-Linux` | 0 | false | 3.235 s | PASS |

The Linux log contains all three progress diagnostics and the exact success
marker from the second frame callback. The check remains blocking; neither
failure was ignored. This is activated X11 startup evidence on the hosted
software-rendered display, not an unattended-startup, Wayland, accessibility or
visual-correctness claim.

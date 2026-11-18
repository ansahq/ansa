# Foundation acceptance follow-up — 18 November 2026

This follows merged [PR #1](https://github.com/ansahq/ansa/pull/1), commit
`7b22edc`, whose main CI passed all eight jobs. Changes are published through
[PR #2](https://github.com/ansahq/ansa/pull/2), not directly to main.

## Ownership and verification

- Sol retained architecture, acceptance decisions and final verification.
- DeepSeek V4.1 Flash 1017 via OpenCode Go High implemented native smoke tooling
  and CI in one bounded assignment, and reuse evidence/tooling in a separate,
  non-overlapping assignment.
- DeepSeek Max was assigned independent review of the native-smoke change and
  reference closure boundary. Reviews are not themselves runtime evidence.

## Local Windows results

Sol ran the checked-in exact toolchain, without `-Toolchain` override:

```powershell
pwsh -NoProfile -File scripts/validate.ps1 `
  -PythonExecutable 'C:\Users\abhir\AppData\Local\Temp\opencode\python-embed\python.exe' `
  -Offline -NativeSmoke
```

After the Linux platform-guard correction, this passed:

- formatting and Clippy (`-D warnings`);
- 138 Rust unit tests and one doctest; one UI example doctest remains ignored;
- locked/offline native desktop build;
- 30 dependency tests, 41 reference tests, 27 reuse-tooling tests and 14 native
  smoke-helper tests (112 tooling tests total in the final `c261854` rerun);
- generated inventory check: 717 packages, 65 not observed for declared filters;
- native startup and 30-stage synthetic widget/action smokes.

Sol also independently ran `scripts/build/native_smoke.py --timeout 120` on the
built Windows artifact. The exact in-app marker appeared on stdout and exit was
zero. This does not validate physical input, screen readers, focus restoration
or pixels. The existing proc-macro-error2 future-incompatibility warning remains.

## Hosted startup failure and correction

Initial [run 44246643923](https://github.com/ansahq/ansa/actions/runs/44246643923)
passed seven jobs, including Windows/macOS startup, but failed Linux startup.
The actual backtrace identified GPUI 0.2.2's unimplemented X11
`HasWindowHandle::window_handle`, called by Ansa's Windows-HWND adapter during
`Launcher::new`. The adapter is now compiled only on Windows; other targets
return `None` without calling the unsupported API. Normal Linux startup gets
the same fix; no smoke-only bypass or suppressed failure was introduced.

The corrected hosted run is
[44247153074](https://github.com/ansahq/ansa/actions/runs/44247153074).
Windows/macOS passed again; Linux no longer panicked but timed out after 120
seconds with no marker or stderr. The helper killed it and correctly failed
the job. Linux frame scheduling/display setup remains under investigation;
neither run is Linux startup acceptance.

After the smoke explicitly activated its real X11 window, hosted
[run 44250538706](https://github.com/ansahq/ansa/actions/runs/44250538706) passed
**all eight jobs** on `c2618547be8bc03f9d82cbbb55ac336474c09b92`, including the
three actual built artifacts' startup checks. Sol downloaded their logs and
verified exit 0, no timeout and PASS summaries: Linux 3.235 s, macOS 1.197 s,
Windows 0.781 s. Linux stdout contained the exact second-frame-callback marker
and stderr contained the three progress diagnostics. This establishes activated
startup under Xvfb/software Vulkan, not unattended/inactive X11 first-presentation,
Wayland, visual, physical-input or accessibility acceptance. See the
[native smoke boundary](build/native-smoke.md); the GPUI pin was not changed.

## Reference boundary

The 100-row planning ledger validates; identities remain explicitly unknown and
runtime observations remain zero. Independent review correctly distinguishes
the passing planning checks and ten-row design/source audit from runtime
capture. ANS-5 remains open against the versioned behavioral-reference goal and
seed capture/replay plan: there are no established isolated reference hosts or
verified builds, and this session exposes no native desktop-control tools.
The [capture procedure](reference/procedures/seed-capture.md) records the future
schema migration and direct runtime-evidence binding requirements. No personal
profile or unrelated application data was captured.

## Reuse decision boundary

The [ANS-6 closure assessment](reuse/closure.md) records the final greenfield,
zero-candidate-source-reuse decision. Sol independently reran the static scanner
against the five origin-matching cached checkouts and a fresh canonical anyrun
checkout. Cached anyrun's historical origin was not rewritten. The reproduced
inventory contains 3,466 Cargo lock packages, 704 npm entries and 836 pnpm entries;
licence-unknown classes remain explicit. Sol corrected a scanner aggregate that
had labelled only licence-unknown npm entries as the total, and added assertions
distinguishing the total from the unknown count. All 26 reuse-tooling tests passed.
An additional source-preservation test subsequently passed with a new guard
rejecting scanner output inside candidate checkouts (27 reuse-tooling tests).
DeepSeek Max's read-only reuse review found no closure blocker. Sol resolved its
nonblocking evidence-hygiene findings: test-count consistency, historical fetch
wording, scan-destination wording and a misleading aggregate field name. The
aggregate now says missing licence **metadata**, not resolved licences; npm
entries containing unverified text fields are not claimed cleared. The updated
27-test suite passed again after those corrections.

Five candidate build/configure attempts ended at documented failures and Flare
was intentionally historical-only. No candidate test suite executed, and there
is no per-package licence resolution, transitive C++/Qt notice inventory, legal
approval or runtime acceptance. Those limits do not reopen the zero-reuse scope
decision; future source reuse requires a new audited proposal.

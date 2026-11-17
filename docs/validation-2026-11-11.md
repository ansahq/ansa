# Initial local validation — 2026-11-11

Environment: Windows 11 Home Single Language, build 26200, x86-64; Intel Core i5-12450H (8 cores / 12 logical processors), 15.6 GiB reported RAM; active power scheme named Turbo. Rust 1.99.1, Cargo 1.99.1, GPUI 0.2.2. No reference-product binary was exercised.

## Preliminary search measurement

Command:

```sh
cargo run --release --locked --offline -p ansa-core --example search_bench
```

Synthetic in-memory corpus: 10,000 commands (`fixture:command-00000` through `fixture:command-09999`, titles `Project NNNNN Terminal`). Five queries cover broad token, prefix, subsequence, missing and exact title cases. 50 warmup queries, 500 measured queries, one process run.

| Local query + ranking + result cloning | Microseconds |
|---|---:|
| p50 | 3,985 |
| p95 | 4,660 |
| p99 | 5,324 |

This is a **preliminary microbenchmark**, not F09 closure: no raw timing artifact, repeated-run distribution, external input/display capture, UI frame measurement, memory/power trace or real filesystem corpus was collected. It cannot establish the warm summon, typing-to-display, idle CPU or resident-memory targets. Rerun after changes; do not treat one run as a regression threshold.

## Observations

First-slice local commands (superseded counts are retained as history):

| Command | Outcome |
|---|---|
| `cargo +stable test --workspace --locked --offline` | 77 unit tests and one calculator doctest passed; one documentation-only UI example ignored |
| `cargo +stable clippy --workspace --all-targets --locked --offline -- -D warnings` | Passed; upstream future-incompatibility notice remains |
| `cargo +stable fmt --all -- --check` | Passed |
| `cargo +stable build -p ansa-desktop --locked --offline` | Native debug executable built |
| `cargo +stable run -p ansa-desktop --locked --offline -- --smoke-test` | Exited successfully after two GPUI frame callbacks; no startup error reported |

The smoke-test flag starts the actual native app and requests exit after two frame callbacks. It does not inspect screenshots, wait for background discovery, simulate user workflows or validate accessibility.

- Core, search, identity and native-adapter unit tests exercised local state and synthetic fixtures. Native shortcut testing creates a real registration, dispatches a synthetic message to that registration's worker, checks conflict handling and cleanup; it is not physical keyboard/focus-cycle validation.
- CLI arithmetic `(24 + 8) / 2` returned `16`.
- CLI app discovery exercised the machine's actual Start Menu. No discovered app was automatically launched as part of this validation.
- A CLI search exposed accidental fuzzy matches in full-path subtitles. Fuzzy matching now excludes subtitles while preserving their token matches; a regression test covers this.
- GPUI's pinned Windows app-wide `hide()` is a no-op. The application uses its own main-thread HWND visibility adapter instead; focus restoration was deferred in this first slice (see follow-up below).
- `cargo check -p ansa-desktop --locked --offline` succeeded. Cargo reported an upstream future-incompatibility warning for `proc-macro-error2 2.0.1`; review before framework upgrades.

Real display, screen-reader, IME, mixed-DPI and lifecycle acceptance remain open. Build output and automated tests are not evidence that those journeys pass.

## Follow-up: interaction and checked dismissal

The next slice adds independently searchable, snapshot-bound actions; session pins and usage signals; clear calculator errors and explicit `=` routing; bounded/coalesced background catalog refresh; and conservative Windows focus return.

| Command | Local outcome |
|---|---|
| `cargo +stable test --workspace --locked --offline` | 116 unit tests and one doctest passed; one documentation-only UI example ignored |
| `cargo +stable clippy --workspace --all-targets --locked --offline -- -D warnings` | Passed, with the previously noted upstream future-incompatibility notice |
| `cargo +stable run --locked --offline -p ansa-desktop -- --interaction-smoke-test` | Native debug executable ran all 18 synthetic stages and exited successfully |

The debug-only smoke fixture exercises actual GPUI event subscriptions, action dispatch, input replacement and focus handles: open actions, focus action search, filter/pin, retain the root query, restore search focus, display division-by-zero failure, handle empty action matches, Escape, Tab/Shift+Tab, Enter on the footer control, and reject a stale menu after catalog replacement. It deliberately does **not** launch apps or write the clipboard. Programmatic dispatch is not a physical-keybinding, IME, screen-reader, visual screenshot or focus-restoration acceptance test.

Focus tests cover pure capture/verification/dismissal policy plus invalid/foreign handles and owned hidden windows. The target retains a query-only process handle and process creation time, PID, HWND and creating thread. HWND reuse within the same process/thread cannot be distinguished. Foreground ownership is re-checked before hiding; a different foreground app observed afterward is left alone. Windows foreground changes are not atomic, so this remains a best-effort request with explicit skipped/denied outcomes, not a security token or an automatic paste route. No successful foreground-stealing operation was exercised by these tests.

Source inspection also identified the blocking [framework accessibility gate](framework-gate.md). No native accessibility or physical-input acceptance milestone is marked passed.

## Reproducible raw search sample

The same fixed synthetic search example now supports `--jsonl`. A follow-up release-mode run on the environment above produced [`benchmarks/2026-11-11-search.jsonl`](benchmarks/2026-11-11-search.jsonl): one metadata record, 500 individual samples and one summary, verified by parsing every line as JSON. Results in microseconds were p50 3,799 / p95 4,475 / p99 4,828. This is a different run from the earlier summary, not a replacement of those observations. It still does not measure input/display latency, native focus cycles, frames, memory or power and does not close F09.

A bounded reuse investigation of Gauntlet is recorded in [ADR 0003](adr/0003-reuse-evaluation.md). Its inspected README explicitly says development ended; no code was imported and no runtime compatibility or security claim is inferred from that project's documentation.

## Follow-up: shared input and real UIA candidate

`pwsh -NoProfile -File scripts/validate.ps1 -Offline -NativeSmoke` passed: **133 unit tests,
one doctest**, workspace formatting/Clippy/desktop check, native startup and all **26 synthetic
interaction stages**. Input now has atomic 4,096-scalar size rejection and bounded undo/redo
(100 history entries / 1 MiB saved text across both stacks). Repeated marked-text updates commit
one undo transaction; cancelled compositions do not create history or destroy redo. Undo/redo
are inert while composition owns the field; clearing transient input drops its edit history.
The new native stages check rejection without query mutation, synthetic undo/redo keybindings,
marked-text undo deferral, and a single undo after composition commit. They do not exercise an
actual keyboard layout or external IME.

The separate newer-GPUI candidate compiled/built on Windows and passed native startup plus a
real owned-process UIA probe. Search is labelled, focusable/focused and exposes a value pattern;
the list exposes 9 realized items with logical position 1 of 10,000 and selected state. A real
UIA `SetValue` request updated the shared field and filtered the fixture to one expected row.
The saved [UIA observation](../experiments/gpui-accessibility/uia-observation.json) explicitly
keeps screen-reader and text-range/selection acceptance false. The shipping dependency pin
remains unchanged; see [ADR 0002](adr/0002-accessibility-investigation.md) for source identity,
compile changes, the initially incorrect index/container semantics and remaining blockers.

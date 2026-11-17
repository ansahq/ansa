# Framework accessibility gate — blocking

Status: **open/blocking before broad feature expansion**, updated 2026-11-12.

The design's current-main GPUI accessibility research is not evidence about the published version used here. Ansa pins `gpui = 0.2.2`, whose crates.io archive checksum is `979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707` (also recorded in Cargo.lock).

## Local source inspection

A case-insensitive search of that archive's `src/` and `Cargo.toml` for `accesskit`, `accessibility`, `UIAutomation`, and `IAccessible` found no accessibility bridge or semantic element API. Broader `accessible` matches were unrelated documentation/clipboard comments. There is no AccessKit dependency in this GPUI manifest. Ansa's custom input supports the pinned native text-input/IME contract, but that is not a screen-reader semantic implementation.

This is a bounded **source-inspection finding**, not a live NVDA/Narrator test and not a claim about all GPUI versions. Native two-frame and synthetic action tests do not validate accessibility trees, announcements, text navigation or IME behavior.

## Required decision

1. Evaluate an immutable newer upstream GPUI revision with its actual semantic bridge and platform crate dependencies. Keep it in a bounded feasibility slice, record the revision and license inventory, and demonstrate the same native interactions.
2. Validate useful search-field semantics, logical result count/position, stable selection announcements, action-menu navigation, text selection and error messages with NVDA/Narrator, then equivalent journeys on other declared OSes.
3. If the blocker cannot be bounded, perform the blueprint's same-slice Slint comparison and licensing review. Do not silently replace the selected Rust UI with a webview.

Until then, **do not claim the app is accessible, close F08/F10, declare production support, or use successful keyboard-only automation as substitute evidence**. Persistent settings/onboarding and other major productivity features should not outrun this framework gate.

## Bounded newer-revision experiment

The independent [candidate workspace](../experiments/gpui-accessibility/README.md) now builds
against upstream revision `a19e19440d4102668830a9da579c3ee71d2fbf1a`. Its actual Windows UIA
probe verifies search role/label/value/focus, list/item roles, selected state, logical position
1 of 10,000 and a `SetValue` request flowing through the shared input into filtered results.
Details and corrected index/container semantics are in [ADR 0002](adr/0002-accessibility-investigation.md).
This advances the structural spike, **not** the screen-reader/text-range or framework release gate.

The [12 November validation](validation-2026-11-12.md) additionally runs the actual launcher shell
on the same immutable candidate. Automated UIA checks cover text-range reads/selection/geometry,
atomic edit rejection, action search/invocation, stable identities across pinning, details/Back and
calculator errors. The small fixture also verifies actual polite live-region events and end/start
realization of the 10k list. Both backends pass the shell's 26-stage synthetic smoke. These bounded
checks do not establish NVDA/Narrator, real IME, RTL/mixed-DPI or cross-platform acceptance; the
main pin remains unchanged and the framework release gate remains open.

## Other pinned-build maintenance finding

`cargo report future-incompatibilities --id 1` identifies `proc-macro-error2 2.0.1` re-exporting a private `extern crate proc_macro` (future error E0365, Rust issue [127909](https://github.com/rust-lang/rust/issues/127909)). `cargo tree -p ansa-desktop -i proc-macro-error2 --locked --offline` resolves the chain as GPUI → `stacksafe 0.1.4` → `stacksafe-macro 0.1.4` → `proc-macro-error2 2.0.1`. Current Rust 1.99.1 builds pass; this is not a claim of indefinite future compiler compatibility. No cached dependency was patched or silently vendored.

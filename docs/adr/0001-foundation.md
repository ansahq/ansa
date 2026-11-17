# ADR 0001: first native feasibility slice

Date: 2026-11-11 · Status: provisional, pending native framework gate

Follow the supplied Rust/GPUI architecture. Call the original product **Ansa**, a working name, not a trademark-clearance claim. Start a small modular workspace rather than creating empty crates for every roadmap feature.

The domain, search and core crates do not depend on GPUI. Native integration is isolated behind a platform API. The first adapter targets the available Windows environment. GPUI is pinned to the published Apache-2.0 `0.2.2` release, not mutable upstream main; this release is not assumed to contain current-main accessibility work. CI selects Rust 1.99.1; local `rust-toolchain.toml` selects stable (currently 1.99.1 here) to reuse the installed toolchain. A fully immutable local toolchain remains an F03 follow-up. Cargo.lock captures transitive resolution.

Greenfield is a **provisional implementation choice**, not closure of backlog F02: no thorough Gauntlet reuse/license/maintainer evaluation has been completed. Preserve this open decision before production investment.

No Node runtime, IPC service, AI provider, remote search or sensitive persistence is introduced. Opening an app is a typed native operation. Application paths are checked at action time, and discovery ignores reparse points. The CLI cannot execute arbitrary shell expressions. Calculator input is bounded and parsed locally.

Ctrl+Space is a development default, not a completed shortcut recorder/onboarding flow. Failure to register must leave the app recoverable. Hidden work must be event-driven, not a recurring UI poll. The follow-up slice adds conservative best-effort focus return on user dismissal, with identity checks and explicit denied/skipped outcomes. It does not add automatic insertion; copy is never represented as paste success. Same-process/thread HWND reuse and non-atomic OS foreground races remain limitations, not security guarantees.

## Gate remains open

- Validate the pinned GPUI release on real Windows, macOS and Linux machines.
- Complete native input, IME, accessibility and screen-reader journeys.
- Capture focus/monitor/sleep lifecycle and 1,000-cycle results.
- Measure 10k-result query/frame budgets with named hardware and raw traces.
- Compare Slint if critical input/accessibility blockers cannot be bounded.

CI compilation and unit tests do not satisfy these gates. No production platform support or Raycast parity is implied.

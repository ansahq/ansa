# ADR 0001: first native feasibility slice

Date: 2026-11-11 · Status: provisional, pending native framework gate

Follow the supplied Rust/GPUI architecture. Call the original product **Ansa**, a working name, not a trademark-clearance claim. Start a small modular workspace rather than creating empty crates for every roadmap feature.

The domain, search and core crates do not depend on GPUI. Native integration is isolated behind a platform API. The first adapter targets the available Windows environment. GPUI is pinned to the published Apache-2.0 `0.2.2` release, not mutable upstream main; this release is not assumed to contain current-main accessibility work. Cargo.lock captures transitive resolution. The original local toolchain used mutable stable; ANS-7 tracks the exact pin and dependency inventory in the [build documentation](../build/). See the [integration validation record](../foundation-validation-2026-11-17.md) for the distinction between the selected pin, locally available toolchains, and actually executed checks.

The original greenfield choice and its subsequent scope decision are tracked in
[ADR 0003](0003-reuse-evaluation.md). A scope decision does not authorize source
copying: per-file and dependency license review must precede any proposed reuse.
Build, runtime, and accessibility evidence gaps remain release gates even when
greenfield is the chosen direction.

No Node runtime, IPC service, AI provider, remote search or sensitive persistence is introduced. Opening an app is a typed native operation. Application paths are checked at action time, and discovery ignores reparse points. The CLI cannot execute arbitrary shell expressions. Calculator input is bounded and parsed locally.

Ctrl+Space is a development default, not a completed shortcut recorder/onboarding flow. Failure to register must leave the app recoverable. Hidden work must be event-driven, not a recurring UI poll. The follow-up slice adds conservative best-effort focus return on user dismissal, with identity checks and explicit denied/skipped outcomes. It does not add automatic insertion; copy is never represented as paste success. Same-process/thread HWND reuse and non-atomic OS foreground races remain limitations, not security guarantees.

## Gate remains open

- Validate the pinned GPUI release on real Windows, macOS and Linux machines.
- Complete native input, IME, accessibility and screen-reader journeys.
- Capture focus/monitor/sleep lifecycle and 1,000-cycle results.
- Measure 10k-result query/frame budgets with named hardware and raw traces.
- Compare Slint if critical input/accessibility blockers cannot be bounded.

CI compilation and unit tests do not satisfy these gates. No production platform support or Raycast parity is implied.

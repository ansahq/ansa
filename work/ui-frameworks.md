# UI framework research and decision

Research date: 11 November 2026. These are public documentation/source observations and engineering recommendations, not measured benchmarks or observations from an installed Raycast binary. `main`/`master` source capabilities may not exist in the chosen released dependency version; pin and validate a release before implementation.

## Recommendation

Use **GPUI for the application UI, with a Rust domain core and Rust platform adapters**. This follows the user's clarified preference for an all-Rust application UI. The launcher, settings, notes, chat and extension-view renderer are Rust components; no embedded browser is required for those surfaces. Optional third-party JavaScript/TypeScript extensions can still execute in isolated compatibility workers and send declarative view data to the Rust renderer.

GPUI fits the custom, keyboard-driven visual design and provides a Rust rendering model with current desktop platform backends. Treat this as a conditional production choice: budget for an owned component library, text editing, accessibility semantics, and framework upgrades. Preserve a renderer-neutral domain model and extension view protocol. Run an initial two-week GPUI viability gate, with a further bounded two weeks for a focused Slint comparison or upstream fixes if necessary. Slint is the strongest no-WebView fallback if its declarative UI language and licensing are acceptable; Iced remains a candidate only after its pinned version demonstrates required accessibility. Do not silently switch to Tauri if the gate fails.

Raycast itself reports that v2 combines platform-specific native hosts, a shared React/TypeScript WebView frontend, Node, and Rust. It also reports rejecting Tauri during its evaluation because it needed finer native control. This makes web UI a credible route but leaves the exact shell behavior as the crucial validation risk. [Raycast technical deep dive, May 2026](https://www.raycast.com/blog/a-technical-deep-dive-into-the-new-raycast)

## Framework comparison

| Option | Useful established facts | Fit for this product | Main production burden | Decision |
|---|---|---|---|---|
| Tauri 2 + React/TS | Rust host; WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux; official tray/global-shortcut integrations; scoped frontend capabilities | Good hybrid alternative, but conflicts with the selected all-Rust UI approach | Real OS WebView testing, native panel/focus glue, renderer memory, privileged IPC design | Reference comparison; not selected |
| GPUI | Rust GPU UI; current main documents macOS, Windows and Linux; AccessKit integration; Apache-2.0 reusable GPUI components | Best fit for the selected custom launcher with Rust-authored UI | Pre-1.0 breaking changes, app widget kit, accessibility semantics, text/IME and OS integration QA; development tied to Zed | Primary choice, conditional on gate |
| Iced | Rust Elm-style architecture; Windows/macOS/Linux; GPU and software renderers; MIT | Clear state/update model and portable custom widgets | Upstream calls it experimental; accessibility work remains open; polish/rich editing/OS glue | Reconsider after accessibility proof, not default |
| Slint | Rust API with declarative Slint UI language; desktop platforms; Winit backend lists AccessKit and menus; multiple renderers | Strong no-WebView fallback with declarative controls; UI source is Slint DSL rather than exclusively Rust syntax | GPLv3/community/commercial license choice; advanced editor/extension rendering adapter; launcher-specific native glue | Main fallback if GPUI fails gate and DSL is accepted |
| egui/eframe | Rust immediate-mode UI; MIT OR Apache-2.0; AccessKit widget semantics and testing support | Excellent internal tools, diagnostics, lightweight prototypes | High styling and rich-text editing effort for this consumer UX; native integration still needed | Use for diagnostics if desired, not main UI |

Framework observations: [Tauri engines](https://v2.tauri.app/reference/webview-versions/), [Tauri repo/license](https://github.com/tauri-apps/tauri), [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md), [GPUI accessibility guide](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/_accessibility.rs), [Zed license boundaries](https://zed.dev/software-overview), [Iced README](https://github.com/iced-rs/iced), [Iced accessibility issue](https://github.com/iced-rs/iced/issues/552), [Slint backend documentation](https://github.com/slint-ui/slint/blob/master/docs/development/window-backend-integration.md), [Slint licensing options](https://slint.dev/get-started), [egui accessibility](https://github.com/emilk/egui/blob/main/docs/accessibility.md), [egui repository/license](https://github.com/emilk/egui).

The fit/risk columns are engineering judgments. No source establishes a head-to-head latency or memory ranking. Do not equate native rendering with native OS widgets or guaranteed performance. Do not label GPUI macOS-only or inaccessible: that would be stale. Likewise, do not infer complete application accessibility merely from an AccessKit dependency.

## Rust UI and optional React extension compatibility

A constrained JSX extension SDK can remain familiar without a web-rendered main app. A Raycast extension is not an arbitrary web page that can be mounted unchanged. Implement a React custom reconciler in the isolated compatibility worker that emits List/Grid/Detail/Form/ActionPanel view nodes; GPUI widgets render these nodes. Reimplement lifecycle, callback routing, storage, authorization and host calls separately. Source compatibility still needs a compatible JavaScript runtime and, for some extensions, Node packages or platform-specific commands. A Rust renderer neither guarantees compatibility nor grants redistribution permission. The official tutorial demonstrates TypeScript/TSX and npm development. [Raycast extension tutorial](https://developers.raycast.com/basics/create-your-first-extension)

Run third-party JavaScript in separate supervised workers and accept a bounded, validated declarative view tree plus opaque callback identifiers. The Rust renderer rejects unknown nodes, excessive depth, oversized properties and stale view revisions. No extension receives a GPUI pointer, a native window handle or arbitrary code execution in the UI process. Rust-authored extensions should preferably use a versioned WASM interface or the same out-of-process protocol rather than a Rust dylib ABI.

## Host boundaries

The Rust domain core must compile without GPUI. Define `LauncherHost` for show/hide, focus acquisition/restoration, monitor geometry, theme, menu/tray and display capabilities. Define OS service adapters separately for system-wide operations. Use native handles and small audited platform modules when generic window methods cannot meet the contract; do not scatter raw OS calls through widgets. GPUI entities own presentation state, while domain state and cancellation live in renderer-independent services.

The Rust extension broker owns capabilities and validates every privileged request. Process separation alone is not an OS sandbox for Node/native extensions. Treat trusted Node compatibility as a distinct mode until filesystem, network and process capabilities can be enforced with a supported sandbox. No UI framework supplies that security architecture automatically.

Retain one initialized launcher window; hide it on dismiss rather than paying renderer creation cost per hotkey. Make this an optimization to test against residency limits. Stop animations, polling and rendering while hidden. Keep index workers and extension workers independently idleable. Lazy-initialize notes/chat/settings after the shell is responsive. Avoid synchronous disk access or network work on the hotkey-to-first-frame path.

Own the global shortcut and tray adapters explicitly; GPUI window support does not establish universal Wayland activation or Raycast-equivalent window control. Provide tray-menu and CLI activation fallbacks where supported. Keep compositor capabilities visible in a diagnostics panel and avoid presenting unavailable actions as functioning commands.

## Two-week framework gate

Build the interaction slice on GPUI first. If it hits a framework-level blocker, evaluate the same slice on Slint before committing to extensive framework changes. Use release builds with a pinned toolchain and exact dependency versions or commit. Record machine, power mode, OS, compositor, screen scale and graphics driver. Do not benchmark empty windows.

The slice must contain a 10,000-item indexed command corpus, virtualized results, action panel, nested navigation, markdown detail, an editable form, a small rich-text note, light/dark themes, and a mock slow extension. Use identical data and interactions.

Pass gates (proposed product targets, not measured facts):

1. Hotkey to usable warm window: p95 <= 75 ms and p99 <= 120 ms on named reference hardware; report physical display/video latency separately from internal timestamps.
2. Input to correct local results: p95 <= 50 ms for the agreed corpus; no stale result replaces a newer query. Main-thread frame work p95 below one 60 Hz frame budget during typing and scrolling.
3. Hide/show/focus: 1,000 cycles without blank frames, wrong monitor placement, stuck keys, or lost restoration; test fullscreen windows, Spaces/virtual desktops, multi-monitor changes, sleep/resume, screen lock, elevated windows and remote sessions. Some cases require explicit documented capability limits.
4. Accessibility: complete keyboard-only flows; NVDA on Windows, VoiceOver on macOS, Orca on supported Linux sessions; stable focus/selection announcements, 200% text scaling, high contrast, reduced motion. Automated DOM checks are supplemental.
5. Text: Japanese/Chinese/Korean IME composition, dead keys, AltGr, emoji, Hindi shaping, RTL text and bidirectional selections. Enter/Escape must not execute/dismiss while the IME owns the event. Candidate windows must track the actual caret after display scaling and window movement.
6. Idle: no continuous repaint, no repeated wakeups from internal polling, no sustained core activity when hidden. Measure full process-tree memory, CPU and wakeups after 30 minutes; propose a 200 MiB core+renderer target before extensions/models, then adjust only from real baseline evidence. Report per-platform private/resident/proportional metrics rather than pretending they are interchangeable.
7. Recovery: kill renderer and extension worker independently; restore launcher without corrupting persisted state or rerunning actions. A hung extension cannot block input or hide/show.

Stop and reconsider GPUI if exact focus/panel or accessibility semantics require a permanent, wide fork, if text editing cannot pass the multilingual matrix, or if residency/latency cannot meet the budget after a bounded optimization pass. Select Slint only after its equivalent slice passes and its DSL/license choice is accepted. If Rust syntax everywhere is mandatory, Slint is not literally that: retain GPUI and fund missing work, or gate Iced on demonstrated accessibility rather than labeling it a safe fallback by default.

## Testing implications

Use Rust unit/property tests for reducer/state behavior and GPUI test contexts for component actions. Add deterministic view-tree and screenshot tests, then real bundled-app tests with Windows UIA, macOS AX and Linux AT-SPI. Screen-reader sessions are required even when tree snapshots pass. Keep all automation hooks test-only and exclude them from release builds. Test process isolation, native shortcuts and compositor behavior using the shipping OS adapters, not mocked widgets alone.

## Evidence supporting the GPUI choice and its limits

GPUI's current API is expressly pre-1.0, and upstream documentation says breaking changes are frequent. Pin a verified release or an exact tested commit; never use a wildcard version from a getting-started example. Its Apache-2.0 license does not cover every Zed application widget: Zed distinguishes reusable GPUI from GPLv3 application code. Avoid copying components based only on the GPUI crate's license. Validate each source file/component. [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md), [Zed software overview](https://zed.dev/software-overview)

The upstream text-input example implements grapheme navigation, UTF-8/UTF-16 conversion, marked text, selection notifications, replacement and caret geometry. These are useful composition/input foundations, not evidence that every IME, bidirectional edit or screen reader works. Build one reusable `TextInput` and an editor model with explicit grapheme/UTF-16 conversion boundaries; do not build independent input handling for every field. Treat rich-text notes as a substantial subsystem with undo transactions, composition-safe edits, selection, formatting, pasted content normalization and accessible text navigation. [GPUI input example](https://github.com/zed-industries/zed/blob/main/crates/gpui/examples/input.rs)

Current source declares `accesskit_macos`, `accesskit_windows`, and `accesskit_unix` in their respective platform crates. The GPUI accessibility guide requires stable element IDs and roles and explicit action handling for custom widgets. This establishes adapter wiring and authoring requirements, not end-to-end screen-reader quality. Require NVDA/VoiceOver/Orca acceptance on actual supported operating systems before release. Virtualized lists must expose logical count/position and maintain the focused node across result updates. [macOS manifest](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/Cargo.toml), [Windows manifest](https://github.com/zed-industries/zed/blob/main/crates/gpui_windows/Cargo.toml), [Linux manifest](https://github.com/zed-industries/zed/blob/main/crates/gpui_linux/Cargo.toml), [GPUI accessibility guide](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/_accessibility.rs)

The platform abstraction includes window activation policies, monitor events and different visibility observations by OS. Its comments explicitly note limits in detecting covered windows and display sleep on some platforms. Consequently, the app must stop its own unnecessary tasks on explicit hide and should not assume the renderer's visibility signal is a complete power-management policy. [GPUI platform abstraction](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/platform.rs)

Slint is the alternative if its declarative language and license model are acceptable and its editor/widget spike passes. Iced is not the safe default for a product with non-negotiable screen-reader support until the pinned version demonstrates it. Egui should not be dismissed as incapable; it simply shifts more of this particular product's visual/editor work onto the team.

## Remaining unknowns

No benchmark executed; no target binary inspected; no native-shell behavior measured. Actual release-specific GPUI accessibility completeness, platform IME quality, graphics-driver behavior and global shortcut coverage require the spike. Framework choice cannot remove Wayland policy restrictions, user permission boundaries, or the cost of reproducing an extension ecosystem.

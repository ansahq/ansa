# Ansa

An open-source, local-first keyboard launcher. Rust core, Rust GPUI interface, no account and no webview.

**Status: Phase 1 daily-launcher alpha slice, not a Raycast-compatible release.** The design and research in [`outputs/`](outputs/) remain the long-term plan. The 2026-11-19 slice adds a command/action registry, packaged app discovery, a bounded provider coordinator, persistent SQLite settings, structured quicklinks, an expanded calculator, licensed emoji search, onboarding state and an alpha harness. It does not close the human dogfooding, accessibility or release gates. See [`docs/alpha/`](docs/alpha/) for the alpha scope and known limitations.

## Development

Prerequisites: Rust 1.99.1, a C/C++ linker and the native dependencies required by [GPUI 0.2.2](https://docs.rs/gpui/0.2.2). On Windows use the Visual Studio C++ Build Tools and Windows SDK. The desktop renderer requires a supported GPU/driver.

Foundation validators also require Python 3.11+ (standard library only). The
[toolchain guide](docs/build/toolchain.md) describes pin updates and the current
host's broken named-toolchain installation; fallback checks are explicitly
separate from verification of the exact pin.

```sh
cargo test --locked
cargo run --locked -p ansa-desktop
cargo run --locked -p ansa-cli -- search terminal
```

Run `cargo test --workspace --locked` to include the UI text-model tests, or `cargo run --locked -p ansa-desktop -- --smoke-test` for an automatically closing native startup check. The local validation record is in [`docs/validation-2026-11-20.md`](docs/validation-2026-11-20.md).

The alpha CLI exposes the Phase 1 surfaces without the desktop UI:

```sh
cargo run --offline -p ansa-cli -- doctor
cargo run --offline -p ansa-cli -- calc "10 km to mi"
cargo run --offline -p ansa-cli -- emoji rocket
cargo run --offline -p ansa-cli -- alpha-smoke --cycles 1000
```

For the debug-only synthetic widget/action test, run `cargo run --locked -p ansa-desktop -- --interaction-smoke-test`. It uses a built-in fixture, runs 40 stages and exits without writing the clipboard or launching another app. It includes synthetic keybindings, input rejection, IME transaction boundaries, appearance toggling, setup navigation and emoji copy-mode parsing, but is not a physical keyboard, real-IME, screen-reader or focus-restoration test.

On Windows, `pwsh -File scripts/validate.ps1 -NativeSmoke -Offline` runs all local checks against already-downloaded dependencies. Omit `-Offline` when dependencies need fetching.

The default workspace builds the renderer-independent core and CLI. Build the native renderer explicitly with `-p ansa-desktop`. No web preview substitutes for the native application.

## Initial slice

- Deterministic command/app search with stable identities and exact/prefix/token/fuzzy tiers, a golden relevance corpus and held-out metrics.
- Virtualized GPUI results, original dark styling and an independently searchable action panel.
- Session-only pins/usage/recency signals; snapshot-bound actions reject changed targets.
- Typed command/action registry with opaque action references, kind capabilities, explicit confirmation and no automatic retry of uncertain effects.
- Windows classic and packaged application discovery, a bounded Shell Link parser, argument/working-directory-aware identity, broken-entry reporting and launch-or-switch outcomes.
- Bounded provider coordinator with revisions, deadlines, quotas, cancellation, partial/empty/failed states and stable selection.
- Persistent SQLite settings, user records, secret references, a rebuildable cache, transactional migrations and verified backup/restore.
- Structured quicklinks with RFC 3986 encoding, scheme/path/opener policy, preview/commit protection and versioned import/export.
- Bounded offline arithmetic, percentages, exponentiation and length/mass/time/temperature units; no shell evaluation.
- Licensed Unicode 16.0 emoji/symbol search with grapheme integrity, accessible variants and bounded recents/favorites.
- First-run onboarding state with recoverable shortcut conflicts, permission explanations and JSON persistence.
- Windows Ctrl+Space summon, with shortcut registration errors shown explicitly.
- Checked, best-effort Windows focus return on dismissal; no input injection or automatic paste.
- No query telemetry, cloud calls, extension execution, or clipboard-history capture.

The desktop now consumes the `DailyDriver` composition root for persisted quicklinks, a basic `Start Setup` onboarding surface and an explicit `emoji <query>` copy mode. Quicklink authoring, full emoji browsing/variant selection and complete onboarding screens remain follow-up work, and the two-week human dogfooding gate is open; see [`docs/alpha/known-limitations.md`](docs/alpha/known-limitations.md).

Use ↑/↓ and Page Up/Down to navigate, Enter to act, Ctrl+K for actions, and Escape to close the current layer. Type in the action panel to filter; Tab/Shift+Tab reach the footer control and leave the panel without trapping focus. Pins are marked ★ and last only until exit. Ctrl+Q exits. Root Escape hides only when a global shortcut was successfully registered; otherwise it stays recoverable. Calculator Enter copies the result; it does not inject text into another app.

Arithmetic uses finite IEEE-754 `f64`, not exact decimal finance arithmetic. The supported grammar is numbers, unary signs, `+ - * /`, postfix `%`, right-associative `^`, parentheses, and a bounded unit catalogue for length, mass, time and temperature, bounded to 256 characters and 32 nesting levels. A leading `=` explicitly requests calculation and is required for units and constants. Invalid arithmetic displays a typed error and cannot fall through to app execution; names such as `7-Zip` remain search queries. Results use a documented 12-significant-digit display policy and preserve units. Currencies, dates, time zones and natural language are explicitly unsupported.

### Appearance

The launcher is frameless: no native title bar, window border or minimize/maximize/close controls.
It uses a rounded, monochrome surface with a soft shadow on all sides. Glass is the default;
the material fills the window with no invisible shadow gutter. On Windows, a native rounded
region clips every child at the corners, and the compositor owns any available external shadow.
Per-pixel DirectComposition alpha is enabled without GPUI's native gradient backdrop. Text stays opaque.
On Windows 11, Glass uses a native compositor layer to blur the live scene behind the launcher.
On macOS, Glass uses the window's native vibrancy view. GPUI paints an 80% opaque themed tint over
the blur on both platforms. Desktop colors can show through, but text, menus and selected rows stay
opaque. The launcher does not read wallpaper files or capture the screen.

- **Glass: On/Off** or **Ctrl+Shift+G** toggles translucency without resizing the window.
- **Theme: Dark/Light** or **Ctrl+Shift+T** switches the entire palette, including both inputs,
  selection, caret, menus and error states.
- Dark is the default theme. Use `--light` to start light or `--opaque` to disable translucency.

For example:

```sh
cargo run --locked -p ansa-desktop -- --light
```

These choices last for this process only; they are not persistent settings. If Windows transparency
effects are off at setup, DirectComposition is disabled, or the native layer cannot be created,
the app reports **Glass: Unavailable** and stays opaque rather than showing clear glass.
Escape dismisses the launcher and Ctrl+Q exits, without needing native window controls.

## Boundaries

Foundation planning and reproducibility work is tracked separately from runtime
features: the [reference ledger](docs/reference/), [reuse evaluation](docs/adr/0003-reuse-evaluation.md),
and [build/inventory tooling](docs/build/). See the
[ANS-5/6/7 validation record](docs/foundation-validation-2026-11-17.md) for actual
checks and outstanding capture, audit, and cross-platform acceptance gaps.

Windows is the first integration target. macOS/Linux native adapters, signed installation, clipboard history UI, file indexing, extension execution and cross-device sync are **not implemented**. Packaged application discovery and classic launch-or-switch exist on Windows; packaged switching remains unsupported, icon extraction exists but desktop icon rendering is not wired, and `CatalogWatcher` is exercised by examples rather than the desktop. Reveal/copy-identity shell APIs exist. Focus return is conservative: exited/changed targets and unrelated foreground windows are left alone; Windows may deny activation, and same-process/thread HWND recycling cannot be completely detected. The desktop still keeps ranking signals in memory. Quicklink authoring, full emoji variant browsing and complete onboarding screens remain follow-up work. The renderer and core remain separable. Screen-reader support, IME behavior on real devices, DPI handling, real foreground switching and performance targets have not passed their release gates. See the blocking [framework accessibility gate](docs/framework-gate.md) before expanding the feature surface.

See [`docs/implementation-status.md`](docs/implementation-status.md) for verification and remaining work, and [`docs/adr/0001-foundation.md`](docs/adr/0001-foundation.md) for the provisional architecture decision.

An independent [newer GPUI accessibility experiment](experiments/gpui-accessibility/README.md) now builds and exposes structural search/list semantics through Windows UIA. It is not the main launcher, does not change its dependency pin, and has not passed screen-reader or accessible-text acceptance.

## Layout

`apps/desktop` · `apps/cli` · `crates/{domain,search,core,registry,coordinator,storage,quicklinks,emoji,onboarding,alpha,platform-api,platform-windows,ui-components,ui-gpui}`

## License

Apache-2.0. Upstream GPUI example attribution is preserved in `crates/ui-components/THIRD_PARTY.md`. No Raycast code or assets are included. Dependency licenses are separate; a release SBOM/license inventory remains a release gate.

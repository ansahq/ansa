> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.

# Ansa

An open-source, local-first keyboard launcher. Rust core, Rust GPUI interface, no account and no webview.

**Status: foundation prototype, not a Raycast-compatible release.** The design and research in [`outputs/`](outputs/) remain the long-term plan. This implementation starts the feasibility gate rather than claiming feature parity.

## Development

Prerequisites: Rust 1.99.1, a C/C++ linker and the native dependencies required by [GPUI 0.2.2](https://docs.rs/gpui/0.2.2). On Windows use the Visual Studio C++ Build Tools and Windows SDK. The desktop renderer requires a supported GPU/driver.

```sh
cargo test --locked
cargo run --locked -p ansa-desktop
cargo run --locked -p ansa-cli -- search terminal
```

Run `cargo test --workspace --locked` to include the UI text-model tests, or `cargo run --locked -p ansa-desktop -- --smoke-test` for an automatically closing native startup check. The local validation record is in [`docs/validation-2026-11-11.md`](docs/validation-2026-11-11.md).

For the debug-only synthetic widget/action test, run `cargo run --locked -p ansa-desktop -- --interaction-smoke-test`. It uses a built-in fixture, runs 30 stages and exits without writing the clipboard or launching another app. It includes synthetic keybindings, input rejection, IME transaction boundaries and appearance toggling, but is not a physical keyboard, real-IME, screen-reader or focus-restoration test.

On Windows, `pwsh -File scripts/validate.ps1 -NativeSmoke -Offline` runs all local checks against already-downloaded dependencies. Omit `-Offline` when dependencies need fetching.

The default workspace builds the renderer-independent core and CLI. Build the native renderer explicitly with `-p ansa-desktop`. No web preview substitutes for the native application.

## Initial slice

- Deterministic command/app search with stable identities and exact/prefix/fuzzy tiers.
- Virtualized GPUI results, original dark styling and an independently searchable action panel.
- Session-only pins/usage signals; snapshot-bound actions reject changed targets.
- Shared text input with native composition plumbing and Unicode grapheme navigation.
- Composition-safe undo/redo, bounded transient edit history, and atomic rejection of queries exceeding 4,096 Unicode scalar values.
- Bounded, offline arithmetic parser; no shell evaluation.
- Windows Start Menu shortcut discovery and native opening, without interpolating shell commands.
- Windows Ctrl+Space summon, with shortcut registration errors shown explicitly.
- Checked, best-effort Windows focus return on dismissal; no input injection or automatic paste.
- No query telemetry, cloud calls, extension execution, or clipboard-history capture.

Use ↑/↓ and Page Up/Down to navigate, Enter to act, Ctrl+K for actions, and Escape to close the current layer. Type in the action panel to filter; Tab/Shift+Tab reach the footer control and leave the panel without trapping focus. Pins are marked ★ and last only until exit. Ctrl+Q exits. Root Escape hides only when a global shortcut was successfully registered; otherwise it stays recoverable. Calculator Enter copies the result; it does not inject text into another app.

Arithmetic uses finite IEEE-754 `f64`, not exact decimal finance arithmetic. The supported grammar is numbers, unary signs, `+ - * /`, and parentheses, bounded to 256 characters and 32 nesting levels. A leading `=` explicitly requests calculation. Invalid arithmetic displays an error and cannot fall through to app execution; names such as `7-Zip` remain search queries.

### Appearance

The launcher is frameless: no native title bar, window border or minimize/maximize/close controls.
It uses a rounded, monochrome surface with a soft shadow on all sides. Glass is the default;
the material fills the window with no invisible shadow gutter. On Windows, a native rounded
region clips every child at the corners, and the compositor owns any available external shadow.
Per-pixel DirectComposition alpha is enabled without GPUI's native gradient backdrop. Text stays opaque.
The glass tint is 78% opaque (22% see-through); secondary text is contrast-tuned for both themes,
while menus and selected rows stay solid for reliable interaction.

- **Glass: On/Off** or **Ctrl+Shift+G** toggles translucency without resizing the window.
- **Theme: Dark/Light** or **Ctrl+Shift+T** switches the entire palette, including both inputs,
  selection, caret, menus and error states.
- Dark is the default theme. Use `--light` to start light or `--opaque` to disable translucency.

For example:

```sh
cargo run --locked -p ansa-desktop -- --light
```

These choices last for this process only; they are not persistent settings. Compositor support
is required for transparency. The surface is tinted for readability; it does not apply native blur.
If DirectComposition is disabled or the native setup fails, the app reports **Glass: Unavailable**
and uses an opaque surface rather than claiming glass is enabled.
Escape dismisses the launcher and Ctrl+Q exits, without needing native window controls.

## Boundaries

Windows is the first integration target. macOS/Linux native adapters, packaged Windows app discovery, launch/switch semantics, configurable shortcuts, persistent settings, clipboard history, indexing and extension compatibility are **not implemented**. Focus return is conservative: exited/changed targets and unrelated foreground windows are left alone; Windows may deny activation, and same-process/thread HWND recycling cannot be completely detected. The renderer and core remain separable. Screen-reader support, IME behavior on real devices, DPI handling and performance targets have not passed their release gates. See the blocking [framework accessibility gate](docs/framework-gate.md) before expanding the feature surface.

See [`docs/implementation-status.md`](docs/implementation-status.md) for verification and remaining work, and [`docs/adr/0001-foundation.md`](docs/adr/0001-foundation.md) for the provisional architecture decision.

An independent [newer GPUI accessibility experiment](experiments/gpui-accessibility/README.md) now builds and exposes structural search/list semantics through Windows UIA. It is not the main launcher, does not change its dependency pin, and has not passed screen-reader or accessible-text acceptance.

## Layout

`apps/desktop` · `apps/cli` · `crates/{domain,search,core,platform-api,platform-windows,ui-components,ui-gpui}`

## License

Apache-2.0. Upstream GPUI example attribution is preserved in `crates/ui-components/THIRD_PARTY.md`. No Raycast code or assets are included. Dependency licenses are separate; a release SBOM/license inventory remains a release gate.

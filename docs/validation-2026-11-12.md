# Accessibility feasibility validation — 2026-11-12

This continues the [11 November record](validation-2026-11-11.md). Same local Windows test
environment and immutable GPUI candidate revision. The default main-workspace GPUI pin remains
unchanged. No screen reader or external IME was enabled, and no production gate is closed here.

## Shared text input

The newer-backend feature now owns a single labelled/focusable semantic input with a TextRun,
current text selection and explicit SetValue, SetTextSelection and ReplaceSelectedText handlers.
The candidate's former outer search wrapper was removed to avoid duplicate semantic fields and
duplicate focus registration. Native IME editing remains the same owned widget/model.

`cargo test --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline
--target x86_64-pc-windows-msvc -p ansa-ui-components` passed **38 tests**, including 13
candidate-specific tests. The default widget configuration separately passed its **25 tests**.
Tests cover foreign/unpublished/stale selection targets, invalid indices, reversed selection,
scalar/UTF-8/UTF-16 conversions, atomic size rejection, history and composition ownership. A
review found and corrected an intrabyte rounding case so an offset inside a scalar rounds down
without splitting UTF-8.

## Real owned-process Windows UIA probe

Command:

```powershell
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 `
  -OutputPath experiments/gpui-accessibility/uia-observation-2026-11-12.json
```

The [UTF-8 observation](../experiments/gpui-accessibility/uia-observation-2026-11-12.json)
records successful checks through actual Windows UIA:

- Search is a labelled, focused/focusable Edit with a value pattern.
- SelectionItem.Select selects a row without executing it or taking text focus.
- Last/first navigation realizes the correct virtualized rows, reports positions 10,000 and 1
  in the 10k fixture, and retains the query/focus.
- Each navigation produced an actual UIA live-region event naming the selected result.
  AccessKit's Status role alone does not imply a live setting; the owned status element sets
  `Live::Polite` explicitly. This verifies event delivery, not what NVDA/Narrator would speak.
- ValuePattern.SetValue filters the core results.
- TextPattern.DocumentRange returns the expected Unicode text, including an astral emoji,
  combining accent and CJK character.
- Character-range movement selects the emoji without splitting its UTF-8/UTF-16 representation;
  range.Select round-trips through the model, GetSelection and GetText.
- Selection bounds have positive area, lie inside the field and support a point-to-text range
  within the selected scalar.
- An oversized SetValue request is acknowledged through a status update and leaves both text
  and selection unchanged. The probe waits on UIA events rather than sleep-based polling.

The probe only queries the fixture process it starts, never other applications' trees. It closes
that process afterward. It does not invoke clipboard actions or external application launches.

## Synthetic keyboard/virtualization smoke

The candidate's `--interaction-smoke-test` passed **8 stages**: Down, Page Down, Alt+End,
Alt+Home, actual selected-row realization at both ends of the 10k list, retention of text focus
and query, and deferral of arrow navigation while marked-text composition owns the field.
The realization instrumentation exists only in debug builds. Internal synthetic keystrokes are
not physical-keyboard or real-IME acceptance.

## Actual launcher shell

The optional `launcher-shell` feature builds `ansa-launcher-accessibility-candidate`, using the
same `crates/ui-gpui` renderer through `run_with_application(gpui_platform::application())`.
Version differences are feature-gated; the default dependency pin is unchanged. The debug-only
`--a11y-fixture` skips foreground capture, discovery/background catalog and hotkey registration,
loads three built-ins and reports its owned HWND after a frame. It does not auto-run actions.

```powershell
cargo +stable build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc --features launcher-shell --bins
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -LauncherShell -OutputPath experiments/gpui-accessibility/launcher-uia-observation-2026-11-12.json
```

The [shell observation](../experiments/gpui-accessibility/launcher-uia-observation-2026-11-12.json)
passed all assertions: three fixture rows; UIA row selection without executing; independent action
search; invocable pin/unpin menu commands; stable result RuntimeId across reordering; explicit
About execution and Back with returned field focus; exposed division-by-zero reason; and the
same Unicode text-range, selection, geometry and oversized-rejection checks as the small fixture.
Menu commands deliberately omit selected-state metadata, which would suppress UIA InvokePattern.
Result rows retain SelectionItem semantics and select-only accessibility Click behavior.

The actual shell's **26-stage interaction smoke passed on the candidate**. After final formatting
and builds, the small fixture's **8-stage smoke**, both UIA probes and the **38 widget tests** passed
again. Candidate Clippy passed with `--features launcher-shell --all-targets -- -D warnings`.
`pwsh -NoProfile -File scripts/validate.ps1 -Offline -NativeSmoke` passed the default workspace's
**133 unit tests, one doctest, Clippy, startup smoke and 26-stage interaction smoke**. The known
old-backend `proc-macro-error2` future-compatibility warning remains; no dependency was patched.

## Remaining boundaries

### Frameless window and optional glass appearance follow-up

Removed native window chrome on both backends and added a footer glass toggle, Ctrl+Shift+G and
the `--glass` startup flag. Opaque remains the default; the choice is session-only. Glass uses
GPUI's native `Blurred` appearance and a translucent dark surface, without reducing text opacity.
Windows can supply a default caption for a zero-style popup, so the checked native adapter now
explicitly sets WS_POPUP, clears caption/frame/control styles and refreshes the non-client frame
only on the window's owning UI thread. Invalid handles are rejected.

The shell UIA probe asserts absence of native caption/frame/system-menu/minimize/maximize style
bits, invokes glass on/off and checks retained search focus. The dated shell JSON was refreshed
with these passing assertions. This does not measure perceived blur quality or validate all
Windows transparency/accessibility settings. The full default validation passed again, and the
expanded **28-stage** interaction smoke passed on both backends starting with `--glass`.
Candidate Clippy also passed again with warnings denied. Existing 26-stage counts above record
the earlier run before these two appearance stages were added.

### Monochrome rounded-surface follow-up

Replaced whole-window acrylic with a fixed transparent viewport, a rounded 12px surface and a
soft shadow contained within a 16px gutter. Glass is now on by default; `--opaque` opts out.
Neither glass nor theme toggles call native background/geometry APIs. The new light/dark toggle
(footer, Ctrl+Shift+T, `--light` startup option) updates the shell and both input palettes together.
All theme tokens are neutral; errors retain explicit text labels rather than relying on color.

Latest checks: **135 default unit tests and one doctest passed**, default Clippy/checks passed,
**40 candidate widget tests passed**, candidate build and Clippy passed. The candidate's expanded
**30-stage smoke passed** both with default dark/glass and with `--light --opaque`.
Palette tests verify neutral saturation and at least 4.5:1 text contrast on opaque palette surfaces;
this does not certify translucent contrast over arbitrary desktop content.

The full default native validation could not finish: Windows denied replacement of the running
`target/debug/ansa-desktop.exe`. No user process was terminated. The extended UIA probe includes
default-glass, light/dark round-trip and stable window/input bounds assertions, but the latest
run was blocked by Windows denying foreground activation of the owned fixture. Previous saved
JSON observations remain historical results, not evidence for the final appearance changes.
Close the running desktop and rerun native validation and the shell probe; visual acceptance of
shadow, transparency and clipping across DPI/compositor settings remains open.

### Higher-transparency glass follow-up

Reduced glass tint opacity from 92% to 78%, leaving the viewport, rounded clipping and shadow
unchanged. Main text/caret remain opaque; secondary labels were contrast-tuned in both palettes,
and the dark text-selection wash was softened. Menus and selected rows remain opaque.
Added a neutral sRGB source-over test checking at least 4.5:1 text, secondary-label and caret
contrast over black and white desktop extremes, plus primary-text contrast under selection.
This is a palette/compositing calculation, not real-desktop visual or accessibility acceptance.

The **28 default widget tests passed**, including that new contrast test. Both desktop backends
built, candidate Clippy passed, and the default launcher's **30-stage smoke passed**. The prior
executable lock no longer prevented rebuilding the default app. The UIA/visual release gates
above were not reclassified by these checks.

### Native compositor, clipping and hitbox correction

The user's screenshot disproved the earlier assumption that lowering the GPUI surface alpha
was enough: the visible surface remained opaque, some corner backdrops were square and the
16px shadow gutter enlarged the invisible input region. Removed that gutter and the custom
layout shadow. The surface now fills the native client area. Native rounded-window clipping is
reapplied on size/DPI changes so child backgrounds cannot escape at the corners; native DWM
configuration owns any available shadow without adding a layout/input margin.

The Windows adapter explicitly disables GPUI's accent-gradient backdrop and enables client
alpha for the DirectComposition window using an off-client DWM blur region. It never applies
whole-window opacity to text. Unsupported/failed alpha setup now displays Glass: Unavailable
and switches to an opaque surface, rather than claiming translucency is enabled.

The first **default-backend native geometry probe failed**, revealing another real difference:
old GPUI reserves resize-frame insets even on this non-resizable popup. A checked own-thread
window subclass now handles WM_NCCALCSIZE for this launcher only, accepting the whole client
rectangle; all other messages retain GPUI behavior and the subclass removes itself on destruction.
No cached upstream source was changed.

After that fix, the probes passed on both backends:

- [Default native observation](../experiments/gpui-accessibility/default-native-appearance-observation-2026-11-12.json):
  client/window dimensions match, native chrome absent, DirectComposition enabled, all four
  corner pixels excluded and the native region reaches all four edges.
- [Candidate appearance observation](../experiments/gpui-accessibility/appearance-observation-2026-11-12.json):
  the same native checks plus no side content gutter and unchanged window/list bounds when
  glass is toggled. No screen capture or unrelated app tree was read.

The final default `scripts/validate.ps1 -Offline -NativeSmoke` run passed **138 unit tests, one
doctest, Clippy, startup smoke and the 30-stage interaction smoke**. The candidate rebuilt and
passed its 30-stage smoke (including light mode); candidate Clippy passed again after the final
native-frame adapter. Two new native tests cover region shape,
absence of insets, invalid radii/handles, disabled composition and thread ownership.

Both observations deliberately set `DesktopPixelTransparencyVerified=false`. Native APIs and
geometry checks are not a screenshot comparison. Visible translucency, shadow quality and
mixed-DPI clipping still need confirmation on the user's desktop after relaunching the new build.

### General unverified release gates

AccessKit text character units are Unicode scalars; the widget's ordinary arrow editing remains
grapheme-aware. Word-start metadata is omitted for runs above the upstream u8 index capacity.
The geometry sample above is an LTR fixture and an emoji selection, not a passed RTL/mixed-bidi,
complex shaping or mixed-DPI geometry matrix. Full offscreen provider enumeration, screen-reader
speech/focus behavior, multilingual IME, cross-platform adapters, performance and lifecycle
acceptance remain open. The saved observation intentionally leaves general screen-reader and
text-range/selection **release acceptance** false despite the bounded automated smoke checks.

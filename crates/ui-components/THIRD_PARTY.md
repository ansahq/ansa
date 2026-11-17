# Third-party notices

This crate contains code adapted from GPUI.

## GPUI `examples/input.rs`

- Project: GPUI / Zed (`https://github.com/zed-industries/zed`)
- Version used: `gpui = "=0.2.2"`, published on crates.io
- Local pinned source: `%USERPROFILE%\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-0.2.2\examples\input.rs`
- Copyright: Zed Industries, Inc. and GPUI contributors
- License: Apache License 2.0 (`https://www.apache.org/licenses/LICENSE-2.0`)

`crates/ui-components/src/text_input.rs` adapts the example's single-line editor
state machine, UTF-16 selection conversion, grapheme boundary handling, IME
marked-text plumbing (`EntityInputHandler`), custom `Element` painting, caret
and selection geometry, mouse selection and keyboard actions.

Modifications from the upstream example:

- Replaced the free-form example view with the reusable public `TextInput`
  entity and a headless `TextModel` with unit tests.
- Added a typed `Changed(String)` event, `is_composing()`, `text()` and
  `clear(window, cx)`.
- Scoped all keybindings to the `TextInput` key context and selected Ctrl or
  Command modifiers per platform.
- Added a dark theme with a purple accent, 32 px field height, 20 px text and
  the Ansa search placeholder; the field itself paints no background.
- Sanitized inserted text to a single line.
- Corrected the IME candidate selection mapping for `new_selected_range`, which
  upstream offsets from the replaced document range rather than from the new
  text.
- Guarded mouse-to-character mapping for the empty placeholder state.

## GPUI accessibility guide and settings input (`upstream-candidate` only)

- Project: GPUI / Zed (`https://github.com/zed-industries/zed`)
- Revision inspected: `a19e19440d4102668830a9da579c3ee71d2fbf1a`
- Source read: `crates/gpui/src/_accessibility.rs` (text runs and synthetic
  children) and `crates/settings_ui/src/components/input_field.rs` (text-run
  shape, UTF-8 character lengths and selection positions)
- Copyright: Zed Industries, Inc. and GPUI contributors
- License: Apache License 2.0 (`https://www.apache.org/licenses/LICENSE-2.0`)

`crates/ui-components/src/text_input/a11y.rs` reimplements the documented
scalar/UTF-8/UTF-16 conversions and text-run publication for Ansa's byte-offset
model. It is compiled only with the `upstream-candidate` feature; the default
crates.io pin does not contain these APIs. No upstream GPUI source is vendored
or modified here.

No other third-party source is vendored in this crate.

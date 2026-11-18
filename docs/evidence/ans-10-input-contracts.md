# ANS-10 / F05 — shared input and text contracts

Date: 2026-11-18
Ticket: ANS-10 `[F05] Harden shared input widget and text contracts`
Branch: `ans-foundation-evidence`
Status: implementation and automated evidence complete; **F05 remains open** because the
physical IME, assistive-technology and scaled RTL checks in the acceptance criteria cannot
be run in this environment.

## Scope of this change

Files owned and changed:

- `crates/ui-components/src/text_input.rs`
- `crates/ui-components/src/text_input/model.rs`
- `crates/ui-components/src/text_input/a11y.rs`
- `crates/ui-components/src/text_input/contracts.rs` (new)
- `crates/ui-components/src/lib.rs` (additive exports)
- `crates/ui-components/tests/input_contracts.rs` (new)
- this document

No dependencies were added, no root workspace file was edited, and no OS clipboard or
network access was used by tests.

## Implemented contracts

### 1. Grapheme-aware editing and movement

- Caret placement (`move_to`), selection extension (`select_to`), Backspace, Delete and
  pointer-driven placement snap to grapheme-cluster boundaries. A position inside a
  cluster rounds down for movement/selection and up for the post-edit caret, so a caret is
  never stored inside a cluster.
- Word movement uses `unicode-segmentation` word boundaries, not ASCII whitespace. New
  actions: WordLeft/WordRight, SelectWordLeft/SelectWordRight, DeleteWordBackward/
  DeleteWordForward. Bindings are Control+arrow / Control+Shift+arrow / Control+Backspace /
  Control+Delete on Windows, and Option equivalents on macOS; the `TextInput` key context
  is unchanged.
- Tests cover combining marks (`e` + U+0301), flags (`🇯🇵`, `🇮🇳`), skin-tone emoji
  (`👍🏽`), ZWJ family and rainbow sequences, and Devanagari clusters (`क्`, `क्ष`).

### 2. Undo/redo, selection and coalescing

- Every snapshot stores text, selection range and selection direction. Undo and redo
  restore all three; redo is discarded by a newer committed edit.
- Coalescing policy is explicit: one `replace_text`, accessibility replacement, cut or paste
  is one transaction; all marked updates between the first composition update and its
  commit/cancel are one transaction. Cancel records nothing and keeps redo.
- History stays bounded by `MAX_HISTORY_ENTRIES` (100) and `MAX_HISTORY_BYTES` (1 MiB),
  including composition commits.
- Tests: `undo_and_redo_restore_text_and_selection_direction`,
  `composition_commit_undoes_to_the_pre_composition_selection`,
  `composition_commits_count_as_single_bounded_transactions`,
  `undo_and_redo_are_exact_on_random_history`.

### 3. IME composition as one transaction; action hierarchy

- `set_marked`/`replace_and_mark_text`, commit (`replace_text`/`unmark_text`) and cancel are
  separate paths. `TextModel::cancel_composition` restores the exact pre-composition text,
  selection and direction without adding history.
- New typed query for hosts: `EditIntent` (`Undo`, `Redo`, `Copy`, `Cut`, `Paste`,
  `MoveCaret`, `ExtendSelection`, `Backspace`, `Delete`, `Enter`, `Escape`),
  `EditOutcome` (`Execute`, `DeferToComposition`, `CancelComposition`) and
  `EditIntents` (`TextInput::edit_intents`, `is_action_allowed`, `action_outcome`,
  `cancel_composition`).
- While composing, every intent except Escape defers to the input method; Escape returns
  `CancelComposition`, so a host dismisses the composition instead of the launcher view.
  All editing/navigation/clipboard action handlers now return early while composing, so a
  key cannot mutate marked text behind the IME's back.
- Tests: `ime_updates_commit_as_one_undo_step_on_unmark`,
  `ime_updates_commit_as_one_undo_step_on_replace_text`,
  `cancel_composition_restores_text_selection_and_direction`,
  `cancel_composition_keeps_redo_history`, `composition_defers_every_action_intent`.

### 4. UTF-16 host boundary

- `offset_from_utf16`, `offset_to_utf16`, `range_from_utf16` and `range_to_utf16` clamp
  out-of-bounds offsets, normalise reversed ranges and round offsets inside a surrogate pair
  down to the start of the scalar. Host ranges therefore never slice mid-codepoint.
- Fixed two real conversion defects: an offset inside a surrogate previously mapped past the
  astral character instead of to its start, and a byte offset inside a multi-byte scalar
  previously rounded up.
- `text_for_range`, `bounds_for_range`, `selected_text_range`, `marked_text_range`,
  `unmark_text`, `replace_text_in_range`, `replace_and_mark_text_in_range`,
  `character_index_for_point` and the feature-gated `set_selected_text_range` all use these
  conversions or clamp the layout index before conversion.
- Tests: `utf16_conversion_rounds_down_inside_surrogate_pairs`,
  `host_ranges_clamp_and_normalise_instead_of_slicing`,
  `composition_selection_rounds_to_grapheme_boundaries`, plus property tests that inject
  reversed and out-of-bounds UTF-16 ranges.

### 5. Clipboard without the OS clipboard

- Pure model operations: `selected_text` (copy), `cut_selection` (one undo transaction) and
  `paste_text` (selection replacement, size checked atomically). They return `None` while a
  composition owns the text.
- The widget writes to/reads from the OS clipboard only at the action boundary; no test
  touches the OS clipboard.
- Tests: `model_clipboard_operations_round_trip_without_an_os_clipboard`,
  `clipboard_operations_are_deferred_during_composition`,
  `paste_and_marked_text_reject_at_exactly_one_over_the_limit`.

### 6. Atomic oversized-edit rejection

- The scalar limit remains 4,096. Checking happens before mutation, so a rejected edit leaves
  text, selection, marked range and history untouched. Marked-text, paste and accessibility
  replacements use the same check. `EditRejected::message()` carries the user-visible reason.
- Tests cover exactly-at-limit and one-over-limit ASCII, astral and grapheme-heavy text,
  oversized marked updates and oversized paste.

### 7. Dead keys and AltGr (model level only)

- Combining marks, `€`, `@`, `ß`, `æ`, `œ` and similar AltGr characters are inserted as
  ordinary text transactions; a combining mark bonds to its base cluster and deletes with it.
- Physical keyboard layout behaviour is **not** tested; see the unverified list.

### 8. RTL and mixed bidi

- All stored indexes remain logical UTF-8 offsets. Movement iterates logical grapheme
  boundaries in increasing/decreasing order and never uses visual order.
- AccessKit `TextDirection` is now derived from the first strong character of the run
  (Hebrew, Arabic, Syriac, Thaana, NKo, Samaritan, Mandaic, Adlam and presentation ranges
  map to RTL; no strong character defaults to LTR) instead of always left-to-right.
- Tests: `movement_through_mixed_bidi_text_is_logical`,
  `text_direction_uses_the_first_strong_character`,
  `logical_selection_is_independent_of_reported_scale`.
- Visual caret/selection placement on real RTL hardware and at scaled resolutions is
  **not** verified.

### 9. Accessibility value, selection and text ranges

- The feature-gated path publishes one `SearchInput`/`TextInput` node with the value,
  placeholder, AccessKit selection and one `TextRun` child carrying UTF-8 character lengths,
  word starts and optional geometry.
- Selection indices are AccessKit character (Unicode scalar) indices and remain logical for
  mixed Hebrew/Arabic + Latin text. Scalars, not graphemes, are the AccessKit character unit,
  which is documented in the module and tested.
- Geometry converts logical bounds to physical pixels with the window scale factor. Tests
  pin 100 %, 150 % and 200 % conversion and show that selection mapping is independent of
  scale.
- No screen-reader session was run; see unverified items.

### 10. Property-style tests

`text_input/contracts.rs` uses a deterministic xorshift PRNG with seeds `0x5eed1234`,
`0xa11ce000`, `0xdeadbeef` and 4,000 operations per seed (12,000 total) over a curated
Unicode corpus. Each operation asserts:

- text length ≤ `MAX_TEXT_SCALARS`;
- selection and marked ranges in bounds and on UTF-8 boundaries; selection on grapheme
  boundaries;
- history entry/byte bounds and composition/undo consistency;
- UTF-16 round trip for selection endpoints;
- rejected edits changed nothing;
- no panic for reversed, out-of-range and mid-surrogate host ranges.

A second test drives 600 random edit/undo/redo steps per seed against a shadow state list and
asserts exact text and selection restoration.

## Verification commands and results

Run on Windows 11, Rust 1.99.1, branch `ans-foundation-evidence`.

| Command | Result |
| --- | --- |
| `cargo fmt -p ansa-ui-components -- --check` | exit 0 |
| `cargo test -p ansa-ui-components` | 45 lib tests passed, 4 integration tests passed, 0 failed; 1 doctest ignored |
| `cargo clippy -p ansa-ui-components --all-targets -- -D warnings` | exit 0 |
| `cargo check -p ansa-ui-gpui` | exit 0 |
| `cargo test --manifest-path experiments/gpui-accessibility/Cargo.toml -p ansa-ui-components` | 61 lib tests passed (includes 16 `upstream-candidate`/AccessKit tests), 4 integration tests passed, 0 failed |
| `cargo clippy --manifest-path experiments/gpui-accessibility/Cargo.toml -p ansa-ui-components --all-targets -- -D warnings` | exit 0 |
| `cargo test -p ansa-ui-components --features upstream-candidate` | not supported against the crates.io GPUI 0.2.2 pin: 12 pre-existing unresolved-API errors (AccessKit imports, `set_selected_text_range`, `text_length_utf16`, `is_a11y_active`, `role`, changed `ShapedLine::paint` signature). The error set is identical before and after this change. The feature is exercised in the pinned newer-GPUI experiment workspace instead. |

Focused test names for review:

- `text_input::model::tests::*` (grapheme, word, composition, UTF-16, clipboard, limits)
- `text_input::contracts::random_editing_sequences_preserve_model_invariants`
- `text_input::contracts::undo_and_redo_are_exact_on_random_history`
- `text_input::a11y::tests::*` (feature build only)
- `tests/input_contracts.rs::composition_defers_every_action_intent`

## Public API and behaviour notes

Additive, source-compatible exports from `ansa_ui_components`:

- `EditIntent`, `EditOutcome`, `EditIntents`.
- `TextInput::edit_intents`, `is_action_allowed`, `action_outcome`,
  `cancel_composition`.
- New `actions!` variants for word movement and word deletion, and their keybindings inside
  the `TextInput` key context.

Intentional behaviour fixes:

- Pointer/editing positions inside a grapheme now snap to a cluster boundary instead of
  storing a scalar or byte offset.
- Host UTF-16 ranges that are reversed or out of bounds now clamp/normalise instead of
  producing a reversed or out-of-range slice.
- Editing, navigation and clipboard actions are no-ops during an active composition; the
  widget still emits the same `Changed` events for actual text changes.
- `TextInput::cut` now goes through the model's cut operation, so it is one undo transaction.

## Unverified — do not claim these pass

- Physical CJK IME confirm and cancel on Windows/macOS (candidate window, composition commit
  keys, Escape handling) on real hardware.
- Hindi/Devanagari shaping and IME behaviour with a real input method.
- Visual caret and selection placement for RTL and mixed bidi text at 100 %, 150 % and 200 %
  scaling on a real display.
- Dead-key and AltGr sequences on physical European keyboard layouts.
- NVDA/Narrator UIA text-range probes and any assistive-technology session behaviour
  (SetValue, ReplaceSelectedText, SetTextSelection) beyond the published AccessKit node and
  model-level conversions.
- OS clipboard integration and clipboard history; only the pure model operations are tested.
- The `upstream-candidate` feature against the crates.io GPUI pin (unsupported by design;
  use the pinned experiment workspace).

Because these are explicit acceptance-criteria items, ANS-10 is **not** marked done or
closed here.

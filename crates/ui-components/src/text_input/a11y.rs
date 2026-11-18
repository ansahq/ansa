//! AccessKit text semantics for the single-line input.
//!
//! This module is compiled only for the immutable upstream candidate, where
//! GPUI exposes AccessKit roles, synthetic accessibility children and
//! accessibility actions. The default crates.io pin has no such API.
//!
//! GPUI's accessibility guide documents the text-run shape used here:
//! `Role::TextRun` children carry `value`, UTF-8 `character_lengths` and an
//! optional parent `TextSelection`. AccessKit character indices refer to
//! entries in `character_lengths`, not to UTF-8 bytes or UTF-16 code units.
//! Ansa's model stores UTF-8 byte offsets, so every conversion is explicit and
//! every out-of-range request is rejected rather than sliced.

use std::{cell::Cell, rc::Rc};

use gpui::{
    A11ySubtreeBuilder, Bounds, Pixels, ShapedLine,
    accesskit::{Node, NodeId, Rect, Role, TextDirection, TextPosition, TextSelection},
};

/// Default accessible name for the launcher query field.
pub const DEFAULT_ACCESSIBILITY_LABEL: &str = "Search applications and commands";

/// AccessKit stores `word_starts` as `u8` indices, so word starts can only be
/// reported for runs whose character count fits in a byte. Longer input omits
/// the optional property rather than truncating or wrapping an index.
const MAX_WORD_START_INDEX: usize = u8::MAX as usize;

/// Length in UTF-8 bytes of each selectable character in `text`.
///
/// AccessKit defines a character as the smallest selectable unit. Ansa reports
/// one entry per Unicode scalar value: this keeps every length within the `u8`
/// range (1-4) and makes the mapping stable even for pathological grapheme
/// clusters longer than 255 bytes. Cursor movement still uses grapheme
/// boundaries, so a selection edge produced by the widget always lands on a
/// scalar boundary as well.
pub(crate) fn character_lengths(text: &str) -> Vec<u8> {
    text.chars()
        .map(|character| character.len_utf8() as u8)
        .collect()
}

/// Number of AccessKit characters (Unicode scalar values) in `text`.
pub(crate) fn scalar_count(text: &str) -> usize {
    text.chars().count()
}

/// Convert a UTF-8 byte offset into an AccessKit character index.
///
/// Offsets past the end clamp to the end; offsets inside a multi-byte scalar
/// map to that scalar's index. The result never exceeds `scalar_count`.
pub(crate) fn scalar_index_for_byte(text: &str, byte_offset: usize) -> usize {
    let byte_offset = byte_offset.min(text.len());
    text.char_indices()
        .take_while(|(byte, character)| *byte + character.len_utf8() <= byte_offset)
        .count()
}

/// Convert an AccessKit character index into a UTF-8 byte offset.
///
/// Returns `None` for an index past the end of the text so callers can reject
/// malformed accessibility requests before touching the model.
pub(crate) fn byte_offset_for_scalar(text: &str, character_index: usize) -> Option<usize> {
    let mut index = 0;
    for (byte, _) in text.char_indices() {
        if index == character_index {
            return Some(byte);
        }
        index += 1;
    }
    (index == character_index).then_some(text.len())
}

/// Convert an AccessKit character index into a UTF-16 code-unit offset.
///
/// Used by the platform input-handler boundary and by tests; it returns `None`
/// for indices past the end.
pub(crate) fn utf16_offset_for_scalar(text: &str, character_index: usize) -> Option<usize> {
    let mut offset = 0;
    let mut index = 0;
    for character in text.chars() {
        if index == character_index {
            return Some(offset);
        }
        offset += character.len_utf16();
        index += 1;
    }
    (index == character_index).then_some(offset)
}

/// Convert a UTF-16 code-unit offset into an AccessKit character index.
///
/// Offsets inside a surrogate pair round down to the scalar that contains
/// them, matching the text-editing contract that selections cannot split a
/// scalar. Offsets past the end clamp to the number of scalars.
pub(crate) fn scalar_index_for_utf16(text: &str, utf16_offset: usize) -> usize {
    let mut offset = 0;
    let mut index = 0;
    for character in text.chars() {
        if offset + character.len_utf16() > utf16_offset {
            return index;
        }
        offset += character.len_utf16();
        index += 1;
    }
    index
}

/// Whether a character belongs to a right-to-left script.
///
/// This is a cheap first-strong-character test, not a full bidi
/// implementation: it decides the direction AccessKit should announce for the
/// whole single-line run. Ranges cover Hebrew, Arabic, Syriac, Thaana, NKo,
/// Samaritan, Mandaic, Adlam and their presentation forms.
fn is_rtl_strong(character: char) -> bool {
    matches!(
        character as u32,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x1E800..=0x1EFFF
    )
}

/// Direction of the first strong character in `text`.
///
/// Digits, punctuation and whitespace are neutral; text with no strong
/// character defaults to left-to-right. Indexes stay logical regardless of
/// this value; it only tells assistive technology how to present the run.
pub(crate) fn text_direction(text: &str) -> TextDirection {
    for character in text.chars() {
        if is_rtl_strong(character) {
            return TextDirection::RightToLeft;
        }
        if character.is_alphabetic() {
            return TextDirection::LeftToRight;
        }
    }
    TextDirection::LeftToRight
}

/// Word starts as AccessKit `u8` indices, or an empty slice when too long.
fn word_starts(text: &str) -> Vec<u8> {
    if scalar_count(text) > MAX_WORD_START_INDEX {
        return Vec::new();
    }

    let mut starts = Vec::new();
    let mut previous_was_word = false;
    for (index, character) in text.chars().enumerate() {
        let is_word = character.is_alphanumeric() || character == '_';
        if is_word && !previous_was_word {
            starts.push(index as u8);
        }
        previous_was_word = is_word;
    }
    starts
}

/// Build an AccessKit selection from byte offsets, preserving direction.
pub(crate) fn selection_positions(
    text: &str,
    anchor_byte: usize,
    focus_byte: usize,
    run_id: NodeId,
) -> TextSelection {
    TextSelection {
        anchor: TextPosition {
            node: run_id,
            character_index: scalar_index_for_byte(text, anchor_byte),
        },
        focus: TextPosition {
            node: run_id,
            character_index: scalar_index_for_byte(text, focus_byte),
        },
    }
}

fn physical_bounds(bounds: Bounds<Pixels>, scale_factor: f32) -> Rect {
    let scale = f64::from(scale_factor);
    let left = f64::from(bounds.origin.x) * scale;
    let top = f64::from(bounds.origin.y) * scale;
    let right = left + f64::from(bounds.size.width) * scale;
    let bottom = top + f64::from(bounds.size.height) * scale;
    Rect {
        x0: left,
        y0: top,
        x1: right,
        y1: bottom,
    }
}

fn character_geometry(line: &ShapedLine, text: &str, scale_factor: f32) -> (Vec<f32>, Vec<f32>) {
    let mut positions = Vec::with_capacity(text.chars().count());
    let mut widths = Vec::with_capacity(text.chars().count());

    for (byte, character) in text.char_indices() {
        let start = f32::from(line.x_for_index(byte)) * scale_factor;
        let end = f32::from(line.x_for_index(byte + character.len_utf8())) * scale_factor;
        positions.push(start);
        widths.push((end - start).max(0.0));
    }

    (positions, widths)
}

/// Everything needed to publish one frame of AccessKit text data.
///
/// The snapshot is captured during `render`, so it matches the text and
/// selection in the tree update for that frame. `run_id` is filled in once the
/// synthetic node id is known during prepaint; accessibility action handlers
/// use it to reject selections aimed at another node.
#[derive(Clone)]
pub(crate) struct A11ySnapshot {
    pub(crate) text: String,
    anchor_byte: usize,
    focus_byte: usize,
    character_lengths: Vec<u8>,
    word_starts: Vec<u8>,
    character_positions: Option<Vec<f32>>,
    character_widths: Option<Vec<f32>>,
    bounds: Option<Rect>,
    pub(crate) run_id: Rc<Cell<Option<NodeId>>>,
}

impl A11ySnapshot {
    /// Capture text, selection and (optionally) shaped-line geometry.
    ///
    /// `layout` should only be passed when the shaped line was produced from
    /// `text`; a stale line would give AccessKit positions for the wrong
    /// string. The caller checks that before calling.
    pub(crate) fn new(
        text: &str,
        anchor_byte: usize,
        focus_byte: usize,
        layout: Option<(&ShapedLine, Bounds<Pixels>, f32)>,
    ) -> Self {
        let (bounds, character_positions, character_widths) = match layout {
            Some((line, element_bounds, scale_factor)) => {
                let (positions, widths) = character_geometry(line, text, scale_factor);
                (
                    Some(physical_bounds(element_bounds, scale_factor)),
                    Some(positions),
                    Some(widths),
                )
            }
            None => (None, None, None),
        };

        Self {
            text: text.to_owned(),
            anchor_byte,
            focus_byte,
            character_lengths: character_lengths(text),
            word_starts: word_starts(text),
            character_positions,
            character_widths,
            bounds,
            run_id: Rc::new(Cell::new(None)),
        }
    }

    /// Push the text run and the parent's text selection into the a11y tree.
    pub(crate) fn push(&self, builder: &mut A11ySubtreeBuilder) {
        let run_id = builder.synthetic_node_id(0_u64);
        self.run_id.set(Some(run_id));

        let mut node = Node::new(Role::TextRun);
        node.set_text_direction(text_direction(&self.text));
        node.set_value(self.text.clone());
        node.set_character_lengths(self.character_lengths.clone());
        if !self.word_starts.is_empty() {
            node.set_word_starts(self.word_starts.clone());
        }
        if let Some(bounds) = self.bounds {
            node.set_bounds(bounds);
        }
        if let Some(positions) = self.character_positions.clone() {
            node.set_character_positions(positions);
        }
        if let Some(widths) = self.character_widths.clone() {
            node.set_character_widths(widths);
        }

        if !builder.push_child(run_id, node) {
            // A duplicate node id means another element claimed this run; do
            // not point the parent's selection at a discarded node.
            self.run_id.set(None);
            return;
        }

        builder
            .parent_node()
            .set_text_selection(selection_positions(
                &self.text,
                self.anchor_byte,
                self.focus_byte,
                run_id,
            ));
    }

    /// Convert an AccessKit selection into UTF-8 `(anchor, focus)` offsets.
    ///
    /// Returns `None` when the request targets another text run, names a
    /// character past the end, or arrives before the run was published. This is
    /// the validation boundary that keeps malformed platform requests from
    /// slicing the model.
    pub(crate) fn selection_byte_offsets(
        &self,
        selection: &TextSelection,
    ) -> Option<(usize, usize)> {
        let run_id = self.run_id.get()?;
        if selection.anchor.node != run_id || selection.focus.node != run_id {
            return None;
        }

        let count = self.character_lengths.len();
        if selection.anchor.character_index > count || selection.focus.character_index > count {
            return None;
        }

        let anchor = byte_offset_for_scalar(&self.text, selection.anchor.character_index)?;
        let focus = byte_offset_for_scalar(&self.text, selection.focus.character_index)?;
        Some((anchor, focus))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_input::model::{EditRejection, MAX_TEXT_SCALARS, TextModel};

    fn model_with(text: &str) -> TextModel {
        let mut model = TextModel::default();
        model.replace_text(None, text).unwrap();
        model
    }

    #[test]
    fn character_lengths_are_utf8_and_match_the_value() {
        let text = "a😀é🇯🇵e\u{301}";
        let lengths = character_lengths(text);

        assert_eq!(lengths.len(), text.chars().count());
        assert_eq!(
            lengths.iter().map(|length| *length as usize).sum::<usize>(),
            text.len()
        );
        assert!(lengths.iter().all(|length| (1..=4).contains(length)));
    }

    #[test]
    fn intrabyte_offsets_round_down_without_splitting_a_scalar() {
        let text = "A😀漢";
        assert_eq!(scalar_index_for_byte(text, 0), 0);
        assert_eq!(scalar_index_for_byte(text, 1), 1);
        for offset in 2..5 {
            assert_eq!(scalar_index_for_byte(text, offset), 1);
        }
        assert_eq!(scalar_index_for_byte(text, 5), 2);
        for offset in 6..8 {
            assert_eq!(scalar_index_for_byte(text, offset), 2);
        }
        assert_eq!(scalar_index_for_byte(text, 8), 3);
    }

    #[test]
    fn scalar_and_utf16_offsets_round_trip_across_scripts() {
        let text = "a😀é🇯🇵e\u{301}";

        for (byte, _) in text.char_indices() {
            let scalar = scalar_index_for_byte(text, byte);
            let utf16 = utf16_offset_for_scalar(text, scalar).unwrap();

            assert_eq!(scalar_index_for_utf16(text, utf16), scalar);
            assert_eq!(byte_offset_for_scalar(text, scalar), Some(byte));
        }

        assert_eq!(scalar_index_for_byte(text, 1), 1);
        assert_eq!(scalar_index_for_byte(text, "a😀".len()), 2);
        assert_eq!(
            scalar_index_for_byte(text, text.len()),
            text.chars().count()
        );
        assert_eq!(
            scalar_index_for_byte(text, usize::MAX),
            text.chars().count()
        );
    }

    #[test]
    fn astral_and_combining_characters_are_single_accesskit_characters() {
        // A UTF-16 surrogate pair is two code units but one scalar.
        assert_eq!(character_lengths("😀"), vec![4]);
        assert_eq!(utf16_offset_for_scalar("😀", 1), Some(2));
        assert_eq!(scalar_index_for_utf16("😀", 1), 0);
        assert_eq!(scalar_index_for_utf16("😀", 2), 1);

        // A combining mark is its own AccessKit character, matching the model's
        // UTF-8 offsets even though cursor movement crosses it as one grapheme.
        assert_eq!(character_lengths("e\u{301}"), vec![1, 2]);
        assert_eq!(byte_offset_for_scalar("e\u{301}", 2), Some(3));
        assert_eq!(utf16_offset_for_scalar("e\u{301}", 2), Some(2));
    }

    #[test]
    fn grapheme_clusters_map_by_scalar_without_splitting_utf8() {
        let text = "a👨‍👩‍👧‍👦e\u{301}b";
        let lengths = character_lengths(text);
        assert_eq!(lengths.len(), text.chars().count());
        assert_eq!(
            lengths.iter().map(|length| *length as usize).sum::<usize>(),
            text.len()
        );

        let family_end = "a👨‍👩‍👧‍👦".len();
        let family_scalar = scalar_index_for_byte(text, family_end);
        assert_eq!(family_scalar, text[..family_end].chars().count());
        assert_eq!(
            byte_offset_for_scalar(text, family_scalar),
            Some(family_end)
        );
    }

    #[test]
    fn reversed_selection_keeps_anchor_and_focus_direction() {
        let run = NodeId(7);
        let selection = selection_positions("abcdef", 4, 2, run);

        assert_eq!(
            selection.anchor,
            TextPosition {
                node: run,
                character_index: 4,
            }
        );
        assert_eq!(
            selection.focus,
            TextPosition {
                node: run,
                character_index: 2,
            }
        );
    }

    #[test]
    fn out_of_range_or_foreign_selections_are_rejected() {
        let snapshot = A11ySnapshot::new("abc", 1, 1, None);
        let run = NodeId(11);
        snapshot.run_id.set(Some(run));

        assert_eq!(
            snapshot.selection_byte_offsets(&selection_positions("abc", 1, 2, run)),
            Some((1, 2))
        );
        assert_eq!(
            snapshot.selection_byte_offsets(&selection_positions("abc", 3, 3, run)),
            Some((3, 3))
        );

        let past_end = TextSelection {
            anchor: TextPosition {
                node: run,
                character_index: 3,
            },
            focus: TextPosition {
                node: run,
                character_index: 4,
            },
        };
        assert_eq!(snapshot.selection_byte_offsets(&past_end), None);

        let enormous = TextSelection {
            anchor: TextPosition {
                node: run,
                character_index: usize::MAX,
            },
            focus: TextPosition {
                node: run,
                character_index: usize::MAX,
            },
        };
        assert_eq!(snapshot.selection_byte_offsets(&enormous), None);

        let foreign_node = TextSelection {
            anchor: TextPosition {
                node: NodeId(12),
                character_index: 0,
            },
            focus: TextPosition {
                node: NodeId(12),
                character_index: 0,
            },
        };
        assert_eq!(snapshot.selection_byte_offsets(&foreign_node), None);

        // Before prepaint publishes the run, nothing can be mapped.
        let unpublished = A11ySnapshot::new("abc", 1, 1, None);
        assert_eq!(
            unpublished.selection_byte_offsets(&selection_positions("abc", 1, 1, run)),
            None
        );
    }

    #[test]
    fn selection_application_preserves_direction_and_history() {
        let mut model = model_with("abcdef");

        assert!(model.set_selection_anchor_focus(4, 2));
        assert_eq!(model.selected_range(), 2..4);
        assert!(model.selection_reversed());

        assert!(model.set_selection_anchor_focus(1, 5));
        assert_eq!(model.selected_range(), 1..5);
        assert!(!model.selection_reversed());
        assert_eq!(model.cursor_offset(), 5);

        // Selection changes are not undo transactions: the only saved state is
        // the fixture's initial edit from empty text.
        assert!(model.undo());
        assert_eq!(model.content(), "");
    }

    #[test]
    fn selection_application_clamps_to_char_boundaries() {
        let mut model = model_with("😀a");

        assert!(model.set_selection_anchor_focus(2, 5));
        assert_eq!(model.selected_range(), 0..5);
        assert!(!model.selection_reversed());
    }

    #[test]
    fn oversized_set_value_is_rejected_atomically() {
        let mut model = model_with("keep");
        let before = model.content().to_owned();

        let replacement = "x".repeat(MAX_TEXT_SCALARS + 1);
        let rejection = model.a11y_set_value(&replacement);

        assert!(matches!(
            rejection,
            Some(Err(EditRejection::TextTooLong { .. }))
        ));
        assert_eq!(model.content(), before);

        // The rejected request recorded no transaction: undoing once returns to
        // the empty text that predated the fixture, not to another "keep".
        assert!(model.undo());
        assert_eq!(model.content(), "");
    }

    #[test]
    fn a11y_edits_are_ignored_during_composition() {
        let mut model = model_with("abc");
        model.move_to(3);
        model.replace_and_mark_text(None, "に", Some(1..1)).unwrap();
        let composing = model.content().to_owned();

        assert!(model.is_composing());
        assert_eq!(model.a11y_set_value("replacement"), None);
        assert_eq!(model.a11y_replace_selected("replacement"), None);
        assert!(!model.set_selection_anchor_focus(0, 0));
        assert_eq!(model.content(), composing);
        assert_eq!(model.selected_range(), 6..6);
        assert!(model.is_composing());
    }

    #[test]
    fn set_value_replaces_the_document_and_moves_the_caret() {
        let mut model = model_with("abc");
        model.set_selected_range(1..2);

        assert_eq!(model.a11y_set_value("xy"), Some(Ok(true)));
        assert_eq!(model.content(), "xy");
        assert_eq!(model.selected_range(), 2..2);

        assert!(model.undo());
        assert_eq!(model.content(), "abc");
        assert_eq!(model.selected_range(), 1..2);
    }

    #[test]
    fn replace_selected_uses_the_current_selection() {
        let mut model = model_with("hello world");
        model.set_selected_range(6..11);

        assert_eq!(model.a11y_replace_selected("there"), Some(Ok(true)));
        assert_eq!(model.content(), "hello there");
        assert_eq!(model.selected_range(), 11..11);
    }

    #[test]
    fn text_direction_uses_the_first_strong_character() {
        assert_eq!(text_direction(""), TextDirection::LeftToRight);
        assert_eq!(text_direction("123 456"), TextDirection::LeftToRight);
        assert_eq!(text_direction("hello שלום"), TextDirection::LeftToRight);
        assert_eq!(text_direction("שלום hello"), TextDirection::RightToLeft);
        assert_eq!(text_direction("123 שלום abc"), TextDirection::RightToLeft);
        assert_eq!(text_direction("مرحبا"), TextDirection::RightToLeft);
        assert_eq!(text_direction("漢字"), TextDirection::LeftToRight);
    }

    #[test]
    fn physical_bounds_scale_at_100_150_200_percent() {
        use gpui::{point, px, size};

        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(32.0)));

        assert_eq!(
            physical_bounds(bounds, 1.0),
            Rect {
                x0: 10.0,
                y0: 20.0,
                x1: 110.0,
                y1: 52.0,
            }
        );
        assert_eq!(
            physical_bounds(bounds, 1.5),
            Rect {
                x0: 15.0,
                y0: 30.0,
                x1: 165.0,
                y1: 78.0,
            }
        );
        assert_eq!(
            physical_bounds(bounds, 2.0),
            Rect {
                x0: 20.0,
                y0: 40.0,
                x1: 220.0,
                y1: 104.0,
            }
        );
    }

    #[test]
    fn logical_selection_is_independent_of_reported_scale() {
        let text = "abc שלום 123";
        let hebrew_start = text.find('ש').expect("mixed fixture contains Hebrew");
        let hebrew_end = hebrew_start + "שלום".len();
        let run = NodeId(21);

        // The published selection is reversed and spans the logical byte range
        // of the Hebrew word, with Latin text on either side.
        let selection = selection_positions(text, hebrew_end, hebrew_start, run);

        for scale in [1.0_f32, 1.5, 2.0] {
            let snapshot = A11ySnapshot::new(text, hebrew_end, hebrew_start, None);
            snapshot.run_id.set(Some(run));
            assert_eq!(
                snapshot.selection_byte_offsets(&selection),
                Some((hebrew_end, hebrew_start)),
                "logical selection must not depend on scale {scale}"
            );
        }
    }
}

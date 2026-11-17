//! Headless single-line text state.
//!
//! The model owns the text, selection, IME marked range and bounded undo/redo
//! history. It has no GPUI dependency so grapheme, UTF-16 and history behavior
//! can be tested without a window. All offsets stored here are UTF-8 byte
//! offsets; the platform-facing conversions live in the same type.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Maximum number of Unicode scalar values a single-line query may contain.
///
/// The limit counts Unicode scalar values (Rust `char`s), not bytes or UTF-16
/// code units, so astral characters and combining marks behave identically on
/// every platform. An edit that would exceed the limit is rejected before the
/// text, selection or history changes.
pub const MAX_TEXT_SCALARS: usize = 4096;

/// Maximum number of undo/redo transactions kept across both stacks.
///
/// The oldest undo entry is discarded first. One transaction stores one saved
/// text snapshot, never a keystroke log.
pub const MAX_HISTORY_ENTRIES: usize = 100;

/// Maximum total bytes of saved text across the undo and redo stacks.
///
/// The oldest entries are discarded until the bound is met. One query can hold
/// at most 16 KiB of UTF-8 text (4096 scalar values), so at least one snapshot
/// always fits.
pub const MAX_HISTORY_BYTES: usize = 1024 * 1024;

/// Why the model refused an edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditRejection {
    /// Applying the edit would exceed [`MAX_TEXT_SCALARS`].
    TextTooLong {
        /// The configured scalar-value limit.
        limit: usize,
        /// Scalar values the edit would have produced.
        would_be: usize,
    },
}

impl EditRejection {
    /// A short user-facing message suitable for a status line.
    pub fn message(self) -> &'static str {
        match self {
            Self::TextTooLong { .. } => "Search is limited to 4,096 characters",
        }
    }
}

/// The text and selection captured by one undo transaction.
#[derive(Debug, Clone)]
struct Snapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

impl Snapshot {
    fn bytes(&self) -> usize {
        self.content.len()
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TextModel {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    undo_bytes: usize,
    redo_bytes: usize,
    /// Pre-composition state for the transaction that is still in progress.
    ///
    /// The first marked update captures it; repeated marked updates reuse it;
    /// `replace_text` or `unmark` commits it as a single undo entry.
    composition_origin: Option<Snapshot>,
}

impl TextModel {
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub(crate) fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    pub(crate) fn selected_range(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    pub(crate) fn selection_reversed(&self) -> bool {
        self.selection_reversed
    }

    pub(crate) fn marked_range(&self) -> Option<Range<usize>> {
        self.marked_range.clone()
    }

    /// Whether [`Self::undo`] would restore an edit right now.
    ///
    /// Always `false` during an IME composition, because the input method owns
    /// the marked text until it commits.
    pub(crate) fn can_undo(&self) -> bool {
        !self.is_composing() && self.composition_origin.is_none() && !self.undo.is_empty()
    }

    /// Whether [`Self::redo`] would reapply an edit right now.
    pub(crate) fn can_redo(&self) -> bool {
        !self.is_composing() && self.composition_origin.is_none() && !self.redo.is_empty()
    }

    /// The active edge of the selection, in UTF-8 bytes.
    pub(crate) fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub(crate) fn move_to(&mut self, offset: usize) {
        let offset = offset.min(self.content.len());
        self.selected_range = offset..offset;
        self.selection_reversed = false;
    }

    pub(crate) fn select_to(&mut self, offset: usize) {
        let offset = offset.min(self.content.len());
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
    }

    pub(crate) fn select_all(&mut self) {
        self.move_to(0);
        self.select_to(self.content.len());
    }

    pub(crate) fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    pub(crate) fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    pub(crate) fn offset_from_utf16(&self, offset: usize) -> usize {
        utf16_offset_to_utf8(&self.content, offset)
    }

    pub(crate) fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf8_count = 0;
        let mut utf16_count = 0;

        for character in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += character.len_utf8();
            utf16_count += character.len_utf16();
        }

        utf16_count
    }

    pub(crate) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub(crate) fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    /// The fixed and active ends of the selection, in UTF-8 bytes.
    ///
    /// AccessKit's `TextSelection` calls these `anchor` and `focus`; the
    /// anchor does not move while a selection is expanded.
    #[cfg(feature = "upstream-candidate")]
    pub(crate) fn selection_anchor_focus(&self) -> (usize, usize) {
        if self.selection_reversed {
            (self.selected_range.end, self.selected_range.start)
        } else {
            (self.selected_range.start, self.selected_range.end)
        }
    }

    /// Round `offset` down to a character boundary within the text.
    #[cfg(feature = "upstream-candidate")]
    fn clamp_char_boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while offset > 0 && !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    /// Select `start..end` without changing text or history.
    #[cfg(feature = "upstream-candidate")]
    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        let start = self.clamp_char_boundary(range.start);
        let end = self.clamp_char_boundary(range.end).max(start);
        self.selected_range = start..end;
        self.selection_reversed = false;
    }

    /// Apply an accessibility selection, preserving selection direction.
    ///
    /// Returns `false` while a composition is active, because the input method
    /// owns the marked text and its selection.
    #[cfg(feature = "upstream-candidate")]
    pub(crate) fn set_selection_anchor_focus(&mut self, anchor: usize, focus: usize) -> bool {
        if self.is_composing() {
            return false;
        }

        let anchor = self.clamp_char_boundary(anchor);
        let focus = self.clamp_char_boundary(focus);
        self.selected_range = anchor.min(focus)..anchor.max(focus);
        self.selection_reversed = focus < anchor;
        true
    }

    /// Replace the whole document for an accessibility `SetValue` request.
    ///
    /// Returns `None` when an IME composition is active: the input method owns
    /// the text until it commits. Oversized values are rejected atomically by
    /// [`Self::replace_text`], so text, selection and history stay unchanged.
    #[cfg(feature = "upstream-candidate")]
    pub(crate) fn a11y_set_value(&mut self, new_text: &str) -> Option<Result<bool, EditRejection>> {
        if self.is_composing() {
            return None;
        }
        let end = self.offset_to_utf16(self.content.len());
        Some(self.replace_text(Some(0..end), new_text))
    }

    /// Replace the selection for an accessibility `ReplaceSelectedText` request.
    ///
    /// Returns `None` during an IME composition. The normal size check and
    /// undo transaction still apply.
    #[cfg(feature = "upstream-candidate")]
    pub(crate) fn a11y_replace_selected(
        &mut self,
        new_text: &str,
    ) -> Option<Result<bool, EditRejection>> {
        if self.is_composing() {
            return None;
        }
        Some(self.replace_text(None, new_text))
    }

    /// The document range a platform replacement applies to.
    ///
    /// An explicit platform range wins, then an active IME marked range, then
    /// the current selection.
    fn replacement_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        let start = range.start.min(self.content.len());
        let end = range.end.max(start).min(self.content.len());
        start..end
    }

    /// Reject the edit when the resulting text would be too long.
    ///
    /// Runs before any mutation, so a rejected edit leaves text, selection,
    /// composition state and history untouched.
    fn check_size(&self, range: &Range<usize>, new_text: &str) -> Result<(), EditRejection> {
        let scalars = self.content[..range.start].chars().count()
            + new_text.chars().count()
            + self.content[range.end..].chars().count();
        if scalars > MAX_TEXT_SCALARS {
            Err(EditRejection::TextTooLong {
                limit: MAX_TEXT_SCALARS,
                would_be: scalars,
            })
        } else {
            Ok(())
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.content = snapshot.content;
        self.selected_range = snapshot.selected_range;
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.composition_origin = None;
    }

    /// Save the pre-edit state as one undo transaction and drop redo.
    fn record_undo(&mut self) {
        self.redo.clear();
        self.redo_bytes = 0;

        let snapshot = self.snapshot();
        self.undo_bytes += snapshot.bytes();
        self.undo.push(snapshot);
        self.trim_history();
    }

    /// Finish a pending composition transaction.
    ///
    /// Called after the final text is in place. Nothing is recorded when the
    /// composition ended up restoring the origin text (an IME cancellation), so
    /// cancelling never creates an undo step or clears redo.
    fn finish_composition(&mut self, origin: Snapshot) {
        if origin.content == self.content {
            return;
        }

        self.redo.clear();
        self.redo_bytes = 0;
        self.undo_bytes += origin.bytes();
        self.undo.push(origin);
        self.trim_history();
    }

    fn trim_history(&mut self) {
        while self.undo.len() + self.redo.len() > MAX_HISTORY_ENTRIES {
            if !self.drop_oldest_undo() && !self.drop_oldest_redo() {
                break;
            }
        }
        while self.undo_bytes + self.redo_bytes > MAX_HISTORY_BYTES {
            if !self.drop_oldest_undo() && !self.drop_oldest_redo() {
                break;
            }
        }
    }

    fn drop_oldest_undo(&mut self) -> bool {
        if self.undo.is_empty() {
            return false;
        }
        let oldest = self.undo.remove(0);
        self.undo_bytes -= oldest.bytes();
        true
    }

    fn drop_oldest_redo(&mut self) -> bool {
        if self.redo.is_empty() {
            return false;
        }
        let oldest = self.redo.remove(0);
        self.redo_bytes -= oldest.bytes();
        true
    }

    /// Undo the most recent committed edit.
    ///
    /// Restores both text and selection. Returns `false` while a composition is
    /// active, because the input method owns the marked text.
    pub(crate) fn undo(&mut self) -> bool {
        if self.is_composing() || self.composition_origin.is_some() {
            return false;
        }
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.undo_bytes -= previous.bytes();

        let current = self.snapshot();
        self.redo_bytes += current.bytes();
        self.redo.push(current);

        self.restore(previous);
        true
    }

    /// Reapply the most recently undone edit.
    ///
    /// Returns `false` while a composition is active or when redo history was
    /// cleared by a newer committed edit.
    pub(crate) fn redo(&mut self) -> bool {
        if self.is_composing() || self.composition_origin.is_some() {
            return false;
        }
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.redo_bytes -= next.bytes();

        let current = self.snapshot();
        self.undo_bytes += current.bytes();
        self.undo.push(current);

        self.restore(next);
        true
    }

    /// Replace a platform range (or the marked/selected range) with `new_text`.
    ///
    /// Returns `true` when the document text changed. Selection and composing
    /// state always settle after the replacement. A pending IME composition is
    /// committed as one transaction; otherwise a changed edit records its own.
    pub(crate) fn replace_text(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
    ) -> Result<bool, EditRejection> {
        let range = self.replacement_range(range_utf16);
        self.check_size(&range, new_text)?;

        let changed = self.content.get(range.clone()) != Some(new_text);
        let composition_origin = self.composition_origin.take();

        if composition_origin.is_none() && changed {
            self.record_undo();
        }

        self.content.replace_range(range.clone(), new_text);
        let caret = range.start + new_text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;

        if let Some(origin) = composition_origin {
            self.finish_composition(origin);
        }

        Ok(changed)
    }

    /// Replace a range and mark the inserted text as an IME composition.
    ///
    /// `new_selected_range_utf16` is relative to `new_text`, matching the
    /// platform input-handler contract. The first marked update captures the
    /// pre-composition state; repeated updates reuse it. The transaction is
    /// committed by [`Self::replace_text`] or [`Self::unmark`].
    pub(crate) fn replace_and_mark_text(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) -> Result<bool, EditRejection> {
        let range = self.replacement_range(range_utf16);
        self.check_size(&range, new_text)?;

        if self.composition_origin.is_none() {
            self.composition_origin = Some(self.snapshot());
        }

        let changed = self.content.get(range.clone()) != Some(new_text);

        self.content.replace_range(range.clone(), new_text);

        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }

        let caret = range.start + new_text.len();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                let start = range.start + utf16_offset_to_utf8(new_text, selected.start);
                let end = range.start + utf16_offset_to_utf8(new_text, selected.end);
                start..end
            })
            .unwrap_or(caret..caret);
        self.selection_reversed = false;

        if self.marked_range.is_none()
            && let Some(origin) = self.composition_origin.take()
        {
            self.finish_composition(origin);
        }

        Ok(changed)
    }

    /// Clear the mark and commit the composition transaction, if any.
    pub(crate) fn unmark(&mut self) {
        self.marked_range = None;
        if let Some(origin) = self.composition_origin.take() {
            self.finish_composition(origin);
        }
    }

    /// Reset all text, selection and composition state, and discard history.
    ///
    /// Undo and redo history is always dropped, even when the text was already
    /// empty, because cleared queries are transient and must not be restorable.
    ///
    /// Returns `true` when there was text to clear.
    pub(crate) fn clear(&mut self) -> bool {
        let had_content = !self.content.is_empty();
        self.content.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.composition_origin = None;
        self.undo.clear();
        self.redo.clear();
        self.undo_bytes = 0;
        self.redo_bytes = 0;
        had_content
    }
}

/// Convert a UTF-16 code-unit offset within `text` to a UTF-8 byte offset.
fn utf16_offset_to_utf8(text: &str, offset: usize) -> usize {
    let mut utf16_count = 0;
    for (index, character) in text.char_indices() {
        if utf16_count >= offset {
            return index;
        }
        utf16_count += character.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with(text: &str) -> TextModel {
        let mut model = TextModel::default();
        assert_eq!(model.replace_text(None, text).unwrap(), !text.is_empty());
        model
    }

    /// A model with text but no history, so history assertions start clean.
    fn model_without_history(text: &str) -> TextModel {
        TextModel {
            content: text.to_owned(),
            selected_range: text.len()..text.len(),
            ..Default::default()
        }
    }

    #[test]
    fn utf16_round_trips_astral_and_combining_text() {
        let model = model_with("a😀é🇯🇵");

        for offset in [0, 1, 5, 7, 11, 15] {
            assert_eq!(
                model.offset_from_utf16(model.offset_to_utf16(offset)),
                offset
            );
        }
        for offset in [0, 1, 3, 4, 6, 8] {
            assert_eq!(
                model.offset_to_utf16(model.offset_from_utf16(offset)),
                offset
            );
        }

        assert_eq!(model.offset_to_utf16("a".len()), 1);
        assert_eq!(model.offset_to_utf16("a😀".len()), 3);
        assert_eq!(model.offset_to_utf16("a😀é".len()), 4);
    }

    #[test]
    fn grapheme_boundaries_skip_zwj_sequences_and_marks() {
        let model = model_with("a👨‍👩‍👧‍👦e\u{301}b");

        let after_first = "a".len();
        let after_family = "a👨‍👩‍👧‍👦".len();
        let after_mark = "a👨‍👩‍👧‍👦e\u{301}".len();

        assert_eq!(model.next_boundary(after_first), after_family);
        assert_eq!(model.next_boundary(after_family), after_mark);
        assert_eq!(model.previous_boundary(after_mark), after_family);
        assert_eq!(model.previous_boundary(after_family), after_first);
        assert_eq!(model.previous_boundary(0), 0);
        assert_eq!(
            model.next_boundary(model.content().len()),
            model.content().len()
        );
    }

    #[test]
    fn replace_uses_selection_and_resets_composition() {
        let mut model = model_with("hello world");
        model.move_to(6);
        model.select_to(11);

        assert!(model.replace_text(None, "there").unwrap());
        assert_eq!(model.content(), "hello there");
        assert_eq!(model.selected_range(), 11..11);
        assert!(!model.is_composing());
        assert!(model.replace_text(None, "!").unwrap());
        assert_eq!(model.content(), "hello there!");
    }

    #[test]
    fn composing_replaces_marked_range_and_maps_relative_selection() {
        let mut model = model_with("abc");
        model.move_to(3);

        assert!(
            model
                .replace_and_mark_text(None, "にほん", Some(0..3))
                .unwrap()
        );
        assert_eq!(model.content(), "abcにほん");
        assert_eq!(model.marked_range(), Some(3..12));
        assert!(model.is_composing());
        assert_eq!(model.selected_range(), 3..12);

        assert!(
            model
                .replace_and_mark_text(None, "日本", Some(2..2))
                .unwrap()
        );
        assert_eq!(model.content(), "abc日本");
        assert_eq!(model.marked_range(), Some(3..9));
        assert_eq!(model.selected_range(), 9..9);

        model.unmark();
        assert!(!model.is_composing());
    }

    #[test]
    fn replace_text_removes_marked_text_instead_of_duplicating_it() {
        let mut model = model_with("query ");
        model.replace_and_mark_text(None, "に", Some(1..1)).unwrap();
        assert_eq!(model.content(), "query に");

        model.replace_text(None, "日本").unwrap();
        assert_eq!(model.content(), "query 日本");
        assert!(!model.is_composing());
    }

    #[test]
    fn selection_direction_is_preserved_across_drag() {
        let mut model = model_with("abcdef");
        model.move_to(4);
        model.select_to(2);

        assert_eq!(model.selected_range(), 2..4);
        assert!(model.selection_reversed());
        assert_eq!(model.cursor_offset(), 2);

        model.select_to(5);
        assert_eq!(model.selected_range(), 4..5);
        assert!(!model.selection_reversed());
        assert_eq!(model.cursor_offset(), 5);
    }

    #[test]
    fn clear_resets_everything_and_reports_previous_content() {
        let mut model = model_with("text");
        model.select_all();

        assert!(model.clear());
        assert_eq!(model.content(), "");
        assert_eq!(model.selected_range(), 0..0);
        assert!(!model.selection_reversed());
        assert!(!model.is_composing());
        assert!(!model.clear());
    }

    #[test]
    fn undo_and_redo_restore_text_and_selection_direction() {
        let mut model = model_without_history("abcdef");
        model.move_to(3);
        model.select_to(1);

        assert!(model.replace_text(None, "X").unwrap());
        assert_eq!(model.content(), "aXdef");
        assert_eq!(model.selected_range(), 2..2);
        assert!(!model.selection_reversed());

        assert!(model.undo());
        assert_eq!(model.content(), "abcdef");
        assert_eq!(model.selected_range(), 1..3);
        assert!(model.selection_reversed());
        assert!(model.can_redo());

        assert!(model.redo());
        assert_eq!(model.content(), "aXdef");
        assert_eq!(model.selected_range(), 2..2);
        assert!(!model.selection_reversed());
    }

    #[test]
    fn redo_is_cleared_by_a_new_committed_edit() {
        let mut model = model_without_history("abc");
        assert!(model.replace_text(None, "d").unwrap());
        assert!(model.replace_text(None, "e").unwrap());
        assert!(model.undo());
        assert_eq!(model.content(), "abcd");
        assert!(model.can_redo());

        assert!(model.replace_text(None, "f").unwrap());
        assert_eq!(model.content(), "abcdf");
        assert!(!model.can_redo());
        assert!(!model.redo());
        assert_eq!(model.content(), "abcdf");
    }

    #[test]
    fn ime_updates_commit_as_one_undo_step_on_unmark() {
        let mut model = model_without_history("q");
        model.move_to(1);

        assert!(model.replace_and_mark_text(None, "に", Some(1..1)).unwrap());
        assert!(
            model
                .replace_and_mark_text(None, "にほ", Some(2..2))
                .unwrap()
        );
        assert!(
            model
                .replace_and_mark_text(None, "日本", Some(2..2))
                .unwrap()
        );
        assert!(model.is_composing());

        // The input method owns edits until it commits.
        assert!(!model.can_undo());
        assert!(!model.undo());

        model.unmark();
        assert!(!model.is_composing());
        assert_eq!(model.content(), "q日本");

        assert_eq!(model.undo.len(), 1);
        assert!(model.undo());
        assert_eq!(model.content(), "q");
        assert_eq!(model.selected_range(), 1..1);
        assert!(!model.can_undo());
    }

    #[test]
    fn ime_updates_commit_as_one_undo_step_on_replace_text() {
        let mut model = model_without_history("abc");
        model.move_to(3);

        assert!(model.replace_and_mark_text(None, "に", Some(1..1)).unwrap());
        assert!(
            model
                .replace_and_mark_text(None, "にほん", Some(2..2))
                .unwrap()
        );
        assert_eq!(model.content(), "abcにほん");

        assert!(model.replace_text(None, "日本").unwrap());
        assert_eq!(model.content(), "abc日本");
        assert!(!model.is_composing());

        assert_eq!(model.undo.len(), 1);
        assert!(model.undo());
        assert_eq!(model.content(), "abc");
        assert_eq!(model.selected_range(), 3..3);
    }

    #[test]
    fn cancelled_composition_leaves_no_history() {
        let mut model = model_without_history("abc");
        model.move_to(3);
        assert!(model.replace_and_mark_text(None, "に", Some(1..1)).unwrap());
        assert!(
            model
                .replace_and_mark_text(None, "日本", Some(2..2))
                .unwrap()
        );

        assert!(model.replace_text(None, "").unwrap());
        assert_eq!(model.content(), "abc");
        assert!(!model.is_composing());
        assert!(!model.can_undo());
        assert!(!model.undo());
    }

    #[test]
    fn empty_marked_update_ends_composition_without_adding_history() {
        let mut model = model_without_history("abc");
        model.move_to(3);
        assert!(model.replace_and_mark_text(None, "に", Some(1..1)).unwrap());

        assert!(model.replace_and_mark_text(None, "", None).unwrap());
        assert_eq!(model.content(), "abc");
        assert!(!model.is_composing());
        assert!(!model.can_undo());
    }

    #[test]
    fn cancelled_composition_preserves_redo_history() {
        let mut model = model_without_history("abc");
        assert!(model.replace_text(None, "d").unwrap());
        assert!(model.replace_text(None, "e").unwrap());
        assert!(model.undo());
        assert_eq!(model.content(), "abcd");
        assert_eq!(model.redo.len(), 1);

        model.move_to(model.content().len());
        assert!(model.replace_and_mark_text(None, "に", Some(1..1)).unwrap());
        assert_eq!(model.redo.len(), 1);
        assert!(!model.undo());
        assert!(!model.redo());

        assert!(model.replace_text(None, "").unwrap());
        assert_eq!(model.content(), "abcd");
        assert!(model.can_redo());
        assert!(model.redo());
        assert_eq!(model.content(), "abcde");
    }

    #[test]
    fn undo_and_redo_are_noops_during_composition() {
        let mut model = model_without_history("abc");
        assert!(model.replace_text(None, "d").unwrap());
        assert!(model.undo());
        assert_eq!(model.content(), "abc");
        assert_eq!(model.redo.len(), 1);

        model.move_to(3);
        assert!(model.replace_and_mark_text(None, "に", Some(1..1)).unwrap());
        let composing = model.content().to_owned();

        assert!(!model.undo());
        assert!(!model.redo());
        assert_eq!(model.content(), composing);
        assert!(model.is_composing());
        assert_eq!(model.redo.len(), 1);
    }

    #[test]
    fn clear_discards_text_and_history() {
        let mut model = model_without_history("abc");
        assert!(model.replace_text(None, "d").unwrap());
        assert!(model.undo());
        assert!(model.can_redo());

        assert!(model.clear());
        assert_eq!(model.content(), "");
        assert!(!model.can_undo());
        assert!(!model.can_redo());
        assert!(!model.undo());
        assert!(!model.redo());
        assert_eq!(model.undo.len() + model.redo.len(), 0);

        // Clearing again has nothing to report but still discards history.
        assert!(!model.clear());
    }

    #[test]
    fn clear_discards_a_pending_composition() {
        let mut model = model_without_history("abc");
        model.move_to(3);
        assert!(model.replace_and_mark_text(None, "に", Some(1..1)).unwrap());

        assert!(model.clear());
        assert_eq!(model.content(), "");
        assert!(!model.is_composing());
        assert!(!model.can_undo());
        assert_eq!(model.undo.len() + model.redo.len(), 0);
    }

    #[test]
    fn identical_replacement_records_no_history() {
        let mut model = model_without_history("abc");
        model.select_all();

        assert!(!model.replace_text(None, "abc").unwrap());
        assert_eq!(model.content(), "abc");
        assert!(!model.can_undo());
    }

    #[test]
    fn oversize_ascii_insertion_is_rejected_atomically() {
        let at_limit = "a".repeat(MAX_TEXT_SCALARS);
        let mut model = model_without_history(&at_limit);
        model.move_to(at_limit.len());
        let selection = model.selected_range();

        let rejection = model.replace_text(None, "b").unwrap_err();
        assert_eq!(
            rejection,
            EditRejection::TextTooLong {
                limit: MAX_TEXT_SCALARS,
                would_be: MAX_TEXT_SCALARS + 1,
            }
        );
        assert_eq!(rejection.message(), "Search is limited to 4,096 characters");
        assert_eq!(model.content(), at_limit.as_str());
        assert_eq!(model.selected_range(), selection);
        assert!(!model.can_undo());
        assert_eq!(model.undo.len(), 0);
    }

    #[test]
    fn oversize_limit_counts_scalars_not_bytes_or_utf16() {
        let emoji = "😀";
        let at_limit = emoji.repeat(MAX_TEXT_SCALARS);
        assert!(at_limit.len() > MAX_TEXT_SCALARS * 2);

        let mut model = model_without_history(&at_limit);
        model.move_to(0);
        model.select_to(emoji.len());

        // One astral scalar replaces one astral scalar: still within the limit.
        assert!(model.replace_text(None, "x").unwrap());
        assert_eq!(model.content().chars().count(), MAX_TEXT_SCALARS);

        model.move_to(model.content().len());
        let rejection = model.replace_text(None, emoji).unwrap_err();
        assert_eq!(
            rejection,
            EditRejection::TextTooLong {
                limit: MAX_TEXT_SCALARS,
                would_be: MAX_TEXT_SCALARS + 1,
            }
        );
        assert_eq!(model.content().chars().count(), MAX_TEXT_SCALARS);
    }

    #[test]
    fn oversize_composition_update_keeps_marked_text_and_history() {
        let mut model = model_without_history("abc");
        model.move_to(3);
        assert!(model.replace_and_mark_text(None, "に", Some(1..1)).unwrap());

        let content = model.content().to_owned();
        let marked = model.marked_range();
        assert!(
            model
                .replace_and_mark_text(None, &"a".repeat(MAX_TEXT_SCALARS), None)
                .is_err()
        );

        assert_eq!(model.content(), content.as_str());
        assert_eq!(model.marked_range(), marked);
        assert!(model.is_composing());
        assert!(!model.can_undo());
        assert!(!model.undo());

        model.unmark();
        assert_eq!(model.content(), "abcに");
        assert!(model.undo());
        assert_eq!(model.content(), "abc");
    }

    #[test]
    fn undo_history_is_bounded_by_entries() {
        let mut model = model_without_history("");
        for _ in 0..(MAX_HISTORY_ENTRIES + 20) {
            assert!(model.replace_text(None, "a").unwrap());
        }
        assert_eq!(model.content().len(), MAX_HISTORY_ENTRIES + 20);
        assert_eq!(model.undo.len(), MAX_HISTORY_ENTRIES);

        let mut undone = 0;
        while model.undo() {
            undone += 1;
        }
        assert_eq!(undone, MAX_HISTORY_ENTRIES);
        assert_eq!(model.content().len(), 20);
        assert!(model.undo.len() + model.redo.len() <= MAX_HISTORY_ENTRIES);
    }

    #[test]
    fn undo_history_is_bounded_by_saved_text_bytes() {
        let emoji = "😀";
        let at_limit = emoji.repeat(MAX_TEXT_SCALARS);
        let mut model = model_without_history(&at_limit);

        // Keep the text at a fixed 16 KiB while creating 80 transactions.
        for index in 0..80 {
            model.move_to(0);
            model.select_to(emoji.len());
            let replacement = if index % 2 == 0 { "😁" } else { emoji };
            assert!(model.replace_text(None, replacement).unwrap());
        }

        let per_snapshot = at_limit.len();
        assert_eq!(per_snapshot, 16 * 1024);
        assert_eq!(model.undo.len(), MAX_HISTORY_BYTES / per_snapshot);
        assert!(model.undo.len() < MAX_HISTORY_ENTRIES);
        assert!(model.undo_bytes + model.redo_bytes <= MAX_HISTORY_BYTES);

        let mut undone = 0;
        while model.undo() {
            undone += 1;
        }
        assert_eq!(undone, MAX_HISTORY_BYTES / per_snapshot);
        assert!(model.redo_bytes <= MAX_HISTORY_BYTES);
        assert_eq!(
            model.undo.len() + model.redo.len(),
            MAX_HISTORY_BYTES / per_snapshot
        );
    }
}

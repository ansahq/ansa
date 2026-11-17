//! Reusable single-line text input for GPUI 0.2.2.
//!
//! Adapted from GPUI's `examples/input.rs`, Copyright Zed Industries,
//! licensed under Apache-2.0. See `THIRD_PARTY.md` at the crate root. The IME
//! plumbing, UTF-16 selection reporting and element-based caret drawing are
//! deliberately preserved; the view state, theme, events and public API are
//! Ansa's.
//!
//! Use it as an entity:
//!
//! ```ignore
//! let input = cx.new(TextInput::new);
//! cx.subscribe(&input, |_, _, event: &Changed, _, _| {
//!     let query = event.0.clone();
//! });
//! ```
//!
//! Call [`install_keybindings`] once during application startup. Bindings are
//! scoped to the `TextInput` key context so they never steal keys from other
//! controls.
//!
//! Edits are undoable with a bounded history ([`MAX_HISTORY_ENTRIES`] and
//! [`MAX_HISTORY_BYTES`]). An IME composition is one transaction: repeated
//! marked updates are grouped until the platform commits or unmarks them.
//! Input that would exceed [`MAX_TEXT_SCALARS`] is rejected without changing
//! text, selection or history, and is reported through [`EditRejected`].
//!
//! With the `upstream-candidate` feature (only for the immutable newer GPUI
//! revision), the field publishes a single AccessKit `SearchInput` node: the
//! same focus-tracked element carries the label, value, placeholder, current
//! text selection and one `TextRun` child with UTF-8 character lengths. Text
//! pattern selection, selected-text replacement and value replacement route
//! through the existing bounded, composition-aware edit path. The default
//! crates.io pin has no accessibility API, and this feature must not be enabled
//! against it.

mod model;

#[cfg(feature = "upstream-candidate")]
mod a11y;

use std::{borrow::Cow, ops::Range};

#[cfg(feature = "upstream-candidate")]
use gpui::AccessibleAction;
#[cfg(feature = "upstream-candidate")]
use gpui::accesskit::{ActionData, Role, TextSelection};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    Render, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
    actions, div, fill, point, prelude::*, px, relative, size,
};
use model::TextModel;

#[cfg(feature = "upstream-candidate")]
pub use a11y::DEFAULT_ACCESSIBILITY_LABEL;
pub use model::{EditRejection, MAX_HISTORY_BYTES, MAX_HISTORY_ENTRIES, MAX_TEXT_SCALARS};

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Undo,
        Redo,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

/// Key context used by the input and by all of its keybindings.
pub const KEY_CONTEXT: &str = "TextInput";

/// Placeholder shown while the field is empty.
pub const DEFAULT_PLACEHOLDER: &str = "Search applications, commands, or calculate…";

/// Text size shared by painting and accessibility geometry.
const TEXT_SIZE: f32 = 20.0;

/// Emitted after the field's text changes.
///
/// The payload is the complete current text, already sanitized to one line.
/// Composition updates also emit so a live search can track IME input; use
/// [`TextInput::is_composing`] to defer work that should wait for commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed(pub String);

/// Emitted when an edit is refused before it reaches the model.
///
/// The field keeps its previous text, selection, composition state and undo
/// history. Subscribe to this event to show a status line; [`Self::message`]
/// returns a ready-made user-facing message. The input also rings the system
/// attention sound for a rejection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditRejected {
    /// The specific reason the edit was refused.
    pub reason: EditRejection,
}

impl EditRejected {
    /// A short user-facing message suitable for a status line.
    pub fn message(&self) -> &'static str {
        self.reason.message()
    }
}

/// A themed, single-line text input entity.
pub struct TextInput {
    focus_handle: FocusHandle,
    field_id: ElementId,
    model: TextModel,
    placeholder: SharedString,
    theme: crate::Theme,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    #[cfg(feature = "upstream-candidate")]
    accessibility_label: SharedString,
    #[cfg(feature = "upstream-candidate")]
    accessibility_role: Role,
}

impl TextInput {
    /// Create an empty input in the caller's context.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            field_id: ElementId::NamedInteger("ansa-text-input".into(), cx.entity_id().as_u64()),
            model: TextModel::default(),
            placeholder: DEFAULT_PLACEHOLDER.into(),
            theme: crate::Theme::dark(),
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            #[cfg(feature = "upstream-candidate")]
            accessibility_label: DEFAULT_ACCESSIBILITY_LABEL.into(),
            #[cfg(feature = "upstream-candidate")]
            accessibility_role: Role::SearchInput,
        }
    }

    /// The current text.
    pub fn text(&self) -> &str {
        self.model.content()
    }

    /// Change the hint without replacing text, selection, or composition.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    /// Whether an IME composition is currently marked.
    pub fn is_composing(&self) -> bool {
        self.model.is_composing()
    }

    /// Whether [`Self::undo`] would currently restore an edit.
    ///
    /// Always `false` while an IME composition is active.
    pub fn can_undo(&self) -> bool {
        self.model.can_undo()
    }

    /// Whether [`Self::redo`] would currently reapply an edit.
    ///
    /// Always `false` while an IME composition is active.
    pub fn can_redo(&self) -> bool {
        self.model.can_redo()
    }

    /// Undo the most recent committed edit.
    ///
    /// Restores both the text and the selection that were current before the
    /// edit. Returns `true` when something was undone. While an IME composition
    /// is active this is a no-op, because the input method owns the marked
    /// text; call it again after the composition commits or cancels. Emits
    /// [`Changed`] when the text is restored.
    pub fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.model.undo() {
            return false;
        }
        self.last_layout = None;
        self.last_bounds = None;
        self.is_selecting = false;
        self.after_text_change(true, cx);
        true
    }

    /// Reapply the most recently undone edit.
    ///
    /// Returns `true` when an edit was restored. Redo history is cleared by any
    /// newer committed edit and is unavailable during an IME composition.
    /// Emits [`Changed`] when the text is restored.
    pub fn redo(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.model.redo() {
            return false;
        }
        self.last_layout = None;
        self.last_bounds = None;
        self.is_selecting = false;
        self.after_text_change(true, cx);
        true
    }

    /// Replace the design tokens used by this field.
    pub fn set_theme(&mut self, theme: crate::Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Change the accessible name announced for the field.
    ///
    /// The default is [`DEFAULT_ACCESSIBILITY_LABEL`]. Callers that already
    /// label the field around it should set that same text here instead of
    /// adding a second semantic wrapper, so the tree contains one input node.
    #[cfg(feature = "upstream-candidate")]
    pub fn set_accessibility_label(
        &mut self,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.accessibility_label = label.into();
        cx.notify();
    }

    /// The accessible name currently announced for the field.
    #[cfg(feature = "upstream-candidate")]
    pub fn accessibility_label(&self) -> &SharedString {
        &self.accessibility_label
    }

    /// Choose the AccessKit role for this field.
    ///
    /// The default is [`gpui::Role::SearchInput`]; callers embedding a
    /// general-purpose field can use [`gpui::Role::TextInput`]. Roles other
    /// than text-input roles do not expose the text pattern.
    #[cfg(feature = "upstream-candidate")]
    pub fn set_accessibility_role(&mut self, role: Role, cx: &mut Context<Self>) {
        self.accessibility_role = role;
        cx.notify();
    }

    #[cfg(feature = "upstream-candidate")]
    fn apply_a11y_set_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = single_line(value);
        match self.model.a11y_set_value(value.as_ref()) {
            None => {}
            Some(Ok(changed)) => {
                self.after_text_change(changed, cx);
                if changed {
                    window.invalidate_character_coordinates();
                }
            }
            Some(Err(rejection)) => self.reject_edit(rejection, cx),
        }
    }

    #[cfg(feature = "upstream-candidate")]
    fn apply_a11y_replace_selected(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = single_line(value);
        match self.model.a11y_replace_selected(value.as_ref()) {
            None => {}
            Some(Ok(changed)) => {
                self.after_text_change(changed, cx);
                if changed {
                    window.invalidate_character_coordinates();
                }
            }
            Some(Err(rejection)) => self.reject_edit(rejection, cx),
        }
    }

    #[cfg(feature = "upstream-candidate")]
    fn apply_a11y_selection(
        &mut self,
        snapshot: &a11y::A11ySnapshot,
        selection: TextSelection,
        cx: &mut Context<Self>,
    ) {
        // The action refers to character indices in the run that was published
        // for `snapshot.text`. If the text has moved on, applying them would
        // select the wrong span, so drop the stale request.
        if self.model.content() != snapshot.text.as_str() {
            return;
        }

        if let Some((anchor, focus)) = snapshot.selection_byte_offsets(&selection)
            && self.model.set_selection_anchor_focus(anchor, focus)
        {
            cx.notify();
        }
    }

    /// Geometry for the AccessKit text run, shaped for the current text.
    ///
    /// The painted line is shaped during the child element's prepaint, after
    /// the parent's synthetic a11y children are built. Reuse it when it already
    /// matches, and otherwise shape once here so the same frame's tree carries
    /// caret and selection rectangles. This only runs while a screen reader is
    /// attached.
    #[cfg(feature = "upstream-candidate")]
    fn a11y_layout(
        &self,
        window: &Window,
        content: &str,
    ) -> Option<(ShapedLine, Bounds<Pixels>, f32)> {
        let bounds = self.last_bounds?;
        let scale_factor = window.scale_factor();

        if let Some(line) = self.last_layout.as_ref()
            && line.text.as_str() == content
        {
            return Some((line.clone(), bounds, scale_factor));
        }

        if content.is_empty() {
            return None;
        }

        let style = window.text_style();
        let run = TextRun {
            len: content.len(),
            font: style.font(),
            color: self.theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        // `render` runs before this field's `text_size` style is applied to the
        // window, so use the same constant the renderer paints with.
        let line =
            window
                .text_system()
                .shape_line(content.to_owned().into(), px(TEXT_SIZE), &[run], None);

        Some((line, bounds, scale_factor))
    }

    /// Clear text, selection and any marked composition.
    ///
    /// Undo and redo history are always discarded, even when the field was
    /// already empty: a cleared query is transient and must not be restorable
    /// with Ctrl+Z. Emits [`Changed`] only when text was actually removed. The
    /// window hook repositions the platform IME candidate panel after the
    /// programmatic edit.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_content = self.model.clear();
        self.last_layout = None;
        self.last_bounds = None;
        self.is_selecting = false;

        if had_content {
            cx.emit(Changed(String::new()));
        }
        cx.notify();
        window.invalidate_character_coordinates();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.model.selected_range().is_empty() {
            let offset = self.model.previous_boundary(self.model.cursor_offset());
            self.move_to(offset, cx);
        } else {
            self.move_to(self.model.selected_range().start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.model.selected_range().is_empty() {
            let offset = self.model.next_boundary(self.model.cursor_offset());
            self.move_to(offset, cx);
        } else {
            self.move_to(self.model.selected_range().end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.model.previous_boundary(self.model.cursor_offset());
        self.select_to(offset, cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.model.next_boundary(self.model.cursor_offset());
        self.select_to(offset, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.model.select_all();
        cx.notify();
    }

    fn undo_action(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if self.undo(cx) {
            window.invalidate_character_coordinates();
        }
    }

    fn redo_action(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if self.redo(cx) {
            window.invalidate_character_coordinates();
        }
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.model.content().len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.selected_range().is_empty() {
            let offset = self.model.previous_boundary(self.model.cursor_offset());
            self.select_to(offset, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.selected_range().is_empty() {
            let offset = self.model.next_boundary(self.model.cursor_offset());
            self.select_to(offset, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        if event.modifiers.shift {
            let offset = self.index_for_mouse_position(event.position);
            self.select_to(offset, cx);
        } else {
            let offset = self.index_for_mouse_position(event.position);
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            let offset = self.index_for_mouse_position(event.position);
            self.select_to(offset, cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = single_line(&text);
            self.replace_text_in_range(None, text.as_ref(), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let selected_range = self.model.selected_range();
        if !selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.model.content()[selected_range].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        let selected_range = self.model.selected_range();
        if !selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.model.content()[selected_range].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.model.move_to(offset);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.model.select_to(offset);
        cx.notify();
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.model.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.model.content().len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    /// Emit the single content-change event and wake observers.
    fn after_text_change(&mut self, changed: bool, cx: &mut Context<Self>) {
        if changed {
            cx.emit(Changed(self.model.content().to_owned()));
        }
        cx.notify();
    }

    /// Report a refused edit without touching text, selection or history.
    fn reject_edit(&self, rejection: EditRejection, cx: &mut Context<Self>) {
        match rejection {
            EditRejection::TextTooLong { .. } => ring_bell(),
        }
        cx.emit(EditRejected { reason: rejection });
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.model.range_from_utf16(&range_utf16);
        actual_range.replace(self.model.range_to_utf16(&range));
        Some(self.model.content()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let range = self.model.selected_range();
        Some(UTF16Selection {
            range: self.model.range_to_utf16(&range),
            reversed: self.model.selection_reversed(),
        })
    }

    /// Programmatic selection changes from the platform's text service.
    #[cfg(feature = "upstream-candidate")]
    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model.is_composing() {
            return;
        }

        // Round a native UTF-16 range down to scalar and then UTF-8
        // boundaries. A code-unit offset inside a surrogate pair cannot select
        // half an astral character.
        let content = self.model.content();
        let start = a11y::byte_offset_for_scalar(
            content,
            a11y::scalar_index_for_utf16(content, range_utf16.start),
        )
        .unwrap_or(0);
        let end = a11y::byte_offset_for_scalar(
            content,
            a11y::scalar_index_for_utf16(content, range_utf16.end),
        )
        .unwrap_or(content.len());

        self.model.set_selected_range(start..end);
        cx.notify();
    }

    /// Total document length in UTF-16 code units, for native text services.
    #[cfg(feature = "upstream-candidate")]
    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let content = self.model.content();
        Some(
            a11y::utf16_offset_for_scalar(content, a11y::scalar_count(content))
                .unwrap_or_else(|| content.encode_utf16().count()),
        )
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.model
            .marked_range()
            .as_ref()
            .map(|range| self.model.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.model.unmark();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_text = single_line(new_text);
        match self.model.replace_text(range_utf16, new_text.as_ref()) {
            Ok(changed) => self.after_text_change(changed, cx),
            Err(rejection) => self.reject_edit(rejection, cx),
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_text = single_line(new_text);
        match self.model.replace_and_mark_text(
            range_utf16,
            new_text.as_ref(),
            new_selected_range_utf16,
        ) {
            Ok(changed) => self.after_text_change(changed, cx),
            Err(rejection) => self.reject_edit(rejection, cx),
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.model.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        if self.model.is_empty() {
            return Some(0);
        }

        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.model.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

#[cfg(not(feature = "upstream-candidate"))]
impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.model.content().to_owned();
        let selected_range = input.model.selected_range();
        let marked_range = input.model.marked_range();
        let cursor = input.model.cursor_offset();
        let theme = input.theme;
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), theme.muted_text)
        } else {
            (content.into(), theme.text)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(theme.accent),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.0), bounds.bottom() - bounds.top()),
                    ),
                    theme.accent,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    theme.selection,
                )),
                None,
            )
        };

        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        let line = prepaint
            .line
            .take()
            .expect("shaped line is always prepared");
        #[cfg(not(feature = "upstream-candidate"))]
        let painted = line.paint(bounds.origin, window.line_height(), window, cx);
        #[cfg(feature = "upstream-candidate")]
        let painted = line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
        painted.expect("text paint should not fail");

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "upstream-candidate")]
        let a11y_snapshot = {
            let (anchor, focus) = self.model.selection_anchor_focus();
            let layout = if _window.is_a11y_active() {
                self.a11y_layout(_window, self.model.content())
            } else {
                None
            };
            let layout = layout
                .as_ref()
                .map(|(line, bounds, scale_factor)| (line, *bounds, *scale_factor));
            a11y::A11ySnapshot::new(self.model.content(), anchor, focus, layout)
        };

        let root = div()
            .id(self.field_id.clone())
            .flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::undo_action))
            .on_action(cx.listener(Self::redo_action))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .h(px(32.0))
            .line_height(px(32.0))
            .text_size(px(TEXT_SIZE))
            .text_color(self.theme.text)
            .overflow_hidden();

        #[cfg(feature = "upstream-candidate")]
        let root = {
            let run_snapshot = a11y_snapshot.clone();
            let selection_snapshot = a11y_snapshot.clone();
            let value_input = cx.entity().downgrade();
            let replace_input = cx.entity().downgrade();
            let selection_input = cx.entity().downgrade();

            root.role(self.accessibility_role)
                .aria_label(self.accessibility_label.clone())
                .aria_value(self.model.content().to_owned())
                .aria_placeholder(self.placeholder.clone())
                .a11y_synthetic_children(move |builder| run_snapshot.push(builder))
                .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                    let Some(ActionData::Value(value)) = data else {
                        return;
                    };
                    let value: &str = &**value;
                    if value_input
                        .update(cx, |input, cx| {
                            input.apply_a11y_set_value(value, window, cx);
                        })
                        .is_err()
                    {
                        // The field was released between the tree update and this request.
                    }
                })
                .on_a11y_action(
                    AccessibleAction::ReplaceSelectedText,
                    move |data, window, cx| {
                        let Some(ActionData::Value(value)) = data else {
                            return;
                        };
                        let value: &str = &**value;
                        if replace_input
                            .update(cx, |input, cx| {
                                input.apply_a11y_replace_selected(value, window, cx);
                            })
                            .is_err()
                        {
                            // The field was released between the tree update and this request.
                        }
                    },
                )
                .on_a11y_action(
                    AccessibleAction::SetTextSelection,
                    move |data, _window, cx| {
                        let Some(ActionData::SetTextSelection(selection)) = data else {
                            return;
                        };
                        let selection = *selection;
                        if selection_input
                            .update(cx, |input, cx| {
                                input.apply_a11y_selection(&selection_snapshot, selection, cx);
                            })
                            .is_err()
                        {
                            // The field was released between the tree update and this request.
                        }
                    },
                )
        };

        root.child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<Changed> for TextInput {}
impl EventEmitter<EditRejected> for TextInput {}

/// Install the input's keybindings for the `TextInput` key context.
///
/// Uses Command on macOS and Control on Windows and other platforms. Undo is
/// `cmd-z` on macOS and `ctrl-z` elsewhere; redo is `cmd-shift-z` on macOS and
/// `ctrl-shift-z` or `ctrl-y` elsewhere. Call this once while constructing the
/// application.
pub fn install_keybindings(cx: &mut App) {
    #[cfg(target_os = "macos")]
    const UNDO: &str = "cmd-z";
    #[cfg(target_os = "macos")]
    const SELECT_ALL: &str = "cmd-a";
    #[cfg(target_os = "macos")]
    const PASTE: &str = "cmd-v";
    #[cfg(target_os = "macos")]
    const COPY: &str = "cmd-c";
    #[cfg(target_os = "macos")]
    const CUT: &str = "cmd-x";

    #[cfg(not(target_os = "macos"))]
    const UNDO: &str = "ctrl-z";
    #[cfg(not(target_os = "macos"))]
    const SELECT_ALL: &str = "ctrl-a";
    #[cfg(not(target_os = "macos"))]
    const PASTE: &str = "ctrl-v";
    #[cfg(not(target_os = "macos"))]
    const COPY: &str = "ctrl-c";
    #[cfg(not(target_os = "macos"))]
    const CUT: &str = "ctrl-x";

    let mut bindings = vec![
        KeyBinding::new("backspace", Backspace, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", Delete, Some(KEY_CONTEXT)),
        KeyBinding::new("left", Left, Some(KEY_CONTEXT)),
        KeyBinding::new("right", Right, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(KEY_CONTEXT)),
        KeyBinding::new(UNDO, Undo, Some(KEY_CONTEXT)),
        KeyBinding::new(SELECT_ALL, SelectAll, Some(KEY_CONTEXT)),
        KeyBinding::new(PASTE, Paste, Some(KEY_CONTEXT)),
        KeyBinding::new(COPY, Copy, Some(KEY_CONTEXT)),
        KeyBinding::new(CUT, Cut, Some(KEY_CONTEXT)),
        KeyBinding::new("home", Home, Some(KEY_CONTEXT)),
        KeyBinding::new("end", End, Some(KEY_CONTEXT)),
    ];

    #[cfg(target_os = "macos")]
    let character_palette = vec![KeyBinding::new(
        "ctrl-cmd-space",
        ShowCharacterPalette,
        Some(KEY_CONTEXT),
    )];
    #[cfg(not(target_os = "macos"))]
    let character_palette: Vec<KeyBinding> = Vec::new();

    #[cfg(target_os = "macos")]
    let redo = vec![KeyBinding::new("cmd-shift-z", Redo, Some(KEY_CONTEXT))];
    #[cfg(not(target_os = "macos"))]
    let redo = vec![
        KeyBinding::new("ctrl-shift-z", Redo, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-y", Redo, Some(KEY_CONTEXT)),
    ];

    bindings.extend(character_palette);
    bindings.extend(redo);
    cx.bind_keys(bindings);
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBeep(u_type: u32) -> i32;
}

#[cfg(target_os = "macos")]
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSBeep();
}

/// Ring the platform's standard attention sound for a refused edit.
///
/// GPUI 0.2.2 exposes no bell API, so this calls the native primitive directly
/// and does nothing on platforms without one.
#[cfg(target_os = "windows")]
fn ring_bell() {
    // `u32::MAX` asks Windows to play the default system sound.
    unsafe {
        MessageBeep(u32::MAX);
    }
}

#[cfg(target_os = "macos")]
fn ring_bell() {
    unsafe {
        NSBeep();
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn ring_bell() {
    // No portable attention sound exists; observers can react to EditRejected.
}

/// Flatten platform-inserted text to a single line.
fn single_line(text: &str) -> Cow<'_, str> {
    if text
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '\t'))
    {
        Cow::Owned(
            text.chars()
                .map(|character| match character {
                    '\r' | '\n' | '\t' => ' ',
                    other => other,
                })
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::{EditRejected, EditRejection, MAX_TEXT_SCALARS, single_line};

    #[test]
    fn inserted_text_is_flattened_to_one_line() {
        assert_eq!(single_line("one line").as_ref(), "one line");
        assert_eq!(
            single_line("one\ntwo\r\nthree\tfour").as_ref(),
            "one two  three four"
        );
        assert!(matches!(
            single_line("already flat"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn rejected_edit_carries_a_status_message() {
        let rejection = EditRejection::TextTooLong {
            limit: MAX_TEXT_SCALARS,
            would_be: MAX_TEXT_SCALARS + 1,
        };
        let event = EditRejected { reason: rejection };

        assert_eq!(event.message(), "Search is limited to 4,096 characters");
        assert_eq!(event.reason, rejection);
    }
}

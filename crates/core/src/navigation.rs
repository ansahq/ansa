//! Typed child-view stack and the explicit Escape hierarchy.
//!
//! Every non-root surface (for example About or Integration Status) is a
//! [`ViewFrame`] identified by an opaque [`ViewId`]. Pushing a frame captures
//! the parent's query text, selected command identity, logical scroll anchor and
//! action-panel marker. Popping returns exactly one [`RestoredView`] and marks
//! the frame consumed, so a delayed callback cannot restore the same parent
//! twice.
//!
//! Identity rules: a [`ViewId`] is a short ASCII label such as `ansa:about`.
//! Whitespace and user-facing prose are rejected, so a view can never be
//! identified by whatever text the user typed.
//!
//! Escape never uses stringly comparisons. [`EscapeAction`] is a typed plan:
//! cancel an IME composition, close the action panel, restore one view, hide the
//! root, or do nothing.

use std::fmt;

use ansa_domain::CommandId;

/// Maximum number of child frames held at once.
///
/// The launcher has no use for deep navigation, and a hard bound keeps a buggy
/// caller from growing the stack without limit.
pub const MAX_VIEW_DEPTH: usize = 32;

/// Maximum number of Unicode scalar values in a [`ViewId`].
pub const MAX_VIEW_ID_CHARS: usize = 64;

/// Why a view identity or push was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewError {
    /// The identity was empty.
    EmptyIdentity,
    /// The identity exceeded [`MAX_VIEW_ID_CHARS`].
    IdentityTooLong,
    /// The identity contained whitespace, a control character or other
    /// punctuation that would make it display text rather than a stable label.
    InvalidIdentityCharacter { character: char },
    /// [`MAX_VIEW_DEPTH`] frames were already pushed.
    DepthExceeded { limit: usize },
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyIdentity => f.write_str("view identity must not be empty"),
            Self::IdentityTooLong => write!(
                f,
                "view identity must be at most {MAX_VIEW_ID_CHARS} characters"
            ),
            Self::InvalidIdentityCharacter { character } => {
                write!(f, "view identity contains invalid character {character:?}")
            }
            Self::DepthExceeded { limit } => {
                write!(f, "view stack depth limit of {limit} reached")
            }
        }
    }
}

impl std::error::Error for ViewError {}

/// Opaque, validated identity of a child view.
///
/// Only short ASCII labels are accepted. Display text (spaces, punctuation)
/// cannot become an identity, which is what keeps view comparisons typed
/// instead of stringly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewId(String);

impl ViewId {
    /// Validates a stable view label.
    pub fn new(raw: impl Into<String>) -> Result<Self, ViewError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(ViewError::EmptyIdentity);
        }
        if raw.chars().count() > MAX_VIEW_ID_CHARS {
            return Err(ViewError::IdentityTooLong);
        }
        if let Some(character) = raw
            .chars()
            .find(|character| !is_label_character(*character))
        {
            return Err(ViewError::InvalidIdentityCharacter { character });
        }
        Ok(Self(raw))
    }

    /// The identity of the launcher root. The root is never a pushed frame.
    pub fn root() -> Self {
        Self("root".to_owned())
    }

    /// The stable label, e.g. `ansa:about`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ViewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_label_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
}

/// A logical scroll position, expressed as a result row rather than pixels.
///
/// The renderer maps this back to its own scroll container. Pixel-exact,
/// pointer-driven restoration is a renderer concern and is not claimed here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollAnchor {
    row: u32,
}

impl ScrollAnchor {
    /// The top of the list.
    pub const TOP: Self = Self { row: 0 };

    /// An anchor at `row`, saturated to the platform-neutral `u32` range.
    pub fn at(row: usize) -> Self {
        Self {
            row: row.min(u32::MAX as usize) as u32,
        }
    }

    /// The anchored row.
    pub fn row(self) -> usize {
        self.row as usize
    }
}

/// Typed marker for the parent's action-panel state.
///
/// The engine captures whether the panel was open; the renderer owns the panel
/// contents and decides whether to reopen it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ParentActions {
    /// No action panel was open when the child view was pushed.
    #[default]
    Closed,
    /// An action panel was open when the child view was pushed.
    Open,
}

/// The parent state captured when a child view was pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentState {
    query: String,
    selected: Option<CommandId>,
    scroll: ScrollAnchor,
    actions: ParentActions,
}

impl ParentState {
    pub fn new(
        query: impl Into<String>,
        selected: Option<CommandId>,
        scroll: ScrollAnchor,
        actions: ParentActions,
    ) -> Self {
        Self {
            query: query.into(),
            selected,
            scroll,
            actions,
        }
    }

    /// Exact query text the parent had.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Stable identity of the parent's selected command, if any.
    pub fn selected(&self) -> Option<&CommandId> {
        self.selected.as_ref()
    }

    /// Logical scroll anchor the parent had.
    pub fn scroll(&self) -> ScrollAnchor {
        self.scroll
    }

    /// Typed marker for the parent's action panel.
    pub fn actions(&self) -> ParentActions {
        self.actions
    }
}

/// One pushed child view plus the parent state it captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFrame {
    id: ViewId,
    parent: ParentState,
    generation: u64,
    consumed: bool,
}

impl ViewFrame {
    /// The child view's identity.
    pub fn id(&self) -> &ViewId {
        &self.id
    }

    /// The captured parent state.
    pub fn parent(&self) -> &ParentState {
        &self.parent
    }

    /// Monotonically increasing generation assigned when this frame was pushed.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether this frame has already been restored (popped) once.
    pub fn is_consumed(&self) -> bool {
        self.consumed
    }
}

/// The single restore result a pop produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredView {
    view: ViewId,
    query: String,
    selected: Option<CommandId>,
    scroll: ScrollAnchor,
    actions: ParentActions,
    frame_generation: u64,
    generation: u64,
}

impl RestoredView {
    /// The child view that was closed.
    pub fn view(&self) -> &ViewId {
        &self.view
    }

    /// The parent query text to re-establish, exactly once.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The parent selected command identity, if any.
    pub fn selected(&self) -> Option<&CommandId> {
        self.selected.as_ref()
    }

    /// The parent logical scroll anchor.
    pub fn scroll(&self) -> ScrollAnchor {
        self.scroll
    }

    /// The parent's action-panel marker.
    pub fn actions(&self) -> ParentActions {
        self.actions
    }

    /// Generation of the frame that was consumed.
    pub fn frame_generation(&self) -> u64 {
        self.frame_generation
    }

    /// Generation issued after the pop. Any action reference issued before the
    /// push carries an older generation and is therefore stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// Typed result of the Escape hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscapeAction {
    /// An active IME composition/modal owns Escape: cancel it and consume the
    /// key. The host must not dismiss anything.
    CancelComposition,
    /// Close the action panel.
    CloseActions,
    /// Pop one child view and re-establish the captured parent state.
    RestoreView(RestoredView),
    /// Hide the root launcher.
    HideRoot,
    /// Nothing was open; Escape is a no-op.
    None,
}

/// Bounded, restore-once stack of child views.
#[derive(Debug, Clone, Default)]
pub struct ViewStack {
    frames: Vec<ViewFrame>,
    generation: u64,
    restored: u64,
}

impl ViewStack {
    /// An empty stack positioned at the root.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes `id`, capturing `parent`, and returns the new frame generation.
    ///
    /// Fails with [`ViewError::DepthExceeded`] once [`MAX_VIEW_DEPTH`] frames
    /// are present.
    pub fn push(&mut self, id: ViewId, parent: ParentState) -> Result<u64, ViewError> {
        if self.frames.len() >= MAX_VIEW_DEPTH {
            return Err(ViewError::DepthExceeded {
                limit: MAX_VIEW_DEPTH,
            });
        }
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        self.frames.push(ViewFrame {
            id,
            parent,
            generation,
            consumed: false,
        });
        Ok(generation)
    }

    /// Pops the newest frame, returning its captured parent state once.
    ///
    /// The popped frame is marked consumed before the value is returned. Since
    /// the frame also leaves the stack, a second pop can only ever pop the next
    /// frame (or the root, which is never poppable).
    pub fn pop(&mut self) -> Option<RestoredView> {
        let frame = self.frames.last_mut()?;
        if frame.consumed {
            // Defensive: a consumed frame must never be restored again.
            return None;
        }
        frame.consumed = true;
        let view = frame.id.clone();
        let parent = frame.parent.clone();
        let frame_generation = frame.generation;
        self.frames.pop();
        self.generation = self.generation.saturating_add(1);
        self.restored = self.restored.saturating_add(1);
        Some(RestoredView {
            view,
            query: parent.query,
            selected: parent.selected,
            scroll: parent.scroll,
            actions: parent.actions,
            frame_generation,
            generation: self.generation,
        })
    }

    /// The current view identity, or the root identity when no frame is pushed.
    pub fn current(&self) -> ViewId {
        self.frames
            .last()
            .map(|frame| frame.id.clone())
            .unwrap_or_else(ViewId::root)
    }

    /// Removes the newest frame without producing a restore value.
    ///
    /// This still advances the generation so action references issued in the
    /// discarded frame are stale. Returns `false` at the root.
    pub fn discard_top(&mut self) -> bool {
        if self.frames.pop().is_none() {
            return false;
        }
        self.generation = self.generation.saturating_add(1);
        true
    }

    /// Whether a child view is pushed (the root itself is not a frame).
    pub fn is_root(&self) -> bool {
        self.frames.is_empty()
    }

    /// Number of pushed child frames.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// The newest generation issued by a push or pop.
    ///
    /// Starts at zero for a fresh stack. Any action reference must match this
    /// value to belong to the current frame instance.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of frames restored so far.
    pub fn restored_count(&self) -> u64 {
        self.restored
    }

    /// The pushed frames, oldest first.
    pub fn frames(&self) -> &[ViewFrame] {
        &self.frames
    }
}

impl Default for ParentState {
    fn default() -> Self {
        Self::new(
            String::new(),
            None,
            ScrollAnchor::TOP,
            ParentActions::Closed,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(id: &str) -> CommandId {
        CommandId::parse(id).unwrap()
    }

    #[test]
    fn view_ids_reject_display_text_and_accept_labels() {
        assert!(ViewId::new("ansa:about").is_ok());
        assert!(ViewId::new("settings.v2").is_ok());
        assert_eq!(ViewId::new(""), Err(ViewError::EmptyIdentity));
        assert!(matches!(
            ViewId::new("About Ansa"),
            Err(ViewError::InvalidIdentityCharacter { character: ' ' })
        ));
        assert!(matches!(
            ViewId::new("line\nbreak"),
            Err(ViewError::InvalidIdentityCharacter { .. })
        ));
        assert_eq!(
            ViewId::new("x".repeat(MAX_VIEW_ID_CHARS + 1)),
            Err(ViewError::IdentityTooLong)
        );
        assert_eq!(ViewId::root().as_str(), "root");
    }

    #[test]
    fn push_captures_and_pop_restores_exactly_once() {
        let mut stack = ViewStack::new();
        assert_eq!(stack.depth(), 0);
        assert_eq!(stack.generation(), 0);
        assert!(stack.pop().is_none(), "root is never popped");

        let parent = ParentState::new(
            "parent query",
            Some(command("app:alpha")),
            ScrollAnchor::at(4),
            ParentActions::Open,
        );
        let frame_generation = stack
            .push(ViewId::new("ansa:about").unwrap(), parent.clone())
            .unwrap();
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current().as_str(), "ansa:about");
        assert!(frame_generation > 0);
        assert!(stack.generation() >= frame_generation);
        assert_eq!(stack.frames()[0].parent(), &parent);
        assert!(!stack.frames()[0].is_consumed());

        let restored = stack.pop().expect("one restore");
        assert_eq!(restored.view().as_str(), "ansa:about");
        assert_eq!(restored.query(), "parent query");
        assert_eq!(restored.selected(), Some(&command("app:alpha")));
        assert_eq!(restored.scroll(), ScrollAnchor::at(4));
        assert_eq!(restored.actions(), ParentActions::Open);
        assert_eq!(restored.frame_generation(), frame_generation);
        assert!(restored.generation() > frame_generation);
        assert_eq!(stack.depth(), 0);
        assert_eq!(stack.restored_count(), 1);

        assert!(stack.pop().is_none(), "a frame cannot restore twice");
        assert_eq!(stack.restored_count(), 1);
    }

    #[test]
    fn nested_frames_restore_in_reverse_order() {
        let mut stack = ViewStack::new();
        stack
            .push(
                ViewId::new("first").unwrap(),
                ParentState::new(
                    "one",
                    Some(command("app:one")),
                    ScrollAnchor::at(1),
                    ParentActions::Closed,
                ),
            )
            .unwrap();
        stack
            .push(
                ViewId::new("second").unwrap(),
                ParentState::new(
                    "two",
                    Some(command("app:two")),
                    ScrollAnchor::at(2),
                    ParentActions::Closed,
                ),
            )
            .unwrap();

        let second = stack.pop().unwrap();
        assert_eq!(second.view().as_str(), "second");
        assert_eq!(second.query(), "two");
        let first = stack.pop().unwrap();
        assert_eq!(first.view().as_str(), "first");
        assert_eq!(first.query(), "one");
        assert_ne!(second.frame_generation(), first.frame_generation());
        assert_ne!(second.generation(), first.generation());
        assert!(stack.pop().is_none());
    }

    #[test]
    fn depth_is_bounded() {
        let mut stack = ViewStack::new();
        for index in 0..MAX_VIEW_DEPTH {
            stack
                .push(
                    ViewId::new(format!("v{index}")).unwrap(),
                    ParentState::default(),
                )
                .unwrap();
        }
        assert_eq!(stack.depth(), MAX_VIEW_DEPTH);
        assert_eq!(
            stack.push(ViewId::new("overflow").unwrap(), ParentState::default()),
            Err(ViewError::DepthExceeded {
                limit: MAX_VIEW_DEPTH
            })
        );
        assert_eq!(stack.depth(), MAX_VIEW_DEPTH);
    }
}

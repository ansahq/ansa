//! Renderer-independent searchable action panel.
//!
//! A panel is a bounded snapshot of the actions available for one invocation
//! target. It does not execute anything: the host resolves an [`ActionId`] only
//! after [`ActionPanel::validated_action`] confirms that the snapshot still
//! matches the engine's current selection, query and [`QueryToken`].
//!
//! Selection is tracked by stable [`ActionId`] rather than by row position, so
//! filtering keeps the same action selected whenever it remains visible.

use ansa_domain::CommandId;

use crate::{DEFAULT_PAGE_SIZE, Engine, QueryToken};

/// Maximum number of Unicode scalar values accepted in a panel query.
///
/// The panel is an interactive filter, not a search index: bounding the query
/// keeps repeated filtering cheap even for a pasted adversarial string.
pub const MAX_PANEL_QUERY_CHARS: usize = 256;

/// Stable identity of a built-in panel action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionId {
    /// The target's default action, e.g. opening the selected command.
    Primary,
    /// Copies the target's copyable representation.
    CopyTarget,
    /// Pins or unpins the selected command.
    ToggleFavorite,
}

/// The engine state a panel was opened against.
///
/// A target is a snapshot, not a live reference. It must be revalidated before
/// invoking an action because the selection, query or invocation may have
/// changed while the panel was visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelTarget {
    /// Actions for the command selected when the panel opened.
    Command {
        id: CommandId,
        token: crate::QueryToken,
    },
    /// Actions for an exact calculator expression.
    Calculator {
        expression: String,
        token: QueryToken,
    },
}

impl PanelTarget {
    /// The query token captured when this target was created.
    pub fn token(&self) -> QueryToken {
        match self {
            Self::Command { token, .. } | Self::Calculator { token, .. } => *token,
        }
    }

    /// Whether this target still describes the engine's current state.
    ///
    /// Command targets require the same selected command identity and query
    /// token. Calculator targets require the exact same query text and token;
    /// matching the text alone is not enough because a new revision can carry
    /// a different evaluation result.
    pub fn is_current(&self, engine: &Engine) -> bool {
        self.validates_current(
            engine.query_token(),
            engine.selected().map(|command| &command.id),
            engine.query(),
        )
    }

    /// Renderer/host-facing validation without borrowing an [`Engine`].
    ///
    /// `selected_id` is ignored for calculator targets; `current_expression` is
    /// ignored for command targets.
    pub fn validates_current(
        &self,
        current_token: QueryToken,
        selected_id: Option<&CommandId>,
        current_expression: &str,
    ) -> bool {
        match self {
            Self::Command { id, token } => *token == current_token && selected_id == Some(id),
            Self::Calculator { expression, token } => {
                *token == current_token && expression.as_str() == current_expression
            }
        }
    }
}

/// One row in the action panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionEntry {
    pub id: ActionId,
    pub title: String,
    pub shortcut: Option<String>,
}

impl ActionEntry {
    pub fn new(id: ActionId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            shortcut: None,
        }
    }

    /// Sets or clears the secondary shortcut label.
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        let shortcut = shortcut.into();
        self.shortcut = if shortcut.trim().is_empty() {
            None
        } else {
            Some(shortcut)
        };
        self
    }
}

/// A searchable snapshot of actions for one invocation target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionPanel {
    target: PanelTarget,
    entries: Vec<ActionEntry>,
    query: String,
    visible: Vec<ActionEntry>,
    selected: Option<ActionId>,
}

impl ActionPanel {
    /// Opens a panel for `target` with the supplied entries.
    ///
    /// The first visible entry is selected initially. An empty entry list is
    /// valid and yields no selection.
    pub fn open(target: PanelTarget, entries: Vec<ActionEntry>) -> Self {
        let mut panel = Self {
            target,
            entries,
            query: String::new(),
            visible: Vec::new(),
            selected: None,
        };
        panel.refilter();
        panel
    }

    /// Replaces the filter query, truncating it to
    /// [`MAX_PANEL_QUERY_CHARS`].
    ///
    /// Filtering is deterministic and case-insensitive. Every whitespace-
    /// separated query token must appear as a substring of an entry title.
    /// The previously selected action stays selected when it is still visible;
    /// otherwise the first visible entry becomes selected.
    pub fn set_query(&mut self, query: &str) {
        self.query = query.chars().take(MAX_PANEL_QUERY_CHARS).collect();
        self.refilter();
    }

    /// The bounded raw query, exactly as supplied by the panel search field.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Every entry supplied when the panel opened, in original order.
    pub fn entries(&self) -> &[ActionEntry] {
        &self.entries
    }

    /// Entries matching the current query, in original order.
    pub fn visible_entries(&self) -> &[ActionEntry] {
        &self.visible
    }

    /// The selected action's stable identity, if any row is visible.
    pub fn selected_id(&self) -> Option<ActionId> {
        self.selected_index().map(|index| self.visible[index].id)
    }

    /// The selected position within [`ActionPanel::visible_entries`].
    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.visible.iter().position(|entry| entry.id == id)
    }

    /// The selected entry, or `None` when the panel has no visible rows.
    pub fn selected(&self) -> Option<&ActionEntry> {
        self.selected_index()
            .and_then(|index| self.visible.get(index))
    }

    /// Selects a visible row by position. Returns `false` when out of range.
    pub fn select_index(&mut self, index: usize) -> bool {
        let Some(entry) = self.visible.get(index) else {
            return false;
        };
        self.selected = Some(entry.id);
        true
    }

    /// Moves selection by `delta` rows, clamped to the visible list.
    ///
    /// Negative values move up. At the first or last row the selection stays
    /// put rather than wrapping. With no visible rows the selection is cleared.
    pub fn move_selection(&mut self, delta: i32) {
        if self.visible.is_empty() {
            self.selected = None;
            return;
        }

        let current = self.selected_index().unwrap_or(0);
        let last = self.visible.len() - 1;
        let next = (current as i64 + delta as i64).clamp(0, last as i64) as usize;
        self.selected = Some(self.visible[next].id);
    }

    /// Moves by `page_size` rows in `direction` (negative is up).
    pub fn move_selection_page(&mut self, direction: i32, page_size: usize) {
        if direction == 0 || page_size == 0 {
            return;
        }
        let amount = page_size.min(i32::MAX as usize) as i32;
        self.move_selection(if direction < 0 { -amount } else { amount });
    }

    /// Page Up using [`DEFAULT_PAGE_SIZE`].
    pub fn page_up(&mut self) {
        self.move_selection(-(DEFAULT_PAGE_SIZE as i32));
    }

    /// Page Down using [`DEFAULT_PAGE_SIZE`].
    pub fn page_down(&mut self) {
        self.move_selection(DEFAULT_PAGE_SIZE as i32);
    }

    /// The snapshot target this panel was opened for.
    pub fn target(&self) -> &PanelTarget {
        &self.target
    }

    /// Whether the target still matches the engine.
    pub fn is_current(&self, engine: &Engine) -> bool {
        self.target.is_current(engine)
    }

    /// The selected entry, but only when the snapshot target is still current.
    ///
    /// A stale panel yields `None` instead of running an action against a
    /// command, query or invocation that no longer exists.
    pub fn validated_entry(&self, engine: &Engine) -> Option<&ActionEntry> {
        if self.is_current(engine) {
            self.selected()
        } else {
            None
        }
    }

    /// The selected action, but only when the snapshot target is still current.
    ///
    /// This is the model's execution gate. Calling it has no side effects; the
    /// host executes the returned action only after this check succeeds.
    pub fn validated_action(&self, engine: &Engine) -> Option<ActionId> {
        self.validated_entry(engine).map(|entry| entry.id)
    }

    fn refilter(&mut self) {
        let tokens: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();

        if tokens.is_empty() {
            self.visible.clone_from(&self.entries);
        } else {
            self.visible.clear();
            self.visible.extend(
                self.entries
                    .iter()
                    .filter(|entry| {
                        let title = entry.title.to_lowercase();
                        tokens.iter().all(|token| title.contains(token))
                    })
                    .cloned(),
            );
        }

        let retained = self
            .selected
            .filter(|id| self.visible.iter().any(|entry| entry.id == *id));
        self.selected = retained.or_else(|| self.visible.first().map(|entry| entry.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansa_domain::{BuiltinCommand, Command, CommandKind};

    fn command(id: &str, title: &str) -> Command {
        Command::new(
            CommandId::parse(id).unwrap(),
            title,
            CommandKind::Builtin(BuiltinCommand::About),
        )
    }

    fn engine() -> Engine {
        Engine::new(vec![
            command("app:alpha", "Alpha"),
            command("app:beta", "Beta"),
        ])
    }

    fn entries(pin_title: &str) -> Vec<ActionEntry> {
        vec![
            ActionEntry::new(ActionId::Primary, "Open selected").with_shortcut("Enter"),
            ActionEntry::new(ActionId::CopyTarget, "Copy target").with_shortcut("Ctrl+Shift+C"),
            ActionEntry::new(ActionId::ToggleFavorite, pin_title),
        ]
    }

    fn command_panel(engine: &Engine, pin_title: &str) -> ActionPanel {
        ActionPanel::open(
            PanelTarget::Command {
                id: engine.selected().unwrap().id.clone(),
                token: engine.query_token(),
            },
            entries(pin_title),
        )
    }

    #[test]
    fn filters_case_insensitively_by_title_token() {
        let engine = engine();
        let mut panel = command_panel(&engine, "Pin to favorites");

        panel.set_query("OPEN sel");
        assert_eq!(panel.query(), "OPEN sel");
        assert_eq!(panel.visible_entries().len(), 1);
        assert_eq!(panel.selected().unwrap().id, ActionId::Primary);

        panel.set_query("TARGET");
        assert_eq!(panel.visible_entries().len(), 1);
        assert_eq!(panel.selected().unwrap().id, ActionId::CopyTarget);

        panel.set_query("favorites");
        assert_eq!(panel.visible_entries().len(), 1);
        assert_eq!(panel.selected().unwrap().id, ActionId::ToggleFavorite);

        panel.set_query("");
        assert_eq!(panel.visible_entries().len(), 3);
    }

    #[test]
    fn bounds_query_length_by_characters() {
        let engine = engine();
        let mut panel = command_panel(&engine, "Pin to favorites");
        let long = "x".repeat(MAX_PANEL_QUERY_CHARS + 40);
        panel.set_query(&long);

        assert_eq!(panel.query().chars().count(), MAX_PANEL_QUERY_CHARS);
        assert!(panel.visible_entries().is_empty());
    }

    #[test]
    fn no_match_means_no_selection_and_no_action() {
        let engine = engine();
        let mut panel = command_panel(&engine, "Pin to favorites");

        panel.set_query("no-such-action");
        assert!(panel.visible_entries().is_empty());
        assert!(panel.selected().is_none());
        assert_eq!(panel.selected_index(), None);
        assert_eq!(panel.selected_id(), None);
        assert!(panel.validated_action(&engine).is_none());

        panel.move_selection(1);
        assert!(panel.selected().is_none());
        assert!(!panel.select_index(0));
    }

    #[test]
    fn selection_preserves_stable_id_across_filtering() {
        let engine = engine();
        let mut panel = command_panel(&engine, "Pin to favorites");
        assert_eq!(panel.selected().unwrap().id, ActionId::Primary);

        assert!(panel.select_index(2));
        assert_eq!(panel.selected_id(), Some(ActionId::ToggleFavorite));

        panel.set_query("pin");
        assert_eq!(panel.visible_entries().len(), 1);
        assert_eq!(panel.selected_id(), Some(ActionId::ToggleFavorite));

        panel.set_query("");
        assert_eq!(panel.selected_id(), Some(ActionId::ToggleFavorite));

        panel.set_query("target");
        assert_eq!(panel.selected_id(), Some(ActionId::CopyTarget));

        panel.set_query("open");
        assert_eq!(panel.selected_id(), Some(ActionId::Primary));
    }

    #[test]
    fn move_selection_clamps_and_pages() {
        let engine = engine();
        let mut panel = command_panel(&engine, "Pin to favorites");

        panel.move_selection(-5);
        assert_eq!(panel.selected_index(), Some(0));
        panel.move_selection(2);
        assert_eq!(panel.selected_index(), Some(2));
        panel.move_selection(99);
        assert_eq!(panel.selected_index(), Some(2));
        panel.move_selection(-1);
        assert_eq!(panel.selected_index(), Some(1));

        panel.move_selection_page(1, 1);
        assert_eq!(panel.selected_index(), Some(2));
        panel.move_selection_page(-1, 1);
        assert_eq!(panel.selected_index(), Some(1));
        panel.move_selection_page(1, 0);
        assert_eq!(panel.selected_index(), Some(1));
        panel.move_selection_page(0, 8);
        assert_eq!(panel.selected_index(), Some(1));

        panel.page_down();
        assert_eq!(panel.selected_index(), Some(2));
        panel.page_up();
        assert_eq!(panel.selected_index(), Some(0));
    }

    #[test]
    fn stale_command_revision_is_rejected() {
        let mut engine = engine();
        let panel = command_panel(&engine, "Pin to favorites");
        assert_eq!(panel.validated_action(&engine), Some(ActionId::Primary));

        engine.set_query("alpha");
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:alpha");
        assert!(!panel.is_current(&engine));
        assert_eq!(panel.validated_action(&engine), None);
        assert_eq!(panel.selected_id(), Some(ActionId::Primary));
    }

    #[test]
    fn stale_invocation_is_rejected() {
        let mut engine = engine();
        let panel = command_panel(&engine, "Pin to favorites");

        engine.begin_invocation();
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:alpha");
        assert!(!panel.is_current(&engine));
        assert!(panel.validated_entry(&engine).is_none());
        assert_eq!(panel.validated_action(&engine), None);
    }

    #[test]
    fn command_target_requires_same_selected_identity() {
        let mut engine = engine();
        assert!(engine.select_index(0));
        let panel = command_panel(&engine, "Pin to favorites");

        assert!(engine.select_index(1));
        assert_ne!(engine.selected().unwrap().id.as_str(), "app:alpha");
        assert!(!panel.is_current(&engine));
        assert_eq!(panel.validated_action(&engine), None);
    }

    #[test]
    fn calculator_target_requires_exact_query_and_token() {
        let mut engine = Engine::default();
        engine.set_query("1 + 2");
        let token = engine.query_token();
        let panel = ActionPanel::open(
            PanelTarget::Calculator {
                expression: "1 + 2".into(),
                token,
            },
            entries("Pin to favorites"),
        );

        assert!(panel.is_current(&engine));
        assert_eq!(panel.validated_action(&engine), Some(ActionId::Primary));

        engine.set_query("1 + 3");
        assert!(!panel.is_current(&engine));
        assert_eq!(panel.validated_action(&engine), None);

        // The same text in a later revision produces a new token and must not
        // revive the old snapshot.
        engine.set_query("1 + 2");
        assert_eq!(engine.query(), "1 + 2");
        assert!(!panel.is_current(&engine));
        assert_eq!(panel.validated_action(&engine), None);
    }

    #[test]
    fn calculator_expression_mismatch_is_rejected_with_matching_token() {
        let mut engine = Engine::default();
        engine.set_query("1 + 2");
        let token = engine.query_token();
        let target = PanelTarget::Calculator {
            expression: "1 + 2".into(),
            token,
        };

        assert!(target.validates_current(token, None, "1 + 2"));
        assert!(!target.validates_current(token, None, "1 + 3"));
        assert!(!target.validates_current(token, None, ""));
    }

    #[test]
    fn calculator_target_rejects_stale_invocation_with_same_query() {
        let mut engine = Engine::default();
        engine.set_query("2 * 3");
        let panel = ActionPanel::open(
            PanelTarget::Calculator {
                expression: "2 * 3".into(),
                token: engine.query_token(),
            },
            entries("Pin to favorites"),
        );
        assert!(panel.is_current(&engine));

        engine.begin_invocation();
        assert_eq!(engine.query(), "2 * 3");
        assert!(!panel.is_current(&engine));
        assert_eq!(panel.validated_action(&engine), None);
    }

    #[test]
    fn favorites_filtering_follows_supplied_pin_title() {
        let engine = engine();
        let mut pinned = command_panel(&engine, "Pin to favorites");
        pinned.set_query("unpin");
        assert!(pinned.visible_entries().is_empty());
        assert!(pinned.selected().is_none());

        let mut unpinned = command_panel(&engine, "Unpin from favorites");
        unpinned.set_query("unpin");
        assert_eq!(unpinned.visible_entries().len(), 1);
        assert_eq!(unpinned.selected_id(), Some(ActionId::ToggleFavorite));

        unpinned.set_query("favorites");
        assert_eq!(unpinned.visible_entries().len(), 1);
        assert_eq!(unpinned.selected_id(), Some(ActionId::ToggleFavorite));
    }

    #[test]
    fn empty_entries_are_safe() {
        let mut engine = Engine::default();
        engine.set_query("1 + 2");
        let mut panel = ActionPanel::open(
            PanelTarget::Calculator {
                expression: "1 + 2".into(),
                token: engine.query_token(),
            },
            Vec::new(),
        );

        assert!(panel.entries().is_empty());
        assert!(panel.visible_entries().is_empty());
        assert!(panel.selected().is_none());
        assert_eq!(panel.selected_index(), None);
        assert!(!panel.select_index(0));
        panel.move_selection(1);
        panel.page_down();
        assert!(panel.selected().is_none());

        // The target itself is current, but there is no action to run.
        assert!(panel.is_current(&engine));
        assert_eq!(panel.validated_action(&engine), None);
    }

    #[test]
    fn target_accessor_and_pure_validation_agree() {
        let engine = engine();
        let panel = command_panel(&engine, "Pin to favorites");
        let selected = engine.selected().unwrap().id.clone();

        assert_eq!(
            *panel.target(),
            PanelTarget::Command {
                id: selected.clone(),
                token: engine.query_token(),
            }
        );
        assert!(panel.target().validates_current(
            engine.query_token(),
            Some(&selected),
            engine.query()
        ));
        assert!(
            !panel
                .target()
                .validates_current(engine.query_token(), None, engine.query())
        );
        assert!(panel.target().validates_current(
            engine.query_token(),
            Some(&selected),
            "unrelated"
        ));
    }

    #[test]
    fn repeated_queries_do_not_execute_or_mutate_engine() {
        let engine = engine();
        let panel = command_panel(&engine, "Pin to favorites");
        let before_query = engine.query().to_owned();
        let before_selection = engine.selected().unwrap().id.clone();

        for query in ["open", "zzz", "copy", "favorites"] {
            let mut local = panel.clone();
            local.set_query(query);
            let _ = local.validated_action(&engine);
            assert_eq!(engine.query(), before_query.as_str());
            assert_eq!(engine.selected().unwrap().id, before_selection);
        }
    }
}

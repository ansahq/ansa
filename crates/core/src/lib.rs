//! Renderer-independent launcher orchestration.
//!
//! [`Engine`] owns the command catalog, the current query, ranked results,
//! stable selection, the action panel, view-stack navigation, and query
//! revision/invocation tokens used to reject late asynchronous batches.
//!
//! The UI layer should treat this type as the source of truth for launcher
//! state. Rendering, platform focus, IME ownership, and actual execution are
//! intentionally outside this crate.

pub mod action_panel;
pub mod action_state;
pub mod calculator;
pub mod navigation;
pub mod query;
pub mod refresh;

use ansa_domain::{Command, CommandId};
use ansa_search::{SearchSignals, rank};
use navigation::{
    EscapeAction, ParentActions, ParentState, RestoredView, ScrollAnchor, ViewId, ViewStack,
};
use std::collections::HashSet;
use std::sync::OnceLock;

pub use ansa_search::{MAX_USAGE_SIGNAL, MatchQuality, MatchTier, SearchResult};

/// Default number of rows moved by page-up/page-down.
pub const DEFAULT_PAGE_SIZE: usize = 8;

/// Identity of one query revision within a launcher invocation.
///
/// A provider may return a batch carrying this token. If either the invocation
/// or the revision has moved on, [`Engine::apply_batch`] rejects the batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueryToken {
    invocation: u64,
    revision: u64,
}

impl QueryToken {
    pub fn invocation(&self) -> u64 {
        self.invocation
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// What the Escape key did, in the required hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeOutcome {
    /// The launcher root was visible and has been hidden.
    Dismiss,
    /// The action panel was open and has been closed.
    ActionsClosed,
    /// A child view was popped back to its parent.
    ViewPopped,
    /// An active IME composition/modal consumed Escape, or nothing was open.
    /// Launcher state did not change.
    Ignored,
}

/// The renderer-independent launcher state machine.
#[derive(Debug, Clone)]
pub struct Engine {
    catalog: Vec<Command>,
    results: OnceLock<Vec<Command>>,
    query: String,
    selected_id: Option<CommandId>,
    actions_open: bool,
    composition_active: bool,
    hidden: bool,
    view_stack: ViewStack,
    invocation: u64,
    revision: u64,
    signals: SearchSignals,
}

impl Engine {
    /// Creates an engine from a catalog.
    ///
    /// Duplicate [`CommandId`] values are ignored after the first occurrence.
    /// Construction is O(n) identity de-duplication only: ranking is lazy, so
    /// a large catalog does not block the first summon. The initial query is
    /// empty, so every command is available in browse order and the first
    /// result is selected.
    pub fn new(commands: Vec<Command>) -> Self {
        Self {
            catalog: deduplicate(commands),
            results: OnceLock::new(),
            query: String::new(),
            selected_id: None,
            actions_open: false,
            composition_active: false,
            hidden: false,
            view_stack: ViewStack::new(),
            invocation: 1,
            revision: 1,
            signals: SearchSignals::new(),
        }
    }

    /// The current raw query, exactly as supplied by the search field.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Ranked results, best first. Selected identity is maintained separately.
    ///
    /// Ranking runs once per query/signal/catalog revision, on the first call
    /// after invalidation.
    pub fn results(&self) -> &[Command] {
        self.results.get_or_init(|| {
            rank(&self.catalog, &self.query, &self.signals)
                .into_iter()
                .map(|result| result.command.clone())
                .collect()
        })
    }

    pub fn result_count(&self) -> usize {
        self.results().len()
    }

    /// The selected command, resolved by stable identity.
    pub fn selected(&self) -> Option<&Command> {
        let index = self.selected_index()?;
        self.results().get(index)
    }

    /// The current selected position in [`Engine::results`].
    ///
    /// If no explicit selection has been made, or the previously selected ID
    /// is no longer present, the first result is the effective selection.
    pub fn selected_index(&self) -> Option<usize> {
        let results = self.results();
        if results.is_empty() {
            return None;
        }
        if let Some(id) = &self.selected_id
            && let Some(index) = results.iter().position(|command| &command.id == id)
        {
            return Some(index);
        }
        Some(0)
    }

    pub fn selected_id(&self) -> Option<&CommandId> {
        self.selected().map(|command| &command.id)
    }

    /// Selects a result by position. Returns `false` when out of range.
    pub fn select_index(&mut self, index: usize) -> bool {
        let id = self.results().get(index).map(|command| command.id.clone());
        match id {
            Some(id) => {
                self.selected_id = Some(id);
                true
            }
            None => false,
        }
    }

    /// The catalog currently owned by the engine.
    pub fn catalog(&self) -> &[Command] {
        &self.catalog
    }

    /// Updates the query, invalidating prior query tokens and closing the
    /// action panel. The selected stable ID is retained when still present.
    ///
    /// The returned token can be ignored by a purely local caller. Asynchronous
    /// providers should carry it and submit results through
    /// [`Engine::apply_batch`].
    pub fn set_query(&mut self, query: &str) -> QueryToken {
        self.query.clear();
        self.query.push_str(query);
        self.actions_open = false;
        self.bump_revision();
        let token = self.query_token();
        self.invalidate_results();
        token
    }

    /// Moves selection by `delta` rows, clamped to the result list.
    ///
    /// Negative values move up. At the first or last row the selection stays
    /// put rather than wrapping, which avoids surprising long jumps.
    pub fn move_selection(&mut self, delta: i32) {
        let results = self.results();
        if results.is_empty() {
            self.selected_id = None;
            return;
        }

        let current = self.selected_index().unwrap_or(0);
        let last = results.len() - 1;
        let next = (current as i64 + delta as i64).clamp(0, last as i64) as usize;
        let next_id = results[next].id.clone();
        self.selected_id = Some(next_id);
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

    /// Toggles the action panel for the selected command.
    ///
    /// The panel cannot open with no results or while the root is hidden.
    pub fn toggle_actions(&mut self) {
        if self.actions_open {
            self.actions_open = false;
        } else if !self.hidden && self.selected().is_some() {
            self.actions_open = true;
        }
    }

    pub fn actions_open(&self) -> bool {
        self.actions_open
    }

    pub fn close_actions(&mut self) {
        self.actions_open = false;
    }

    /// Sets whether an IME composition or modal owns Escape.
    ///
    /// While active, [`Engine::escape`] returns
    /// [`EscapeOutcome::Ignored`] and changes nothing.
    pub fn set_composition_active(&mut self, active: bool) {
        self.composition_active = active;
    }

    pub fn composition_active(&self) -> bool {
        self.composition_active
    }

    /// Pushes a typed child view, capturing the parent query, selected command,
    /// logical scroll anchor and action-panel marker.
    ///
    /// Returns the new frame generation, or [`navigation::ViewError`] when the
    /// stack depth limit is reached. Pushing does not change the query or run
    /// any command; it only records where the parent was.
    pub fn push_view(
        &mut self,
        view: ViewId,
        scroll: ScrollAnchor,
    ) -> Result<u64, navigation::ViewError> {
        let parent = ParentState::new(
            self.query.clone(),
            self.selected().map(|command| command.id.clone()),
            scroll,
            if self.actions_open {
                ParentActions::Open
            } else {
                ParentActions::Closed
            },
        );
        self.view_stack.push(view, parent)
    }

    /// Pops one child view and re-establishes the captured parent state.
    ///
    /// Returns `None` at the root: the root is never popped.
    ///
    /// Restoration is deliberate and documented: it re-applies the exact parent
    /// query text and selection, closes transient actions, bumps the query
    /// revision and issues a new view generation. Any [`action_state::ActionRef`]
    /// issued before the push is therefore stale, including references to the
    /// parent's own command. The parent command is never re-run.
    pub fn pop_view(&mut self) -> Option<RestoredView> {
        let restored = self.view_stack.pop()?;
        self.restore_parent(&restored);
        Some(restored)
    }

    /// The current view identity (the root identity when no child is pushed).
    pub fn current_view(&self) -> ViewId {
        self.view_stack.current()
    }

    /// Discards the newest child view without restoring the parent.
    ///
    /// Used when the user replaces the query instead of navigating back: the
    /// captured parent state is obsolete and must not overwrite their input.
    /// Returns `false` at the root.
    pub fn discard_view(&mut self) -> bool {
        let discarded = self.view_stack.discard_top();
        if discarded {
            // A discarded frame still invalidates references issued in it.
            self.bump_revision();
        }
        discarded
    }

    /// Number of pushed child views.
    pub fn view_depth(&self) -> usize {
        self.view_stack.depth()
    }

    /// The current view-stack generation. Action references must match it.
    pub fn view_generation(&self) -> u64 {
        self.view_stack.generation()
    }

    /// The view stack, for issuing and resolving action references.
    pub fn views(&self) -> &ViewStack {
        &self.view_stack
    }

    /// Opens the action panel in the model, when a command is selected.
    pub fn open_actions(&mut self) {
        if !self.hidden && self.selected().is_some() {
            self.actions_open = true;
        }
    }

    /// Applies the Escape hierarchy and returns a typed plan:
    ///
    /// 1. active IME composition/modal -> cancel it and consume Escape
    /// 2. open action panel -> close it
    /// 3. pushed child view -> pop and restore the parent exactly once
    /// 4. visible root -> hide it
    /// 5. otherwise -> nothing
    ///
    /// The comparison is entirely typed: no user text or string identity is
    /// consulted.
    pub fn resolve_escape(&mut self) -> EscapeAction {
        if self.composition_active {
            return EscapeAction::CancelComposition;
        }
        if self.actions_open {
            self.actions_open = false;
            return EscapeAction::CloseActions;
        }
        if let Some(restored) = self.pop_view() {
            return EscapeAction::RestoreView(restored);
        }
        if !self.hidden {
            self.hidden = true;
            return EscapeAction::HideRoot;
        }
        EscapeAction::None
    }

    /// Applies the Escape hierarchy and collapses the typed plan into the
    /// legacy [`EscapeOutcome`]. A composition is reported as
    /// [`EscapeOutcome::Ignored`] because the host owns the actual cancellation.
    pub fn escape(&mut self) -> EscapeOutcome {
        match self.resolve_escape() {
            EscapeAction::CancelComposition | EscapeAction::None => EscapeOutcome::Ignored,
            EscapeAction::CloseActions => EscapeOutcome::ActionsClosed,
            EscapeAction::RestoreView(_) => EscapeOutcome::ViewPopped,
            EscapeAction::HideRoot => EscapeOutcome::Dismiss,
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Shows the root launcher and closes transient action state.
    pub fn show_root(&mut self) {
        self.hidden = false;
        self.actions_open = false;
    }

    /// Hides the root launcher and closes transient action state.
    pub fn hide_root(&mut self) {
        self.hidden = true;
        self.actions_open = false;
    }

    /// The current query token.
    pub fn query_token(&self) -> QueryToken {
        QueryToken {
            invocation: self.invocation,
            revision: self.revision,
        }
    }

    /// Starts a fresh launcher invocation, invalidating all outstanding query
    /// tokens. The query text is left intact for the caller to reset or reuse.
    pub fn begin_invocation(&mut self) -> QueryToken {
        self.invocation = self.invocation.saturating_add(1);
        self.bump_revision();
        self.actions_open = false;
        self.hidden = false;
        self.query_token()
    }

    /// Replaces the catalog, invalidating outstanding batches. Results are
    /// re-ranked lazily against the current query. The selected stable ID is
    /// preserved when it still exists; otherwise the first result is selected.
    pub fn replace_catalog(&mut self, commands: Vec<Command>) {
        self.catalog = deduplicate(commands);
        self.invocation = self.invocation.saturating_add(1);
        self.bump_revision();
        self.actions_open = false;
        self.invalidate_results();
    }

    /// Applies an asynchronous result batch.
    ///
    /// Returns `false` without changing state when the token belongs to an old
    /// invocation or revision. Accepted batch items are used in provider order;
    /// this entry point is for providers that have already ranked their items.
    pub fn apply_batch(&mut self, token: QueryToken, commands: Vec<Command>) -> bool {
        if token != self.query_token() {
            return false;
        }

        let previous_selection = self.selected_id.clone();
        self.store_results(deduplicate(commands));
        self.reconcile_selection();

        if self.actions_open
            && (self.selected_id.is_none() || self.selected_id != previous_selection)
        {
            self.actions_open = false;
        }
        true
    }

    /// Read-only access to locally held ranking signals.
    pub fn signals(&self) -> &SearchSignals {
        &self.signals
    }

    /// Marks or clears a favorite and re-ranks while preserving selection.
    pub fn set_favorite(&mut self, id: &CommandId, favorite: bool) {
        self.signals.set_favorite(id, favorite);
        self.bump_revision();
        self.invalidate_results();
    }

    /// Records one use (capped) and re-ranks while preserving selection.
    pub fn record_use(&mut self, id: &CommandId) {
        if self.catalog.iter().any(|command| &command.id == id) {
            self.signals.record_use(id);
            self.bump_revision();
            self.invalidate_results();
        }
    }

    pub fn clear_ranking_signals(&mut self) {
        self.signals.clear();
        self.bump_revision();
        self.invalidate_results();
    }

    fn restore_parent(&mut self, restored: &RestoredView) {
        // Re-establish the exact parent text in one mutation. `set_query` is
        // intentionally not reused because it would also reset the panel and
        // selection twice; the revision is bumped once here.
        self.query.clear();
        self.query.push_str(restored.query());
        self.actions_open = false;
        self.bump_revision();
        self.invalidate_results();
        let first = self.results().first().map(|command| command.id.clone());
        let still_present = restored
            .selected()
            .is_some_and(|id| self.results().iter().any(|command| &command.id == id));
        self.selected_id = if still_present {
            restored.selected().cloned()
        } else {
            first
        };
    }

    fn bump_revision(&mut self) {
        // Saturating rather than wrapping: a wrapped revision could make an
        // ancient token valid again, which is exactly what this guard exists to
        // prevent.
        self.revision = self.revision.saturating_add(1);
    }

    fn invalidate_results(&mut self) {
        self.results = OnceLock::new();
    }

    fn store_results(&mut self, commands: Vec<Command>) {
        let cell = OnceLock::new();
        let _ = cell.set(commands);
        self.results = cell;
    }

    fn reconcile_selection(&mut self) {
        let first = self.results().first().map(|command| command.id.clone());
        let still_present = self
            .selected_id
            .as_ref()
            .is_some_and(|id| self.results().iter().any(|command| &command.id == id));
        if !still_present {
            self.selected_id = first;
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

fn deduplicate(commands: Vec<Command>) -> Vec<Command> {
    let mut seen = HashSet::with_capacity(commands.len());
    commands
        .into_iter()
        .filter(|command| seen.insert(command.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansa_domain::{BuiltinCommand, CommandKind};

    fn command(id: &str, title: &str) -> Command {
        Command::new(
            CommandId::parse(id).unwrap(),
            title,
            CommandKind::Builtin(BuiltinCommand::About),
        )
    }

    fn engine() -> Engine {
        Engine::new(vec![
            command("app:term", "Terminal").with_alias("shell"),
            command("app:shell", "Shell Settings"),
        ])
    }

    #[test]
    fn new_engine_browses_and_selects_first() {
        let mut engine = Engine::new(vec![command("app:b", "Beta"), command("app:a", "Alpha")]);
        assert_eq!(engine.query(), "");
        assert_eq!(engine.result_count(), 2);
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:a");
        assert_eq!(engine.selected_index(), Some(0));
        assert!(!engine.actions_open());
        assert!(!engine.is_hidden());

        engine.move_selection(1);
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:b");

        assert!(engine.select_index(0));
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:a");
        assert!(!engine.select_index(99));
        assert_eq!(engine.selected_index(), Some(0));
    }

    #[test]
    fn query_is_retained_and_selection_follows_stable_id() {
        let mut engine = engine();
        // Browse order is Shell Settings, Terminal.
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:shell");
        engine.move_selection(1);
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:term");

        // "sh" makes Shell Settings a title-prefix match and Terminal a token
        // match through its alias. Terminal stays selected even though it is
        // now the second row.
        engine.set_query("  SH  ");
        assert_eq!(engine.query(), "  SH  ");
        assert_eq!(engine.results()[0].id.as_str(), "app:shell");
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:term");
        assert_eq!(engine.selected_index(), Some(1));

        // Selection falls back to the first row when the old identity vanishes.
        engine.set_query("zzzz");
        assert!(engine.results().is_empty());
        assert!(engine.selected().is_none());
    }

    #[test]
    fn move_selection_clamps_and_pages() {
        let mut engine = Engine::new(vec![
            command("app:a", "Alpha"),
            command("app:b", "Beta"),
            command("app:c", "Gamma"),
            command("app:d", "Delta"),
        ]);

        engine.move_selection(-5);
        assert_eq!(engine.selected_index(), Some(0));
        engine.move_selection(2);
        assert_eq!(engine.selected_index(), Some(2));
        engine.move_selection(99);
        assert_eq!(engine.selected_index(), Some(3));
        engine.move_selection(-2);
        assert_eq!(engine.selected_index(), Some(1));

        engine.move_selection_page(1, 2);
        assert_eq!(engine.selected_index(), Some(3));
        engine.move_selection_page(-1, 2);
        assert_eq!(engine.selected_index(), Some(1));

        engine.page_down();
        assert_eq!(engine.selected_index(), Some(3));
        engine.page_up();
        assert_eq!(engine.selected_index(), Some(0));
    }

    #[test]
    fn action_panel_requires_results_and_query_change_closes_it() {
        let mut engine = engine();
        engine.toggle_actions();
        assert!(engine.actions_open());
        engine.toggle_actions();
        assert!(!engine.actions_open());

        engine.toggle_actions();
        assert!(engine.actions_open());
        engine.set_query("sh");
        assert!(!engine.actions_open());

        engine.set_query("no-such-command");
        engine.toggle_actions();
        assert!(!engine.actions_open());
    }

    #[test]
    fn escape_follows_hierarchy_and_composition_guard() {
        let mut engine = engine();
        engine
            .push_view(ViewId::new("actions").unwrap(), ScrollAnchor::TOP)
            .unwrap();
        engine.toggle_actions();
        engine.set_composition_active(true);

        assert_eq!(engine.escape(), EscapeOutcome::Ignored);
        assert!(engine.actions_open());
        assert_eq!(engine.view_depth(), 1);
        assert!(!engine.is_hidden());

        engine.set_composition_active(false);
        assert_eq!(engine.escape(), EscapeOutcome::ActionsClosed);
        assert!(!engine.actions_open());
        assert_eq!(engine.view_depth(), 1);

        assert_eq!(engine.escape(), EscapeOutcome::ViewPopped);
        assert_eq!(engine.view_depth(), 0);

        assert_eq!(engine.escape(), EscapeOutcome::Dismiss);
        assert!(engine.is_hidden());

        assert_eq!(engine.escape(), EscapeOutcome::Ignored);
    }

    #[test]
    fn stale_revision_batch_is_rejected() {
        let mut engine = engine();
        let stale = engine.set_query("term");
        let current = engine.set_query("settings");

        assert!(!engine.apply_batch(stale, vec![command("app:other", "Other")]));
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:shell");

        assert!(engine.apply_batch(current, vec![command("app:provider", "Provider Result")]));
        assert_eq!(engine.results().len(), 1);
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:provider");
    }

    #[test]
    fn stale_invocation_batch_is_rejected() {
        let mut engine = engine();
        let old = engine.set_query("term");
        let fresh = engine.begin_invocation();
        assert!(fresh.invocation() > old.invocation());
        assert!(fresh.revision() > old.revision());

        assert!(!engine.apply_batch(old, vec![command("app:old", "Old")]));
        assert!(engine.apply_batch(fresh, vec![command("app:new", "New")]));
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:new");
    }

    #[test]
    fn replace_catalog_preserves_selection_and_invalidates_tokens() {
        let mut engine = engine();
        engine.move_selection(1);
        let selected = engine.selected().unwrap().id.clone();
        let old = engine.set_query("term");

        engine.replace_catalog(vec![
            command("app:term", "Terminal").with_alias("shell"),
            command("app:new", "New Command"),
        ]);

        assert_eq!(engine.query(), "term");
        assert_eq!(engine.selected().unwrap().id, selected);
        assert!(!engine.apply_batch(old, vec![command("app:old", "Old")]));

        engine.replace_catalog(vec![command("app:new", "Terminal Two")]);
        assert_eq!(engine.selected().unwrap().id.as_str(), "app:new");
    }

    #[test]
    fn duplicate_catalog_ids_are_ignored() {
        let engine = Engine::new(vec![
            command("app:same", "First"),
            command("app:same", "Second"),
            command("app:other", "Other"),
        ]);
        assert_eq!(engine.catalog().len(), 2);
        assert_eq!(engine.results().len(), 2);
    }

    #[test]
    fn ranking_signals_rerank_without_losing_selection() {
        let mut engine = engine();
        // Browse order is Shell Settings, Terminal; select Terminal.
        engine.move_selection(1);
        let selected = engine.selected().unwrap().id.clone();

        engine.record_use(&CommandId::parse("app:shell").unwrap());
        assert_eq!(engine.results()[0].id.as_str(), "app:shell");
        assert_eq!(engine.selected().unwrap().id, selected);
        assert_eq!(engine.selected_index(), Some(1));

        engine.set_favorite(&selected, true);
        assert_eq!(engine.results()[0].id, selected);
        assert_eq!(engine.selected_index(), Some(0));

        engine.clear_ranking_signals();
        assert_eq!(engine.results()[0].id.as_str(), "app:shell");
        assert_eq!(engine.selected().unwrap().id, selected);
    }

    #[test]
    fn empty_engine_is_safe() {
        let mut engine = Engine::default();
        assert!(engine.results().is_empty());
        assert!(engine.selected().is_none());
        engine.move_selection(5);
        engine.page_down();
        engine.toggle_actions();
        assert!(!engine.actions_open());
        assert_eq!(engine.escape(), EscapeOutcome::Dismiss);
        assert_eq!(engine.escape(), EscapeOutcome::Ignored);
    }
}

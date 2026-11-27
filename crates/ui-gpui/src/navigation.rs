//! Selection, the action panel, Escape and Tab focus movement.
//!
//! Navigation never executes a command directly. It changes selection or
//! routes an action id to `dispatch`, which owns command execution.
use crate::*;

pub(crate) fn action_slug(id: ActionId) -> &'static str {
    match id {
        ActionId::Primary => "primary",
        ActionId::CopyTarget => "copy-target",
        ActionId::ToggleFavorite => "toggle-favorite",
    }
}

impl Launcher {
    pub(crate) fn navigate(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if let Some(panel) = &mut self.panel {
            panel.move_selection(delta);
            cx.notify();
            return;
        }
        if self.setup.is_some() {
            return;
        }
        if self.emoji_query.is_some() {
            let len = self.emoji_results.len();
            if len > 0 {
                let current = self.emoji_selected.min(len - 1) as i32;
                self.emoji_selected = (current + delta).clamp(0, len as i32 - 1) as usize;
                self.emoji_scroll
                    .scroll_to_item(self.emoji_selected, ScrollStrategy::Center);
                #[cfg(feature = "upstream-candidate")]
                {
                    self.selection_announcement =
                        self.emoji_results.get(self.emoji_selected).map(|hit| {
                            format!(
                                "{}, emoji {} of {}",
                                hit.accessible_name,
                                self.emoji_selected + 1,
                                len
                            )
                        });
                }
            }
            cx.notify();
            return;
        }
        if self.detail.is_some()
            || matches!(route_query(self.engine.query()), QueryRoute::Calculator(_))
        {
            return;
        }
        self.engine.move_selection(delta);
        #[cfg(feature = "upstream-candidate")]
        self.announce_selection();
        if let Some(index) = self.engine.selected_index() {
            self.scroll
                .scroll_to_item(self.results_slot(index), ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Publish a result title, 1-based position and set size for the polite
    /// live region. Focus deliberately stays in the query field.
    #[cfg(feature = "upstream-candidate")]
    pub(crate) fn announce_selection(&mut self) {
        let count = self.engine.results().len();
        self.selection_announcement = match self.engine.selected() {
            Some(command) => {
                let position = self.engine.selected_index().unwrap_or(0) + 1;
                Some(format!(
                    "{}, result {} of {}",
                    command.title, position, count
                ))
            }
            None => Some("No results".into()),
        };
    }

    pub(crate) fn close_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = None;
        self.engine.close_actions();
        self.action_input
            .update(cx, |input, cx| input.clear(window, cx));
        compat::focus(window, &self.input.focus_handle(cx), cx);
        cx.notify();
    }

    pub(crate) fn toggle_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) || self.detail.is_some() || self.setup.is_some() {
            return;
        }
        if self.emoji_query.is_some() {
            self.status = "Emoji mode: use Up/Down and Enter to copy".into();
            cx.notify();
            return;
        }
        if self.panel.is_some() {
            self.close_actions(window, cx);
            return;
        }
        let entry = |id, title: &str, shortcut: Option<&str>| ActionEntry {
            id,
            title: title.into(),
            shortcut: shortcut.map(str::to_owned),
        };
        let token = self.engine.query_token();
        let (target, entries) = match route_query(self.engine.query()) {
            QueryRoute::Calculator(Ok(_)) => (
                PanelTarget::Calculator {
                    expression: self.engine.query().into(),
                    token,
                },
                vec![
                    entry(ActionId::Primary, "Copy result", Some("Enter")),
                    entry(
                        ActionId::CopyTarget,
                        "Copy expression",
                        Some("Ctrl+Shift+C"),
                    ),
                ],
            ),
            QueryRoute::Calculator(Err(_)) => return,
            QueryRoute::Search => {
                let Some(command) = self.engine.selected() else {
                    return;
                };
                let primary = match command.kind {
                    CommandKind::Application { .. } => "Open application",
                    CommandKind::Quicklink { .. } => "Open link",
                    CommandKind::Builtin(_) => "Run command",
                };
                let pinned = self.engine.signals().is_favorite(&command.id);
                (
                    PanelTarget::Command {
                        id: command.id.clone(),
                        token,
                    },
                    vec![
                        entry(ActionId::Primary, primary, Some("Enter")),
                        entry(ActionId::CopyTarget, "Copy target", Some("Ctrl+Shift+C")),
                        entry(
                            ActionId::ToggleFavorite,
                            if pinned {
                                "Unpin command"
                            } else {
                                "Pin command"
                            },
                            None,
                        ),
                    ],
                )
            }
        };
        self.action_input
            .update(cx, |input, cx| input.clear(window, cx));
        self.panel = Some(ActionPanel::open(target, entries));
        compat::focus(window, &self.action_input.focus_handle(cx), cx);
        cx.notify();
    }

    pub(crate) fn execute_panel(
        &mut self,
        clicked: Option<ActionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_composing(cx) {
            return;
        }
        let Some(panel) = self.panel.as_ref() else {
            return;
        };
        if !panel.target().is_current(&self.engine) {
            self.status = "Action target changed — reopen actions".into();
            self.close_actions(window, cx);
            return;
        }
        let Some(action) = clicked.or_else(|| panel.selected().map(|entry| entry.id)) else {
            return;
        };
        if !panel
            .visible_entries()
            .iter()
            .any(|entry| entry.id == action)
        {
            return;
        }
        self.close_actions(window, cx);
        match action {
            ActionId::Primary => self.execute_primary(window, cx),
            ActionId::CopyTarget => self.copy_target(window, cx),
            ActionId::ToggleFavorite => {
                if let Some(command) = self.engine.selected() {
                    let id = command.id.clone();
                    let pinned = !self.engine.signals().is_favorite(&id);
                    self.engine.set_favorite(&id, pinned);
                    self.status = if pinned {
                        "Pinned for this session"
                    } else {
                        "Unpinned for this session"
                    }
                    .into();
                }
                cx.notify();
            }
        }
    }

    pub(crate) fn tab_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if self.setup.is_some() {
            self.setup_move_focus(forward, window, cx);
            return;
        }
        if self.panel.is_some() {
            self.close_actions(window, cx);
            compat::focus(window, &self.actions_focus, cx);
        } else if self.actions_focus.is_focused(window) {
            compat::focus(window, &self.input.focus_handle(cx), cx);
        } else {
            compat::focus(window, &self.actions_focus, cx);
        }
        cx.notify();
    }

    /// Handles Escape with the same precedence as the action panel: close the
    /// menu, then setup, then details, and finally dismiss the launcher.
    pub(crate) fn on_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if self.panel.is_some() {
            self.close_actions(window, cx);
        } else if self.setup.take().is_some() {
            self.setup_generation = self.setup_generation.wrapping_add(1);
            compat::focus(window, &self.input.focus_handle(cx), cx);
        } else if self.detail.take().is_none() {
            self.dismiss(true, window, cx);
        } else {
            compat::focus(window, &self.input.focus_handle(cx), cx);
        }
        cx.notify();
    }
}

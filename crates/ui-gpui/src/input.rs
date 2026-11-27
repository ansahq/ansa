//! Text editing and IME-aware input handling.
//!
//! Root text changes invalidate panels, emoji mode and selection details.
//! IME composition remains the gate: navigation and execution must not run
//! while a composition is active.
use crate::*;

pub(crate) fn copy_status(subject: &str) -> String {
    if cfg!(windows) {
        format!("{subject} copied — paste manually")
    } else {
        // GPUI dispatches the native write but returns no completion result.
        format!("Copy requested: {subject} — paste manually")
    }
}

impl Launcher {
    pub(crate) fn on_input_changed(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Typing in the root field invalidates any previously prepared menu.
        if self.panel.is_some() {
            self.close_actions(window, cx);
        }
        self.engine.set_query(query);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.emoji_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.detail = None;
        self.update_emoji();
        #[cfg(feature = "upstream-candidate")]
        {
            self.selection_announcement = None;
        }
    }

    pub(crate) fn is_composing(&self, cx: &App) -> bool {
        self.input.read(cx).is_composing() || self.action_input.read(cx).is_composing()
    }

    /// Recomputes the explicit emoji mode from the current query.
    ///
    /// Search is bounded and runs only when the query changes, never on the
    /// render path.
    pub(crate) fn update_emoji(&mut self) {
        self.emoji_query = daily::emoji_mode_query(self.engine.query()).map(str::to_owned);
        self.emoji_selected = 0;
        self.emoji_results.clear();
        self.emoji_error = None;
        let Some(query) = self.emoji_query.as_deref() else {
            return;
        };
        match daily::search_emoji(query, daily::EMOJI_LIMIT) {
            Ok(hits) => self.emoji_results = hits,
            Err(error) => self.emoji_error = Some(error),
        }
    }

    pub(crate) fn copy_emoji(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.emoji_results.get(self.emoji_selected) else {
            self.status = "No emoji selected".into();
            cx.notify();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(hit.grapheme.clone()));
        self.status = if cfg!(windows) {
            format!(
                "Copied {} ({}) — paste manually",
                hit.grapheme, hit.accessible_name
            )
        } else {
            copy_status(&format!("{} ({})", hit.grapheme, hit.accessible_name))
        };
        #[cfg(feature = "upstream-candidate")]
        {
            self.selection_announcement = Some(if cfg!(windows) {
                format!("Copied {}. Paste manually.", hit.accessible_name)
            } else {
                self.status.clone()
            });
        }
        cx.notify();
    }
}

//! Command execution: the bridge from a selected command to its handler.
//!
//! `DispatchTarget` is intentionally small. Built-ins keep their existing
//! reviewed behavior, while future feature commands are routed through
//! `ansa_alpha::FeatureRegistry` by namespace.
use crate::*;

/// The execution path selected for a command.
///
/// This is the shell-side command-to-handler map. Feature crates register
/// richer handlers through `ansa_alpha::FeatureRegistry`; the three core
/// kinds stay in the shell so existing launch semantics are unchanged.
pub(crate) enum DispatchTarget<'a> {
    /// Launch or switch to a desktop application.
    Application {
        /// Validated launch path.
        path: &'a str,
    },
    /// Open a URL with the platform opener.
    Quicklink {
        /// Resolved HTTP(S) URL.
        url: &'a str,
    },
    /// Run a reviewed built-in command.
    Builtin(BuiltinCommand),
}

/// Maps a command identity to its shell handler.
pub(crate) fn dispatch(command: &Command) -> DispatchTarget<'_> {
    match &command.kind {
        CommandKind::Application { path } => DispatchTarget::Application { path },
        CommandKind::Quicklink { url } => DispatchTarget::Quicklink { url },
        CommandKind::Builtin(builtin) => DispatchTarget::Builtin(*builtin),
    }
}

impl Launcher {
    pub(crate) fn execute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if self.panel.is_some() {
            self.execute_panel(None, window, cx);
            return;
        }
        if self.setup.is_some() {
            self.setup_activate_focused(window, cx);
            return;
        }
        if self.actions_focus.is_focused(window) {
            if self.detail.take().is_none() {
                self.toggle_actions(window, cx);
            } else {
                compat::focus(window, &self.input.focus_handle(cx), cx);
                cx.notify();
            }
            return;
        }
        self.execute_primary(window, cx);
    }

    pub(crate) fn execute_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if self.setup.is_some() {
            self.setup_activate_focused(window, cx);
            return;
        }
        if self.emoji_query.is_some() {
            self.copy_emoji(cx);
            return;
        }
        if self.detail.is_some() {
            return;
        }
        if let QueryRoute::Calculator(result) = route_query(self.engine.query()) {
            match result {
                Ok(value) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.format()));
                    self.status = copy_status("Result");
                }
                Err(error) => self.status = format!("Cannot calculate: {error}"),
            }
            cx.notify();
            return;
        }
        let Some(command) = self.engine.selected().cloned() else {
            return;
        };
        if command.id.as_str() == daily::SETUP_COMMAND_ID {
            self.open_setup(window, cx);
            return;
        }
        let result = match dispatch(&command) {
            DispatchTarget::Application { path } => {
                self.execute_application(
                    command.id.clone(),
                    command.title.clone(),
                    path.to_owned(),
                    window,
                    cx,
                );
                return;
            }
            DispatchTarget::Quicklink { url } => ansa_platform_api::open_url(url),
            DispatchTarget::Builtin(BuiltinCommand::RefreshApplications) => {
                self.refresh_catalog(cx);
                return;
            }
            DispatchTarget::Builtin(BuiltinCommand::Settings) => {
                self.detail = Some(self.settings_detail());
                cx.notify();
                return;
            }
            DispatchTarget::Builtin(BuiltinCommand::About) => {
                self.detail = Some("Ansa\n\nA little less friction. A little more flow.\n\nA free, open-source keyboard launcher built in Rust. An original interface, an offline core, and no account required.\n\nFoundation prototype · 0.1.0\nApache-2.0 · GPUI 0.2.2\n\nThis build starts the feasibility phase; it does not claim Raycast extension compatibility or production readiness.".into());
                cx.notify();
                return;
            }
        };
        match result {
            Ok(()) => {
                self.status = format!("Open requested: {}", command.title);
                self.engine.record_use(&command.id);
                self.dismiss(false, window, cx);
            }
            Err(error) => self.status = format!("Could not open: {error}"),
        }
        compat::focus(window, &self.input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Executes an application through the truthful launch-or-switch path.
    ///
    /// Use is recorded and the launcher is dismissed only when the host
    /// reports a positive, observed outcome (`Launched` or `Switched`).
    /// `Denied`, `Failed`, `Unverified` and `NoTarget` stay visible in the
    /// status line and leave the launcher open.
    pub(crate) fn execute_application(
        &mut self,
        id: CommandId,
        title: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(not(windows))]
        let entry = self
            .native_entries
            .iter()
            .find(|entry| entry.id == id.as_str() && entry.path == path)
            .cloned();
        #[cfg(windows)]
        let entry: Option<AppEntry> = None;
        let outcome = self.application_launcher()(&path, id.as_str(), entry.as_ref());
        match &outcome {
            LaunchOutcome::Launched => {
                self.status = format!("{}: {title}", daily::launch_handoff_label());
            }
            LaunchOutcome::Switched { window: handle } => {
                self.status = format!("Switched to {title} (window 0x{handle:x})");
            }
            LaunchOutcome::SwitchedApplication { process_id } => {
                self.status = format!("{title} is the foreground application (PID {process_id})");
            }
            LaunchOutcome::Denied { reason } => {
                self.status = format!("Launch denied: {title} — {reason}");
            }
            LaunchOutcome::Failed { reason } => {
                self.status = format!("Launch failed: {title} — {reason}");
            }
            LaunchOutcome::Unverified { reason } => {
                self.status = format!("Launch unverified: {title} — {reason}");
            }
            LaunchOutcome::NoTarget { reason } => {
                self.status = format!("No launch target: {title} — {reason}");
            }
        }
        if outcome.is_success() {
            self.engine.record_use(&id);
            self.dismiss(false, window, cx);
        }
        compat::focus(window, &self.input.focus_handle(cx), cx);
        cx.notify();
    }

    pub(crate) fn application_launcher(
        &self,
    ) -> impl FnOnce(&str, &str, Option<&AppEntry>) -> LaunchOutcome + use<> {
        #[cfg(windows)]
        return |path: &str, id: &str, _entry: Option<&AppEntry>| {
            ansa_platform_api::launch_or_switch_identified(path, id)
        };
        #[cfg(not(windows))]
        {
            let generation = self.displayed_catalog_generation;
            move |_path: &str, _id: &str, entry: Option<&AppEntry>| {
                // The retained entry, not the rediscovered path/ID, must match
                // the native snapshot token under the rendered generation.
                let Some(entry) = entry else {
                    return LaunchOutcome::NoTarget {
                        reason: "The selected application is not in the current native catalog. Refresh applications and try again.".into(),
                    };
                };
                catalog_dispatch::NATIVE_CATALOG.launch(generation, || {
                    ansa_platform_api::launch_or_switch_selected(entry)
                })
            }
        }
    }

    pub(crate) fn copy_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if self.emoji_query.is_some() {
            self.copy_emoji(cx);
            return;
        }
        if let QueryRoute::Calculator(_) = route_query(self.engine.query()) {
            cx.write_to_clipboard(ClipboardItem::new_string(self.engine.query().to_string()));
            self.status = copy_status("Expression");
            self.close_actions(window, cx);
            return;
        }
        if let Some(command) = self.engine.selected() {
            let text = match &command.kind {
                CommandKind::Application { path } => path.clone(),
                CommandKind::Quicklink { url } => url.clone(),
                _ => command.title.clone(),
            };
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.status = copy_status("Target");
        }
        self.close_actions(window, cx);
        cx.notify();
    }
}

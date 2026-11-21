use ansa_core::{
    Engine,
    action_panel::{ActionEntry, ActionId, ActionPanel, PanelTarget},
    query::{QueryRoute, route_query},
};
use ansa_domain::{BuiltinCommand, CommandId, CommandKind};
use ansa_ui_components::{Changed, EditRejected, TextInput, Theme, install_keybindings};
#[cfg(feature = "upstream-candidate")]
use gpui::Role;
use gpui::{
    App, Application, Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyBinding,
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, actions, div, prelude::*,
    px, size, uniform_list,
};
use std::path::PathBuf;

mod compat;
mod daily;
#[cfg(feature = "upstream-candidate")]
mod live_status;

actions!(
    ansa,
    [
        Next,
        Previous,
        PageDown,
        PageUp,
        Execute,
        Actions,
        Escape,
        Quit,
        CopyTarget,
        TabNext,
        TabPrevious,
        SetupActivate,
        ToggleGlass,
        ToggleTheme
    ]
);

#[cfg(debug_assertions)]
mod smoke;

struct Launcher {
    input: Entity<TextInput>,
    action_input: Entity<TextInput>,
    panel: Option<ActionPanel>,
    actions_focus: FocusHandle,
    engine: Engine,
    scroll: UniformListScrollHandle,
    status: String,
    #[cfg(feature = "upstream-candidate")]
    selection_announcement: Option<String>,
    detail: Option<String>,
    hotkey: Option<ansa_platform_windows::HotkeyRegistration>,
    hotkey_notice: Option<String>,
    visible: bool,
    glass: bool,
    light: bool,
    #[cfg(target_os = "windows")]
    backdrop: Option<ansa_platform_windows::LauncherBackdrop>,
    appearance_notice: Option<String>,
    #[cfg(target_os = "windows")]
    native_shape: Option<(i32, i32, i32)>,
    native_handle: Option<isize>,
    previous_target: Option<ansa_platform_windows::FocusTarget>,
    catalog_refresh: ansa_core::refresh::RefreshGate,
    driver: Option<ansa_alpha::DailyDriver>,
    data_dir: PathBuf,
    storage_error: Option<String>,
    catalog_skipped: Vec<ansa_alpha::SkippedQuicklink>,
    catalog_warning: Option<String>,
    persisted_settings: Option<daily::PersistedSettings>,
    setup: Option<daily::SetupSession>,
    setup_focus: FocusHandle,
    setup_generation: u64,
    smoke_fixture: bool,
    emoji_query: Option<String>,
    emoji_results: Vec<ansa_alpha::EmojiHit>,
    emoji_selected: usize,
    emoji_error: Option<String>,
    emoji_scroll: UniformListScrollHandle,
    _input_subscription: Subscription,
    _action_subscription: Subscription,
    _rejection_subscriptions: Vec<Subscription>,
}

fn action_slug(id: ActionId) -> &'static str {
    match id {
        ActionId::Primary => "primary",
        ActionId::CopyTarget => "copy-target",
        ActionId::ToggleFavorite => "toggle-favorite",
    }
}

fn setup_button(
    control: daily::SetupControl,
    focused: bool,
    theme: Theme,
    cx: &mut Context<Launcher>,
) -> impl IntoElement {
    let button = div()
        .id(control.id())
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(if focused { theme.accent } else { theme.border })
        .text_color(theme.accent)
        .cursor_pointer()
        .when(focused, |el| el.bg(theme.surface_raised))
        .hover(|el| el.bg(theme.surface_raised))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.setup_activate_control(control, window, cx);
        }))
        .child(control.label());
    #[cfg(feature = "upstream-candidate")]
    let button = button
        .role(Role::Button)
        .aria_label(control.label())
        .aria_keyshortcuts("Enter Space");
    button
}

impl Launcher {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        a11y_fixture: bool,
        glass: bool,
        light: bool,
        data_dir: PathBuf,
        smoke_fixture: bool,
    ) -> Self {
        let input = cx.new(TextInput::new);
        let action_input = cx.new(|cx| {
            let mut input = TextInput::new(cx);
            input.set_placeholder("Search actions…", cx);
            input
        });
        let theme = if light { Theme::light() } else { Theme::dark() };
        input.update(cx, |input, cx| input.set_theme(theme, cx));
        action_input.update(cx, |input, cx| input.set_theme(theme, cx));
        #[cfg(feature = "upstream-candidate")]
        action_input.update(cx, |input, cx| {
            input.set_accessibility_label("Search actions", cx);
        });
        compat::focus(window, &input.focus_handle(cx), cx);
        let commands = daily::builtin_commands();
        let status = if a11y_fixture {
            format!(
                "{} built-in commands · accessibility fixture",
                commands.len()
            )
        } else {
            "Discovering applications…".into()
        };
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &Changed, window, cx| {
                // Typing in the root field invalidates any previously prepared menu.
                if this.panel.is_some() {
                    this.close_actions(window, cx);
                }
                this.engine.set_query(&event.0);
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                this.emoji_scroll.scroll_to_item(0, ScrollStrategy::Top);
                this.detail = None;
                this.update_emoji();
                #[cfg(feature = "upstream-candidate")]
                {
                    this.selection_announcement = None;
                }
                cx.notify();
            });
        let action_subscription = cx.subscribe(&action_input, |this, _, event: &Changed, cx| {
            if let Some(panel) = &mut this.panel {
                panel.set_query(&event.0);
            }
            cx.notify();
        });
        let rejection_subscriptions = [&input, &action_input]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &EditRejected, cx| {
                    this.status = format!("Input not changed: {}", event.message());
                    cx.notify();
                })
            })
            .collect();
        Self {
            input,
            action_input,
            panel: None,
            actions_focus: cx.focus_handle(),
            engine: Engine::new(commands),
            status,
            #[cfg(feature = "upstream-candidate")]
            selection_announcement: None,
            detail: None,
            scroll: UniformListScrollHandle::new(),
            hotkey: None,
            hotkey_notice: None,
            visible: true,
            // On Windows the native blur layer must attach first. macOS gets
            // its blur from the window's NSVisualEffectView.
            glass: if cfg!(target_os = "windows") {
                false
            } else {
                glass
            },
            light,
            #[cfg(target_os = "windows")]
            backdrop: None,
            appearance_notice: None,
            #[cfg(target_os = "windows")]
            native_shape: None,
            native_handle: native_handle(window),
            previous_target: None,
            catalog_refresh: Default::default(),
            driver: None,
            data_dir,
            storage_error: None,
            catalog_skipped: Vec::new(),
            catalog_warning: None,
            persisted_settings: None,
            setup: None,
            setup_focus: cx.focus_handle(),
            setup_generation: 0,
            smoke_fixture,
            emoji_query: None,
            emoji_results: Vec::new(),
            emoji_selected: 0,
            emoji_error: None,
            emoji_scroll: UniformListScrollHandle::new(),
            _input_subscription: subscription,
            _action_subscription: action_subscription,
            _rejection_subscriptions: rejection_subscriptions,
        }
    }

    fn toggle_glass(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) || self.appearance_notice.is_some() {
            return;
        }
        self.set_glass(!self.glass, window);
        cx.notify();
    }

    #[cfg(target_os = "windows")]
    fn set_glass(&mut self, enabled: bool, window: &Window) {
        let result = if let Some(backdrop) = &self.backdrop {
            backdrop.set_visible(enabled)
        } else if enabled {
            let (width, height, radius) = launcher_shape(window);
            self.native_handle
                .ok_or_else(|| "Launcher HWND is unavailable".into())
                .and_then(|hwnd| {
                    ansa_platform_windows::LauncherBackdrop::new(hwnd, width, height, radius)
                })
                .map(|backdrop| self.backdrop = Some(backdrop))
        } else {
            Ok(())
        };
        match result {
            Ok(()) => self.glass = enabled,
            Err(error) => self.glass_unavailable(error),
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn set_glass(&mut self, enabled: bool, window: &Window) {
        #[cfg(target_os = "macos")]
        window.set_background_appearance(if enabled {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Transparent
        });
        #[cfg(not(target_os = "macos"))]
        let _ = window;
        self.glass = enabled;
        self.appearance_notice = None;
    }

    #[cfg(target_os = "windows")]
    fn glass_unavailable(&mut self, error: String) {
        self.glass = false;
        self.backdrop = None;
        eprintln!("Launcher frosted glass unavailable: {error}");
        self.appearance_notice = Some(error);
    }

    fn theme(&self) -> Theme {
        if self.light {
            Theme::light()
        } else {
            Theme::dark()
        }
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        self.light = !self.light;
        let theme = self.theme();
        self.input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        self.action_input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        cx.notify();
    }

    fn navigate(&mut self, delta: i32, cx: &mut Context<Self>) {
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
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Publish a result title, 1-based position and set size for the polite
    /// live region. Focus deliberately stays in the query field.
    #[cfg(feature = "upstream-candidate")]
    fn announce_selection(&mut self) {
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

    fn dismiss(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if self.hotkey.is_some() {
            if let Some(handle) = self.native_handle {
                let result = if restore {
                    ansa_platform_windows::dismiss_launcher(handle, self.previous_target.as_ref())
                        .map(|outcome| match outcome {
                            ansa_platform_windows::FocusRestoreOutcome::Restored => {}
                            ansa_platform_windows::FocusRestoreOutcome::Skipped { reason }
                            | ansa_platform_windows::FocusRestoreOutcome::Denied { reason } => {
                                self.status = reason
                            }
                        })
                } else {
                    ansa_platform_windows::set_launcher_visible(handle, false)
                };
                match result {
                    Ok(()) => {
                        self.visible = false;
                        self.engine.hide_root();
                        self.close_actions(window, cx);
                        self.previous_target = None;
                    }
                    Err(error) => self.status = format!("Cannot hide launcher: {error}"),
                }
            }
        } else {
            self.status = "Global summon unavailable — use the window or Ctrl+Q to quit".into();
        }
        cx.notify();
    }

    fn execute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn execute_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    self.status = "Result copied — paste manually".into();
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
        let result = match command.kind.clone() {
            CommandKind::Application { path } => {
                self.execute_application(
                    command.id.clone(),
                    command.title.clone(),
                    path,
                    window,
                    cx,
                );
                return;
            }
            CommandKind::Quicklink { url } => ansa_platform_windows::open_url(&url),
            CommandKind::Builtin(BuiltinCommand::RefreshApplications) => {
                self.refresh_catalog(cx);
                return;
            }
            CommandKind::Builtin(BuiltinCommand::Settings) => {
                self.detail = Some(self.settings_detail());
                cx.notify();
                return;
            }
            CommandKind::Builtin(BuiltinCommand::About) => {
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
    fn execute_application(
        &mut self,
        id: CommandId,
        title: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outcome = ansa_platform_windows::launch_or_switch(&path);
        match &outcome {
            ansa_platform_windows::LaunchOutcome::Launched => {
                self.status = format!("Launched: {title}");
            }
            ansa_platform_windows::LaunchOutcome::Switched { window: handle } => {
                self.status = format!("Switched to {title} (window 0x{handle:x})");
            }
            ansa_platform_windows::LaunchOutcome::Denied { reason } => {
                self.status = format!("Launch denied: {title} — {reason}");
            }
            ansa_platform_windows::LaunchOutcome::Failed { reason } => {
                self.status = format!("Launch failed: {title} — {reason}");
            }
            ansa_platform_windows::LaunchOutcome::Unverified { reason } => {
                self.status = format!("Launch unverified: {title} — {reason}");
            }
            ansa_platform_windows::LaunchOutcome::NoTarget { reason } => {
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

    fn copy_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if self.emoji_query.is_some() {
            self.copy_emoji(cx);
            return;
        }
        if let QueryRoute::Calculator(_) = route_query(self.engine.query()) {
            cx.write_to_clipboard(ClipboardItem::new_string(self.engine.query().to_string()));
            self.status = "Expression copied — paste manually".into();
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
            self.status = "Target copied — paste manually".into();
        }
        self.close_actions(window, cx);
        cx.notify();
    }

    fn is_composing(&self, cx: &App) -> bool {
        self.input.read(cx).is_composing() || self.action_input.read(cx).is_composing()
    }

    /// Recomputes the explicit emoji mode from the current query.
    ///
    /// Search is bounded and runs only when the query changes, never on the
    /// render path.
    fn update_emoji(&mut self) {
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

    fn copy_emoji(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.emoji_results.get(self.emoji_selected) else {
            self.status = "No emoji selected".into();
            cx.notify();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(hit.grapheme.clone()));
        self.status = format!(
            "Copied {} ({}) — paste manually",
            hit.grapheme, hit.accessible_name
        );
        #[cfg(feature = "upstream-candidate")]
        {
            self.selection_announcement =
                Some(format!("Copied {}. Paste manually.", hit.accessible_name));
        }
        cx.notify();
    }

    fn settings_detail(&self) -> String {
        let storage = match (&self.driver, &self.storage_error) {
            (_, Some(error)) => daily::storage_recovery(&self.data_dir, error),
            (Some(driver), None) => format!(
                "Storage open: {} (schema v{}).",
                driver.main_database_path().display(),
                driver.schema_version()
            ),
            (None, None) => format!("Storage is still opening in {}.", self.data_dir.display()),
        };
        let omitted = daily::skipped_warning(&self.catalog_skipped).unwrap_or_else(|| {
            "No enabled quicklinks were omitted from the last catalog composition.".into()
        });
        let catalog_warning = self
            .catalog_warning
            .clone()
            .unwrap_or_else(|| "No quicklink library warning.".into());
        let persisted = self.persisted_settings.as_ref().map_or_else(
            || "Persisted preferences: not loaded yet.".to_owned(),
            |settings| format!("Persisted preferences: {}", settings.summary()),
        );
        format!(
            "Integration status\n\nApplications: Windows Start Menu shortcuts and packaged apps (launched by AUMID). Enter launches or switches; a switch is reported only when the target window is observed as foreground, otherwise the outcome is denied, failed or unverified. Broken shortcuts stay visible with a refresh hint.\n\nSetup: run Start Setup for the real onboarding flow. Tab/Shift+Tab move between controls, Enter or Space activates, and Escape closes. The shortcut is probed against this session's live Ctrl+Space registration even when an older saved choice exists. A launch is recorded only after the real launch-or-switch probe reports a positive outcome. Skipping records a warning when the step had no result.\n\nSummon: Ctrl+Space when registration succeeds. Shortcut editing is not implemented. Dismissal attempts checked, best-effort focus return only if Ansa still owns the foreground. Windows can deny activation.\n\nActions: Ctrl+K, type to filter, arrows to select, Enter to act. Pins and usage signals last only for this process; ranking signals are not yet persisted. Tab reaches the footer control.\n\nQuicklinks: Ansa holds the exclusive storage lock while it runs, so Refresh re-reads only in-process persisted state and cannot pick up an external CLI import live. To import, close Ansa, run `ansa quicklink import <file> --data-dir <data-dir>`, then start Ansa. Enabled static HTTP(S) quicklinks are composed into search results. Required-argument, local-path, unsupported-opener and unresolved-template entries are omitted and reported; their raw templates are never launched. A corrupt record is reported as a warning instead of a silent empty library. Desktop authoring, preview, import and export are not implemented.\n\nEmoji: type emoji followed by a query (for example, emoji smile), then use Up/Down and Enter. Enter copies the selected grapheme and reports paste manually; insertion and automatic paste are not implemented.\n\nCalculator: bounded arithmetic and approximate f64 results; Enter copies. Start with = to request arithmetic, units or constants. Bounded length, mass, time and temperature units are supported. Currencies, dates and time zones are not. Bounded arithmetic also accepts ^ and % without the = prefix.\n\n{persisted}\n\nPersisted preference note: this build still uses CLI flags for appearance and the live Ctrl+Space registration for summon. Persisted theme, launch_mode, maximum_results and alias_prefix are shown but not applied; ranking signals are session-only.\n\nClipboard history, file indexing, extensions, AI and sync are not enabled. No account or network service is required.\n\n{storage}\n\n{catalog_warning}\n\n{omitted}\n\nAccessibility: no screen-reader run exists for this build. The default GPUI pin has no accessibility bridge; candidate-only roles have not been validated. Keyboard-only setup and emoji navigation are covered by component and synthetic smoke tests, not by NVDA/Narrator.",
        )
    }

    fn open_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        self.panel = None;
        self.engine.close_actions();
        self.action_input
            .update(cx, |input, cx| input.clear(window, cx));
        self.detail = None;
        let onboarding = if let Some(driver) = self.driver.as_ref() {
            driver.onboarding()
        } else if self.smoke_fixture {
            // Interaction smoke never opens storage. A fresh in-memory model
            // keeps the keyboard journey testable without touching a profile.
            ansa_onboarding::Onboarding::fresh(ansa_onboarding::MemoryStore::new())
        } else {
            self.status = match &self.storage_error {
                Some(error) => daily::storage_recovery(&self.data_dir, error),
                None => "Storage is still opening — try Start Setup again in a moment.".into(),
            };
            cx.notify();
            return;
        };
        self.setup_generation = self.setup_generation.wrapping_add(1);
        self.setup = Some(daily::SetupSession::new(onboarding));
        compat::focus(window, &self.setup_focus, cx);
        cx.notify();
    }

    fn close_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        self.setup_generation = self.setup_generation.wrapping_add(1);
        self.setup = None;
        compat::focus(window, &self.input.focus_handle(cx), cx);
        cx.notify();
    }

    fn setup_hotkey_probe(&self) -> daily::HotkeyProbe {
        daily::HotkeyProbe::new(
            self.hotkey.is_some(),
            self.hotkey_notice
                .clone()
                .unwrap_or_else(|| "Ctrl+Space registration did not succeed".into()),
        )
    }

    fn setup_move_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) || self.setup.is_none() {
            return;
        }
        if let Some(setup) = self.setup.as_mut() {
            setup.focus_next(forward);
        }
        compat::focus(window, &self.setup_focus, cx);
        cx.notify();
    }

    fn setup_activate_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        let Some(control) = self.setup.as_ref().and_then(daily::SetupSession::focused) else {
            return;
        };
        self.setup_activate_control(control, window, cx);
    }

    /// Runs one setup control from a keyboard activation or a pointer click.
    ///
    /// Every path checks the IME composition guard and then restores focus to
    /// the setup container so the next Tab/Enter/Space stays on the surface.
    fn setup_activate_control(
        &mut self,
        control: daily::SetupControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_composing(cx) || self.setup.is_none() {
            return;
        }
        if control == daily::SetupControl::Close {
            self.close_setup(window, cx);
            return;
        }

        if control == daily::SetupControl::TestLaunch {
            let candidate = {
                let preferred = self.engine.selected_id();
                daily::find_launch_target(self.engine.catalog(), preferred)
            };
            let Some((id, path)) = candidate else {
                if let Some(setup) = self.setup.as_mut() {
                    setup.set_message(
                        "No application is available for the launch test. Refresh the catalog, then try again.",
                    );
                }
                self.status = self
                    .setup
                    .as_ref()
                    .and_then(daily::SetupSession::message)
                    .unwrap_or_default()
                    .to_owned();
                compat::focus(window, &self.setup_focus, cx);
                cx.notify();
                return;
            };
            let title = self
                .engine
                .catalog()
                .iter()
                .find(|command| command.id == id)
                .map(|command| command.title.clone())
                .unwrap_or_else(|| id.to_string());
            let Some(setup) = self.setup.as_mut() else {
                return;
            };
            setup.focus_control(control);
            setup.set_message(format!("Testing {title}…"));
            self.status = format!("Testing {title}…");
            #[cfg(feature = "upstream-candidate")]
            {
                self.selection_announcement = Some(format!("Testing {title}…"));
            }
            compat::focus(window, &self.setup_focus, cx);
            cx.notify();
            let title_for_task = title.clone();
            let generation = self.setup_generation.wrapping_add(1);
            self.setup_generation = generation;
            cx.spawn(async move |this, cx| {
                let (outcome, detail) = cx
                    .background_executor()
                    .spawn(async move {
                        daily::map_launch_outcome(ansa_platform_windows::launch_or_switch(&path))
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.setup_generation != generation {
                        return;
                    }
                    let Some(setup) = this.setup.as_mut() else {
                        return;
                    };
                    if setup.onboarding().step() != ansa_onboarding::Step::TryLaunch {
                        return;
                    }
                    let probe = daily::FixedLaunchProbe::new(outcome);
                    setup.test_launch(&title_for_task, id, &probe);
                    let verification = if setup.launch_verified() {
                        "recorded"
                    } else {
                        "not recorded"
                    };
                    setup.set_message(format!(
                        "{title_for_task}: {detail}. Launch verification: {verification}."
                    ));
                    this.status = setup.message().unwrap_or_default().to_owned();
                    #[cfg(feature = "upstream-candidate")]
                    {
                        this.selection_announcement = setup.message().map(str::to_owned);
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        } else {
            let probe = self.setup_hotkey_probe();
            let Some(setup) = self.setup.as_mut() else {
                return;
            };
            setup.focus_control(control);
            match control {
                daily::SetupControl::Continue | daily::SetupControl::Finish => {
                    setup.continue_setup(&probe);
                }
                daily::SetupControl::CheckShortcut => setup.check_shortcut(&probe),
                daily::SetupControl::Skip => setup.skip(),
                daily::SetupControl::Reset => {
                    setup.reset();
                    self.setup_generation = self.setup_generation.wrapping_add(1);
                }
                daily::SetupControl::TestLaunch | daily::SetupControl::Close => unreachable!(
                    "Test launch and Close are dispatched before the shared setup path"
                ),
            }
        }

        if let Some(setup) = self.setup.as_ref() {
            self.status = setup.message().unwrap_or_default().to_owned();
            #[cfg(feature = "upstream-candidate")]
            {
                self.selection_announcement = setup.message().map(str::to_owned);
            }
        }
        compat::focus(window, &self.setup_focus, cx);
        cx.notify();
    }

    fn refresh_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.catalog_refresh.request() else {
            self.status = "Refresh in progress — one follow-up queued".into();
            cx.notify();
            return;
        };
        self.status = "Refreshing application catalog…".into();
        let data_dir = self.data_dir.clone();
        let driver = self.driver.take();
        cx.spawn(async move |this, cx| {
            let (driver, open_error, settings, result) = cx
                .background_executor()
                .spawn(async move {
                    let (mut driver, open_error) = match driver {
                        Some(driver) => (Some(driver), None),
                        None => match ansa_alpha::DailyDriver::open(&data_dir) {
                            Ok(driver) => (Some(driver), None),
                            Err(error) => (None, Some(error.to_string())),
                        },
                    };
                    let settings = driver
                        .as_ref()
                        .map(daily::read_persisted_settings);
                    let result = match driver.as_mut() {
                        Some(driver) => match ansa_platform_windows::discover_applications() {
                            Ok(apps) => {
                                daily::reload_and_compose(driver, &apps).or_else(|error| {
                                    daily::legacy_catalog().map(|mut load| {
                                        load.warning = Some(format!(
                                            "Quicklink composition failed; using built-ins and applications only: {error}"
                                        ));
                                        load
                                    })
                                })
                            }
                            Err(error) => {
                                Err(format!("application discovery failed: {error}"))
                            }
                        },
                        None => daily::legacy_catalog(),
                    };
                    (driver, open_error, settings, result)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.catalog_refresh.complete(ticket) {
                    return;
                }
                this.driver = driver;
                this.storage_error = open_error.clone();
                this.persisted_settings = settings;
                match result {
                    Ok(load) => {
                        let summary = load.summary();
                        let warning = load.warning.clone();
                        this.catalog_skipped = load.skipped;
                        this.catalog_warning = warning.clone();
                        this.engine.replace_catalog(load.commands);
                        this.status = match (&open_error, &warning) {
                            (Some(error), _) => format!(
                                "{summary} · {}",
                                daily::storage_recovery(&this.data_dir, error)
                            ),
                            (None, Some(warning)) => format!("{summary} · {warning}"),
                            (None, None) => summary,
                        };
                    }
                    Err(error) => {
                        this.status = match &open_error {
                            Some(open_error) => {
                                daily::storage_recovery(&this.data_dir, open_error)
                            }
                            None => format!(
                                "Catalog refresh failed (previous catalog kept): {error}"
                            ),
                        };
                    }
                }
                if this.catalog_refresh.take_queued() {
                    this.refresh_catalog(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn close_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = None;
        self.engine.close_actions();
        self.action_input
            .update(cx, |input, cx| input.clear(window, cx));
        compat::focus(window, &self.input.focus_handle(cx), cx);
        cx.notify();
    }

    fn toggle_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn execute_panel(
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

    fn tab_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
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
}

fn initial_window_background(glass: bool) -> WindowBackgroundAppearance {
    if cfg!(target_os = "windows") {
        WindowBackgroundAppearance::Opaque
    } else if cfg!(target_os = "macos") && glass {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

#[cfg(target_os = "windows")]
fn launcher_shape(window: &Window) -> (i32, i32, i32) {
    let viewport = window.viewport_size();
    let scale = window.scale_factor();
    (
        (f32::from(viewport.width) * scale).round() as i32,
        (f32::from(viewport.height) * scale).round() as i32,
        (12.0 * scale).round().clamp(1.0, 256.0) as i32,
    )
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "windows")]
        if let Some(hwnd) = self.native_handle {
            let shape @ (width, height, radius) = launcher_shape(window);
            if self.native_shape != Some(shape) {
                self.native_shape = Some(shape);
                if let Err(error) = ansa_platform_windows::round_launcher_window(hwnd, radius) {
                    eprintln!("Launcher corner clipping: {error}");
                    self.glass_unavailable(error);
                }
                if let Some(backdrop) = &self.backdrop
                    && let Err(error) = backdrop.resize(width, height, radius)
                {
                    self.glass_unavailable(error);
                }
            }
        }
        let theme = self.theme();
        // Keep invalidated menus visible: closing one here could turn a queued
        // Enter into an unrelated root action. Execution revalidates the snapshot.
        let panel_stale = self
            .panel
            .as_ref()
            .is_some_and(|panel| !panel.target().is_current(&self.engine));
        let route = route_query(self.engine.query());
        let is_calculator = matches!(route, QueryRoute::Calculator(_));
        let calculation = match &route {
            QueryRoute::Calculator(Ok(value)) => Some(value.format()),
            _ => None,
        };
        let calculation_error = match &route {
            QueryRoute::Calculator(Err(error)) => Some(error.to_string()),
            _ => None,
        };
        let count = self.engine.results().len();
        let is_emoji = self.emoji_query.is_some();
        let emoji_count = self.emoji_results.len();
        let setup_view = self
            .setup
            .as_ref()
            .map(|setup| setup.view(self.hotkey.is_some()));
        let setup_message = self
            .setup
            .as_ref()
            .and_then(|setup| setup.message().map(str::to_owned));
        let setup_step = self.setup.as_ref().map(|setup| setup.onboarding().step());
        let setup_controls = self
            .setup
            .as_ref()
            .map(daily::SetupSession::controls)
            .unwrap_or_default();
        let setup_focused = self.setup.as_ref().and_then(daily::SetupSession::focused);
        let setup_header_action = setup_focused.map_or_else(
            || "Tab to choose a control".to_owned(),
            |control| format!("Enter: {}", control.label()),
        );
        let setup_focus_index = self
            .setup
            .as_ref()
            .map_or(0, daily::SetupSession::focus_index);
        let setup_target = setup_view.as_ref().and_then(|view| {
            if view.step != ansa_onboarding::Step::TryLaunch {
                return None;
            }
            let preferred = self.engine.selected_id();
            let predicted = daily::find_launch_target(self.engine.catalog(), preferred)
                .map(|(id, _)| id)
                .or_else(|| {
                    self.setup
                        .as_ref()
                        .and_then(daily::SetupSession::launch_target)
                        .cloned()
                });
            predicted.and_then(|id| {
                self.engine
                    .catalog()
                    .iter()
                    .find(|command| command.id == id)
                    .map(|command| command.title.clone())
                    .or_else(|| Some(id.to_string()))
            })
        });
        let status_text = match &self.hotkey_notice {
            Some(notice) => format!("{notice} · {}", self.status),
            None => self.status.clone(),
        };
        #[cfg(feature = "upstream-candidate")]
        let status_text = self.selection_announcement.clone().unwrap_or(status_text);
        let status_text = match &self.appearance_notice {
            Some(notice) => format!("Appearance unavailable: {notice} · {status_text}"),
            None => status_text,
        };
        let status_content = div()
            .text_color(theme.muted_text)
            .truncate()
            .flex_1()
            .child(status_text.clone());
        #[cfg(feature = "upstream-candidate")]
        let status_node = live_status::LiveStatus::new(status_text, status_content);
        #[cfg(not(feature = "upstream-candidate"))]
        let status_node = status_content;
        let actions_button = div()
            .id("actions-button")
            .track_focus(&self.actions_focus)
            .ml_3()
            .cursor_pointer()
            .text_color(theme.accent)
            .px_2()
            .py_1()
            .border_1()
            .rounded_sm()
            .border_color(if self.actions_focus.is_focused(window) {
                theme.accent
            } else {
                theme.border
            })
            .on_click(cx.listener(|this, _, window, cx| {
                if this.is_composing(cx) {
                    return;
                }
                let had_detail = this.detail.take().is_some();
                let had_setup = this.setup.take().is_some();
                if had_setup {
                    this.setup_generation = this.setup_generation.wrapping_add(1);
                }
                if had_detail || had_setup {
                    compat::focus(window, &this.input.focus_handle(cx), cx);
                    cx.notify();
                } else {
                    this.toggle_actions(window, cx);
                }
            }))
            .child(if self.detail.is_some() || self.setup.is_some() {
                "Back  Esc"
            } else {
                "Actions  Ctrl K"
            });
        #[cfg(feature = "upstream-candidate")]
        let actions_button = actions_button
            .role(Role::Button)
            .aria_label(if self.detail.is_some() || self.setup.is_some() {
                "Back"
            } else {
                "Actions"
            })
            .aria_keyshortcuts(if self.detail.is_some() || self.setup.is_some() {
                "Escape"
            } else {
                "Control+K"
            });
        let appearance_button = div()
            .id("toggle-glass")
            .px_2()
            .py_1()
            .cursor_pointer()
            .text_color(theme.accent)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_glass(window, cx)))
            .child(if self.appearance_notice.is_some() {
                "Glass: Unavailable"
            } else if self.glass {
                "Glass: On"
            } else {
                "Glass: Off"
            });
        #[cfg(feature = "upstream-candidate")]
        let appearance_button = appearance_button
            .role(Role::Button)
            .aria_label(if self.appearance_notice.is_some() {
                "Glass unavailable"
            } else if self.glass {
                "Disable glass appearance"
            } else {
                "Enable glass appearance"
            })
            .aria_keyshortcuts("Control+Shift+G");
        let theme_button = div()
            .id("toggle-theme")
            .px_2()
            .py_1()
            .cursor_pointer()
            .text_color(theme.accent)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
            .child(if self.light {
                "Theme: Light"
            } else {
                "Theme: Dark"
            });
        #[cfg(feature = "upstream-candidate")]
        let theme_button = theme_button
            .role(Role::Button)
            .aria_label(if self.light {
                "Switch to dark theme"
            } else {
                "Switch to light theme"
            })
            .aria_keyshortcuts("Control+Shift+T");
        div()
            .key_context("Launcher")
            .size_full()
            .flex()
            .flex_col()
            // No layout gutter: the native compositor owns the external shadow.
            .bg(theme.window_background(self.glass))
            .rounded(px(12.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .text_color(theme.text)
            .font_family("Segoe UI")
            .text_size(px(14.))
            .on_action(cx.listener(|this, _: &Next, _, cx| this.navigate(1, cx)))
            .on_action(cx.listener(|this, _: &Previous, _, cx| this.navigate(-1, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.navigate(7, cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.navigate(-7, cx)))
            .on_action(cx.listener(|this, _: &Execute, window, cx| this.execute(window, cx)))
            .on_action(cx.listener(|this, _: &CopyTarget, window, cx| {
                if this.panel.is_some() { this.execute_panel(Some(ActionId::CopyTarget), window, cx); }
                else { this.copy_target(window, cx); }
            }))
            .on_action(cx.listener(|this, _: &Actions, window, cx| this.toggle_actions(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleGlass, window, cx| this.toggle_glass(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleTheme, _, cx| this.toggle_theme(cx)))
            .on_action(cx.listener(|this, _: &TabNext, window, cx| {
                this.tab_focus(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TabPrevious, window, cx| {
                this.tab_focus(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SetupActivate, window, cx| {
                this.setup_activate_focused(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.is_composing(cx) {
                    return;
                }
                if this.panel.is_some() {
                    this.close_actions(window, cx);
                } else if this.setup.take().is_some() {
                    this.setup_generation = this.setup_generation.wrapping_add(1);
                    compat::focus(window, &this.input.focus_handle(cx), cx);
                } else if this.detail.take().is_none() {
                    this.dismiss(true, window, cx);
                } else {
                    compat::focus(window, &this.input.focus_handle(cx), cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(if self.glass { theme.surface.opacity(0.35) } else { theme.background })
                    .child(div().text_color(theme.accent).text_size(px(24.)).child("⌕"))
                    .child(div().flex_1().min_w_0().child(self.input.clone()))
                    .child(div().text_xs().text_color(theme.muted_text).child("ANSA")),
            )
            .child(
                div()
                    .px_5()
                    .py_3()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.muted_text)
                    .child(if self.setup.is_some() {
                        "FIRST-RUN SETUP"
                    } else if self.detail.is_some() {
                        "COMMAND DETAILS"
                    } else if is_emoji {
                        "EMOJI"
                    } else if is_calculator {
                        "CALCULATOR"
                    } else if self.engine.query().is_empty() {
                        "YOUR APPLICATIONS & COMMANDS"
                    } else {
                        "SEARCH RESULTS"
                    })
                    .child(if let Some(step) = setup_step {
                        format!(
                            "Step {}/5: {step} · {setup_header_action}",
                            step.index() + 1
                        )
                    } else if self.detail.is_some() {
                        "Esc to go back".into()
                    } else if is_emoji {
                        format!("{emoji_count} emoji · Enter copies · paste manually")
                    } else if is_calculator {
                        "Local · arithmetic and bounded units".into()
                    } else {
                        format!("{count} results")
                    }),
            )
            .when(
                self.setup.is_none()
                    && (!self.catalog_skipped.is_empty() || self.catalog_warning.is_some()),
                |el| {
                    let text = match (
                        self.catalog_warning.as_deref(),
                        daily::skipped_warning(&self.catalog_skipped),
                    ) {
                        (Some(warning), Some(skipped)) => format!("{warning} · {skipped}"),
                        (Some(warning), None) => warning.to_owned(),
                        (None, Some(skipped)) => skipped,
                        (None, None) => String::new(),
                    };
                    el.child(
                        div()
                            .px_5()
                            .py_2()
                            .bg(theme.surface)
                            .text_xs()
                            .text_color(theme.danger)
                            .child(text),
                    )
                },
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when_some(setup_view, |el, view| {
                        let actions = setup_controls.iter().fold(
                            div().flex().flex_wrap().gap_2().pt_2(),
                            |row, control| {
                                row.child(setup_button(
                                    *control,
                                    setup_focused == Some(*control),
                                    theme,
                                    cx,
                                ))
                            },
                        );
                        let hint = match view.step {
                            ansa_onboarding::Step::TryLaunch => {
                                "Tab/Shift+Tab moves · Enter or Space activates · Test launch retries · Skip records a warning · Esc closes"
                            }
                            _ => {
                                "Tab/Shift+Tab moves · Enter or Space activates · Esc closes"
                            }
                        };
                        let message_node = setup_message.map(|message| {
                            let node = div().text_color(theme.accent).child(message);
                            #[cfg(feature = "upstream-candidate")]
                            let node = node.role(Role::Alert).aria_label("Setup result");
                            node
                        });
                        let mut warning_nodes: Vec<String> = view.warnings.clone();
                        if let Some(warning) = view.load_warning.clone() {
                            warning_nodes.insert(0, warning);
                        }
                        let warning_node = (!warning_nodes.is_empty()).then(|| {
                            let node = div()
                                .text_color(theme.danger)
                                .child(format!("Warnings: {}", warning_nodes.join(" · ")));
                            #[cfg(feature = "upstream-candidate")]
                            let node = node.role(Role::Alert).aria_label("Setup warnings");
                            node
                        });
                        let document = div()
                            .id("setup")
                            .track_focus(&self.setup_focus)
                            .key_context("Setup")
                            .size_full()
                            .overflow_y_scroll()
                            .px_6()
                            .py_3()
                            .line_height(px(24.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .child(format!("{} · {}", view.title, view.status.label())),
                            )
                            .child(div().text_color(theme.muted_text).child(format!(
                                "Step {} of 5 · {}",
                                view.step.index() + 1,
                                view.step
                            )))
                            .child(div().text_color(theme.muted_text).child(hint))
                            .child(div().child(format!("Shortcut: {}", view.shortcut)))
                            .child(div().child(format!("Launch: {}", view.launch)))
                            .when_some(setup_target, |el, title| {
                                el.child(
                                    div()
                                        .text_color(theme.muted_text)
                                        .child(format!("Test launch target: {title}")),
                                )
                            })
                            .child(div().text_color(theme.muted_text).child(format!(
                                "Keyboard focus: {} of {}",
                                setup_focus_index + 1,
                                setup_controls.len()
                            )))
                            .when_some(message_node, |el, node| el.child(node))
                            .when_some(warning_node, |el, node| el.child(node))
                            .when(view.is_completed(), |el| {
                                el.child(div().text_color(theme.muted_text).child(
                                    "Setup is complete. Reset setup to run it again.",
                                ))
                            })
                            .child(actions);
                        #[cfg(feature = "upstream-candidate")]
                        let document = document.role(Role::Document).aria_label(format!(
                            "Setup: {} step {}",
                            view.title,
                            view.step.index() + 1
                        ));
                        el.child(document)
                    })
                    .when_some(
                        self.detail.clone().filter(|_| self.setup.is_none()),
                        |el, detail| {
                        #[cfg(feature = "upstream-candidate")]
                        let heading = detail
                            .lines()
                            .next()
                            .unwrap_or("Command details")
                            .to_owned();
                        let document = div()
                            .id("details")
                            .size_full()
                            .overflow_y_scroll()
                            .px_6()
                            .py_3()
                            .line_height(px(26.))
                            .child(detail);
                        #[cfg(feature = "upstream-candidate")]
                        let document = document.role(Role::Document).aria_label(heading);
                        el.child(document)
                    })
                    .when(
                        self.setup.is_none()
                            && !is_emoji
                            && self.detail.is_none()
                            && calculation.is_some(),
                        |el| {
                        let value = calculation.unwrap_or_default();
                        let result = div()
                            .text_size(px(48.))
                            .text_color(theme.accent)
                            .child(value.clone());
                        #[cfg(feature = "upstream-candidate")]
                        let result = result
                            .id("calculator-result")
                            .role(Role::Label)
                            .aria_label(format!("Result: {value}"))
                            .aria_value(value);
                        el.child(
                            div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .justify_center()
                                .items_center()
                                .gap_4()
                                .child(
                                    div()
                                        .text_color(theme.muted_text)
                                        .child(self.engine.query().to_string()),
                                )
                                .child(result)
                                 .child(div().text_color(theme.muted_text).child("↵  Copy result")),
                         )
                     })
                    .when_some(
                        calculation_error.filter(|_| {
                            self.detail.is_none() && self.setup.is_none() && !is_emoji
                        }),
                        |el, error| {
                        let alert = div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .px_6()
                            .child(div().text_size(px(24.)).text_color(theme.danger).child("Cannot calculate"))
                            .child(div().text_color(theme.muted_text).child(error.clone()))
                            .child(div().text_xs().text_color(theme.muted_text).child("Use numbers, + − * / ^ % and parentheses. Type = before units or constants. Enter will not launch a command."));
                        #[cfg(feature = "upstream-candidate")]
                        let alert = alert
                            .id("calculator-error")
                            .role(Role::Alert)
                            .aria_label(format!("Cannot calculate: {error}"));
                        el.child(alert)
                    })
                    .when(
                        self.detail.is_none()
                            && self.setup.is_none()
                            && !is_emoji
                            && !is_calculator
                            && count == 0,
                        |el| {
                            el.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap_3()
                                    .child(div().text_size(px(24.)).child("Nothing here yet"))
                                    .child(div().text_color(theme.muted_text).child(
                                        "Try another name, or type an expression like (24 + 8) / 2",
                                    )),
                            )
                        },
                    )
                    .when(
                        self.detail.is_none()
                            && self.setup.is_none()
                            && !is_emoji
                            && !is_calculator
                            && count > 0,
                        |el| {
                            let list = uniform_list(
                                "results",
                                count,
                                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                    let theme = this.theme();
                                    range
                                        .map(|index| {
                                            let command = &this.engine.results()[index];
                                            let selected =
                                                this.engine.selected_index() == Some(index);
                                            let title = command.title.clone();
                                            let subtitle =
                                                command.subtitle.clone().unwrap_or_default();
                                            let command_id = command.id.clone();
                                            let command_id_text = command.id.as_str().to_owned();
                                            let row_id = SharedString::from(format!(
                                                "result-row:{command_id_text}"
                                            ));
                                            let body_id = SharedString::from(format!(
                                                "result-body:{command_id_text}"
                                            ));
                                            #[cfg(feature = "upstream-candidate")]
                                            let accessible_label = if subtitle.is_empty() {
                                                title.clone()
                                            } else {
                                                format!("{title}. {subtitle}")
                                            };
                                            let token = this.engine.query_token();
                                            let pinned =
                                                this.engine.signals().is_favorite(&command.id);
                                            let icon = match command.kind {
                                                CommandKind::Application { .. } => "↗",
                                                _ => "⌘",
                                            };
                                            let body = div()
                                                .id(body_id)
                                                .size_full()
                                                .flex()
                                                .items_center()
                                                .gap_3()
                                                .px_3()
                                                .rounded_md()
                                                .when(selected, |el| el.bg(theme.surface_raised))
                                                .hover(|el| el.bg(theme.surface_raised))
                                                .cursor_pointer()
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        if this.panel.is_some()
                                                            || this.is_composing(cx)
                                                        {
                                                            return;
                                                        }
                                                        if this.engine.query_token() != token {
                                                            return;
                                                        }
                                                        if let Some(current) = this
                                                            .engine
                                                            .results()
                                                            .iter()
                                                            .position(|command| {
                                                                command.id == command_id
                                                            })
                                                        {
                                                            this.engine.select_index(current);
                                                            // UIA SelectionItem.Select is delivered as Click.
                                                            // Accessibility selection must never launch.
                                                            #[cfg(feature = "upstream-candidate")]
                                                            {
                                                                this.announce_selection();
                                                                compat::focus(
                                                                    window,
                                                                    &this.input.focus_handle(cx),
                                                                    cx,
                                                                );
                                                                cx.notify();
                                                            }
                                                            #[cfg(not(feature = "upstream-candidate"))]
                                                            this.execute_primary(window, cx);
                                                        }
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .size(px(32.))
                                                        .rounded_md()
                                                        .bg(theme.surface)
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .text_color(theme.accent)
                                                        .child(icon),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .flex()
                                                        .flex_col()
                                                        .child(div().truncate().child(title))
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(theme.muted_text)
                                                                .truncate()
                                                                .child(subtitle),
                                                        ),
                                                )
                                                .when(pinned, |el| {
                                                    el.child(
                                                        div()
                                                            .text_color(theme.accent)
                                                            .child("★"),
                                                    )
                                                })
                                                .when(selected, |el| {
                                                    el.child(
                                                        div().text_color(theme.muted_text).child("↵"),
                                                    )
                                                });
                                            #[cfg(feature = "upstream-candidate")]
                                            let body = body
                                                .role(Role::ListItem)
                                                .aria_label(accessible_label)
                                                .aria_selected(selected)
                                                // AccessKit positions are zero-based; the Windows
                                                // adapter adds one and reads set size from the list.
                                                .aria_position_in_set(index);
                                            div().id(row_id).h(px(56.)).px_3().py_1().child(body)
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            );
                            #[cfg(feature = "upstream-candidate")]
                            let list = list.track_scroll(&self.scroll);
                            #[cfg(not(feature = "upstream-candidate"))]
                            let list = list.track_scroll(self.scroll.clone());
                            let list = list.size_full();
                            let results = div().id("results-list").size_full();
                            #[cfg(feature = "upstream-candidate")]
                            let results = results
                                .role(Role::List)
                                .aria_label("Search results")
                                .aria_size_of_set(count);
                            el.child(results.child(list))
                        },
                    )
                    .when(self.setup.is_none() && is_emoji, |el| {
                        if let Some(error) = &self.emoji_error {
                            let alert = div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_3()
                                .px_6()
                                .child(
                                    div()
                                        .text_size(px(24.))
                                        .text_color(theme.danger)
                                        .child("Emoji search failed"),
                                )
                                .child(div().text_color(theme.muted_text).child(error.clone()));
                            #[cfg(feature = "upstream-candidate")]
                            let alert = alert
                                .id("emoji-error")
                                .role(Role::Alert)
                                .aria_label(format!("Emoji search failed: {error}"));
                            el.child(alert)
                        } else if self.emoji_results.is_empty() {
                            el.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap_3()
                                    .child(div().text_size(px(24.)).child("No emoji matched"))
                                    .child(div().text_color(theme.muted_text).child(
                                        "Try emoji smile, emoji heart, or type emoji alone to browse.",
                                    )),
                            )
                        } else {
                            let list = uniform_list(
                                "emoji-results",
                                emoji_count,
                                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                    let theme = this.theme();
                                    range
                                        .map(|index| {
                                            let hit = &this.emoji_results[index];
                                            let selected = this.emoji_selected == index;
                                            let grapheme = hit.grapheme.clone();
                                            let name = hit.name.clone();
                                            let accessible_name = hit.accessible_name.clone();
                                            let row_id =
                                                SharedString::from(format!("emoji-row:{index}"));
                                            let body_id =
                                                SharedString::from(format!("emoji-body:{index}"));
                                            let body = div()
                                                .id(body_id)
                                                .size_full()
                                                .flex()
                                                .items_center()
                                                .gap_3()
                                                .px_3()
                                                .rounded_md()
                                                .when(selected, |el| el.bg(theme.surface_raised))
                                                .hover(|el| el.bg(theme.surface_raised))
                                                .cursor_pointer()
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        if this.is_composing(cx)
                                                            || this.emoji_query.is_none()
                                                            || index >= this.emoji_results.len()
                                                        {
                                                            return;
                                                        }
                                                        this.emoji_selected = index;
                                                        #[cfg(feature = "upstream-candidate")]
                                                        {
                                                            compat::focus(
                                                                window,
                                                                &this.input.focus_handle(cx),
                                                                cx,
                                                            );
                                                            cx.notify();
                                                        }
                                                        #[cfg(not(feature = "upstream-candidate"))]
                                                        {
                                                            let _ = window;
                                                            this.copy_emoji(cx);
                                                        }
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .w(px(44.))
                                                        .text_size(px(26.))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .child(grapheme),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .flex()
                                                        .flex_col()
                                                        .child(div().truncate().child(name))
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(theme.muted_text)
                                                                .truncate()
                                                                .child(accessible_name.clone()),
                                                        ),
                                                )
                                                .when(selected, |el| {
                                                    el.child(
                                                        div()
                                                            .text_color(theme.muted_text)
                                                            .child("↵ copy"),
                                                    )
                                                });
                                            #[cfg(feature = "upstream-candidate")]
                                            let body = body
                                                .role(Role::ListItem)
                                                .aria_label(accessible_name)
                                                .aria_selected(selected)
                                                .aria_position_in_set(index);
                                            div()
                                                .id(row_id)
                                                .h(px(56.))
                                                .px_3()
                                                .py_1()
                                                .child(body)
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            );
                            #[cfg(feature = "upstream-candidate")]
                            let list = list.track_scroll(&self.emoji_scroll);
                            #[cfg(not(feature = "upstream-candidate"))]
                            let list = list.track_scroll(self.emoji_scroll.clone());
                            let list = list.size_full();
                            let results = div().id("emoji-list").size_full();
                            #[cfg(feature = "upstream-candidate")]
                            let results = results
                                .role(Role::List)
                                .aria_label("Emoji results")
                                .aria_size_of_set(emoji_count);
                            el.child(results.child(list))
                        }
                    })
                    .when(self.panel.is_some(), |el| {
                        let panel = self.panel.as_ref().expect("panel is present");
                        let token = panel.target().token();
                        let menu_id = SharedString::from(format!(
                            "actions-menu:{}:{}",
                            token.invocation(),
                            token.revision()
                        ));
                        #[cfg(feature = "upstream-candidate")]
                        let menu_count = panel.visible_entries().len();
                        let menu = div()
                            .id(menu_id)
                            .absolute()
                            .right_4()
                            .bottom_3()
                            .w(px(360.))
                            .p_2()
                            .rounded_lg()
                            .bg(theme.surface)
                            .border_1()
                            .border_color(theme.border)
                            .shadow_lg();
                        #[cfg(feature = "upstream-candidate")]
                        let menu = menu
                            .role(Role::Menu)
                            .aria_label("Actions")
                            .aria_size_of_set(menu_count);
                        el.child(
                            menu
                                .child(
                                    div()
                                        .px_3()
                                        .py_2()
                                        .text_xs()
                                        .text_color(theme.muted_text)
                                        .child(if panel_stale { "TARGET CHANGED · Esc to close" } else { "ACTIONS · ↑↓ to select · Esc to close" }),
                                )
                                .child(
                                    div().px_3().py_2().border_b_1().border_color(theme.border).child(self.action_input.clone()),
                                )
                                .children(panel.visible_entries().iter().enumerate().map(|(index, entry)| {
                                    let action = entry.id;
                                    let target = panel.target().clone();
                                    let selected = panel.selected_index() == Some(index);
                                    let row_id = SharedString::from(format!(
                                        "action:{}:{}:{}",
                                        action_slug(action),
                                        token.invocation(),
                                        token.revision()
                                    ));
                                    let row = div().id(row_id).px_3().py_3().rounded_md().flex().items_center().justify_between().cursor_pointer()
                                        .when(selected, |el| el.bg(theme.surface_raised))
                                        .hover(|el| el.bg(theme.surface_raised))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if this.panel.as_ref().is_some_and(|panel| panel.target() == &target) {
                                                this.execute_panel(Some(action), window, cx);
                                            }
                                        }))
                                        .child(entry.title.clone())
                                        .child(div().text_xs().text_color(theme.muted_text).child(entry.shortcut.clone().unwrap_or_default()));
                                    #[cfg(feature = "upstream-candidate")]
                                    let row = row
                                        .role(Role::MenuItem)
                                        .aria_label(entry.title.clone())
                                        // Menu items are invocable commands. AccessKit's
                                        // invocable check rejects any selected state, which
                                        // would suppress UIA InvokePattern.
                                        .aria_position_in_set(index);
                                    row
                                }))
                                .when(panel.visible_entries().is_empty(), |el| el.child(
                                    div().px_3().py_4().text_color(theme.muted_text).child("No matching actions"))),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(44.))
                    .px_5()
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child(status_node)
                    .child(div().flex().items_center().gap_1().child(theme_button).child(appearance_button).child(actions_button)),
            )
    }
}

/// Run the launcher with the platform application built by this crate.
#[cfg(not(feature = "upstream-candidate"))]
pub fn run() -> Result<(), String> {
    run_with_application(Application::new())
}

/// The upstream candidate has no `Application::new`; a platform factory owns
/// startup. It must be launched through [`run_with_application`].
#[cfg(feature = "upstream-candidate")]
pub fn run() -> Result<(), String> {
    Err("This build uses the upstream accessibility candidate: start it with run_with_application(gpui_platform::application()).".to_string())
}

/// Run the launcher inside an already constructed GPUI application.
///
/// The default entry point builds the crates.io 0.2.2 application itself. The
/// isolated upstream candidate experiment supplies `gpui_platform::application()`
/// here so the real launcher shell can run without adding an unpublished
/// platform dependency to this manifest.
pub fn run_with_application(application: Application) -> Result<(), String> {
    let startup_error = std::rc::Rc::new(std::cell::RefCell::new(None));
    let error_for_app = startup_error.clone();
    let a11y_fixture = std::env::args().any(|arg| arg == "--a11y-fixture");
    let glass = !std::env::args().any(|arg| arg == "--opaque");
    let light = std::env::args().any(|arg| arg == "--light");
    let interaction_smoke =
        cfg!(debug_assertions) && std::env::args().any(|arg| arg == "--interaction-smoke-test");
    let startup_smoke = !interaction_smoke && std::env::args().any(|arg| arg == "--smoke-test");
    let native_smoke = std::env::args().any(|arg| arg == "--smoke-test");
    let fixture_run = a11y_fixture || interaction_smoke || native_smoke;
    let explicit_data_dir = std::env::var_os(daily::DATA_DIR_ENV)
        .is_some_and(|value| !value.to_string_lossy().trim().is_empty());
    let data_dir = if fixture_run && !explicit_data_dir {
        daily::smoke_data_dir()
    } else {
        daily::default_data_dir()
    };
    let initial_target = if a11y_fixture {
        None
    } else {
        ansa_platform_windows::capture_foreground_target(None)
    };
    if startup_smoke {
        // Progress diagnostics go to stderr so a hosted failure shows how far
        // startup got. The success marker below stays exclusive to the second
        // frame callback on stdout.
        eprintln!("Ansa native startup smoke: starting GPUI application.");
    }
    application.run(move |cx: &mut App| {
        compat::quit_when_last_window_closes(cx).detach();
        install_keybindings(cx);
        cx.bind_keys([
            KeyBinding::new("down", Next, Some("Launcher")),
            KeyBinding::new("up", Previous, Some("Launcher")),
            KeyBinding::new("pagedown", PageDown, Some("Launcher")),
            KeyBinding::new("pageup", PageUp, Some("Launcher")),
            KeyBinding::new("enter", Execute, Some("Launcher")),
            KeyBinding::new("ctrl-k", Actions, Some("Launcher")),
            KeyBinding::new("ctrl-shift-g", ToggleGlass, Some("Launcher")),
            KeyBinding::new("ctrl-shift-t", ToggleTheme, Some("Launcher")),
            KeyBinding::new("escape", Escape, Some("Launcher")),
            KeyBinding::new("ctrl-shift-c", CopyTarget, Some("Launcher")),
            KeyBinding::new("ctrl-q", Quit, Some("Launcher")),
            KeyBinding::new("tab", TabNext, Some("Launcher")),
            KeyBinding::new("shift-tab", TabPrevious, Some("Launcher")),
            KeyBinding::new("enter", SetupActivate, Some("Setup")),
            KeyBinding::new("space", SetupActivate, Some("Setup")),
        ]);
        let bounds = Bounds::centered(None, size(px(760.), px(500.)), cx);
        let Ok(handle) = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(600.), px(360.))),
            titlebar: None,
            kind: WindowKind::PopUp,
            is_resizable: false,
            is_minimizable: false,
            // Windows needs its custom composition layer; macOS uses GPUI's
            // native vibrancy. Both disable GPUI's legacy accent.
            window_background: initial_window_background(glass),
            app_id: Some("dev.ansa.launcher".into()),
            ..Default::default()
        }, |window, cx| {
            cx.new(|cx| {
                Launcher::new(
                    window,
                    cx,
                    a11y_fixture,
                    glass,
                    light,
                    data_dir.clone(),
                    interaction_smoke,
                )
            })
        }) else {
            *error_for_app.borrow_mut() = Some("Ansa could not create a GPUI window. Check your GPU driver and native dependencies.".to_string());
            cx.quit(); return;
        };
        if startup_smoke {
            eprintln!("Ansa native startup smoke: GPUI window created.");
        }
        #[cfg(target_os = "windows")]
        if let Err(error) = handle.update(cx, |_, window, _| {
            let hwnd = native_handle(window).ok_or("Launcher HWND is unavailable".to_string())?;
            ansa_platform_windows::make_launcher_frameless(hwnd)
        }).map_err(|error| error.to_string()).and_then(|result| result) {
            *error_for_app.borrow_mut() = Some(error);
            cx.quit();
            return;
        }
        #[cfg(target_os = "windows")]
        let _ = handle.update(cx, |this, window, cx| {
            if this.appearance_notice.is_none() {
                this.set_glass(glass, window);
            }
            cx.notify();
        });
        #[cfg(not(target_os = "windows"))]
        let _ = handle.update(cx, |this, _, cx| {
            this.glass = glass;
            cx.notify();
        });
        if a11y_fixture {
            // Built-ins only, no discovery, account state, clipboard writes or
            // external actions. The probe starts this process and talks to it.
            let _ = handle.update(cx, |_, window, _| {
                window.on_next_frame(|window, _| {
                    use std::io::Write;
                    match native_handle(window) {
                        Some(hwnd) => {
                            println!("ANSA_UIA_READY:{hwnd}");
                            let _ = std::io::stdout().flush();
                        }
                        None => eprintln!(
                            "Ansa accessibility fixture: native HWND unavailable"
                        ),
                    }
                });
            });
        } else {
            let hwnd = handle.update(cx, |this, _, _| {
                this.previous_target = initial_target;
                this.native_handle
            }).ok().flatten();
            let (sender, receiver) = async_channel::bounded(1);
            let hotkey = ansa_platform_windows::HotkeyRegistration::register(move || {
                let captured = ansa_platform_windows::capture_foreground_target(hwnd);
                let _ = sender.try_send((std::time::Instant::now(), captured));
            });
            let _ = handle.update(cx, |this, _, cx| {
                match hotkey {
                    Ok(hotkey) => { this.hotkey = Some(hotkey); this.status.push_str(" · Ctrl+Space to summon"); }
                    Err(error) => this.hotkey_notice = Some(format!("Ctrl+Space unavailable: {error}")),
                }
                // Native smoke is a window-creation check. Opening storage would
                // leave nothing usable behind and can migrate a developer's real
                // profile, so it only runs when ANSA_DATA_DIR is explicit.
                if !interaction_smoke && (!native_smoke || explicit_data_dir) {
                    this.refresh_catalog(cx);
                }
                cx.notify();
            });
            cx.spawn(async move |cx| {
                while let Ok((captured_at, target)) = receiver.recv().await {
                    // A blocked UI must not act on a stale summon intent later.
                    if captured_at.elapsed() > std::time::Duration::from_secs(1) { continue; }
                    if handle.update(cx, |this, window, cx| {
                        if this.visible && this.native_handle.is_some_and(ansa_platform_windows::launcher_is_foreground) {
                            this.dismiss(true, window, cx);
                        }
                        else {
                            this.previous_target = target;
                            if let Some(handle) = this.native_handle
                                && let Err(error) = ansa_platform_windows::set_launcher_visible(handle, true)
                            {
                                this.status = format!("Activation needs attention: {error}");
                            }
                            this.visible = true;
                            this.engine.begin_invocation();
                            window.activate_window();
                            compat::focus(window, &this.input.focus_handle(cx), cx);
                            cx.notify();
                        }
                    }).is_err() { break; }
                }
            }).detach();
        }
        cx.activate(true);
        #[cfg(debug_assertions)]
        if interaction_smoke {
            let _ = handle.update(cx, |_, window, cx| smoke::start(cx.entity(), window));
        }
        if startup_smoke {
            let _ = handle.update(cx, |_, window, _| {
                window.on_next_frame(|window, _| {
                    eprintln!("Ansa native startup smoke: first frame callback reached.");
                    window.on_next_frame(|_, cx| {
                        println!(
                            "Ansa native startup smoke: window opened and two GPUI frame callbacks reached, exiting with success."
                        );
                        cx.quit();
                    });
                });
                // GPUI 0.2.2 starts a window's X11 periodic frame loop only
                // when it processes MapNotify/VisibilityNotify. The map
                // request is checked synchronously, so those events can be read
                // into x11rb's private queue while the socket is left empty;
                // the calloop watcher then never wakes and the callbacks above
                // are never requested. Activating the window issues
                // SetInputFocus, whose FocusIn makes the connection readable
                // again, draining the queued mapping events and starting frame
                // delivery. This is the same activation a real launcher summon
                // performs, not a smoke-only shortcut.
                #[cfg(target_os = "linux")]
                if gpui::guess_compositor() == "X11" {
                    window.activate_window();
                }
            });
        }
    });
    let error = startup_error.borrow_mut().take();
    error.map_or(Ok(()), Err)
}

#[cfg(target_os = "windows")]
fn native_handle(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn native_handle(_window: &Window) -> Option<isize> {
    // This is a Windows HWND adapter, not a cross-platform handle probe.
    // GPUI 0.2.2's X11 HasWindowHandle implementation panics rather than
    // returning an unsupported-handle error. Never call it on other targets.
    None
}

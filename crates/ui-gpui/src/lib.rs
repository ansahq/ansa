use ansa_core::{
    Engine,
    action_panel::{ActionEntry, ActionId, ActionPanel, PanelTarget},
    query::{QueryRoute, route_query},
};
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};
use ansa_ui_components::{Changed, EditRejected, TextInput, Theme, install_keybindings};
#[cfg(feature = "upstream-candidate")]
use gpui::Role;
use gpui::{
    App, Application, Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyBinding,
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, actions, div, prelude::*,
    px, size, uniform_list,
};

mod compat;
mod host_lifecycle;
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
    host: host_lifecycle::LauncherHost,
    hotkey_notice: Option<String>,
    glass: bool,
    light: bool,
    theme_notice: Option<String>,
    appearance_notice: Option<String>,
    #[cfg(target_os = "windows")]
    native_shape: Option<(i32, i32, i32)>,
    native_handle: Option<isize>,
    previous_target: Option<ansa_platform_windows::FocusTarget>,
    catalog_refresh: ansa_core::refresh::RefreshGate,
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

fn builtin(id: &str, title: &str, subtitle: &str, kind: BuiltinCommand) -> Command {
    Command {
        id: CommandId::new(id).expect("static command ID"),
        title: title.into(),
        subtitle: Some(subtitle.into()),
        keywords: vec!["ansa".into()],
        alias: None,
        kind: CommandKind::Builtin(kind),
    }
}

fn builtin_commands() -> Vec<Command> {
    vec![
        builtin(
            "ansa:refresh",
            "Refresh Applications",
            "Rescan Windows Start Menu shortcuts",
            BuiltinCommand::RefreshApplications,
        ),
        builtin(
            "ansa:settings",
            "Integration Status",
            "See what this development build supports",
            BuiltinCommand::Settings,
        ),
        builtin(
            "ansa:about",
            "About Ansa",
            "An open-source shortcut to your everyday",
            BuiltinCommand::About,
        ),
    ]
}

fn catalog() -> Result<Vec<Command>, String> {
    let mut commands = builtin_commands();
    let apps = ansa_platform_windows::discover_applications()?;
    commands.extend(apps.into_iter().map(|app| Command {
        id: CommandId::new(&app.id).expect("platform application ID"),
        title: app.title,
        subtitle: Some("Application · Windows Start Menu".into()),
        keywords: vec!["application".into(), "app".into()],
        alias: None,
        kind: CommandKind::Application { path: app.path },
    }));
    Ok(commands)
}

impl Launcher {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        a11y_fixture: bool,
        glass: bool,
        light: bool,
        system_theme: bool,
    ) -> Self {
        let native_handle = native_handle(window);
        let host = host_lifecycle::create_host(native_handle);
        // Observe the system appearance once at startup. If the value is
        // unavailable, keep the explicit CLI/default choice but report it in
        // the status line instead of silently pretending the OS was read.
        // Deterministic debug fixtures keep the CLI/default choice so the
        // existing interaction smoke does not depend on the machine theme.
        let (observed_light, theme_notice) = if system_theme {
            match host.borrow_mut().observe_theme() {
                Ok(observation) => (
                    observation.appearance == ansa_host::ThemeAppearance::Light,
                    None,
                ),
                Err(error) => (
                    false,
                    Some(format!("System appearance unavailable: {error}")),
                ),
            }
        } else {
            (false, None)
        };
        let light = light || observed_light;
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
        let commands = builtin_commands();
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
                this.detail = None;
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
        let launcher = Self {
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
            host: host.clone(),
            hotkey_notice: None,
            glass,
            light,
            theme_notice,
            appearance_notice: None,
            #[cfg(target_os = "windows")]
            native_shape: None,
            native_handle,
            previous_target: None,
            catalog_refresh: Default::default(),
            _input_subscription: subscription,
            _action_subscription: action_subscription,
            _rejection_subscriptions: rejection_subscriptions,
        };
        // Adopt the window GPUI already created. Window close/quit shutdown is
        // provided by Drop for Launcher (and the controller's Drop backstop),
        // so no per-version `on_window_closed` signature is needed here.
        let _ = host.borrow_mut().adopt_visible();
        launcher
    }

    fn toggle_glass(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) || self.appearance_notice.is_some() {
            return;
        }
        self.glass = !self.glass;
        cx.notify();
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
        self.dismiss_from_host(restore, window, cx);
    }

    fn execute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        if self.panel.is_some() {
            self.execute_panel(None, window, cx);
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
        if self.is_composing(cx) || self.detail.is_some() {
            return;
        }
        if let QueryRoute::Calculator(result) = route_query(self.engine.query()) {
            match result {
                Ok(value) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
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
        let result = match command.kind {
            CommandKind::Application { path } => ansa_platform_windows::open_application(&path),
            CommandKind::Quicklink { url } => ansa_platform_windows::open_url(&url),
            CommandKind::Builtin(BuiltinCommand::RefreshApplications) => {
                self.refresh_catalog(cx);
                return;
            }
            CommandKind::Builtin(BuiltinCommand::Settings) => {
                self.detail = Some("Integration status\n\nApplications: Windows Start Menu shortcuts (.lnk). Packaged apps and switching to running windows are not supported yet.\n\nSummon: Ctrl+Space when registration succeeds. Shortcut editing is not implemented. Dismissal attempts checked, best-effort focus return only if Ansa still owns the foreground. Windows can deny activation.\n\nActions: Ctrl+K, type to filter, arrows to select, Enter to act. Pins and usage signals last only for this process; no settings are saved yet. Tab reaches the footer control.\n\nCalculator: bounded approximate f64 arithmetic; Enter copies. Start with = to explicitly request arithmetic. No units, currencies, or dates yet.\n\nClipboard history, automatic paste, file indexing, extensions, AI and sync are not enabled. No account or network service is required.\n\nAccessibility and multilingual input still require real-device validation.".into());
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

    fn copy_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
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

    fn refresh_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.catalog_refresh.request() else {
            self.status = "Refresh in progress — one follow-up queued".into();
            cx.notify();
            return;
        };
        self.status = "Refreshing application catalog…".into();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async { catalog() }).await;
            let _ = this.update(cx, |this, cx| {
                if !this.catalog_refresh.complete(ticket) {
                    return;
                }
                match result {
                    Ok(commands) => {
                        let count = commands.len().saturating_sub(builtin_commands().len());
                        this.engine.replace_catalog(commands);
                        this.status = format!("{count} applications · local only");
                    }
                    Err(error) => {
                        this.status =
                            format!("Catalog refresh failed (previous catalog kept): {error}")
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
        if self.is_composing(cx) || self.detail.is_some() {
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

    fn tab_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
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

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "windows")]
        if let Some(hwnd) = self.native_handle {
            let viewport = window.viewport_size();
            let scale = window.scale_factor();
            let radius = (12.0 * scale).round().clamp(1.0, 256.0) as i32;
            let shape = (
                (f32::from(viewport.width) * scale).round() as i32,
                (f32::from(viewport.height) * scale).round() as i32,
                radius,
            );
            if self.native_shape != Some(shape) {
                self.native_shape = Some(shape);
                if let Err(error) = ansa_platform_windows::round_launcher_window(hwnd, radius) {
                    eprintln!("Launcher corner clipping: {error}");
                    self.appearance_notice = Some(error);
                    self.glass = false;
                }
                // Reapply physical placement when the window size or scale
                // factor changes. This never re-shows, refocuses or issues a
                // new invocation.
                if let Err(error) = self
                    .host
                    .borrow_mut()
                    .reapply_placement(ansa_host::Size::new(shape.0, shape.1))
                {
                    eprintln!("Launcher placement: {error}");
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
            QueryRoute::Calculator(Ok(value)) => Some(*value),
            _ => None,
        };
        let calculation_error = match &route {
            QueryRoute::Calculator(Err(error)) => Some(error.to_string()),
            _ => None,
        };
        let count = self.engine.results().len();
        let status_text = match &self.hotkey_notice {
            Some(notice) => format!("{notice} · {}", self.status),
            None => self.status.clone(),
        };
        let status_text = match &self.theme_notice {
            Some(notice) => format!("{notice} · {status_text}"),
            None => status_text,
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
                if this.detail.take().is_some() {
                    compat::focus(window, &this.input.focus_handle(cx), cx);
                    cx.notify();
                } else {
                    this.toggle_actions(window, cx);
                }
            }))
            .child(if self.detail.is_some() {
                "Back  Esc"
            } else {
                "Actions  Ctrl K"
            });
        #[cfg(feature = "upstream-candidate")]
        let actions_button = actions_button
            .role(Role::Button)
            .aria_label(if self.detail.is_some() {
                "Back"
            } else {
                "Actions"
            })
            .aria_keyshortcuts(if self.detail.is_some() {
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
            // No layout gutter: native clipping and the compositor own the
            // corners/shadow, so invisible padding cannot intercept clicks.
            .bg(if self.glass { theme.glass_background() } else { theme.background })
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
            .on_action(cx.listener(|this, _: &TabNext, window, cx| this.tab_focus(window, cx)))
            .on_action(cx.listener(|this, _: &TabPrevious, window, cx| this.tab_focus(window, cx)))
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.is_composing(cx) {
                    return;
                }
                if this.panel.is_some() {
                    this.close_actions(window, cx);
                } else if this.detail.take().is_none() {
                    this.dismiss(true, window, cx);
                } else {
                    compat::focus(window, &this.input.focus_handle(cx), cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                this.shutdown_host();
                cx.quit();
            }))
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
                    .child(if self.detail.is_some() {
                        "COMMAND DETAILS"
                    } else if is_calculator {
                        "CALCULATOR"
                    } else if self.engine.query().is_empty() {
                        "YOUR APPLICATIONS & COMMANDS"
                    } else {
                        "SEARCH RESULTS"
                    })
                    .child(if self.detail.is_some() {
                        "Esc to go back".into()
                    } else if is_calculator {
                        "Local · approximate f64 arithmetic".into()
                    } else {
                        format!("{count} results")
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when_some(self.detail.clone(), |el, detail| {
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
                    .when(self.detail.is_none() && calculation.is_some(), |el| {
                        let value = calculation.unwrap_or_default().to_string();
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
                    .when_some(calculation_error.filter(|_| self.detail.is_none()), |el, error| {
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
                            .child(div().text_xs().text_color(theme.muted_text).child("Use numbers, + − * / and parentheses. Enter will not launch a command."));
                        #[cfg(feature = "upstream-candidate")]
                        let alert = alert
                            .id("calculator-error")
                            .role(Role::Alert)
                            .aria_label(format!("Cannot calculate: {error}"));
                        el.child(alert)
                    })
                    .when(
                        self.detail.is_none() && !is_calculator && count == 0,
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
                        self.detail.is_none() && !is_calculator && count > 0,
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

impl Drop for Launcher {
    fn drop(&mut self) {
        // Window close and app quit drop the root view. Run the ordered
        // shutdown here so the hotkey and worker channel are released even if
        // no explicit Ctrl+Q path ran.
        self.shutdown_host();
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
    #[cfg(debug_assertions)]
    let lifecycle_smoke = std::env::args().any(|arg| arg == "--lifecycle-smoke-test");
    #[cfg(not(debug_assertions))]
    let lifecycle_smoke = false;
    let system_theme = !a11y_fixture && !interaction_smoke && !lifecycle_smoke;
    let initial_target = if a11y_fixture {
        None
    } else {
        ansa_platform_windows::capture_foreground_target(None)
    };
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
        ]);
        let bounds = Bounds::centered(None, size(px(760.), px(500.)), cx);
        let Ok(handle) = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(600.), px(360.))),
            titlebar: None,
            kind: WindowKind::PopUp,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some("dev.ansa.launcher".into()),
            ..Default::default()
        }, |window, cx| cx.new(|cx| Launcher::new(window, cx, a11y_fixture, glass, light, system_theme))) else {
            *error_for_app.borrow_mut() = Some("Ansa could not create a GPUI window. Check your GPU driver and native dependencies.".to_string());
            cx.quit(); return;
        };
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
        if let Err(error) = handle.update(cx, |this, window, cx| {
            let hwnd = native_handle(window).ok_or("Launcher HWND is unavailable".to_string())?;
            match ansa_platform_windows::enable_launcher_alpha(hwnd) {
                Ok(()) => Ok(()),
                Err(error) => {
                    eprintln!("Launcher transparency unavailable: {error}");
                    this.appearance_notice = Some(error);
                    this.glass = false;
                    cx.notify();
                    Ok::<(), String>(())
                }
            }
        }).map_err(|error| error.to_string()).and_then(|result| result) {
            *error_for_app.borrow_mut() = Some(error);
            cx.quit();
            return;
        }
        #[cfg(debug_assertions)]
        if lifecycle_smoke {
            let cycles = match host_lifecycle::lifecycle_cycles() {
                Ok(cycles) => cycles,
                Err(error) => {
                    *error_for_app.borrow_mut() = Some(error);
                    cx.quit();
                    return;
                }
            };
            let failure = error_for_app.clone();
            let _ = handle.update(cx, |_, window, cx| {
                host_lifecycle::run_lifecycle_smoke(cx.entity(), window, cycles, failure);
            });
            cx.activate(true);
            return;
        }
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
            let hotkey = ansa_platform_windows::HotkeyRegistration::register({
                let sender = sender.clone();
                move || {
                    let captured = ansa_platform_windows::capture_foreground_target(hwnd);
                    let _ = sender.try_send((std::time::Instant::now(), captured));
                }
            });
            let _ = handle.update(cx, |this, _, cx| {
                match hotkey {
                    Ok(hotkey) => {
                        this.install_hotkey(hotkey, sender);
                        this.status.push_str(" · Ctrl+Space to summon");
                    }
                    Err(error) => this.hotkey_notice = Some(format!("Ctrl+Space unavailable: {error}")),
                }
                if !interaction_smoke { this.refresh_catalog(cx); }
                cx.notify();
            });
            host_lifecycle::watch_environment(handle, cx);
            cx.spawn(async move |cx| {
                while let Ok((captured_at, target)) = receiver.recv().await {
                    // A blocked UI must not act on a stale summon intent later.
                    if captured_at.elapsed() > std::time::Duration::from_secs(1) { continue; }
                    if handle.update(cx, |this, window, cx| {
                        if this.host.borrow().state().is_visible()
                            && this.native_handle.is_some_and(ansa_platform_windows::launcher_is_foreground)
                        {
                            this.dismiss(true, window, cx);
                        } else {
                            this.summon_from_host(target, window, cx);
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
        if !interaction_smoke && std::env::args().any(|arg| arg == "--smoke-test") {
            let _ = handle.update(cx, |_, window, _| {
                window.on_next_frame(|window, _| {
                    window.on_next_frame(|_, cx| cx.quit());
                });
            });
        }
    });
    let error = startup_error.borrow_mut().take();
    error.map_or(Ok(()), Err)
}

fn native_handle(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

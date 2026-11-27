//! Launcher shell: window, focus and the root render tree.
//!
//! This module owns the retained GPUI window and the top-level render.
//! Selection, text editing and command dispatch live in `navigation`,
//! `input` and `dispatch`; feature surfaces live under `views`.

use crate::*;

pub(crate) struct Launcher {
    pub(crate) input: Entity<TextInput>,
    pub(crate) action_input: Entity<TextInput>,
    pub(crate) panel: Option<ActionPanel>,
    pub(crate) actions_focus: FocusHandle,
    pub(crate) engine: Engine,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) status: String,
    #[cfg(feature = "upstream-candidate")]
    pub(crate) selection_announcement: Option<String>,
    pub(crate) detail: Option<String>,
    #[cfg(windows)]
    pub(crate) hotkey: Option<ansa_platform_windows::HotkeyRegistration>,
    #[cfg(not(windows))]
    pub(crate) native_host: native_host::NativeHost,
    #[cfg(windows)]
    pub(crate) hotkey_notice: Option<String>,
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) visible: bool,
    pub(crate) glass: bool,
    #[cfg(target_os = "macos")]
    pub(crate) glass_request: MacGlassRequest,
    pub(crate) light: bool,
    #[cfg(target_os = "windows")]
    pub(crate) backdrop: Option<ansa_platform_windows::LauncherBackdrop>,
    pub(crate) appearance_notice: Option<String>,
    #[cfg(target_os = "windows")]
    pub(crate) native_shape: Option<(i32, i32, i32)>,
    #[cfg(windows)]
    pub(crate) native_handle: Option<isize>,
    #[cfg(windows)]
    pub(crate) previous_target: Option<ansa_platform_windows::FocusTarget>,
    pub(crate) catalog_refresh: ansa_core::refresh::RefreshGate,
    pub(crate) driver: Option<ansa_alpha::DailyDriver>,
    pub(crate) data_dir: PathBuf,
    pub(crate) storage_error: Option<String>,
    pub(crate) catalog_skipped: Vec<ansa_alpha::SkippedQuicklink>,
    pub(crate) catalog_warning: Option<String>,
    pub(crate) icon_sources: std::collections::BTreeMap<CommandId, String>,
    #[cfg(not(windows))]
    pub(crate) host_capabilities: Option<ansa_platform_api::HostCapabilities>,
    #[cfg(not(windows))]
    pub(crate) catalog_generation: Option<u64>,
    #[cfg(not(windows))]
    pub(crate) displayed_catalog_generation: Option<u64>,
    /// The exact native entries rendered by the current catalog generation.
    /// Execution uses their selection tokens, not a rediscovered path and ID.
    #[cfg(not(windows))]
    pub(crate) native_entries: Vec<AppEntry>,
    pub(crate) persisted_settings: Option<daily::PersistedSettings>,
    pub(crate) setup: Option<daily::SetupSession>,
    pub(crate) setup_focus: FocusHandle,
    pub(crate) setup_generation: u64,
    pub(crate) smoke_fixture: bool,
    pub(crate) emoji_query: Option<String>,
    pub(crate) emoji_results: Vec<ansa_alpha::EmojiHit>,
    pub(crate) emoji_selected: usize,
    pub(crate) emoji_error: Option<String>,
    pub(crate) emoji_scroll: UniformListScrollHandle,
    pub(crate) _input_subscription: Subscription,
    pub(crate) _action_subscription: Subscription,
    pub(crate) _rejection_subscriptions: Vec<Subscription>,
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
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        a11y_fixture: bool,
        appearance: InitialAppearance,
        light: bool,
        data_dir: PathBuf,
        smoke_fixture: bool,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx);
            input.set_placeholder("Search apps and commands…", cx);
            input.set_typography(17., 26., cx);
            input
        });
        let action_input = cx.new(|cx| {
            let mut input = TextInput::new(cx);
            input.set_placeholder("Search actions…", cx);
            input.set_typography(13., 20., cx);
            input
        });
        let theme = Palette::new(light).input_theme();
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
                this.on_input_changed(&event.0, window, cx);
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
        // GPUI exposes activation but no accessibility display-option change
        // notification. Re-evaluate the OS Reduce Transparency state on every
        // activation; retain the requested glass preference separately from the
        // effective opaque fallback.
        #[cfg(target_os = "macos")]
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                return;
            }
            let before = (this.glass, this.appearance_notice.clone());
            this.set_glass(this.glass_request, window);
            if before != (this.glass, this.appearance_notice.clone()) {
                cx.notify();
            }
        })
        .detach();
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
            #[cfg(windows)]
            hotkey: None,
            #[cfg(not(windows))]
            native_host: native_host::NativeHost::new(),
            #[cfg(windows)]
            hotkey_notice: None,
            #[cfg(any(windows, target_os = "macos"))]
            visible: true,
            // Windows enables tint only after its separate native layer attaches.
            glass: appearance.background == WindowBackgroundAppearance::Blurred,
            #[cfg(target_os = "macos")]
            glass_request: MacGlassRequest::new(appearance.requested),
            light,
            #[cfg(target_os = "windows")]
            backdrop: None,
            appearance_notice: appearance.unavailable.map(str::to_owned),
            #[cfg(target_os = "windows")]
            native_shape: None,
            #[cfg(windows)]
            native_handle: native_handle(window),
            #[cfg(windows)]
            previous_target: None,
            catalog_refresh: Default::default(),
            driver: None,
            data_dir,
            storage_error: None,
            catalog_skipped: Vec::new(),
            catalog_warning: None,
            icon_sources: Default::default(),
            #[cfg(not(windows))]
            host_capabilities: None,
            #[cfg(not(windows))]
            catalog_generation: None,
            #[cfg(not(windows))]
            displayed_catalog_generation: None,
            #[cfg(not(windows))]
            native_entries: Vec::new(),
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

    pub(crate) fn toggle_glass(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        #[cfg(windows)]
        if self.appearance_notice.is_none() {
            self.set_glass(!self.glass, window);
        }
        #[cfg(target_os = "macos")]
        self.set_glass(self.glass_request.toggled(), window);
        #[cfg(target_os = "linux")]
        if self.appearance_notice.is_none() {
            let appearance = InitialAppearance::for_linux(!self.glass, linux_wayland());
            window.set_background_appearance(appearance.background);
            self.glass = appearance.background == WindowBackgroundAppearance::Blurred;
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        let _ = window;
        cx.notify();
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn set_glass(&mut self, enabled: bool, window: &Window) {
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

    #[cfg(target_os = "macos")]
    pub(crate) fn set_glass(&mut self, request: MacGlassRequest, window: &Window) {
        // Keep the requested preference even when Reduce Transparency forces
        // the effective surface opaque. Clearing the OS setting later must
        // restore the requested vibrancy, not silently turn glass off.
        self.glass_request = request;
        let appearance = request.evaluate(macos_reduce_transparency());
        window.set_background_appearance(appearance.background);
        self.glass = appearance.background == WindowBackgroundAppearance::Blurred;
        self.appearance_notice = appearance.unavailable.map(str::to_owned);
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn glass_unavailable(&mut self, error: String) {
        self.glass = false;
        self.backdrop = None;
        eprintln!("Launcher frosted glass unavailable: {error}");
        self.appearance_notice = Some(error);
    }

    pub(crate) fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        self.light = !self.light;
        let theme = Palette::new(self.light).input_theme();
        self.input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        self.action_input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        cx.notify();
    }
}

impl Launcher {
    #[cfg(windows)]
    pub(crate) fn dismiss(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
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
}

impl Launcher {
    #[cfg(not(windows))]
    pub(crate) fn dismiss(&mut self, escape: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        #[cfg(target_os = "macos")]
        if self.hotkey_registered() && self.native_host.lifecycle.can_hide_retained() {
            match self.native_host.set_macos_visible(window, false) {
                Ok(()) => {
                    self.visible = false;
                    self.engine.hide_root();
                    self.close_actions(window, cx);
                    // AppKit decides what receives focus next. Hiding the
                    // window does not establish a verified focus restoration.
                    self.native_host.clear_focus();
                }
                Err(error) => self.status = format!("Cannot hide launcher: {error}"),
            }
            cx.notify();
            return;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = window;
        // Leave the current desktop window usable unless there is a verified
        // hide/show route. A successful external handoff keeps its own status.
        if escape {
            self.status = self.native_host.dismiss_fallback().to_owned();
            cx.notify();
        }
    }
}

impl Launcher {
    pub(crate) fn settings_detail(&self) -> String {
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
        #[cfg(windows)]
        let applications = "Windows Start Menu shortcuts and packaged apps (launched by AUMID). Enter launches or switches; a switch is reported only when the target window is observed as foreground, otherwise the outcome is denied, failed or unverified. Broken shortcuts stay visible with a refresh hint.".to_owned();
        #[cfg(not(windows))]
        let applications = match &self.host_capabilities {
            Some(host) => format!(
                "{} · catalog: {} · launch: {} · HTTP(S) opener: {}. Enter hands the application to the desktop. A handoff does not verify the foreground window.",
                host.adapter, host.application_catalog, host.application_launch, host.url_open,
            ),
            None => "Catalog and opener capabilities have not been probed yet.".to_owned(),
        };
        #[cfg(windows)]
        let summon = "Ctrl+Space when registration succeeds. Shortcut editing is not implemented. Dismissal attempts checked, best-effort focus return only if Ansa still owns the foreground. Windows can deny activation.".to_owned();
        #[cfg(not(windows))]
        let summon = self.native_host.summon_detail();
        let appearance = self.appearance_notice.as_deref().unwrap_or(
            if cfg!(target_os = "macos") {
                "GPUI can request native macOS vibrancy but cannot verify the compositor effect."
            } else {
                "Glass and theme are session-only."
            },
        );
        let vault = match self.driver.as_ref() {
            Some(driver) => match driver.vault_status() {
                Some(status) => format!(
                    "{:?} · {:?} · {}",
                    status.mode,
                    status.state,
                    driver.vault_path().display()
                ),
                None => format!(
                    "unavailable — {}",
                    driver
                        .vault_error()
                        .unwrap_or("secure storage did not open in this session")
                ),
            },
            None => "not opened yet".to_owned(),
        };
        let features = views::scaffold_status();
        format!(
            "Integration status\n\nApplications: {applications}\n\n\
             Setup: run Start Setup. Tab/Shift+Tab move between controls, Enter or Space activates, and Escape closes. The shortcut is checked against this session's live registration even when an older saved choice exists. Setup records a successful desktop launch handoff or verified switch, not merely a request in progress. Skipping an unverified step records a warning.\n\n\
             Summon: {summon}\n\n\
             Appearance: {appearance}\n\n\
             Vault: {vault}\n\n\
             Actions: Ctrl+K, type to filter, arrows to select, Enter to act. Pins and usage signals last only for this process. Tab reaches the footer control.\n\n\
             Quicklinks: enabled static HTTP(S) links use the desktop's URL handler. Required-argument, local-path, unsupported-opener and unresolved-template entries are omitted and reported; raw templates are never launched. To import, close Ansa (it holds the exclusive storage lock), run `ansa quicklink import <file> --data-dir <data-dir>`, then start Ansa. Refresh re-reads in-process persisted state only. Desktop authoring, preview, file reveal, import and export are not implemented.\n\n\
             Emoji: type emoji followed by a query (for example, emoji smile), then use Up/Down and Enter. Paste manually; insertion and automatic paste are not implemented.\n\n\
             Calculator: start with = to request arithmetic, units or constants. Bounded length, mass, time and temperature units are supported. Currencies, dates and time zones are not. Enter copies the result.\n\n\
             {persisted}\n\n\
             Persisted preference note: CLI flags control starting appearance. The live shortcut is separate from stored settings. Persisted theme, launch_mode, maximum_results and alias_prefix are shown but not applied; ranking signals are session-only.\n\n\
             {features}\n\nNo account or network service is required.\n\n\
             {storage}\n\n{catalog_warning}\n\n{omitted}\n\n\
             Accessibility: the default GPUI pin has no accessibility bridge. Keyboard-only setup and emoji navigation are covered by synthetic interaction tests, not screen-reader validation.",
        )
    }

    pub(crate) fn open_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(crate) fn close_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing(cx) {
            return;
        }
        self.setup_generation = self.setup_generation.wrapping_add(1);
        self.setup = None;
        compat::focus(window, &self.input.focus_handle(cx), cx);
        cx.notify();
    }

    pub(crate) fn setup_hotkey_probe(&self) -> daily::HotkeyProbe {
        #[cfg(windows)]
        return daily::HotkeyProbe::new(
            self.hotkey_registered(),
            self.hotkey_notice
                .clone()
                .unwrap_or_else(|| "Ctrl+Space registration did not succeed".into()),
        );
        #[cfg(not(windows))]
        daily::HotkeyProbe::from_outcome(self.native_host.shortcut_probe())
    }

    pub(crate) fn hotkey_registered(&self) -> bool {
        #[cfg(windows)]
        return self.hotkey.is_some();
        #[cfg(not(windows))]
        self.native_host.hotkey_registered()
    }

    pub(crate) fn setup_move_focus(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_composing(cx) || self.setup.is_none() {
            return;
        }
        if let Some(setup) = self.setup.as_mut() {
            setup.focus_next(forward);
        }
        compat::focus(window, &self.setup_focus, cx);
        cx.notify();
    }

    pub(crate) fn setup_activate_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(crate) fn setup_activate_control(
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
            let launch = self.application_launcher();
            let id_for_launch = id.clone();
            #[cfg(not(windows))]
            let launch_entry = self
                .native_entries
                .iter()
                .find(|entry| entry.id == id.as_str() && entry.path == path)
                .cloned();
            #[cfg(windows)]
            let launch_entry: Option<AppEntry> = None;
            cx.spawn(async move |this, cx| {
                let (outcome, detail) = cx
                    .background_executor()
                    .spawn(async move {
                        daily::map_launch_outcome(launch(
                            &path,
                            id_for_launch.as_str(),
                            launch_entry.as_ref(),
                        ))
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

    pub(crate) fn refresh_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.catalog_refresh.request() else {
            self.status = "Refresh in progress — one follow-up queued".into();
            cx.notify();
            return;
        };
        self.status = "Refreshing application catalog…".into();
        #[cfg(not(windows))]
        {
            self.catalog_generation = None;
            self.displayed_catalog_generation = None;
        }
        let data_dir = self.data_dir.clone();
        let driver = self.driver.take();
        cx.spawn(async move |this, cx| {
            let (driver, open_error, settings, result, _native_snapshot, _retained_entries) = cx
                .background_executor()
                .spawn(async move {
                    let (mut driver, open_error) = match driver {
                        Some(driver) => (Some(driver), None),
                        None => match ansa_alpha::DailyDriver::open(&data_dir) {
                            Ok(driver) => (Some(driver), None),
                            Err(error) => (None, Some(error.to_string())),
                        },
                    };
                    let settings = driver.as_ref().map(daily::read_persisted_settings);
                    #[cfg(windows)]
                    let (result, native_snapshot, retained_entries) = {
                        let discovery = ansa_platform_api::discover_applications();
                        let retained = discovery.as_ref().ok().cloned().unwrap_or_default();
                        (
                            daily::refresh_catalog(driver.as_mut(), discovery),
                            (),
                            retained,
                        )
                    };
                    #[cfg(not(windows))]
                    let (result, native_snapshot, retained_entries) = {
                        let (generation, (discovery, capabilities)) =
                            catalog_dispatch::NATIVE_CATALOG.refresh(|| {
                                let capabilities = ansa_platform_api::host_capabilities();
                                (ansa_platform_api::discover_applications(), capabilities)
                            });
                        // Retain the exact entries whose selection tokens were
                        // discovered for this generation. Composition consumes
                        // the discovery result, so clone before it moves.
                        let retained = discovery.as_ref().ok().cloned().unwrap_or_default();
                        (
                            daily::refresh_catalog(driver.as_mut(), discovery),
                            (generation, capabilities),
                            retained,
                        )
                    };
                    (
                        driver,
                        open_error,
                        settings,
                        result,
                        native_snapshot,
                        retained_entries,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.catalog_refresh.complete(ticket) {
                    return;
                }
                this.driver = driver;
                let icon_sources = _retained_entries
                    .iter()
                    .filter_map(|entry: &AppEntry| {
                        Some((CommandId::new(&entry.id).ok()?, entry.icon_source.clone()?))
                    })
                    .collect();
                #[cfg(not(windows))]
                {
                    this.host_capabilities = Some(_native_snapshot.1);
                    this.catalog_generation = Some(_native_snapshot.0);
                    this.native_entries = _retained_entries;
                }
                this.storage_error = open_error.clone();
                this.persisted_settings = settings;
                match result {
                    Ok(load) => {
                        this.icon_sources = icon_sources;
                        let summary = load.summary();
                        let warning = load.warning.clone();
                        this.catalog_skipped = load.skipped;
                        this.catalog_warning = warning.clone();
                        this.engine.replace_catalog(load.commands);
                        this.status = match (&open_error, &warning) {
                            (Some(error), warning) => {
                                let mut status = format!(
                                    "{summary} · {}",
                                    daily::storage_recovery(&this.data_dir, error)
                                );
                                if let Some(warning) = warning {
                                    status.push_str(&format!(" · {warning}"));
                                }
                                status
                            }
                            (None, Some(warning)) => format!("{summary} · {warning}"),
                            (None, None) => summary,
                        };
                    }
                    Err(error) => {
                        this.status = match &open_error {
                            Some(open_error) => daily::storage_recovery(&this.data_dir, open_error),
                            None => {
                                format!("Catalog refresh failed (previous catalog kept): {error}")
                            }
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
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InitialAppearance {
    pub(crate) background: WindowBackgroundAppearance,
    pub(crate) unavailable: Option<&'static str>,
    /// What the user asked for, before an OS accessibility fallback.
    /// MacGlassRequest consumes this on macOS; tests also inspect it.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(crate) requested: bool,
}

impl InitialAppearance {
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn for_linux(glass: bool, wayland: bool) -> Self {
        if !wayland {
            return Self::new(Platform::Linux, glass, false);
        }
        Self {
            background: if glass {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Transparent
            },
            unavailable: None,
            requested: glass,
        }
    }

    pub(crate) fn new(platform: Platform, glass: bool, reduce_transparency: bool) -> Self {
        use WindowBackgroundAppearance::{Blurred, Opaque, Transparent};
        let (background, unavailable) = match platform {
            // Reduce Transparency only makes a requested effect unavailable.
            // A user who chose opaque keeps the ordinary Off state.
            Platform::MacOs if glass && reduce_transparency => {
                (Opaque, Some("macOS Reduce Transparency is enabled"))
            }
            Platform::MacOs if glass => (Blurred, None),
            // Only rounded panel corners are clear, not the opaque themed surface.
            Platform::MacOs => (Transparent, None),
            // The Win32 composition layer may attach later.
            Platform::Windows => (Opaque, None),
            Platform::Linux => (Opaque, Some("Linux glass requires Wayland")),
            Platform::Other => (Opaque, Some("No native blur backend is implemented")),
        };
        Self {
            background,
            unavailable,
            requested: glass,
        }
    }
}

/// The user's requested glass preference, independent of the native effect.
/// GPUI has no macOS accessibility display-option observer, so this state is
/// re-evaluated when the window is activated or the user toggles glass.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MacGlassRequest {
    pub(crate) requested: bool,
}

#[cfg(any(target_os = "macos", test))]
impl MacGlassRequest {
    const fn new(requested: bool) -> Self {
        Self { requested }
    }

    const fn toggled(self) -> Self {
        Self {
            requested: !self.requested,
        }
    }

    pub(crate) fn evaluate(self, reduce_transparency: bool) -> InitialAppearance {
        InitialAppearance::new(Platform::MacOs, self.requested, reduce_transparency)
    }
}

#[cfg(not(target_os = "linux"))]
fn macos_reduce_transparency() -> bool {
    #[cfg(target_os = "macos")]
    return objc2_app_kit::NSWorkspace::sharedWorkspace()
        .accessibilityDisplayShouldReduceTransparency();
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "linux")]
fn linux_wayland() -> bool {
    gpui::guess_compositor() == "Wayland"
}

fn glass_label(platform: Platform, glass: bool, unavailable: bool) -> &'static str {
    if unavailable {
        "Glass: Unavailable"
    } else if !glass {
        "Glass: Off"
    } else if matches!(platform, Platform::MacOs | Platform::Linux) {
        // GPUI accepts a blur request without confirming the result.
        "Glass: Requested"
    } else {
        "Glass: On"
    }
}

#[cfg(target_os = "windows")]
fn launcher_shape(window: &Window) -> (i32, i32, i32) {
    let viewport = window.viewport_size();
    let scale = window.scale_factor();
    (
        (f32::from(viewport.width) * scale).round() as i32,
        (f32::from(viewport.height) * scale).round() as i32,
        (visual::CORNER_RADIUS * scale).round().clamp(1.0, 256.0) as i32,
    )
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(not(windows))]
        {
            self.displayed_catalog_generation = self.catalog_generation;
        }
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
        let palette = Palette::new(self.light);
        let theme = palette.theme;
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
            .map(|setup| setup.view(self.hotkey_registered()));
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
        #[cfg(windows)]
        let hotkey_notice = self.hotkey_notice.as_deref();
        #[cfg(not(windows))]
        let native_notice = self.native_host.hotkey_notice();
        #[cfg(not(windows))]
        let hotkey_notice = native_notice.as_deref();
        let status_text = match hotkey_notice {
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
        let back = self.detail.is_some() || self.setup.is_some();
        let show_back = back || self.panel.is_some() || is_calculator || is_emoji;
        let input_focused = self.input.focus_handle(cx).is_focused(window);
        let show_hotkey = !show_back
            && self.engine.query().is_empty()
            && self.hotkey_registered()
            && window.viewport_size().width >= px(640.);
        let search_back = icon_control("search-back", Icon::Back, palette.subtle, palette)
            .on_click(cx.listener(move |this, _, window, cx| {
                if (is_calculator || is_emoji) && this.panel.is_none() && !back {
                    if !this.is_composing(cx) {
                        this.input.update(cx, |input, cx| input.clear(window, cx));
                        compat::focus(window, &this.input.focus_handle(cx), cx);
                    }
                } else {
                    window.dispatch_action(Box::new(Escape), cx);
                }
            }));
        #[cfg(feature = "upstream-candidate")]
        let search_back = search_back.role(Role::Button).aria_label("Back");
        let primary = if back {
            None
        } else if let Some(panel) = &self.panel {
            panel
                .is_current(&self.engine)
                .then(|| panel.selected())
                .flatten()
                .map(|entry| entry.title.clone())
        } else if is_emoji {
            (emoji_count > 0).then(|| "Copy Emoji".into())
        } else if is_calculator {
            calculation.as_ref().map(|_| "Copy Answer".into())
        } else {
            self.engine.selected().map(|command| match command.kind {
                CommandKind::Builtin(BuiltinCommand::RefreshApplications) => "Run".into(),
                _ => "Open".into(),
            })
        };
        let primary_button = primary.map(|label| {
            let button = control("launcher-primary", palette)
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.panel.is_some() {
                        this.execute_panel(None, window, cx);
                    } else {
                        this.execute_primary(window, cx);
                    }
                }))
                .child(icon(Icon::Right, 14., palette.subtle))
                .child(label.clone())
                .child(keys(&["↵"], palette));
            #[cfg(feature = "upstream-candidate")]
            let button = button
                .role(Role::Button)
                .aria_label(label)
                .aria_keyshortcuts("Enter");
            button
        });
        let actions_button = control("actions-button", palette)
            .track_focus(&self.actions_focus)
            .border_1()
            .border_color(if self.actions_focus.is_focused(window) {
                theme.accent
            } else {
                gpui::transparent_black()
            })
            .when(self.panel.is_some(), |el| el.bg(theme.surface_raised))
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
            .child(if back { "Back" } else { "Actions" })
            .child(keys(if back { &["Esc"] } else { &["Ctrl", "K"] }, palette));
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
        let glass_caption = glass_label(
            Platform::current(),
            self.glass,
            self.appearance_notice.is_some(),
        );
        let appearance_button = icon_control(
            "toggle-glass",
            Icon::Glass,
            if self.glass {
                theme.accent
            } else {
                palette.subtle
            },
            palette,
        )
        .when(self.appearance_notice.is_some(), |el| el.opacity(0.45))
        .tooltip(move |_, cx| cx.new(|_| Caption(glass_caption, theme)).into())
        .on_click(cx.listener(|this, _, window, cx| this.toggle_glass(window, cx)));
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
        let theme_caption = if self.light {
            "Switch to dark theme"
        } else {
            "Switch to light theme"
        };
        let theme_button = icon_control("toggle-theme", Icon::Theme, palette.subtle, palette)
            .tooltip(move |_, cx| cx.new(|_| Caption(theme_caption, theme)).into())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)));
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
            .rounded(px(visual::CORNER_RADIUS))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .text_color(theme.text)
            .font_family(visual::SANS)
            .text_size(px(14.))
            .line_height(px(21.))
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
                this.on_escape(window, cx);
            }))
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .child(
                div()
                    .h(px(58.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(if input_focused { theme.accent.opacity(0.45) } else { palette.line })
                    .when(show_back, |el| el.child(search_back))
                    .when(!show_back, |el| el.child(icon(Icon::Search, 18., palette.subtle)))
                    .child(div().flex_1().min_w_0().when(is_calculator, |el| el.font_family(visual::MONO)).child(self.input.clone()))
                    .when(show_hotkey, |el| el.child(keys(&["Ctrl", "Space"], palette))),
            )
            .when(self.setup.is_some() || self.detail.is_some() || is_emoji || is_calculator, |el| el.child(
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
            ))
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
                            .child(
                                div()
                                    .font_family(visual::SERIF)
                                    .text_size(px(22.))
                                    .child(detail.lines().next().unwrap_or("Command details").to_owned()),
                            )
                            .child(detail.lines().skip(1).collect::<Vec<_>>().join("\n"));
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
                            .font_family(visual::MONO)
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
                        |el| el.child(empty_state(Icon::Empty, "No results found", "Try a different search or type an expression to calculate.", palette)),
                    )
                    .when(
                        self.detail.is_none()
                            && self.setup.is_none()
                            && !is_emoji
                            && !is_calculator
                            && count > 0,
                        |el| el.child(self.render_results(palette, cx)),
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
                                    let theme = Palette::new(this.light).theme;
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
                    .when_some(self.panel.as_ref(), |el, panel| el.child(self.actions_panel(panel, palette, window, cx))),
            )
            .child(
                div()
                    .h(px(42.))
                    .flex_none()
                    .px_3()
                    .bg(if self.glass { palette.footer.opacity(0.70) } else { palette.footer })
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child(icon(Icon::Ansa, 16., palette.subtle))
                    .gap_2()
                    .child(status_node)
                    .child(div().flex().items_center().gap_1().child(theme_button).child(appearance_button).children(primary_button).child(actions_button)),
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
    #[cfg(windows)]
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
    #[cfg(target_os = "macos")]
    let reopen_handle = {
        let handle = std::rc::Rc::new(std::cell::Cell::new(None));
        let for_reopen = handle.clone();
        application.on_reopen(move |cx| {
            if let Some(handle) = for_reopen.get() {
                native_host::open_from_desktop(cx, handle);
            }
        });
        handle
    };
    application.with_assets(visual::Icons).run(move |cx: &mut App| {
        if let Err(error) = visual::register_fonts(cx) {
            eprintln!("Could not load Geist fonts: {error}");
        }
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
        let bounds = Bounds::centered(None, size(px(780.), px(552.)), cx);
        #[cfg(target_os = "linux")]
        let appearance = InitialAppearance::for_linux(glass, linux_wayland());
        #[cfg(not(target_os = "linux"))]
        let appearance = InitialAppearance::new(Platform::current(), glass, macos_reduce_transparency());
        let Ok(handle) = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(600.), px(360.))),
            titlebar: None,
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui::WindowDecorations::Client),
            // On X11 the pinned GPUI treats PopUp as a notification window.
            // Use a regular frameless desktop window on macOS/Linux so it
            // can be selected again even if global registration fails.
            kind: if cfg!(windows) { WindowKind::PopUp } else { WindowKind::Normal },
            is_resizable: false,
            is_minimizable: false,
            // Windows needs its custom composition layer; macOS uses GPUI's
            // native vibrancy. Both disable GPUI's legacy accent.
            window_background: appearance.background,
            app_id: Some("dev.ansa.launcher".into()),
            ..Default::default()
        }, |window, cx| {
            cx.new(|cx| {
                Launcher::new(
                    window,
                    cx,
                    a11y_fixture,
                    appearance,
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
            #[cfg(windows)]
            {
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
            #[cfg(not(windows))]
            if !fixture_run {
                native_host::install_summon(cx, handle);
            }

            // Native smoke is a window-creation check. Opening storage would
            // leave nothing usable behind and can migrate a developer's real
            // profile, so it only runs when ANSA_DATA_DIR is explicit.
            if !interaction_smoke && (!native_smoke || explicit_data_dir) {
                let _ = handle.update(cx, |this, _, cx| this.refresh_catalog(cx));
            }
        }
        #[cfg(target_os = "macos")]
        reopen_handle.set(Some(handle));
        #[cfg(not(windows))]
        let _ = handle.update(cx, |_, window, _| window.set_window_title("Ansa"));
        #[cfg(windows)]
        cx.activate(true);
        #[cfg(target_os = "macos")]
        if fixture_run {
            cx.activate(true);
        }
        #[cfg(not(windows))]
        if !fixture_run {
            native_host::open_from_desktop(cx, handle);
        }
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
            });
        }
        // GPUI's synchronous X11 MapWindow check can leave mapping events
        // queued without waking calloop. Activation wakes delivery; fixtures
        // otherwise skip normal summon setup.
        #[cfg(target_os = "linux")]
        if (startup_smoke || interaction_smoke) && gpui::guess_compositor() == "X11" {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
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

#[cfg(test)]
mod platform_appearance_tests {
    use super::*;

    #[test]
    pub(crate) fn linux_x11_keeps_an_opaque_fallback() {
        for requested in [false, true] {
            let appearance = InitialAppearance::for_linux(requested, false);
            assert_eq!(appearance.background, WindowBackgroundAppearance::Opaque);
            assert_eq!(
                glass_label(
                    Platform::Linux,
                    appearance.background == WindowBackgroundAppearance::Blurred,
                    appearance.unavailable.is_some(),
                ),
                "Glass: Unavailable"
            );
        }
    }

    #[test]
    pub(crate) fn linux_wayland_requests_compositor_blur_and_can_turn_it_off() {
        let on = InitialAppearance::for_linux(true, true);
        assert_eq!(on.background, WindowBackgroundAppearance::Blurred);
        assert!(on.requested);
        assert_eq!(on.unavailable, None);
        assert_eq!(
            glass_label(Platform::Linux, true, false),
            "Glass: Requested"
        );

        let off = InitialAppearance::for_linux(false, true);
        assert_eq!(off.background, WindowBackgroundAppearance::Transparent);
        assert!(!off.requested);
        assert_eq!(off.unavailable, None);
        assert_eq!(glass_label(Platform::Linux, false, false), "Glass: Off");
    }

    #[test]
    pub(crate) fn macos_reports_requested_vibrancy_and_honors_reduce_transparency() {
        let requested = InitialAppearance::new(Platform::MacOs, true, false);
        assert_eq!(requested.background, WindowBackgroundAppearance::Blurred);
        assert_eq!(requested.unavailable, None);
        assert_eq!(
            glass_label(
                Platform::MacOs,
                requested.background == WindowBackgroundAppearance::Blurred,
                requested.unavailable.is_some(),
            ),
            "Glass: Requested"
        );
        let fallback = InitialAppearance::new(Platform::MacOs, true, true);
        assert_eq!(fallback.background, WindowBackgroundAppearance::Opaque);
        assert!(fallback.unavailable.is_some());
        assert_eq!(
            glass_label(
                Platform::MacOs,
                fallback.background == WindowBackgroundAppearance::Blurred,
                fallback.unavailable.is_some(),
            ),
            "Glass: Unavailable"
        );
        let disabled = InitialAppearance::new(Platform::MacOs, false, false);
        assert_ne!(disabled.background, WindowBackgroundAppearance::Blurred);
        assert_eq!(glass_label(Platform::MacOs, false, false), "Glass: Off");
        // A user who chose Off is not Unavailable, even while the OS Reduce
        // Transparency setting is enabled.
        let disabled = InitialAppearance::new(Platform::MacOs, false, true);
        assert_eq!(disabled.background, WindowBackgroundAppearance::Transparent);
        assert!(disabled.unavailable.is_none());
        assert_eq!(glass_label(Platform::MacOs, false, false), "Glass: Off");
    }

    #[test]
    pub(crate) fn macos_reduce_transparency_re_evaluation_preserves_the_requested_glass() {
        // Requested -> OS forces an opaque fallback -> OS setting cleared.
        // The saved request, not the effective background, drives the restore.
        let request = MacGlassRequest::new(true);
        let unavailable = request.evaluate(true);
        assert_eq!(unavailable.background, WindowBackgroundAppearance::Opaque);
        assert!(unavailable.requested);
        assert!(unavailable.unavailable.is_some());
        assert_eq!(
            glass_label(Platform::MacOs, false, true),
            "Glass: Unavailable"
        );

        let restored = request.evaluate(false);
        assert_eq!(restored.background, WindowBackgroundAppearance::Blurred);
        assert!(restored.requested);
        assert!(restored.unavailable.is_none());
        assert_eq!(
            glass_label(Platform::MacOs, true, false),
            "Glass: Requested"
        );

        // The same re-evaluation after an explicit Off must stay off.
        let off = request.toggled();
        assert_eq!(
            off.evaluate(true).background,
            WindowBackgroundAppearance::Transparent
        );
        assert!(off.evaluate(true).unavailable.is_none());
        let still_off = off.evaluate(false);
        assert_eq!(
            still_off.background,
            WindowBackgroundAppearance::Transparent
        );
        assert!(still_off.unavailable.is_none());
        assert_eq!(glass_label(Platform::MacOs, false, false), "Glass: Off");
    }

    #[test]
    pub(crate) fn unsupported_platforms_are_explicitly_opaque() {
        let appearance = InitialAppearance::new(Platform::Other, true, false);
        assert_eq!(appearance.background, WindowBackgroundAppearance::Opaque);
        assert!(appearance.unavailable.is_some());
        assert_eq!(
            glass_label(Platform::Other, false, appearance.unavailable.is_some()),
            "Glass: Unavailable"
        );
    }

    #[test]
    pub(crate) fn windows_starts_opaque_until_native_backdrop_attaches() {
        let startup = InitialAppearance::new(Platform::Windows, true, false);
        assert_eq!(startup.background, WindowBackgroundAppearance::Opaque);
        assert!(startup.unavailable.is_none());
    }
}

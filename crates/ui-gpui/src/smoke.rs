//! Debug-only synthetic widget/action routing checks, not physical key or IME tests.
use super::*;
use ansa_onboarding::Step;
use gpui::EntityInputHandler;

fn replace(input: &Entity<TextInput>, text: &str, window: &mut Window, cx: &mut App) {
    input.update(cx, |input, cx| {
        let length = input.text().encode_utf16().count();
        input.replace_text_in_range(Some(0..length), text, window, cx);
    });
}

fn key(source: &'static str, window: &mut Window, cx: &mut App) {
    window.dispatch_keystroke(
        gpui::Keystroke::parse(source).expect("fixture keystroke"),
        cx,
    );
}

pub(super) fn start(view: Entity<Launcher>, window: &mut Window) {
    window.on_next_frame(move |window, cx| step(view, 0, None, window, cx));
}

fn step(
    view: Entity<Launcher>,
    stage: u8,
    mut appearance_bounds: Option<Bounds<gpui::Pixels>>,
    window: &mut Window,
    cx: &mut App,
) {
    if (26..=29).contains(&stage) {
        assert_eq!(
            Some(window.bounds()),
            appearance_bounds,
            "appearance toggles must not move or resize the launcher"
        );
    }
    match stage {
        0 => {
            let families = cx.text_system().all_font_names();
            for expected in [visual::SANS, visual::MONO, visual::SERIF] {
                assert!(
                    families.iter().any(|family| family == expected),
                    "bundled font family {expected} was not registered"
                );
            }
            key("ctrl-k", window, cx);
        }
        1 => {
            let this = view.read(cx);
            assert!(this.panel.is_some(), "Ctrl+K action must open the panel");
            assert!(this.action_input.focus_handle(cx).is_focused(window));
            let input = this.action_input.clone();
            replace(&input, "pin", window, cx);
        }
        2 => {
            let this = view.read(cx);
            let panel = this
                .panel
                .as_ref()
                .expect("panel must stay open while filtering");
            assert_eq!(panel.visible_entries().len(), 1);
            assert_eq!(panel.selected().unwrap().id, ActionId::ToggleFavorite);
            assert_eq!(
                this.engine.query(),
                "",
                "action query must not change root query"
            );
            key("down", window, cx);
            key("enter", window, cx);
        }
        3 => {
            let this = view.read(cx);
            assert!(this.panel.is_none());
            assert!(
                this.engine
                    .signals()
                    .is_favorite(&this.engine.selected().unwrap().id)
            );
            assert!(this.input.focus_handle(cx).is_focused(window));
            let input = this.input.clone();
            replace(&input, "1/0", window, cx);
        }
        4 => {
            assert!(matches!(
                route_query(view.read(cx).engine.query()),
                QueryRoute::Calculator(Err(_))
            ));
            window.dispatch_action(Box::new(Execute), cx);
        }
        5 => {
            let this = view.read(cx);
            assert!(this.status.contains("division by zero"));
            assert!(
                this.detail.is_none(),
                "invalid arithmetic must not execute a command"
            );
            let input = this.input.clone();
            replace(&input, "", window, cx);
        }
        6 => window.dispatch_action(Box::new(Actions), cx),
        7 => {
            let input = view.read(cx).action_input.clone();
            replace(&input, "zz-no-such-action", window, cx);
        }
        8 => {
            assert!(view.read(cx).panel.as_ref().unwrap().selected().is_none());
            window.dispatch_action(Box::new(Execute), cx);
        }
        9 => {
            assert!(
                view.read(cx).panel.is_some(),
                "Enter with no actions must be a no-op"
            );
            key("escape", window, cx);
        }
        10 => {
            let this = view.read(cx);
            assert!(this.panel.is_none());
            assert!(this.input.focus_handle(cx).is_focused(window));
            assert_eq!(this.action_input.read(cx).text(), "");
            key("tab", window, cx);
        }
        11 => {
            assert!(
                view.read(cx).actions_focus.is_focused(window),
                "Tab must reach the footer control"
            );
            key("enter", window, cx);
        }
        12 => {
            assert!(
                view.read(cx).panel.is_some(),
                "Enter on footer must reopen actions"
            );
            key("tab", window, cx);
        }
        13 => {
            assert!(view.read(cx).panel.is_none());
            assert!(view.read(cx).actions_focus.is_focused(window));
            key("shift-tab", window, cx);
        }
        14 => {
            assert!(view.read(cx).input.focus_handle(cx).is_focused(window));
            window.dispatch_action(Box::new(Actions), cx);
        }
        15 => {
            assert!(view.read(cx).panel.is_some());
            view.update(cx, |this, cx| {
                this.engine.replace_catalog(vec![]);
                cx.notify();
            });
        }
        16 => {
            let this = view.read(cx);
            assert!(
                this.panel.is_some(),
                "invalidated menu must not silently expose root Enter"
            );
            assert!(
                !this
                    .panel
                    .as_ref()
                    .unwrap()
                    .target()
                    .is_current(&this.engine)
            );
            window.dispatch_action(Box::new(Execute), cx);
        }
        17 => {
            let this = view.read(cx);
            assert!(this.panel.is_none());
            assert!(this.detail.is_none());
            assert!(this.status.contains("target changed"));
            let input = this.input.clone();
            replace(&input, &"😀".repeat(4097), window, cx);
        }
        18 => {
            let this = view.read(cx);
            assert!(this.status.starts_with("Input not changed:"));
            assert_eq!(this.input.read(cx).text(), "");
            assert_eq!(this.engine.query(), "");
            let input = this.input.clone();
            replace(&input, "undo-check", window, cx);
        }
        19 => {
            assert_eq!(view.read(cx).engine.query(), "undo-check");
            window.dispatch_keystroke(
                gpui::Keystroke::parse(if cfg!(target_os = "macos") {
                    "cmd-z"
                } else {
                    "ctrl-z"
                })
                .unwrap(),
                cx,
            );
        }
        20 => {
            assert_eq!(view.read(cx).engine.query(), "");
            window.dispatch_keystroke(
                gpui::Keystroke::parse(if cfg!(target_os = "macos") {
                    "cmd-shift-z"
                } else {
                    "ctrl-y"
                })
                .unwrap(),
                cx,
            );
        }
        21 => {
            let this = view.read(cx);
            assert_eq!(this.engine.query(), "undo-check");
            let input = this.input.clone();
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(
                    Some(0..input.text().encode_utf16().count()),
                    "漢",
                    Some(1..1),
                    window,
                    cx,
                );
            });
        }
        22 => {
            assert!(view.read(cx).input.read(cx).is_composing());
            window.dispatch_keystroke(
                gpui::Keystroke::parse(if cfg!(target_os = "macos") {
                    "cmd-z"
                } else {
                    "ctrl-z"
                })
                .unwrap(),
                cx,
            );
        }
        23 => {
            let input = view.read(cx).input.clone();
            assert_eq!(input.read(cx).text(), "漢");
            assert!(
                input.read(cx).is_composing(),
                "undo must not interrupt marked text"
            );
            input.update(cx, |input, cx| input.unmark_text(window, cx));
        }
        24 => {
            window.dispatch_keystroke(
                gpui::Keystroke::parse(if cfg!(target_os = "macos") {
                    "cmd-z"
                } else {
                    "ctrl-z"
                })
                .unwrap(),
                cx,
            );
        }
        25 => {
            let this = view.read(cx);
            assert_eq!(this.engine.query(), "undo-check");
            assert!(!this.input.read(cx).is_composing());
            appearance_bounds = Some(window.bounds());
            key("ctrl-shift-g", window, cx);
        }
        26 => {
            let expected = std::env::args().any(|arg| arg == "--opaque");
            let this = view.read(cx);
            assert_eq!(this.glass, expected && this.appearance_notice.is_none());
            assert_eq!(view.read(cx).engine.query(), "undo-check");
            assert!(view.read(cx).input.focus_handle(cx).is_focused(window));
            key("ctrl-shift-g", window, cx);
        }
        27 => {
            let this = view.read(cx);
            assert_eq!(
                this.glass,
                !std::env::args().any(|arg| arg == "--opaque") && this.appearance_notice.is_none()
            );
            #[cfg(target_os = "windows")]
            if let Some(reason) = &this.appearance_notice {
                assert!(this.backdrop.is_none());
                assert!(
                    !std::env::args().any(|arg| arg == "--require-frost"),
                    "Native frosted backdrop unavailable: {reason}"
                );
                println!("Ansa appearance smoke: opaque fallback ({reason})");
            } else {
                assert!(
                    this.backdrop.is_some(),
                    "both toggles must retain the native backdrop"
                );
                println!("Ansa appearance smoke: native backdrop attached and toggled");
            }
            #[cfg(target_os = "linux")]
            {
                assert!(!this.glass);
                assert_eq!(
                    crate::glass_label(
                        ansa_platform_api::Platform::Linux,
                        this.glass,
                        this.appearance_notice.is_some(),
                    ),
                    "Glass: Unavailable"
                );
                assert!(
                    !std::env::args().any(|arg| arg == "--require-frost"),
                    "No native Linux frosted backdrop is implemented"
                );
                println!("Ansa appearance smoke: opaque fallback; native blur unimplemented");
            }
            #[cfg(target_os = "macos")]
            println!(
                "Ansa appearance smoke: GPUI request or opaque fallback checked; actual vibrancy not verified"
            );
            key("ctrl-shift-t", window, cx);
        }
        28 => {
            assert_eq!(
                view.read(cx).light,
                !std::env::args().any(|arg| arg == "--light")
            );
            assert_eq!(view.read(cx).engine.query(), "undo-check");
            assert!(view.read(cx).input.focus_handle(cx).is_focused(window));
            key("ctrl-shift-t", window, cx);
        }
        29 => {
            assert_eq!(
                view.read(cx).light,
                std::env::args().any(|arg| arg == "--light")
            );
            let input = view.read(cx).input.clone();
            replace(&input, "emoji smile", window, cx);
        }
        30 => {
            let this = view.read(cx);
            assert_eq!(this.emoji_query.as_deref(), Some("smile"));
            assert!(!this.emoji_results.is_empty());
            assert!(this.emoji_results.len() <= daily::EMOJI_LIMIT);
            assert_eq!(this.emoji_selected, 0);
            assert!(this.emoji_error.is_none());
            key("down", window, cx);
        }
        31 => {
            let this = view.read(cx);
            let expected = 1.min(this.emoji_results.len().saturating_sub(1));
            assert_eq!(this.emoji_selected, expected);
            assert_eq!(this.engine.query(), "emoji smile");
            let input = this.input.clone();
            replace(&input, "", window, cx);
        }
        32 => {
            let this = view.read(cx);
            assert!(this.emoji_query.is_none());
            assert!(this.emoji_results.is_empty());
            assert!(
                daily::builtin_commands()
                    .iter()
                    .any(|command| command.id.as_str() == daily::SETUP_COMMAND_ID),
                "Start Setup must be available without opening storage"
            );
            view.update(cx, |this, cx| {
                this.engine.replace_catalog(daily::builtin_commands());
                let index = this
                    .engine
                    .catalog()
                    .iter()
                    .position(|command| command.id.as_str() == daily::SETUP_COMMAND_ID)
                    .expect("Start Setup in fixture catalog");
                this.engine.select_index(index);
                cx.notify();
            });
            window.dispatch_action(Box::new(Execute), cx);
        }
        33 => {
            let this = view.read(cx);
            let setup = this.setup.as_ref().expect("setup fixture must open");
            assert_eq!(setup.onboarding().step(), Step::Welcome);
            assert_eq!(setup.focused(), Some(daily::SetupControl::Continue));
            assert!(this.setup_focus.is_focused(window));
            key("space", window, cx);
        }
        34 => {
            let this = view.read(cx);
            let setup = this.setup.as_ref().expect("setup must stay open");
            assert_eq!(setup.onboarding().step(), Step::Shortcut);
            assert_eq!(setup.focused(), Some(daily::SetupControl::Continue));
            key("tab", window, cx);
            key("tab", window, cx);
            let setup = view.read(cx).setup.as_ref().expect("setup");
            assert_eq!(setup.focused(), Some(daily::SetupControl::Skip));
            key("enter", window, cx);
        }
        35 => {
            let this = view.read(cx);
            let setup = this.setup.as_ref().expect("setup");
            assert_eq!(setup.onboarding().step(), Step::TryLaunch);
            assert!(
                setup
                    .onboarding()
                    .warning_for(Step::Shortcut)
                    .is_some_and(|warning| warning.contains("Shortcut setup was skipped"))
            );
            assert_eq!(setup.focused(), Some(daily::SetupControl::Continue));
            key("tab", window, cx);
            let setup = view.read(cx).setup.as_ref().expect("setup");
            assert_eq!(setup.focused(), Some(daily::SetupControl::TestLaunch));
            key("tab", window, cx);
            let setup = view.read(cx).setup.as_ref().expect("setup");
            assert_eq!(setup.focused(), Some(daily::SetupControl::Skip));
            key("space", window, cx);
        }
        36 => {
            let this = view.read(cx);
            let setup = this.setup.as_ref().expect("setup");
            assert_eq!(setup.onboarding().step(), Step::Integrations);
            assert!(
                setup
                    .onboarding()
                    .warning_for(Step::TryLaunch)
                    .is_some_and(|warning| warning.contains("No successful application launch"))
            );
            assert_eq!(setup.focused(), Some(daily::SetupControl::Continue));
            key("enter", window, cx);
        }
        37 => {
            let this = view.read(cx);
            let setup = this.setup.as_ref().expect("setup");
            assert_eq!(setup.onboarding().step(), Step::Done);
            assert!(!setup.onboarding().status().is_completed());
            assert_eq!(setup.focused(), Some(daily::SetupControl::Finish));
            key("enter", window, cx);
        }
        38 => {
            let this = view.read(cx);
            let setup = this.setup.as_ref().expect("setup");
            assert!(setup.onboarding().status().is_completed());
            assert_eq!(setup.focused(), Some(daily::SetupControl::Reset));
            key("enter", window, cx);
            let setup = view.read(cx).setup.as_ref().expect("setup");
            assert_eq!(setup.onboarding().step(), Step::Welcome);
        }
        39 => {
            assert!(view.read(cx).setup.is_some());
            key("escape", window, cx);
            let this = view.read(cx);
            assert!(this.setup.is_none());
            assert!(this.input.focus_handle(cx).is_focused(window));
            println!(
                "Ansa interaction smoke: 40 synthetic widget/action stages passed, including glass, light/dark theme toggling, bounded emoji navigation and keyboard-only setup traversal. No clipboard writes or external app launches."
            );
            cx.quit();
            return;
        }
        _ => unreachable!(),
    }
    window.on_next_frame(move |window, cx| step(view, stage + 1, appearance_bounds, window, cx));
}

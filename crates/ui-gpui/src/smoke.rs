//! Debug-only synthetic widget/action routing checks, not physical key or IME tests.
use super::*;
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
    window.on_next_frame(move |window, cx| step(view, 0, window, cx));
}

fn step(view: Entity<Launcher>, stage: u8, window: &mut Window, cx: &mut App) {
    match stage {
        0 => key("ctrl-k", window, cx),
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
            key("ctrl-shift-g", window, cx);
        }
        26 => {
            let expected = std::env::args().any(|arg| arg == "--opaque");
            assert_eq!(view.read(cx).glass, expected);
            assert_eq!(view.read(cx).engine.query(), "undo-check");
            assert!(view.read(cx).input.focus_handle(cx).is_focused(window));
            key("ctrl-shift-g", window, cx);
        }
        27 => {
            assert_eq!(
                view.read(cx).glass,
                !std::env::args().any(|arg| arg == "--opaque")
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
            println!(
                "Ansa interaction smoke: 30 synthetic widget/action stages passed, including glass and light/dark theme toggling. No clipboard writes or external app launches."
            );
            cx.quit();
            return;
        }
        _ => unreachable!(),
    }
    window.on_next_frame(move |window, cx| step(view, stage + 1, window, cx));
}

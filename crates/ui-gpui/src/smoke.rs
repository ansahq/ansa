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
            assert!(!this.panel.as_ref().unwrap().is_current(&this.engine));
            window.dispatch_action(Box::new(Execute), cx);
        }
        17 => {
            let this = view.read(cx);
            assert!(this.panel.is_none());
            assert!(this.detail.is_none());
            assert!(
                this.status.contains("invocation expired"),
                "stale action refs must report a typed status, saw {:?}",
                this.status
            );
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
            // Stage 15 emptied the catalog to invalidate the old panel; restore
            // the built-ins and start the navigation block from a known query.
            view.update(cx, |this, cx| {
                this.engine.replace_catalog(builtin_commands());
                cx.notify();
            });
            let input = view.read(cx).input.clone();
            replace(&input, "About", window, cx);
        }
        30 => {
            let this = view.read(cx);
            assert_eq!(this.engine.query(), "About");
            assert_eq!(this.engine.selected().unwrap().title, "About Ansa");
            window.dispatch_action(Box::new(Execute), cx);
        }
        31 => {
            let this = view.read(cx);
            assert!(this.detail.is_some(), "About must open a child view");
            assert_eq!(this.detail.as_ref().unwrap().id.as_str(), "ansa:about");
            assert_eq!(this.engine.view_depth(), 1);
            assert_eq!(this.engine.current_view().as_str(), "ansa:about");
            let probe = this.engine.query_token();
            assert!(probe.invocation() > 0);
            // Marked text owns Escape; the child view must survive.
            let input = this.input.clone();
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(
                    Some(0..input.text().encode_utf16().count()),
                    "か",
                    Some(1..1),
                    window,
                    cx,
                );
            });
            assert!(
                view.read(cx).input.read(cx).is_composing(),
                "fixture must have an active composition"
            );
            assert!(
                view.read(cx).detail.is_some(),
                "marking composition must not pop the child view"
            );
            window.dispatch_action(Box::new(Escape), cx);
        }
        32 => {
            let this = view.read(cx);
            assert!(
                !this.input.read(cx).is_composing(),
                "Escape must cancel the composition first"
            );
            assert!(
                this.detail.is_some(),
                "cancelling composition must not pop the child view"
            );
            assert_eq!(this.engine.view_depth(), 1);
            window.dispatch_action(Box::new(Escape), cx);
        }
        33 => {
            let this = view.read(cx);
            assert!(this.detail.is_none(), "second Escape pops the child view");
            assert_eq!(this.engine.view_depth(), 0);
            assert_eq!(this.engine.current_view().as_str(), "root");
            assert_eq!(this.engine.query(), "About");
            assert_eq!(this.input.read(cx).text(), "About");
            assert_eq!(this.engine.selected().unwrap().title, "About Ansa");
            assert!(this.input.focus_handle(cx).is_focused(window));

            // A delayed callback from a closed frame must not execute.
            let probe = ansa_core::navigation::ViewId::new("ansa:probe").expect("static probe id");
            view.update(cx, |this, cx| {
                this.engine
                    .push_view(probe.clone(), ScrollAnchor::at(1))
                    .unwrap();
                let delayed = ActionRegistry::issue_panel(
                    &this.engine,
                    this.engine.views(),
                    ActionId::Primary,
                )
                .unwrap();
                let restored = this.engine.pop_view().unwrap();
                assert_eq!(restored.view(), &probe);
                assert_eq!(restored.scroll(), ScrollAnchor::at(1));
                assert!(
                    this.engine.pop_view().is_none(),
                    "a frame can only be restored once"
                );
                assert_eq!(
                    ActionRegistry::resolve(&delayed, &this.engine, this.engine.views()),
                    Err(ActionError::CrossView),
                    "the delayed callback belongs to another view"
                );
                let fresh = ActionRegistry::issue_panel(
                    &this.engine,
                    this.engine.views(),
                    ActionId::Primary,
                )
                .unwrap();
                assert!(ActionRegistry::resolve(&fresh, &this.engine, this.engine.views()).is_ok());
                cx.notify();
            });
            window.dispatch_action(Box::new(Actions), cx);
        }
        34 => {
            let this = view.read(cx);
            assert!(this.panel.is_some(), "actions must reopen after restore");
            assert_eq!(
                this.panel.as_ref().unwrap().view_generation(),
                Some(this.engine.view_generation())
            );
            assert_eq!(
                this.panel_refs.len(),
                this.panel.as_ref().unwrap().entries().len()
            );
            let stale = this.panel_refs[0].clone();
            view.update(cx, |this, cx| {
                this.engine.replace_catalog(builtin_commands());
                assert!(
                    ActionRegistry::resolve(&stale, &this.engine, this.engine.views()).is_err(),
                    "the panel ref captured before the catalog swap is stale"
                );
                let fresh = ActionRegistry::issue_panel(
                    &this.engine,
                    this.engine.views(),
                    ActionId::Primary,
                )
                .unwrap();
                assert!(ActionRegistry::resolve(&fresh, &this.engine, this.engine.views()).is_ok());
                cx.notify();
            });
            window.dispatch_action(Box::new(Execute), cx);
        }
        35 => {
            let this = view.read(cx);
            assert!(
                this.panel.is_none(),
                "a stale action ref must close instead of executing"
            );
            assert!(
                this.status.contains("reopen actions"),
                "stale refs must show a typed status, saw {:?}",
                this.status
            );
            assert!(this.input.focus_handle(cx).is_focused(window));
            window.dispatch_action(Box::new(Actions), cx);
        }
        36 => {
            let this = view.read(cx);
            assert!(this.panel.is_some());
            assert!(this.action_input.focus_handle(cx).is_focused(window));
            window.dispatch_action(Box::new(Escape), cx);
        }
        37 => {
            let this = view.read(cx);
            assert!(this.panel.is_none(), "Escape must close the panel");
            assert!(this.input.focus_handle(cx).is_focused(window));
            assert!(this.detail.is_none());
            assert_eq!(this.engine.view_depth(), 0);
            key("tab", window, cx);
        }
        38 => {
            assert!(
                view.read(cx).actions_focus.is_focused(window),
                "Tab must reach the footer control"
            );
            key("tab", window, cx);
        }
        39 => {
            assert!(
                view.read(cx).input.focus_handle(cx).is_focused(window),
                "Tab must return to the query field"
            );
            key("shift-tab", window, cx);
        }
        40 => {
            assert!(
                view.read(cx).actions_focus.is_focused(window),
                "Shift+Tab must move backwards"
            );
            key("shift-tab", window, cx);
        }
        41 => {
            let this = view.read(cx);
            assert!(
                this.input.focus_handle(cx).is_focused(window),
                "Shift+Tab must return to the query field"
            );
            let input = this.input.clone();
            replace(&input, "About", window, cx);
        }
        42 => {
            let this = view.read(cx);
            assert_eq!(this.engine.query(), "About");
            assert_eq!(this.engine.selected().unwrap().title, "About Ansa");
            window.dispatch_action(Box::new(Actions), cx);
        }
        43 => {
            let this = view.read(cx);
            assert!(this.panel.is_some());
            assert!(this.action_input.focus_handle(cx).is_focused(window));
            key("enter", window, cx);
        }
        44 => {
            let this = view.read(cx);
            assert!(this.panel.is_none());
            assert!(
                this.detail.is_some(),
                "the panel's primary action must still run"
            );
            assert_eq!(this.detail.as_ref().unwrap().id.as_str(), "ansa:about");
            assert_eq!(this.engine.view_depth(), 1);
            assert!(this.input.focus_handle(cx).is_focused(window));
            key("escape", window, cx);
        }
        45 => {
            let this = view.read(cx);
            assert!(this.detail.is_none());
            assert_eq!(this.engine.view_depth(), 0);
            assert_eq!(this.engine.query(), "About");
            assert_eq!(this.input.read(cx).text(), "About");
            assert_eq!(this.engine.selected().unwrap().title, "About Ansa");
            assert!(this.input.focus_handle(cx).is_focused(window));
            println!(
                "Ansa interaction smoke: 46 synthetic widget/action stages passed, including typed view restore, Escape ordering, stale action refs, Tab/Shift+Tab cycles, glass and theme toggling. No clipboard writes or external app launches."
            );
            cx.quit();
            return;
        }
        _ => unreachable!(),
    }
    window.on_next_frame(move |window, cx| step(view, stage + 1, window, cx));
}

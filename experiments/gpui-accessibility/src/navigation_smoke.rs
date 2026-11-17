use super::*;
use gpui::EntityInputHandler;

pub(super) fn start(view: Entity<Candidate>, window: &mut Window) {
    window.on_next_frame(move |window, cx| step(view, 0, window, cx));
}

fn key(source: &'static str, window: &mut Window, cx: &mut App) {
    window.dispatch_keystroke(gpui::Keystroke::parse(source).expect("fixture key"), cx);
}

fn selected(view: &Entity<Candidate>, index: usize, cx: &App) {
    let this = view.read(cx);
    assert_eq!(this.engine.selected_index(), Some(index));
    assert!(
        this.realized_range.contains(&index),
        "selection must be realized, not only changed in the model"
    );
}

fn step(view: Entity<Candidate>, stage: u8, window: &mut Window, cx: &mut App) {
    match stage {
        0 => key("down", window, cx),
        1 => {
            selected(&view, 1, cx);
            key("pagedown", window, cx);
        }
        2 => {
            selected(&view, 9, cx);
            key("alt-end", window, cx);
        }
        3 => {
            selected(&view, 9999, cx);
            key("alt-home", window, cx);
        }
        4 => {
            selected(&view, 0, cx);
            assert_eq!(view.read(cx).input.read(cx).text(), "");
            let input = view.read(cx).input.clone();
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "Project", Some(7..7), window, cx)
            });
        }
        5 => {
            assert!(view.read(cx).input.read(cx).is_composing());
            key("down", window, cx);
        }
        6 => {
            selected(&view, 0, cx);
            let input = view.read(cx).input.clone();
            input.update(cx, |input, cx| input.unmark_text(window, cx));
            key("down", window, cx);
        }
        7 => {
            selected(&view, 1, cx);
            let this = view.read(cx);
            assert_eq!(this.input.read(cx).text(), "Project");
            assert_eq!(this.engine.query(), "Project");
            assert!(this.input.focus_handle(cx).is_focused(window));
            println!(
                "Candidate navigation smoke: 8 stages passed, including 10k-list end/start realization and IME-owned arrow deferral."
            );
            cx.quit();
            return;
        }
        _ => unreachable!(),
    }
    // Allow the virtualized list to finish its pending layout/scroll update.
    window.on_next_frame(move |window, _| {
        window.on_next_frame(move |window, cx| step(view, stage + 1, window, cx));
    });
}

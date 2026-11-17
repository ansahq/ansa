//! Structural accessibility candidate, deliberately outside the shipping workspace.
//! Native compilation/startup does not establish screen-reader acceptance.
use ansa_core::Engine;
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};
use ansa_ui_components::{Changed, EditRejected, TextInput, install_keybindings};
use gpui::{
    App, Bounds, Context, Entity, Focusable, KeyBinding, Role, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, WindowBounds, WindowOptions, actions, div,
    prelude::*, px, rgb, size, uniform_list,
};

mod live_status;
#[cfg(debug_assertions)]
mod navigation_smoke;
actions!(
    candidate,
    [
        NextResult,
        PreviousResult,
        NextPage,
        PreviousPage,
        FirstResult,
        LastResult
    ]
);

struct Candidate {
    input: Entity<TextInput>,
    engine: Engine,
    scroll: UniformListScrollHandle,
    notice: Option<String>,
    #[cfg(debug_assertions)]
    realized_range: std::ops::Range<usize>,
    _subscription: Subscription,
    _rejection_subscription: Subscription,
}

impl Candidate {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        input.update(cx, |input, cx| {
            input.set_accessibility_label("Search applications and commands", cx)
        });
        window.focus(&input.focus_handle(cx), cx);
        let subscription = cx.subscribe(&input, |this, _, event: &Changed, cx| {
            this.engine.set_query(&event.0);
            this.notice = None;
            this.scroll.scroll_to_item(
                this.engine.selected_index().unwrap_or(0),
                ScrollStrategy::Center,
            );
            cx.notify();
        });
        let rejection_subscription = cx.subscribe(&input, |this, _, event: &EditRejected, cx| {
            this.notice = Some(format!("Input not changed: {}", event.message()));
            cx.notify();
        });
        let commands = (0..10_000)
            .map(|index| {
                Command::new(
                    CommandId::new(format!("fixture:command-{index:05}")).unwrap(),
                    format!("Project {index:05} Terminal"),
                    CommandKind::Builtin(BuiltinCommand::About),
                )
            })
            .collect();
        Self {
            input,
            engine: Engine::new(commands),
            scroll: UniformListScrollHandle::new(),
            notice: None,
            #[cfg(debug_assertions)]
            realized_range: 0..0,
            _subscription: subscription,
            _rejection_subscription: rejection_subscription,
        }
    }

    fn navigate(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.read(cx).is_composing() {
            return;
        }
        self.engine.move_selection(delta);
        self.notice = None;
        if let Some(index) = self.engine.selected_index() {
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    fn select_result(
        &mut self,
        id: &CommandId,
        token: ansa_core::QueryToken,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.engine.query_token() != token || self.input.read(cx).is_composing() {
            return;
        }
        if let Some(index) = self
            .engine
            .results()
            .iter()
            .position(|command| &command.id == id)
        {
            self.engine.select_index(index);
            self.notice = None;
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
            window.focus(&self.input.focus_handle(cx), cx);
            cx.notify();
        }
    }
}

impl Render for Candidate {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.engine.results().len();
        div()
            .key_context("Candidate")
            .on_action(cx.listener(|this, _: &NextResult, window, cx| this.navigate(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousResult, window, cx| this.navigate(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextPage, window, cx| this.navigate(8, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousPage, window, cx| this.navigate(-8, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FirstResult, window, cx| {
                    this.navigate(i32::MIN, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &LastResult, window, cx| this.navigate(i32::MAX, window, cx)),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x17171d))
            .text_color(rgb(0xeeedf4))
            .child(div().p_4().child(self.input.clone()))
            .child(
                div()
                    .id("candidate-results")
                    .role(Role::List)
                    .aria_label("Search results")
                    .aria_size_of_set(count)
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "candidate-list",
                            count,
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                #[cfg(debug_assertions)]
                                {
                                    this.realized_range = range.clone();
                                }
                                range
                                    .map(|index| {
                                        let command = &this.engine.results()[index];
                                        let id = command.id.clone();
                                        let scroll_id = id.clone();
                                        let token = this.engine.query_token();
                                        let view = cx.weak_entity();
                                        let selected = this.engine.selected_index() == Some(index);
                                        div()
                                            .id(SharedString::from(command.id.as_str().to_owned()))
                                            .role(Role::ListItem)
                                            .aria_label(command.title.clone())
                                            .aria_selected(selected)
                                            .when(selected, |element| element.bg(rgb(0x34303f)))
                                            // AccessKit stores zero-based positions; its Windows
                                            // adapter adds one and reads size from the container.
                                            .aria_position_in_set(index)
                                            // UIA SelectionItem.Select dispatches Click too:
                                            // selection must never launch the item as a side effect.
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.select_result(&id, token, window, cx)
                                            }))
                                            .on_a11y_action(
                                                gpui::AccessibleAction::ScrollIntoView,
                                                move |_, window, cx| {
                                                    if let Err(error) =
                                                        view.update(cx, |this, cx| {
                                                            if this.engine.query_token() == token
                                                                && let Some(index) = this
                                                                    .engine
                                                                    .results()
                                                                    .iter()
                                                                    .position(|command| {
                                                                        command.id == scroll_id
                                                                    })
                                                            {
                                                                this.scroll.scroll_to_item(
                                                                    index,
                                                                    ScrollStrategy::Center,
                                                                );
                                                                cx.notify();
                                                            }
                                                            window.focus(
                                                                &this.input.focus_handle(cx),
                                                                cx,
                                                            );
                                                        })
                                                    {
                                                        eprintln!(
                                                            "Candidate list unavailable: {error}"
                                                        );
                                                    }
                                                },
                                            )
                                            .h(px(44.))
                                            .px_4()
                                            .child(command.title.clone())
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.scroll)
                        .size_full(),
                    ),
            )
            .child(
                div().flex().px_4().gap_4().children(
                    [
                        ("first", "First result", i32::MIN),
                        ("previous", "Previous result", -1),
                        ("next", "Next result", 1),
                        ("last", "Last result", i32::MAX),
                    ]
                    .into_iter()
                    .map(|(id, label, delta)| {
                        div()
                            .id(id)
                            .role(Role::Button)
                            .aria_label(label)
                            .px_2()
                            .py_2()
                            .cursor_pointer()
                            .child(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.navigate(delta, window, cx)
                            }))
                    }),
                ),
            )
            .child(live_status::LiveStatus::new(
                self.notice
                    .clone()
                    .unwrap_or_else(|| match self.engine.selected() {
                        Some(command) => format!(
                            "{}, result {} of {count}",
                            command.title,
                            self.engine.selected_index().unwrap_or(0) + 1
                        ),
                        None => "No results".into(),
                    }),
            ))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        install_keybindings(cx);
        cx.bind_keys([
            KeyBinding::new("down", NextResult, Some("Candidate")), KeyBinding::new("up", PreviousResult, Some("Candidate")),
            KeyBinding::new("pagedown", NextPage, Some("Candidate")), KeyBinding::new("pageup", PreviousPage, Some("Candidate")),
            KeyBinding::new("alt-home", FirstResult, Some("Candidate")), KeyBinding::new("alt-end", LastResult, Some("Candidate")),
        ]);
        cx.on_window_closed(|cx, _| { if cx.windows().is_empty() { cx.quit(); } }).detach();
        let bounds = Bounds::centered(None, size(px(760.), px(500.)), cx);
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default()
        }, |window, cx| cx.new(|cx| Candidate::new(window, cx))).expect("candidate window");
        cx.activate(true);
        #[cfg(debug_assertions)]
        if std::env::args().any(|arg| arg == "--interaction-smoke-test") {
            window.update(cx, |_, window, cx| navigation_smoke::start(cx.entity(), window)).expect("navigation smoke setup");
        }
        if std::env::args().any(|arg| arg == "--uia-probe") {
            window.update(cx, |_, window, _| window.on_next_frame(|window, _| {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                match HasWindowHandle::window_handle(window) {
                    Ok(handle) => match handle.as_raw() {
                        RawWindowHandle::Win32(handle) => println!("ANSA_UIA_READY:{}", handle.hwnd.get()),
                        _ => eprintln!("UIA probe requires Windows"),
                    },
                    Err(error) => eprintln!("Candidate native handle unavailable: {error}"),
                }
            })).expect("candidate probe setup");
        }
        if std::env::args().any(|arg| arg == "--smoke-test") {
            window.update(cx, |_, window, _| window.on_next_frame(|window, _| {
                window.on_next_frame(|_, cx| {
                    println!("Candidate native frames completed; no accessibility tree or screen-reader acceptance was asserted.");
                    cx.quit();
                });
            })).unwrap();
        }
    });
}

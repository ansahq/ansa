use super::*;
use gpui::{Div, FontWeight, Hsla, SharedString, Stateful, div, uniform_list};
use visual::{
    Icon, MONO, Palette, ROW_HEIGHT, SANS, ScrollThumb, command_tile, highlight, icon, keys,
    section, tile,
};

fn command_icon(kind: &CommandKind) -> Icon {
    match kind {
        CommandKind::Application { .. } => Icon::App,
        CommandKind::Quicklink { .. } => Icon::Link,
        CommandKind::Builtin(BuiltinCommand::Settings) => Icon::Settings,
        CommandKind::Builtin(BuiltinCommand::RefreshApplications) => Icon::Refresh,
        CommandKind::Builtin(BuiltinCommand::About) => Icon::Info,
    }
}

fn command_kind(kind: &CommandKind) -> &'static str {
    match kind {
        CommandKind::Builtin(_) => "Command",
        _ => kind.label(),
    }
}

pub(super) fn control(id: &'static str, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .gap(px(6.))
        .h(px(28.))
        .px_2()
        .rounded(px(6.))
        .cursor_pointer()
        .text_color(palette.theme.muted_text)
        .hover(|style| style.bg(palette.hover).text_color(palette.theme.text))
        .active(|style| style.bg(palette.theme.surface_raised))
}

pub(super) fn icon_control(
    id: &'static str,
    glyph: Icon,
    color: Hsla,
    palette: Palette,
) -> Stateful<Div> {
    control(id, palette)
        .size(px(28.))
        .px_0()
        .child(icon(glyph, 16., color))
}

pub(super) fn empty_state(
    glyph: Icon,
    title: &str,
    description: impl Into<SharedString>,
    palette: Palette,
) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .px_8()
        .text_center()
        .child(div().mb_3().child(tile(glyph, 40., palette)))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .child(title.to_owned()),
        )
        .child(
            div()
                .mt_1()
                .max_w(px(300.))
                .text_size(px(12.5))
                .line_height(px(19.))
                .text_color(palette.theme.muted_text)
                .child(description.into()),
        )
}

impl Launcher {
    /// Empty-query ranking already puts all pinned commands first.
    fn favorite_count(&self) -> usize {
        if !self.engine.query().is_empty() {
            return 0;
        }
        self.engine
            .results()
            .partition_point(|command| self.engine.signals().is_favorite(&command.id))
    }

    pub(super) fn results_slot(&self, index: usize) -> usize {
        let favorites = self.favorite_count();
        index
            + usize::from(
                favorites > 0 && favorites < self.engine.results().len() && index >= favorites,
            )
    }

    pub(super) fn render_results(&self, palette: Palette, cx: &Context<Self>) -> impl IntoElement {
        let count = self.engine.results().len();
        let favorites = self.favorite_count();
        let show_divider = favorites > 0 && favorites < count;
        let has_query = !self.engine.query().is_empty();
        let heading = if has_query {
            "Search Results"
        } else if favorites > 0 {
            "Favorites"
        } else {
            "Applications & Commands"
        };
        let list = uniform_list(
            "results",
            count + usize::from(show_divider),
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                range
                    .map(|slot| {
                        if show_divider && slot == favorites {
                            return div()
                                .h(px(ROW_HEIGHT))
                                .flex()
                                .items_end()
                                .child(section("Applications & Commands", palette))
                                .into_any_element();
                        }
                        let index = slot - usize::from(show_divider && slot > favorites);
                        this.result_row(index, palette, window, cx)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .with_decoration(ScrollThumb(palette.scrollbar))
        .size_full();
        #[cfg(feature = "upstream-candidate")]
        let list = list.track_scroll(&self.scroll);
        #[cfg(not(feature = "upstream-candidate"))]
        let list = list.track_scroll(self.scroll.clone());

        let results = div().id("results-list").size_full().flex().flex_col().p_2();
        #[cfg(feature = "upstream-candidate")]
        let results = results
            .role(Role::List)
            .aria_label("Search results")
            .aria_size_of_set(count);
        results
            .child(section(heading, palette).when(has_query, |el| {
                el.child(format!(
                    "{count} {}",
                    if count == 1 { "result" } else { "results" }
                ))
            }))
            .child(div().flex_1().min_h_0().child(list))
    }

    fn result_row(
        &self,
        index: usize,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = palette.theme;
        let command = &self.engine.results()[index];
        let selected = self.engine.selected_index() == Some(index);
        let command_id = command.id.clone();
        let hover_id = command_id.clone();
        let token = self.engine.query_token();
        let pinned = self.engine.signals().is_favorite(&command.id);
        let artwork = command_tile(
            command_icon(&command.kind),
            self.icon_sources.get(&command.id).map(String::as_str),
            &self.data_dir.join("icons"),
            28.,
            palette,
            window,
            cx,
        );
        let title = command.title.clone();
        let subtitle = command.subtitle.clone().unwrap_or_default();
        #[cfg(feature = "upstream-candidate")]
        let label = if subtitle.is_empty() {
            title.clone()
        } else {
            format!("{title}. {subtitle}")
        };
        let row = div()
            .id(SharedString::from(format!("result-body:{command_id}")))
            .size_full()
            .flex()
            .items_center()
            .line_height(px(20.))
            .gap_3()
            .px(px(10.))
            .rounded(px(8.))
            .when(selected, |el| el.bg(theme.surface_raised))
            .hover(|el| el.bg(palette.hover))
            .cursor_pointer()
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.panel.is_none()
                    && !this.is_composing(cx)
                    && this.engine.query_token() == token
                    && this.engine.selected_index() != Some(index)
                    && this
                        .engine
                        .results()
                        .get(index)
                        .is_some_and(|row| row.id == hover_id)
                    && this.engine.select_index(index)
                {
                    #[cfg(feature = "upstream-candidate")]
                    {
                        this.selection_announcement = None;
                    }
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _event, window, cx| {
                if this.panel.is_some()
                    || this.is_composing(cx)
                    || this.engine.query_token() != token
                {
                    return;
                }
                if let Some(current) = this
                    .engine
                    .results()
                    .iter()
                    .position(|command| command.id == command_id)
                {
                    this.engine.select_index(current);
                    #[cfg(feature = "upstream-candidate")]
                    if matches!(_event, gpui::ClickEvent::Keyboard(_)) {
                        // UIA SelectionItem.Select is delivered as a keyboard click.
                        this.announce_selection();
                        compat::focus(window, &this.input.focus_handle(cx), cx);
                        cx.notify();
                        return;
                    }
                    this.execute_primary(window, cx);
                }
            }))
            .child(artwork)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.))
                            .child(highlight(title, self.engine.query(), palette.accent_text)),
                    )
                    .when(!subtitle.is_empty(), |el| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(palette.subtle)
                                .child(subtitle),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(palette.subtle)
                    .when(pinned, |el| el.child(icon(Icon::Pin, 14., theme.accent)))
                    .when_some(command.alias.clone(), |el, alias| {
                        el.child(
                            div()
                                .h(px(20.))
                                .px(px(6.))
                                .rounded(px(5.))
                                .bg(palette.key)
                                .font_family(MONO)
                                .text_size(px(11.))
                                .line_height(px(20.))
                                .child(alias),
                        )
                    })
                    .child(
                        div()
                            .w(px(80.))
                            .text_right()
                            .child(command_kind(&command.kind)),
                    ),
            );
        #[cfg(feature = "upstream-candidate")]
        let row = row
            .role(Role::ListItem)
            .aria_label(label)
            .aria_selected(selected)
            .aria_position_in_set(index);
        div()
            .id(SharedString::from(format!("result-row:{}", command.id)))
            .h(px(ROW_HEIGHT))
            .child(row)
    }

    pub(super) fn panel_target<'a>(
        &'a self,
        panel: &'a ActionPanel,
    ) -> (Icon, &'a str, Option<&'a str>) {
        match panel.target() {
            PanelTarget::Calculator { expression, .. } => {
                (Icon::Calculator, expression.as_str(), None)
            }
            PanelTarget::Command { id, .. } => self
                .engine
                .catalog()
                .iter()
                .find(|command| &command.id == id)
                .map_or((Icon::Warning, "Command unavailable", None), |command| {
                    (
                        command_icon(&command.kind),
                        command.title.as_str(),
                        self.icon_sources.get(&command.id).map(String::as_str),
                    )
                }),
        }
    }

    pub(super) fn actions_panel(
        &self,
        panel: &ActionPanel,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = palette.theme;
        let token = panel.target().token();
        let target_changed = !panel.is_current(&self.engine);
        let (target_icon, target_title, app_path) = self.panel_target(panel);
        let artwork = command_tile(
            target_icon,
            app_path,
            &self.data_dir.join("icons"),
            20.,
            palette,
            window,
            cx,
        );
        let menu = div()
            .id(SharedString::from(format!(
                "actions-menu:{}:{}",
                token.invocation(),
                token.revision()
            )))
            .absolute()
            .right_2()
            .bottom_2()
            .w((window.viewport_size().width - px(18.)).min(px(340.)))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .shadow_2xl()
            .occlude();
        #[cfg(feature = "upstream-candidate")]
        let menu = menu
            .role(Role::Menu)
            .aria_label("Actions")
            .aria_size_of_set(panel.visible_entries().len());

        menu.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(10.))
                .border_b_1()
                .border_color(palette.line)
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .child(artwork)
                .child(div().min_w_0().truncate().child(target_title.to_owned())),
        )
        .when(target_changed, |el| {
            el.child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child("Target changed · Esc to close"),
            )
        })
        .child(
            div()
                .p(px(6.))
                .children(
                    panel
                        .visible_entries()
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| {
                            let action = entry.id;
                            let target = panel.target().clone();
                            let selected = panel.selected_index() == Some(index);
                            let glyph = match action {
                                ActionId::Primary => Icon::Open,
                                ActionId::CopyTarget => Icon::Copy,
                                ActionId::ToggleFavorite => Icon::Pin,
                            };
                            let row = div()
                                .id(SharedString::from(format!(
                                    "action:{}:{}:{}",
                                    action_slug(action),
                                    token.invocation(),
                                    token.revision()
                                )))
                                .h(px(32.))
                                .px_2()
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .text_size(px(13.))
                                .line_height(px(19.5))
                                .when(selected, |el| el.bg(theme.surface_raised))
                                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                    if this.is_composing(cx) {
                                        return;
                                    }
                                    if let Some(panel) = this.panel.as_mut()
                                        && panel.target().token() == token
                                        && panel.selected_id() != Some(action)
                                        && panel
                                            .visible_entries()
                                            .get(index)
                                            .is_some_and(|entry| entry.id == action)
                                    {
                                        panel.select_index(index);
                                        cx.notify();
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this
                                        .panel
                                        .as_ref()
                                        .is_some_and(|panel| panel.target() == &target)
                                    {
                                        this.execute_panel(Some(action), window, cx);
                                    }
                                }))
                                .child(icon(glyph, 16., theme.muted_text))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(entry.title.clone()),
                                )
                                .when_some(entry.shortcut.as_deref(), |el, shortcut| {
                                    let labels: Vec<_> = if shortcut == "Enter" {
                                        vec!["↵"]
                                    } else {
                                        shortcut.split('+').collect()
                                    };
                                    el.child(keys(&labels, palette))
                                });
                            #[cfg(feature = "upstream-candidate")]
                            let row = row
                                .role(Role::MenuItem)
                                .aria_label(entry.title.clone())
                                .aria_position_in_set(index);
                            row
                        }),
                )
                .when(panel.visible_entries().is_empty(), |el| {
                    el.child(
                        div()
                            .px_2()
                            .py_2()
                            .text_size(px(13.))
                            .text_color(theme.muted_text)
                            .child("No matching actions"),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(palette.line)
                .child(icon(Icon::Search, 14., palette.subtle))
                .child(div().flex_1().min_w_0().child(self.action_input.clone())),
        )
    }
}

pub(super) struct Caption(pub &'static str, pub Theme);

impl Render for Caption {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .bg(self.1.surface)
            .border_1()
            .border_color(self.1.border)
            .font_family(SANS)
            .text_size(px(12.))
            .text_color(self.1.text)
            .child(self.0)
    }
}

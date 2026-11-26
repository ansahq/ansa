use std::{
    borrow::Cow,
    ops::Range,
    path::{Path, PathBuf},
};

use ansa_ui_components::Theme;
use gpui::{
    AnyElement, App, Asset, AssetSource, Bounds, Div, FontWeight, HighlightStyle, Hsla, Pixels,
    Point, SharedString, StyledImage, StyledText, UniformListDecoration, Window, div, img,
    prelude::*, px, rgb, svg,
};

pub(crate) const CORNER_RADIUS: f32 = 16.;
pub(crate) const ROW_HEIGHT: f32 = 40.;
pub(crate) const SANS: &str = "Geist";
pub(crate) const SERIF: &str = "Geist Serif";
pub(crate) const MONO: &str = "Geist Mono";

pub(crate) fn register_fonts(cx: &App) -> gpui::Result<()> {
    let fonts: [&[u8]; 6] = [
        include_bytes!("../assets/fonts/Geist-Regular.ttf"),
        include_bytes!("../assets/fonts/Geist-Medium.ttf"),
        include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
        include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
        include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
        include_bytes!("../assets/fonts/GeistSerif-Regular.ttf"),
    ];
    cx.text_system()
        .add_fonts(fonts.into_iter().map(Cow::Borrowed).collect())
}

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) theme: Theme,
    pub(crate) subtle: Hsla,
    pub(crate) line: Hsla,
    pub(crate) hover: Hsla,
    pub(crate) footer: Hsla,
    pub(crate) key: Hsla,
    pub(crate) accent_text: Hsla,
    pub(crate) scrollbar: Hsla,
}

impl Palette {
    pub(crate) fn new(light: bool) -> Self {
        // Both IBM Carbon variants come from the supplied Oxocarbon theme.
        // Use the brighter/darker Carbon blue where an accent is small text.
        let (theme, subtle, hover, accent_text, scrollbar) = if light {
            (Theme::light(), 0x525252, 0xe8e8e8, 0x0043ce, 0xc6c6c6)
        } else {
            (Theme::dark(), 0xa8a8a8, 0x353535, 0x78a9ff, 0x393939)
        };
        Self {
            theme,
            subtle: rgb(subtle).into(),
            line: theme.border,
            hover: rgb(hover).into(),
            footer: theme.surface,
            key: theme.surface_raised,
            accent_text: rgb(accent_text).into(),
            scrollbar: rgb(scrollbar).into(),
        }
    }

    pub(crate) fn input_theme(self) -> Theme {
        Theme {
            muted_text: self.subtle,
            ..self.theme
        }
    }
}

macro_rules! icons {
    ($($name:ident => $path:literal),+ $(,)?) => {
        #[derive(Clone, Copy)]
        pub(crate) enum Icon { $($name),+ }

        impl Icon {
            fn path(self) -> &'static str {
                match self { $(Self::$name => $path),+ }
            }
        }

        pub(crate) struct Icons;

        impl AssetSource for Icons {
            fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
                let bytes: &[u8] = match path {
                    $($path => include_bytes!(concat!("../assets/icons/", $path, ".svg")),)+
                    _ => return Ok(None),
                };
                Ok(Some(Cow::Borrowed(bytes)))
            }

            fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
                Ok([$($path),+].into_iter().filter(|name| name.starts_with(path)).map(Into::into).collect())
            }
        }
    };
}

icons! {
    Ansa => "ansa",
    Search => "search",
    Back => "back",
    Right => "right",
    App => "app",
    Settings => "settings",
    Refresh => "refresh",
    Info => "info",
    Link => "link",
    Calculator => "calculator",
    Copy => "copy",
    Pin => "pin",
    Open => "open",
    Empty => "empty",
    Glass => "glass",
    Theme => "theme",
    Warning => "warning",
}

pub(crate) fn icon(glyph: Icon, size: f32, color: Hsla) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(svg().path(glyph.path()).size_full().text_color(color))
}

pub(crate) fn tile(glyph: Icon, size: f32, palette: Palette) -> Div {
    div()
        .flex()
        .flex_none()
        .size(px(size))
        .items_center()
        .justify_center()
        .rounded(px((size * 0.26).round()))
        .bg(palette.theme.surface)
        .border_1()
        .border_color(palette.theme.border)
        .child(icon(
            glyph,
            ((size * 0.28).round() * 2.).min(20.),
            palette.theme.muted_text,
        ))
}

struct AppIcon;

impl Asset for AppIcon {
    type Source = (String, PathBuf);
    type Output = Option<PathBuf>;

    fn load(
        (source, cache_dir): Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        cx.background_executor().spawn(async move {
            #[cfg(windows)]
            {
                ansa_platform_api::extract_icon_to_ico(&source, &cache_dir)
                    .ok()
                    .map(|icon| PathBuf::from(icon.path))
            }
            #[cfg(not(windows))]
            {
                let _ = cache_dir;
                ansa_platform_api::resolve_application_icon(&source).ok()
            }
        })
    }
}

pub(crate) fn command_tile(
    glyph: Icon,
    icon_source: Option<&str>,
    cache_dir: &Path,
    size: f32,
    palette: Palette,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    if let Some(source) = icon_source
        && let Some(Some(path)) =
            window.use_asset::<AppIcon>(&(source.to_owned(), cache_dir.to_path_buf()), cx)
    {
        return div()
            .size(px(size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(
                img(path)
                    .size(px((size * 0.8).round()))
                    .with_fallback(move || {
                        icon(glyph, 16., palette.theme.muted_text).into_any_element()
                    }),
            );
    }
    tile(glyph, size, palette)
}

pub(crate) fn keys(labels: &[&str], palette: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .flex_none()
        .children(labels.iter().map(|label| {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(20.))
                .min_w(px(20.))
                .px_1()
                .rounded(px(5.))
                .border_1()
                .border_color(palette.theme.border)
                .bg(palette.key)
                .font_family(MONO)
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(11.))
                .line_height(px(13.))
                .text_color(palette.theme.muted_text)
                .child(label.to_string())
        }))
}

pub(crate) fn section(label: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .flex_none()
        .h(px(28.))
        .px(px(10.))
        .text_size(px(12.))
        .line_height(px(18.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.subtle)
        .child(label.into())
}

/// Display title matches using UTF-8 byte ranges for GPUI's text runs.
pub(crate) fn highlight(title: String, query: &str, accent: Hsla) -> StyledText {
    let query: Vec<_> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return StyledText::new(title);
    }
    let chars: Vec<_> = title
        .char_indices()
        .flat_map(|(start, ch)| {
            let end = start + ch.len_utf8();
            ch.to_lowercase().map(move |lower| (lower, start..end))
        })
        .collect();
    let matches = |positions: &[(_, Range<usize>)]| {
        positions
            .iter()
            .map(|(_, range)| range.clone())
            .collect::<Vec<_>>()
    };
    let mut ranges = if let Some(run) = chars
        .windows(query.len())
        .find(|run| run.iter().map(|(ch, _)| *ch).eq(query.iter().copied()))
    {
        matches(run)
    } else {
        let mut remaining = chars.iter();
        query
            .into_iter()
            .map(|wanted| {
                remaining
                    .find(|(ch, _)| *ch == wanted)
                    .map(|(_, range)| range.clone())
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
    };
    ranges.dedup();
    StyledText::new(title).with_highlights(ranges.into_iter().map(|range| {
        (
            range,
            HighlightStyle {
                color: Some(accent),
                ..Default::default()
            },
        )
    }))
}

pub(crate) struct ScrollThumb(pub(crate) Hsla);

impl UniformListDecoration for ScrollThumb {
    fn compute(
        &self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        offset: Point<Pixels>,
        row_height: Pixels,
        count: usize,
        _: &mut Window,
        _: &mut App,
    ) -> AnyElement {
        let viewport = bounds.size.height;
        let content = row_height * count as f32;
        if content <= viewport || viewport <= px(0.) {
            return div().into_any_element();
        }
        let thumb = (viewport * (viewport / content)).max(px(20.)).min(viewport);
        let progress = (-offset.y / (content - viewport)).clamp(0., 1.);
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top(-offset.y + (viewport - thumb) * progress)
                    .w(px(5.))
                    .h(thumb)
                    .rounded(px(2.5))
                    .bg(self.0),
            )
            .into_any_element()
    }
}

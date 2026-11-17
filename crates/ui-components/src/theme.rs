//! Neutral light and dark palettes shared by the launcher and its inputs.

use gpui::{Hsla, rgb, rgba};

/// Colors used by Ansa widgets.
///
/// All values are opaque except [`Theme::selection`], which is a translucent
/// accent wash painted behind selected text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// Window background behind raised surfaces.
    pub background: Hsla,
    /// Raised panels, menus and result rows.
    pub surface: Hsla,
    /// Slightly stronger raised surface for hover and selected rows.
    pub surface_raised: Hsla,
    /// Primary text.
    pub text: Hsla,
    /// Secondary labels, hints and placeholders.
    pub muted_text: Hsla,
    /// Accent used for focus, the caret and active affordances.
    pub accent: Hsla,
    /// Stronger accent for filled affordances.
    pub accent_strong: Hsla,
    /// Translucent accent wash painted behind a text selection.
    pub selection: Hsla,
    /// Hairline borders and separators.
    pub border: Hsla,
    /// Destructive actions and error states.
    pub danger: Hsla,
}

impl Theme {
    /// Only the background is translucent; text, menus and selected rows
    /// retain their own opaque palette colors.
    pub fn glass_background(&self) -> Hsla {
        self.background.opacity(0.78)
    }

    /// The default Ansa dark theme.
    pub fn dark() -> Self {
        Self {
            background: rgb(0x181818).into(),
            surface: rgb(0x232323).into(),
            surface_raised: rgb(0x383838).into(),
            text: rgb(0xf5f5f5).into(),
            muted_text: rgb(0xbcbcbc).into(),
            accent: rgb(0xf0f0f0).into(),
            accent_strong: rgb(0xffffff).into(),
            selection: rgba(0xffffff30).into(),
            border: rgb(0x484848).into(),
            danger: rgb(0xf5f5f5).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            background: rgb(0xf5f5f5).into(),
            surface: rgb(0xffffff).into(),
            surface_raised: rgb(0xdcdcdc).into(),
            text: rgb(0x181818).into(),
            muted_text: rgb(0x4c4c4c).into(),
            accent: rgb(0x202020).into(),
            accent_strong: rgb(0x000000).into(),
            selection: rgba(0x00000025).into(),
            border: rgb(0xbcbcbc).into(),
            danger: rgb(0x181818).into(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(gray: f32) -> f32 {
        if gray <= 0.04045 {
            gray / 12.92
        } else {
            ((gray + 0.055) / 1.055).powf(2.4)
        }
    }

    fn contrast(first: f32, second: f32) -> f32 {
        let first = luminance(first);
        let second = luminance(second);
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    #[test]
    fn glass_keeps_text_and_caret_contrast_over_extreme_backdrops() {
        for theme in [Theme::dark(), Theme::light()] {
            let glass = theme.glass_background();
            assert!((glass.a - 0.78).abs() < 0.0001);
            for backdrop in [0.0, 1.0] {
                // Neutral sRGB source-over compositing, before luminance conversion.
                let background = glass.l * glass.a + backdrop * (1.0 - glass.a);
                for text in [theme.text, theme.muted_text, theme.accent] {
                    assert_eq!(text.a, 1.0);
                    assert!(contrast(text.l, background) >= 4.5);
                }
                let selection = theme.selection;
                let selected_background =
                    selection.l * selection.a + background * (1.0 - selection.a);
                assert!(contrast(theme.text.l, selected_background) >= 4.5);
            }
        }
    }

    #[test]
    fn both_palettes_are_entirely_neutral() {
        for theme in [Theme::dark(), Theme::light()] {
            for color in [
                theme.background,
                theme.surface,
                theme.surface_raised,
                theme.text,
                theme.muted_text,
                theme.accent,
                theme.accent_strong,
                theme.selection,
                theme.border,
                theme.danger,
            ] {
                assert!(color.s.abs() < 0.0001);
            }
        }
    }

    #[test]
    fn text_contrasts_with_each_opaque_surface() {
        for theme in [Theme::dark(), Theme::light()] {
            for text in [theme.text, theme.muted_text] {
                for surface in [theme.background, theme.surface, theme.surface_raised] {
                    let ratio = contrast(text.l, surface.l);
                    assert!(ratio >= 4.5, "text contrast {ratio}");
                }
            }
        }
    }
}

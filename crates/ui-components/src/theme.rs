//! Oxocarbon IBM Carbon palettes shared by the launcher and its inputs.

use gpui::{Background, Hsla, rgb, rgba, solid_background};

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
    pub fn glass_tint(&self) -> Hsla {
        self.background.opacity(0.84)
    }

    /// Paint the base tint over the compositor backdrop when glass is enabled.
    pub fn window_background(&self, glass: bool) -> Background {
        solid_background(if glass {
            self.glass_tint()
        } else {
            self.background
        })
    }

    /// The default Ansa dark theme.
    pub fn dark() -> Self {
        Self {
            background: rgb(0x161616).into(),
            surface: rgb(0x262626).into(),
            surface_raised: rgb(0x393939).into(),
            text: rgb(0xf4f4f4).into(),
            muted_text: rgb(0xc6c6c6).into(),
            accent: rgb(0x4589ff).into(),
            accent_strong: rgb(0x0f62fe).into(),
            selection: rgba(0x4589ff40).into(),
            border: rgb(0x393939).into(),
            danger: rgb(0xff8389).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            background: rgb(0xffffff).into(),
            surface: rgb(0xf4f4f4).into(),
            surface_raised: rgb(0xe0e0e0).into(),
            text: rgb(0x161616).into(),
            muted_text: rgb(0x525252).into(),
            accent: rgb(0x0f62fe).into(),
            accent_strong: rgb(0x0f62fe).into(),
            selection: rgba(0x0f62fe40).into(),
            border: rgb(0xe0e0e0).into(),
            danger: rgb(0xda1e28).into(),
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

    fn channels(color: Hsla) -> [f32; 3] {
        let color: gpui::Rgba = color.into();
        [color.r, color.g, color.b]
    }

    fn composite(color: Hsla, background: [f32; 3]) -> [f32; 3] {
        let foreground = channels(color);
        std::array::from_fn(|index| {
            foreground[index] * color.a + background[index] * (1.0 - color.a)
        })
    }

    fn luminance(color: [f32; 3]) -> f32 {
        color
            .into_iter()
            .zip([0.2126, 0.7152, 0.0722])
            .map(|(channel, weight)| {
                let linear = if channel <= 0.04045 {
                    channel / 12.92
                } else {
                    ((channel + 0.055) / 1.055).powf(2.4)
                };
                linear * weight
            })
            .sum()
    }

    fn contrast(first: [f32; 3], second: [f32; 3]) -> f32 {
        let first = luminance(first);
        let second = luminance(second);
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    #[test]
    fn glass_keeps_text_and_caret_contrast_over_extreme_backdrops() {
        for theme in [Theme::dark(), Theme::light()] {
            let glass = theme.glass_tint();
            assert!((glass.a - 0.84).abs() < 0.0001);
            for backdrop in [0.0, 1.0] {
                let background = composite(glass, [backdrop; 3]);
                for text in [theme.text, theme.muted_text] {
                    assert_eq!(text.a, 1.0);
                    assert!(contrast(channels(text), background) >= 4.5);
                }
                assert!(contrast(channels(theme.accent), background) >= 3.0);
                let selected_background = composite(theme.selection, background);
                assert!(contrast(channels(theme.text), selected_background) >= 4.5);
            }
        }
    }

    #[test]
    fn uses_the_oxocarbon_ibm_carbon_backgrounds_and_accents() {
        assert_eq!(Theme::dark().background, rgb(0x161616).into());
        assert_eq!(Theme::dark().accent, rgb(0x4589ff).into());
        assert_eq!(Theme::light().background, rgb(0xffffff).into());
        assert_eq!(Theme::light().accent, rgb(0x0f62fe).into());
    }

    #[test]
    fn text_contrasts_with_each_opaque_surface() {
        for theme in [Theme::dark(), Theme::light()] {
            for text in [theme.text, theme.muted_text] {
                for surface in [theme.background, theme.surface, theme.surface_raised] {
                    let ratio = contrast(channels(text), channels(surface));
                    assert!(ratio >= 4.5, "text contrast {ratio}");
                }
            }
        }
    }
}

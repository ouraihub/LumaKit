//! Accent presets a host lays over any theme with [`LumaTheme::with_accent`]: LumaDesk's seven,
//! one color per appearance. The light color carries white text and the dark color reads on a
//! dark page, both at WCAG AA (pinned by tests).
//!
//! [`LumaTheme::with_accent`]: super::LumaTheme::with_accent

use gpui_kit::Rgba;

use super::Appearance;

/// One accent preset: its stable id and its color in a light and a dark theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccentPreset {
    pub id: &'static str,
    pub light: Rgba,
    pub dark: Rgba,
}

impl AccentPreset {
    /// The preset's color for a theme of this appearance.
    pub fn color(&self, appearance: Appearance) -> Rgba {
        match appearance {
            Appearance::Light => self.light,
            Appearance::Dark => self.dark,
        }
    }
}

const fn hex(value: u32) -> Rgba {
    Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.,
        g: ((value >> 8) & 0xff) as f32 / 255.,
        b: (value & 0xff) as f32 / 255.,
        a: 1.,
    }
}

const fn preset(id: &'static str, light: u32, dark: u32) -> AccentPreset {
    AccentPreset { id, light: hex(light), dark: hex(dark) }
}

/// Vermilion, blue, green, yellow, cyan, purple, gray: LumaDesk's original accent pairs.
pub const ACCENT_PRESETS: [AccentPreset; 7] = [
    preset("red", 0xae3224, 0xff9c89),
    preset("blue", 0x215ba5, 0x8bbeff),
    preset("green", 0x256c43, 0x87d6a2),
    preset("yellow", 0x7f5b00, 0xefcb65),
    preset("cyan", 0x006977, 0x73d5e2),
    preset("purple", 0x71409f, 0xcba6f4),
    preset("gray", 0x575b61, 0xc3c6cc),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::luminance;

    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn presets_keep_text_readable_and_ids_unique() {
        let white = hex(0xffffff);
        let dark_page = hex(0x1e1d1b);
        for (i, preset) in ACCENT_PRESETS.iter().enumerate() {
            assert!(contrast(white, preset.light) >= 4.5, "{}", preset.id);
            assert!(contrast(preset.dark, dark_page) >= 4.5, "{}", preset.id);
            assert_eq!(preset.color(Appearance::Light), preset.light);
            assert_eq!(preset.color(Appearance::Dark), preset.dark);
            assert!(ACCENT_PRESETS[..i].iter().all(|p| p.id != preset.id));
        }
    }
}

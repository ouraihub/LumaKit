//! Colors: theme files in the original's v2 format (the six bundled ones in
//! `themes/*.theme.json`, copied verbatim, and the user's), the catalog that turns the configured
//! theme into one to install, and the semantic roles the original derives from a palette. Views
//! paint with [`Roles`] only; a palette value is never used directly.

mod accents;
mod catalog;
mod manifest;
mod palette;
mod roles;

use gpui_kit::{App, Global, Rgba, Window, component::Theme as KitTheme};

/// How far an accent's hover state blends toward the page.
const ACCENT_HOVER_MIX: f32 = 0.18;
/// Relative luminance above which text on the accent is black.
const ON_ACCENT_THRESHOLD: f32 = 0.45;
const BLACK: Rgba = Rgba {
    r: 0.,
    g: 0.,
    b: 0.,
    a: 1.,
};
const WHITE: Rgba = Rgba {
    r: 1.,
    g: 1.,
    b: 1.,
    a: 1.,
};

/// WCAG relative luminance of an opaque color.
fn luminance(c: Rgba) -> f32 {
    let lin = |v: f32| {
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// Text color on an accent fill: black on a light accent, white on a dark one.
pub fn on_accent(accent: Rgba) -> Rgba {
    if luminance(accent) > ON_ACCENT_THRESHOLD {
        BLACK
    } else {
        WHITE
    }
}

pub use accents::{ACCENT_PRESETS, AccentPreset};
pub use catalog::{
    DEFAULT_THEME, FileIssue, Resolution, SystemThemes, ThemeCatalog, ThemePair, UserThemeIssue,
    UserThemeIssueKind,
};
pub use manifest::{MAX_NESTING, ManifestError, ManifestErrorKind};
pub use palette::{Ansi, Palette, Syntax};
pub use roles::Roles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contrast {
    Normal,
    High,
}

/// One parsed theme file. A user theme's `id` carries the catalog's `user.` prefix.
#[derive(Debug, Clone)]
pub struct LumaTheme {
    pub id: String,
    pub label: String,
    pub order: i64,
    pub appearance: Appearance,
    pub contrast: Contrast,
    pub palette: Palette,
    pub ansi: Ansi,
    pub syntax: Syntax,
    pub roles: Roles,
}

impl Global for LumaTheme {}

impl LumaTheme {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Installs the theme as the app global and maps it onto the component library's colors,
    /// fonts (the active [`crate::Typography`]) and radii.
    pub fn install(&self, window: Option<&mut Window>, cx: &mut App) {
        let mode = match self.appearance {
            Appearance::Dark => gpui_kit::component::ThemeMode::Dark,
            Appearance::Light => gpui_kit::component::ThemeMode::Light,
        };
        KitTheme::change(mode, window, cx);
        let roles = self.roles;
        KitTheme::update(cx, |kit| {
            crate::theme_map::apply(&mut kit.colors, &roles);
            crate::typography::apply_to_kit(kit);
            kit.radius = crate::metrics::radius::SM;
            kit.radius_lg = crate::metrics::radius::LG;
        });
        cx.set_global(self.clone());
    }

    /// This theme with `accent` (one color per appearance, chosen by the host) as its accent:
    /// every accent-derived palette entry and role follows, and text on the accent stays readable.
    pub fn with_accent(&self, accent: Rgba) -> Self {
        let mut palette = self.palette;
        let page = if self.appearance == Appearance::Dark {
            BLACK
        } else {
            WHITE
        };
        palette.accent = accent;
        palette.accent_solid = accent;
        palette.bubble_accent = accent;
        // Hover moves toward the page so it reads as the same press in both appearances.
        palette.accent_hover = accent.blend(Rgba {
            a: ACCENT_HOVER_MIX,
            ..page
        });
        palette.on_accent = on_accent(accent);
        Self {
            palette,
            roles: Roles::derive(&palette, self.appearance),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests;

//! Appearance preferences shared by LumaKit hosts: a catalog theme (fixed, or a light/dark pair
//! that follows the system), an optional accent override and the density, plus the rules for
//! changing them and the 外观 settings page ([`AppearancePage`]).
//!
//! The serialized form of [`Preferences`] is a host's stored schema: field and variant names
//! are stable. Persistence and migration from older host formats stay in the host.

mod page;

use gpui_kit::{App, Window, WindowAppearance};
use serde::{Deserialize, Serialize};

pub use page::AppearancePage;

use crate::theme::{ACCENT_PRESETS, AccentPreset, DEFAULT_THEME, SystemThemes};
use crate::{LumaTheme, ThemeCatalog, Typography};

/// The pair 跟随系统 starts from on a fresh install.
pub const DEFAULT_PAIR: (&str, &str) = ("light", "dark");
/// Compact density shrinks every type style from the 14 px UI size to 13 px.
pub const COMPACT_SCALE: f32 = 13. / 14.;

/// Accent choice: the theme's own, or one of the [`ACCENT_PRESETS`] laid over it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Accent {
    #[default]
    Theme,
    Red,
    Blue,
    Green,
    Yellow,
    Cyan,
    Purple,
    Gray,
}

/// Every accent with its display name, in swatch order.
pub const ACCENTS: [(Accent, &str); 8] = [
    (Accent::Theme, "主题默认"),
    (Accent::Red, "朱红"),
    (Accent::Blue, "蓝"),
    (Accent::Green, "绿"),
    (Accent::Yellow, "黄"),
    (Accent::Cyan, "青"),
    (Accent::Purple, "紫"),
    (Accent::Gray, "灰"),
];

impl Accent {
    /// The preset this accent lays over the theme; `None` keeps the theme's own.
    pub fn preset(self) -> Option<&'static AccentPreset> {
        let index = match self {
            Accent::Theme => return None,
            Accent::Red => 0,
            Accent::Blue => 1,
            Accent::Green => 2,
            Accent::Yellow => 3,
            Accent::Cyan => 4,
            Accent::Purple => 5,
            Accent::Gray => 6,
        };
        ACCENT_PRESETS.get(index)
    }
}

/// Interface density; compact scales all type down by [`COMPACT_SCALE`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

impl Density {
    /// The [`Typography::scale`] this density installs.
    pub fn scale(self) -> f32 {
        if self == Density::Compact {
            COMPACT_SCALE
        } else {
            1.
        }
    }
}

/// Saved appearance. `theme_id` is the fixed theme; with `follow_system` the
/// `light_theme` / `dark_theme` pair is used by the window's appearance instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub theme_id: String,
    pub follow_system: bool,
    pub light_theme: String,
    pub dark_theme: String,
    pub accent: Accent,
    pub density: Density,
}

impl Default for Preferences {
    /// The catalog's default theme, fixed, with the theme's accent and comfortable density.
    fn default() -> Self {
        Self {
            theme_id: DEFAULT_THEME.to_owned(),
            follow_system: false,
            light_theme: DEFAULT_PAIR.0.to_owned(),
            dark_theme: DEFAULT_PAIR.1.to_owned(),
            accent: Accent::Theme,
            density: Density::Comfortable,
        }
    }
}

/// Maps GPUI's window appearance onto the catalog's two appearances.
pub fn appearance_of(window: WindowAppearance) -> crate::Appearance {
    match window {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => crate::Appearance::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => crate::Appearance::Light,
    }
}

impl Preferences {
    /// The theme to install under the window's appearance, accent override applied.
    pub fn resolve(&self, catalog: &ThemeCatalog, window: WindowAppearance) -> LumaTheme {
        let system = self.follow_system.then(|| SystemThemes {
            light: &self.light_theme,
            dark: &self.dark_theme,
            appearance: appearance_of(window),
        });
        let theme = catalog.resolve(&self.theme_id, system).theme;
        match self.accent.preset() {
            Some(preset) => theme.with_accent(preset.color(theme.appearance)),
            None => (*theme).clone(),
        }
    }

    /// Installs the resolved theme and the density's type scale. The window's appearance only
    /// matters with 跟随系统; hosts call this again when it changes.
    pub fn apply(&self, catalog: &ThemeCatalog, window: &mut Window, cx: &mut App) {
        let theme = self.resolve(catalog, window.appearance());
        Typography::set(
            Typography {
                scale: self.density.scale(),
                ..Typography::current()
            },
            cx,
        );
        theme.install(Some(window), cx);
    }

    /// Whether a theme card is selected: the fixed theme, or either slot of the followed pair.
    pub fn is_selected(&self, theme: &LumaTheme) -> bool {
        if self.follow_system {
            theme.id == self.light_theme || theme.id == self.dark_theme
        } else {
            theme.id == self.theme_id
        }
    }

    /// Picking a theme: fixed, it becomes the theme; following, it fills its appearance's slot.
    pub fn pick_theme(&self, theme: &LumaTheme) -> Self {
        let mut next = self.clone();
        if self.follow_system {
            match theme.appearance {
                crate::Appearance::Light => next.light_theme = theme.id.clone(),
                crate::Appearance::Dark => next.dark_theme = theme.id.clone(),
            }
        }
        next.theme_id = theme.id.clone();
        next
    }

    /// Turning 跟随系统 on pairs the fixed theme with its own family's other appearance (warm
    /// paper stays warm paper); turning it off keeps the theme currently shown.
    pub fn set_follow(
        &self,
        catalog: &ThemeCatalog,
        follow: bool,
        window: WindowAppearance,
    ) -> Self {
        use crate::Appearance::{Dark, Light};
        let mut next = self.clone();
        next.follow_system = follow;
        if follow {
            let fixed = catalog.resolve(&self.theme_id, None).theme;
            let other = match fixed.appearance {
                Light => Dark,
                Dark => Light,
            };
            let pair = catalog.derive_pair(&fixed.id);
            let counterpart = catalog
                .counterpart(&fixed, other, fixed.contrast)
                .map(|t| t.id.clone());
            (next.light_theme, next.dark_theme) = match fixed.appearance {
                Light => (fixed.id.clone(), counterpart.unwrap_or(pair.dark)),
                Dark => (counterpart.unwrap_or(pair.light), fixed.id.clone()),
            };
        } else {
            next.theme_id = self.resolve(catalog, window).id;
        }
        next
    }
}

/// The bundled themes' Chinese names; a user theme shows its own label.
pub fn theme_name(theme: &LumaTheme) -> &str {
    match theme.id.as_str() {
        "dark" => "深色",
        "light" => "浅色",
        "minimal-dark" => "极简深色",
        "minimal-light" => "极简浅色",
        "high-contrast-dark" => "高对比度深色",
        "high-contrast-light" => "高对比度浅色",
        "warm-dark" => "暖纸深色",
        "warm-light" => "暖纸浅色",
        _ => &theme.label,
    }
}

#[cfg(test)]
mod tests;

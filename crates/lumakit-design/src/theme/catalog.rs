//! The theme catalog (original `apps/web/src/themes/runtime.ts` and the family matching of
//! `panels/AppearanceSettings.tsx`): the six bundled themes plus the user's theme files, listed in
//! display order, and the rules that turn the configured theme into the one to install. Fallbacks
//! and derived pair slots only ever pick bundled themes.

mod family;
#[cfg(test)]
mod tests;

use std::{
    cmp::Ordering,
    sync::{Arc, LazyLock},
};

use super::{Appearance, Contrast, ManifestErrorKind, LumaTheme, manifest::is_theme_slug};

/// The configured default theme (original `DEFAULT_CONFIG.theme`); listed first.
pub const DEFAULT_THEME: &str = "dark";
const USER_PREFIX: &str = "user.";
/// Ids the original renamed; a stored old id resolves to the new one.
const RENAMED: [(&str, &str); 1] = [("high-contrast", "high-contrast-dark")];

const BUNDLED: [(&str, &str); 8] = [
    ("dark", include_str!("../../themes/dark.theme.json")),
    ("light", include_str!("../../themes/light.theme.json")),
    (
        "minimal-dark",
        include_str!("../../themes/minimal-dark.theme.json"),
    ),
    (
        "minimal-light",
        include_str!("../../themes/minimal-light.theme.json"),
    ),
    (
        "high-contrast-dark",
        include_str!("../../themes/high-contrast-dark.theme.json"),
    ),
    (
        "high-contrast-light",
        include_str!("../../themes/high-contrast-light.theme.json"),
    ),
    // LumaDesk's warm paper palette, as one family.
    ("warm-dark", include_str!("../../themes/warm-dark.theme.json")),
    ("warm-light", include_str!("../../themes/warm-light.theme.json")),
];

/// Why the app could not hand a user theme file's contents over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileIssue {
    TooLarge,
    TooMany,
    Unreadable(String),
}

/// The original's `UserThemeIssue.error` codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserThemeIssueKind {
    TooLarge,
    TooMany,
    Unreadable,
    InvalidJson,
    InvalidTheme,
    /// `user.` + the manifest id is longer than a theme id may be.
    IdTooLong,
    /// An earlier file already installed this id; the earlier one stays.
    DuplicateId,
}

/// One user theme file that is not in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserThemeIssue {
    pub file_name: String,
    pub kind: UserThemeIssueKind,
    /// English detail (the parser's issues, the error text, or the manifest id).
    pub detail: Option<String>,
}

/// System mode's configured pair and the system's current appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemThemes<'a> {
    pub light: &'a str,
    pub dark: &'a str,
    pub appearance: Appearance,
}

/// A light and a dark theme id (a derived system pair).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePair {
    pub light: String,
    pub dark: String,
}

/// The theme to install for a configuration.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub theme: Arc<LumaTheme>,
    /// The id the configuration asked for (system mode: the slot of the current appearance).
    pub requested_id: String,
    /// The requested theme is missing (or has the wrong appearance) and `theme` replaces it.
    pub fallback: bool,
}

/// Bundled themes, checked once: the default and a fallback per appearance and contrast.
#[derive(Debug)]
struct Bundled {
    themes: Vec<Arc<LumaTheme>>,
    default: Arc<LumaTheme>,
    /// `[normal, high]`: the preferred contrast, else normal, else the lowest order.
    light: [Arc<LumaTheme>; 2],
    dark: [Arc<LumaTheme>; 2],
}

#[derive(Debug, Clone)]
pub struct ThemeCatalog {
    bundled: Arc<Bundled>,
    /// Bundled and user themes in display order.
    themes: Vec<Arc<LumaTheme>>,
    issues: Vec<UserThemeIssue>,
}

/// JavaScript's `<` on strings: UTF-16 code unit order.
fn compare_text(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn by_order(a: &LumaTheme, b: &LumaTheme) -> Ordering {
    a.order
        .cmp(&b.order)
        .then_with(|| compare_text(&a.label, &b.label))
        .then_with(|| compare_text(&a.id, &b.id))
}

/// Display order: the default theme first, then order, label, id.
fn display_order(a: &Arc<LumaTheme>, b: &Arc<LumaTheme>) -> Ordering {
    match (a.id == DEFAULT_THEME, b.id == DEFAULT_THEME) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => by_order(a, b),
    }
}

fn canonical(id: &str) -> &str {
    RENAMED
        .iter()
        .find_map(|(old, new)| (*old == id).then_some(*new))
        .unwrap_or(id)
}

impl Bundled {
    fn load() -> Result<Self, String> {
        let mut themes: Vec<Arc<LumaTheme>> = Vec::new();
        for (name, json) in BUNDLED {
            let theme = LumaTheme::parse(json)
                .map_err(|e| format!("内置主题 {name} 无法解析：{e}"))?;
            if themes.iter().any(|t| t.id == theme.id) || theme.id.starts_with(USER_PREFIX) {
                return Err(format!("内置主题 id 无效或重复：{}", theme.id));
            }
            themes.push(Arc::new(theme));
        }
        let default = themes
            .iter()
            .find(|t| t.id == DEFAULT_THEME)
            .cloned()
            .ok_or_else(|| format!("缺少内置默认主题：{DEFAULT_THEME}"))?;
        let fallbacks = |appearance: Appearance| -> Result<[Arc<LumaTheme>; 2], String> {
            let mut matching: Vec<&Arc<LumaTheme>> = themes
                .iter()
                .filter(|t| t.appearance == appearance)
                .collect();
            matching.sort_by(|a, b| by_order(a, b));
            let with = |contrast: Contrast| matching.iter().find(|t| t.contrast == contrast);
            let normal = with(Contrast::Normal)
                .or(matching.first())
                .map(|t| Arc::clone(t))
                .ok_or_else(|| format!("内置主题缺少{appearance:?}外观"))?;
            let high = with(Contrast::High).map_or_else(|| Arc::clone(&normal), |t| Arc::clone(t));
            Ok([normal, high])
        };
        let (light, dark) = (fallbacks(Appearance::Light)?, fallbacks(Appearance::Dark)?);
        Ok(Self {
            themes,
            default,
            light,
            dark,
        })
    }

    /// The original's `fallbackTheme(appearance, preferredContrast)`; `None` prefers normal.
    fn fallback(&self, appearance: Appearance, contrast: Option<Contrast>) -> &Arc<LumaTheme> {
        let slots = match appearance {
            Appearance::Light => &self.light,
            Appearance::Dark => &self.dark,
        };
        match contrast {
            None | Some(Contrast::Normal) => &slots[0],
            Some(Contrast::High) => &slots[1],
        }
    }
}

impl ThemeCatalog {
    /// The bundled themes alone. They are compiled in and pinned by tests; an `Err` is a
    /// programming error.
    pub fn bundled() -> Result<&'static Self, &'static str> {
        static CATALOG: LazyLock<Result<ThemeCatalog, String>> = LazyLock::new(|| {
            let bundled = Arc::new(Bundled::load()?);
            let mut themes = bundled.themes.clone();
            themes.sort_by(display_order);
            Ok(ThemeCatalog {
                bundled,
                themes,
                issues: Vec::new(),
            })
        });
        CATALOG.as_ref().map_err(String::as_str)
    }

    /// The bundled themes plus the user's files, in the order given (original
    /// `installUserThemes`): ids get the `user.` prefix; a file that cannot be installed is
    /// reported in [`Self::user_issues`] and never affects the others.
    pub fn with_user_themes(
        &self,
        files: impl IntoIterator<Item = (String, Result<String, FileIssue>)>,
    ) -> Self {
        let mut themes = self.bundled.themes.clone();
        let mut issues = Vec::new();
        for (file_name, contents) in files {
            let issue = |kind, detail| UserThemeIssue {
                file_name: file_name.clone(),
                kind,
                detail,
            };
            let text = match contents {
                Ok(text) => text,
                Err(FileIssue::TooLarge) => {
                    issues.push(issue(UserThemeIssueKind::TooLarge, None));
                    continue;
                }
                Err(FileIssue::TooMany) => {
                    issues.push(issue(UserThemeIssueKind::TooMany, None));
                    continue;
                }
                Err(FileIssue::Unreadable(detail)) => {
                    let detail = (!detail.is_empty()).then_some(detail);
                    issues.push(issue(UserThemeIssueKind::Unreadable, detail));
                    continue;
                }
            };
            let mut theme = match LumaTheme::parse(&text) {
                Ok(theme) => theme,
                Err(error) => {
                    let kind = match error.kind {
                        ManifestErrorKind::InvalidJson => UserThemeIssueKind::InvalidJson,
                        ManifestErrorKind::InvalidTheme => UserThemeIssueKind::InvalidTheme,
                    };
                    issues.push(issue(kind, Some(error.detail())));
                    continue;
                }
            };
            let id = format!("{USER_PREFIX}{}", theme.id);
            if !is_theme_slug(&id) {
                issues.push(issue(UserThemeIssueKind::IdTooLong, Some(theme.id)));
            } else if themes.iter().any(|t| t.id == id) {
                issues.push(issue(UserThemeIssueKind::DuplicateId, Some(theme.id)));
            } else {
                theme.id = id;
                themes.push(Arc::new(theme));
            }
        }
        themes.sort_by(display_order);
        Self {
            bundled: Arc::clone(&self.bundled),
            themes,
            issues,
        }
    }

    /// Every theme in display order: the default first, then order, label, id.
    pub fn list(&self) -> &[Arc<LumaTheme>] {
        &self.themes
    }

    /// The user files that are not in the catalog, in the order given.
    pub fn user_issues(&self) -> &[UserThemeIssue] {
        &self.issues
    }

    /// The theme with this id (a renamed id finds its new name).
    pub fn get(&self, id: &str) -> Option<&Arc<LumaTheme>> {
        let id = canonical(id);
        self.themes.iter().find(|t| t.id == id)
    }

    /// The theme to install. Fixed (`system: None`): the theme, else the default. System: the
    /// slot of the current appearance, else — missing or of the other appearance — the bundled
    /// fallback of that appearance.
    pub fn resolve(&self, theme_id: &str, system: Option<SystemThemes<'_>>) -> Resolution {
        let Some(system) = system else {
            let exact = self.get(theme_id);
            return Resolution {
                theme: Arc::clone(exact.unwrap_or(&self.bundled.default)),
                requested_id: theme_id.to_owned(),
                fallback: exact.is_none(),
            };
        };
        let requested = match system.appearance {
            Appearance::Light => system.light,
            Appearance::Dark => system.dark,
        };
        let exact = self
            .get(requested)
            .filter(|t| t.appearance == system.appearance);
        Resolution {
            theme: Arc::clone(
                exact.unwrap_or_else(|| self.bundled.fallback(system.appearance, None)),
            ),
            requested_id: requested.to_owned(),
            fallback: exact.is_none(),
        }
    }

    /// The pair system mode starts with (original `deriveSystemThemePair`): the fixed theme in
    /// its own slot, the other slot the bundled fallback with the fixed theme's contrast.
    pub fn derive_pair(&self, theme_id: &str) -> ThemePair {
        let fixed = self.get(theme_id).unwrap_or(&self.bundled.default);
        let slot = |appearance| {
            if fixed.appearance == appearance {
                fixed.id.clone()
            } else {
                self.bundled
                    .fallback(appearance, Some(fixed.contrast))
                    .id
                    .clone()
            }
        };
        ThemePair {
            light: slot(Appearance::Light),
            dark: slot(Appearance::Dark),
        }
    }

    /// The theme of `from`'s family with this appearance and contrast, else a family-less one
    /// (`dark`, `light`, `high-contrast-*`), else the first listed (original `counterpart`).
    pub fn counterpart(
        &self,
        from: &LumaTheme,
        appearance: Appearance,
        contrast: Contrast,
    ) -> Option<&Arc<LumaTheme>> {
        let family = family::key(&from.id);
        let mut candidates = self
            .themes
            .iter()
            .filter(|t| t.appearance == appearance && t.contrast == contrast);
        let first = candidates.clone().next();
        candidates
            .clone()
            .find(|t| family::key(&t.id) == family)
            .or_else(|| candidates.find(|t| family::key(&t.id).is_empty()))
            .or(first)
    }

    /// The theme the mode switch picks for `appearance` (original `appearanceTarget`): `from`
    /// itself when it already has it, else its counterpart with the same contrast, else the
    /// first listed theme of that appearance.
    pub fn appearance_target<'a>(
        &'a self,
        from: &'a Arc<LumaTheme>,
        appearance: Appearance,
    ) -> Option<&'a Arc<LumaTheme>> {
        if from.appearance == appearance {
            return Some(from);
        }
        self.counterpart(from, appearance, from.contrast)
            .or_else(|| self.themes.iter().find(|t| t.appearance == appearance))
    }
}

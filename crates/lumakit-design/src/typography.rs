//! Runtime typography (08 A9–A11): the user's interface and code fonts, fallbacks, code size,
//! code ligatures and weight offset. Every `.tr-*` type style goes through
//! [`crate::components::TextStyled::text_style`], which runs while elements are built and has no
//! `App`, so the active settings live in a process-wide lock that only the UI thread writes.

#[cfg(test)]
mod tests;

use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use gpui_kit::{
    App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Pixels, SharedString,
    component::Theme as KitTheme, px,
};

use crate::{
    fonts,
    metrics::{self, Family, TextStyle, text},
};

/// The code size the code styles are valued at; each keeps its distance from the chosen size.
pub const DEFAULT_CODE_SIZE: Pixels = metrics::font::CODE;

/// The font settings, as plain values (the app converts its preferences into these).
#[derive(Debug, Clone, PartialEq)]
pub struct Typography {
    pub interface_family: String,
    pub interface_fallbacks: Vec<String>,
    pub code_family: String,
    pub code_fallbacks: Vec<String>,
    pub code_size: Pixels,
    /// Code styles draw ligatures (the original's default is off). The terminal never does.
    pub ligatures: bool,
    /// Added to every style's weight except the brand face's.
    pub weight_offset: f32,
    /// Multiplies every style's size (density): 1.0 is the original's sizes.
    pub scale: f32,
}

/// Chinese glyphs come from these, in order, before the system's own fallback; the bundled
/// faces have none.
pub const CJK_FALLBACKS: [&str; 4] = ["Microsoft YaHei UI", "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC"];

impl Default for Typography {
    /// The bundled faces at their original sizes, with explicit CJK fallbacks.
    fn default() -> Self {
        let cjk = || CJK_FALLBACKS.map(str::to_owned).to_vec();
        Self {
            interface_family: fonts::INTERFACE.to_owned(),
            interface_fallbacks: cjk(),
            code_family: fonts::CODE.to_owned(),
            code_fallbacks: cjk(),
            code_size: DEFAULT_CODE_SIZE,
            ligatures: false,
            weight_offset: 0.,
            scale: 1.,
        }
    }
}

/// A type style as the text system takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    pub family: SharedString,
    pub fallbacks: Option<FontFallbacks>,
    /// Code styles only (ligatures on or off); other styles inherit.
    pub features: Option<FontFeatures>,
    pub weight: FontWeight,
    pub size: Pixels,
    pub line_height: Pixels,
}

#[derive(Debug)]
struct RoleFont {
    family: SharedString,
    fallbacks: Option<FontFallbacks>,
}

impl RoleFont {
    fn new(family: &str, fallbacks: &[String]) -> Self {
        Self {
            family: SharedString::from(family.to_owned()),
            fallbacks: (!fallbacks.is_empty())
                .then(|| FontFallbacks::from_fonts(fallbacks.to_vec())),
        }
    }
}

/// The settings with their fonts built once, so applying a style allocates nothing.
#[derive(Debug)]
struct Active {
    settings: Typography,
    interface: RoleFont,
    code: RoleFont,
    brand: RoleFont,
    /// `calt`, `liga` and `clig` off: CSS `font-variant-ligatures: none` on DirectWrite.
    no_ligatures: FontFeatures,
    code_features: FontFeatures,
}

impl Active {
    fn new(settings: Typography) -> Self {
        let no_ligatures = FontFeatures(Arc::new(
            ["calt", "liga", "clig"]
                .map(|tag| (tag.to_owned(), 0))
                .to_vec(),
        ));
        Self {
            interface: RoleFont::new(&settings.interface_family, &settings.interface_fallbacks),
            code: RoleFont::new(&settings.code_family, &settings.code_fallbacks),
            brand: RoleFont::new(fonts::BRAND, &[]),
            code_features: if settings.ligatures {
                FontFeatures::default()
            } else {
                no_ligatures.clone()
            },
            no_ligatures,
            settings,
        }
    }

    fn resolve(&self, style: &TextStyle) -> ResolvedStyle {
        let offset = self.settings.weight_offset;
        let (role, size, weight, features) = match style.family {
            Family::Interface => (&self.interface, style.size, style.weight.0 + offset, None),
            Family::Code => (
                &self.code,
                style.size + (self.settings.code_size - DEFAULT_CODE_SIZE),
                style.weight.0 + offset,
                Some(self.code_features.clone()),
            ),
            Family::Brand => (&self.brand, style.size, style.weight.0, None),
        };
        let size = size * self.settings.scale;
        ResolvedStyle {
            family: role.family.clone(),
            fallbacks: role.fallbacks.clone(),
            features,
            weight: fonts::snap_weight(&role.family, FontWeight(weight)),
            size,
            line_height: size * style.line_height,
        }
    }
}

static ACTIVE: LazyLock<RwLock<Arc<Active>>> =
    LazyLock::new(|| RwLock::new(Arc::new(Active::new(Typography::default()))));

fn active() -> Arc<Active> {
    Arc::clone(&ACTIVE.read().unwrap_or_else(PoisonError::into_inner))
}

impl Typography {
    /// The active settings.
    pub fn current() -> Self {
        active().settings.clone()
    }

    /// Makes `typography` the active settings and hands its fonts to the component library.
    /// UI thread only; views pick it up on their next render (`refresh_windows`).
    pub fn set(typography: Typography, cx: &mut App) {
        *ACTIVE.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(Active::new(typography));
        if cx.has_global::<KitTheme>() {
            KitTheme::update(cx, apply_to_kit);
        }
    }
}

/// A type style under the active settings.
pub fn resolve(style: &TextStyle) -> ResolvedStyle {
    ACTIVE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .resolve(style)
}

/// The component library's base fonts: the interface and code families at the base sizes.
pub(crate) fn apply_to_kit(kit: &mut KitTheme) {
    let active = active();
    kit.font_family = active.interface.family.clone();
    kit.mono_font_family = active.code.family.clone();
    kit.font_size = metrics::font::UI * active.settings.scale;
    kit.mono_font_size = active.settings.code_size * active.settings.scale;
}

/// The terminal's font and cell (A11): the code font at the code size, never with ligatures;
/// the cell is the `M` advance wide and `CODE_TEXT`'s line height tall.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalFont {
    pub font: Font,
    pub size: Pixels,
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

impl TerminalFont {
    /// Measures the active code font (cheap: font resolution and glyph advances are cached).
    pub fn measure(cx: &App) -> Self {
        let active = active();
        let style = active.resolve(&text::CODE_TEXT);
        let font = Font {
            family: style.family,
            features: active.no_ligatures.clone(),
            fallbacks: style.fallbacks,
            weight: style.weight,
            style: FontStyle::Normal,
        };
        Self {
            cell_width: cell_width(&font, style.size, cx),
            font,
            size: style.size,
            line_height: style.line_height,
        }
    }
}

/// One column of a code style under the active settings: the `M` advance (08 A11, A19).
pub fn code_cell_width(style: &TextStyle, cx: &App) -> Pixels {
    let resolved = resolve(style);
    let font = Font {
        family: resolved.family,
        features: resolved.features.unwrap_or_default(),
        fallbacks: resolved.fallbacks,
        weight: resolved.weight,
        style: FontStyle::Normal,
    };
    cell_width(&font, resolved.size, cx)
}

fn cell_width(font: &Font, size: Pixels, cx: &App) -> Pixels {
    let text_system = cx.text_system();
    let id = text_system.resolve_font(font);
    text_system
        .advance(id, size, 'M')
        .map_or_else(
            |_| text_system.layout_width(id, size, 'M'),
            |advance| advance.width,
        )
        .max(px(1.))
}

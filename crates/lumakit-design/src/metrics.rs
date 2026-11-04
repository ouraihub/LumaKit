//! Sizes, spacing, radii and type styles, valued from the original (`spacing.json`,
//! `typography.json`, panel/shell layout). Views reference these names, never pixel literals.

use gpui_kit::{FontWeight, Pixels, px};

/// The only spacing steps the original allows (`--space-*`).
pub mod space {
    use super::{Pixels, px};
    pub const S0: Pixels = px(0.);
    pub const S2: Pixels = px(2.);
    pub const S4: Pixels = px(4.);
    pub const S6: Pixels = px(6.);
    pub const S8: Pixels = px(8.);
    pub const S12: Pixels = px(12.);
    pub const S16: Pixels = px(16.);
    pub const S24: Pixels = px(24.);
    pub const S32: Pixels = px(32.);
    pub const S40: Pixels = px(40.);
    pub const S64: Pixels = px(64.);
}

/// Shell and panel dimensions.
pub mod size {
    use super::{Pixels, px};
    pub const TOP_BAR: Pixels = px(40.);
    pub const ACTIVITY_BAR: Pixels = px(40.);
    pub const PANEL_HEADER: Pixels = px(28.);
    /// Buttons and selectors inside a panel header.
    pub const HEADER_CONTROL: Pixels = px(24.);
    pub const ROW: Pixels = px(22.);
    pub const PANEL_PADDING: Pixels = px(8.);
    pub const TREE_INDENT: Pixels = px(8.);
    pub const TAB_STRIP: Pixels = px(28.);
    pub const ICON: Pixels = px(16.);
    pub const ICON_SMALL: Pixels = px(14.);
    pub const STATUS_DOT: Pixels = px(8.);
    /// The selected activity item's left accent bar.
    pub const ACCENT_BAR: Pixels = px(2.);
    pub const SETTINGS_ROW: Pixels = px(32.);
    /// A list column beside a detail pane (e.g. a settings page's item list).
    pub const LIST_COLUMN: Pixels = px(208.);
    pub const BORDER: Pixels = px(1.);
    /// Readable column: long-form content, editors and settings pages.
    pub const CONTENT_MAX: Pixels = px(760.);
    /// Below this window width, side-by-side layouts stack.
    pub const NARROW: Pixels = px(700.);
}

pub mod radius {
    use super::{Pixels, px};
    pub const XS: Pixels = px(2.);
    pub const SM: Pixels = px(4.);
    pub const MD: Pixels = px(6.);
    /// Dialogs and the user message bubble.
    pub const LG: Pixels = px(8.);
}

/// Base sizes handed to the component library.
pub mod font {
    use super::{Pixels, px};
    pub const UI: Pixels = px(14.);
    pub const CODE: Pixels = px(13.);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Interface,
    Code,
    Brand,
}

/// One `.tr-*` type style.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub family: Family,
    pub size: Pixels,
    pub weight: FontWeight,
    /// Multiple of `size`.
    pub line_height: f32,
    pub uppercase: bool,
}

const fn style(
    family: Family,
    size: f32,
    weight: f32,
    line_height: f32,
    uppercase: bool,
) -> TextStyle {
    TextStyle {
        family,
        size: px(size),
        weight: FontWeight(weight),
        line_height,
        uppercase,
    }
}

/// `typography.json` weights (`W_*`) and line heights (`LH_*`).
mod scale {
    pub const W_LIGHT: f32 = 370.;
    pub const W_REGULAR: f32 = 400.;
    pub const W_MEDIUM: f32 = 500.;
    pub const W_SEMIBOLD: f32 = 600.;
    pub const W_BUTTON: f32 = 470.;
    pub const W_BRAND: f32 = 400.;
    pub const LH_COMPACT: f32 = 1.25;
    pub const LH_METADATA: f32 = 1.25;
    pub const LH_UI: f32 = 1.3;
    pub const LH_DEFAULT: f32 = 1.3;
    pub const LH_CODE: f32 = 1.4;
    pub const LH_RELAXED: f32 = 1.4;
    pub const LH_READING: f32 = 1.45;
}

pub mod text {
    use super::{Family::*, TextStyle, scale::*, style};
    pub const UI: TextStyle = style(Interface, 14., W_LIGHT, LH_UI, false);
    pub const METADATA: TextStyle = style(Interface, 12., W_LIGHT, LH_METADATA, false);
    pub const EYEBROW: TextStyle = style(Interface, 12., W_MEDIUM, LH_RELAXED, true);
    pub const LABEL_PILL: TextStyle = style(Interface, 12., W_MEDIUM, LH_RELAXED, true);
    pub const CAPTION: TextStyle = style(Interface, 10., W_MEDIUM, LH_COMPACT, true);
    pub const ACTION: TextStyle = style(Interface, 12., W_MEDIUM, LH_DEFAULT, false);
    pub const BUTTON: TextStyle = style(Interface, 14., W_BUTTON, LH_UI, false);
    pub const EMPHASIS: TextStyle = style(Interface, 12., W_MEDIUM, LH_DEFAULT, false);
    pub const READING: TextStyle = style(Interface, 14., W_LIGHT, LH_READING, false);
    pub const TITLE_DIALOG: TextStyle = style(Interface, 14., W_SEMIBOLD, LH_COMPACT, false);
    pub const TITLE_CARD: TextStyle = style(Interface, 14., W_SEMIBOLD, LH_COMPACT, false);
    pub const TITLE_SECTION: TextStyle = style(Interface, 14., W_MEDIUM, LH_DEFAULT, false);
    pub const TITLE_COMPACT: TextStyle = style(Interface, 12., W_MEDIUM, LH_DEFAULT, false);
    pub const TITLE_ENTITY: TextStyle = style(Interface, 14., W_LIGHT, LH_READING, false);
    pub const HEADING_SM: TextStyle = style(Interface, 16., W_SEMIBOLD, LH_COMPACT, false);
    pub const HEADING_MD: TextStyle = style(Interface, 18., W_SEMIBOLD, LH_COMPACT, false);
    pub const HEADING_LG: TextStyle = style(Interface, 20., W_SEMIBOLD, LH_COMPACT, false);
    pub const HEADING_XL: TextStyle = style(Interface, 24., W_SEMIBOLD, LH_COMPACT, false);
    pub const BRAND_HERO: TextStyle = style(Brand, 44., W_BRAND, LH_COMPACT, false);
    pub const CODE_TEXT: TextStyle = style(Code, 13., W_REGULAR, LH_RELAXED, false);
    pub const CODE_DOCUMENT: TextStyle = style(Code, 13., W_REGULAR, LH_CODE, false);
    pub const CODE_SMALL: TextStyle = style(Code, 11., W_REGULAR, LH_CODE, false);
}

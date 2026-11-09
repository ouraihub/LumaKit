//! LumaKit's design system: visual tokens, themes and stateless GPUI components shared by every
//! LumaKit host. Extracted from ThinkRail Native's `crates/ui`; "the original" in these docs means
//! the ThinkRail web app whose tokens that crate reproduced.
//!
//! `theme` owns colors and the theme catalog, `metrics` sizes and type styles, `fonts` the
//! bundled faces and font-file inspection, `typography` the user's font settings, `assets` the
//! icon source; `components` builds on all of them and never touches services or files.
//! `appearance` is the user's theme / accent / density choice, its rules and its settings page;
//! hosts persist it.

pub mod appearance;
mod assets;
pub mod components;
pub mod fonts;
pub mod metrics;
pub mod theme;
mod theme_map;
pub mod typography;

pub use assets::UiAssets;
pub use theme::{Appearance, Contrast, LumaTheme, Roles, ThemeCatalog};
pub use typography::{TerminalFont, Typography};

//! Stateless, props-driven building blocks of a LumaKit shell. They read only the installed
//! [`LumaTheme`]; clicks are reported through the handlers they are given.

mod buttons;
mod panel;
pub mod settings;
mod tabs;
mod top_bar;

use gpui_kit::{App, ClickEvent, ParentElement, SharedString, Styled, Window, div};

use crate::{metrics::TextStyle, theme::LumaTheme, typography};

pub use buttons::{ActivityItem, IconButton};
pub use panel::{PanelHeader, TreeRow};
pub use tabs::{Segment, Tab, TabStrip};
pub use top_bar::{Breadcrumb, Status, StatusDot};

type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type SelectHandler = Box<dyn Fn(&usize, &mut Window, &mut App) + 'static>;

/// Applies a `.tr-*` type style under the active [`typography::Typography`]: family and
/// fallbacks, weight (offset, snapped to a shipped weight for a bundled face), code size and
/// ligatures, line height.
///
/// GPUI's `Styled` has a `text_style(&mut self)` accessor of the same name. A call on an element
/// value (`div().text_style(style)`) resolves to this by-value method before autoref reaches the
/// accessor; the body names the accessor by its trait to reach it.
pub trait TextStyled: Styled + Sized {
    fn text_style(mut self, style: TextStyle) -> Self {
        let resolved = typography::resolve(&style);
        let text = Styled::text_style(&mut self);
        text.font_family = Some(resolved.family);
        text.font_fallbacks = resolved.fallbacks;
        if let Some(features) = resolved.features {
            text.font_features = Some(features);
        }
        self.font_weight(resolved.weight)
            .text_size(resolved.size)
            .line_height(resolved.line_height)
    }
}

impl<T: Styled> TextStyled for T {}

/// Text with a type style, uppercased when the style says so.
pub fn label(style: TextStyle, content: impl Into<SharedString>) -> gpui_kit::Div {
    let content = content.into();
    let content: SharedString = if style.uppercase {
        content.to_uppercase().into()
    } else {
        content
    };
    div().text_style(style).child(content)
}

fn roles(cx: &App) -> crate::theme::Roles {
    LumaTheme::global(cx).roles
}

/// Components hold closures and child elements, which have no `Debug`; name the type only.
macro_rules! opaque_debug {
    ($($ty:ident),* $(,)?) => {$(
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty)).finish_non_exhaustive()
            }
        }
    )*};
}

opaque_debug!(
    IconButton,
    PanelHeader,
    TreeRow,
    Tab,
    TabStrip,
    Segment,
    StatusDot,
    Breadcrumb,
    ActivityItem
);

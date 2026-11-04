//! Fonts: the original's three OFL families, bundled as TTF (DirectWrite cannot load the web
//! build's woff2) and registered once at startup — licenses ship next to the files in
//! `assets/fonts` — plus registering and inspecting the user's font files and probing whether an
//! installed family is monospaced.

mod inspect;

use std::borrow::Cow;

use gpui_kit::{App, FontWeight, font, px};

use crate::metrics::{Family, TextStyle};

pub use inspect::{FaceInfo, InspectError, inspect};

pub const INTERFACE: &str = "Geist";
pub const CODE: &str = "JetBrains Mono";
pub const BRAND: &str = "Orbitron";

const FILES: [&[u8]; 7] = [
    include_bytes!("../assets/fonts/Geist-Light.ttf"),
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
    include_bytes!("../assets/fonts/Orbitron-Variable.ttf"),
];

/// Registers the bundled faces with the platform text system.
pub fn register_bundled(cx: &App) -> anyhow::Result<()> {
    cx.text_system()
        .add_fonts(FILES.iter().map(|bytes| Cow::Borrowed(*bytes)).collect())
}

/// Registers one user font file. The text system cannot unregister it again.
pub fn register(cx: &App, bytes: Vec<u8>) -> anyhow::Result<()> {
    cx.text_system().add_fonts(vec![Cow::Owned(bytes)])
}

pub fn family(family: Family) -> &'static str {
    match family {
        Family::Interface => INTERFACE,
        Family::Code => CODE,
        Family::Brand => BRAND,
    }
}

/// The weights the bundled static faces ship; `None` for any other family.
fn shipped_weights(family: &str) -> Option<&'static [f32]> {
    match family {
        INTERFACE => Some(&[300., 400., 500., 600.]),
        CODE => Some(&[400., 500.]),
        BRAND => Some(&[400.]),
        _ => None,
    }
}

/// `weight` as the family renders it: a bundled face rounds to its nearest shipped weight (the
/// original's variable fonts render 370 / 470 exactly; the static faces give 400 / 500), any
/// other family gets the weight unchanged.
pub fn snap_weight(family: &str, weight: FontWeight) -> FontWeight {
    let Some(shipped) = shipped_weights(family) else {
        return weight;
    };
    let target = weight.0;
    let nearest = shipped
        .iter()
        .copied()
        .min_by(|a, b| {
            (a - target)
                .abs()
                .total_cmp(&(b - target).abs())
                .then(b.total_cmp(a))
        })
        .unwrap_or(target);
    FontWeight(nearest)
}

/// The bundled face closest to a style's weight.
pub fn face_weight(style: &TextStyle) -> FontWeight {
    snap_weight(family(style.family), style.weight)
}

/// Whether an installed family draws `i` and `M` equally wide (A12; the original's probe). A
/// family the system does not have measures the text system's fallback face instead, so check
/// availability (`all_font_names`) first.
pub fn is_monospace(cx: &App, family: &str) -> bool {
    let text_system = cx.text_system();
    let id = text_system.resolve_font(&font(family.to_owned()));
    let size = px(32.);
    let width = |c| {
        text_system
            .advance(id, size, c)
            .map(|advance| advance.width)
    };
    match (width('i'), width('M')) {
        (Ok(i), Ok(m)) => (i - m).abs() < px(0.05),
        _ => false,
    }
}

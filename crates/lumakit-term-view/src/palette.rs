//! Host-supplied palette adapter.
//!
//! The view receives a palette already resolved into `SnapshotRgba` values
//! (lumakit-term defines them); this module only converts those into gpui
//! `Rgba` / `Hsla` so the painter stays free of palette-side concerns.

use gpui_kit::{Hsla, Rgba};
use lumakit_term::SnapshotRgba;

/// Convert `SnapshotRgba` into a gpui `Rgba` (straight alpha, 0..=1).
pub fn to_rgba(c: SnapshotRgba) -> Rgba {
    Rgba {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: f32::from(c.a) / 255.0,
    }
}

/// Convert directly into `Hsla` for gpui text / background APIs.
pub fn to_hsla(c: SnapshotRgba) -> Hsla {
    Hsla::from(to_rgba(c))
}

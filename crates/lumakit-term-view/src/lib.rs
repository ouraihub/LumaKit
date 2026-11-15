//! `lumakit-term-view`: a GPUI `Element` wrapper that paints a
//! `lumakit_term::Frame`, plus input/IME/mouse helpers. The host supplies
//! metrics + palette + overlays; the view never reads product-level state.
//!
//! T1.18 landed the paint pipeline; T1.19 adds `input::encode_key` /
//! `input::replace_composition` / `mouse::encode_press` / `mouse::hit_test`
//! so the host can translate key, IME, and pointer events into VT bytes.

#![warn(missing_docs)]

pub mod boxdraw;
mod element;
mod input;
mod mouse;
mod overlay;
mod paint;
mod palette;

pub use element::{TermElement, TermPrepaint};
pub use input::{encode_key, replace_composition, utf16_byte};
pub use mouse::{
    Cell, Modifiers, MouseButton, Side, ViewPoint, WheelDirection, encode_press, encode_wheel,
    hit_test, side,
};
pub use overlay::{CellRange, GhostText, HighlightBand, Overlays};
pub use paint::{Metrics, paint_frame};
pub use palette::{to_hsla, to_rgba};

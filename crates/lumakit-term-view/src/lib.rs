//! `lumakit-term-view`: a GPUI `Element` wrapper that paints a
//! `lumakit_term::Frame`. The host supplies metrics + palette + overlays; the
//! view never reads product-level state.
//!
//! T1.18 landed the paint pipeline (segments, backgrounds, cursor, IME
//! composition, overlay bands). Input / IME / mouse arrive with T1.19.

#![warn(missing_docs)]

pub mod boxdraw;
mod element;
mod overlay;
mod paint;
mod palette;

pub use element::{TermElement, TermPrepaint};
pub use overlay::{CellRange, GhostText, HighlightBand, Overlays};
pub use paint::{Metrics, paint_frame};
pub use palette::{to_hsla, to_rgba};

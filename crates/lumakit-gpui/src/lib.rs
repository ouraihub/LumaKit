//! GPUI Kit chat adapter. Business state and persistence remain in lumakit-ai; visual tokens,
//! themes and assets come from lumakit-design.
extern crate gpui_kit as gpui;
pub mod chat;
mod capabilities;
pub use capabilities::model_capability_badges;
mod media;
mod markdown;
pub use chat::{ChatPanel, open_media, pick_attachment};

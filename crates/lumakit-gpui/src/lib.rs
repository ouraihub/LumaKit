//! GPUI Kit chat adapter. Business state and persistence remain in lumakit-ai; visual tokens,
//! themes and assets come from lumakit-design.
extern crate gpui_kit as gpui;
mod capabilities;
pub mod chat;
pub use capabilities::model_capability_badges;
mod markdown;
mod media;
pub use chat::{ChatPanel, open_media, pick_attachment};
pub use lumakit_model_picker::PickerAssets;

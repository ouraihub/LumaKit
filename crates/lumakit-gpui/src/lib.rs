//! Shared GPUI model-service and chat presentation. Hosts retain business state and persistence;
//! colors, typography and visual tokens come from lumakit-design.
extern crate gpui_kit as gpui;
#[cfg(feature = "chat")]
mod capabilities;
#[cfg(feature = "chat")]
pub mod chat;
pub mod providers;
#[cfg(feature = "chat")]
pub use capabilities::model_capability_badges;
#[cfg(feature = "chat")]
mod markdown;
#[cfg(feature = "chat")]
mod media;
#[cfg(feature = "chat")]
pub use chat::{ChatPanel, open_media, pick_attachment};
#[cfg(feature = "chat")]
pub use lumakit_model_picker::PickerAssets;

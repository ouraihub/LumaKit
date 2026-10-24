//! 原生 egui 组件；无需 eframe，也不持有产品设置或网络连接。
pub mod chat;
mod chat_media;
pub use chat::{ChatGeometry, ChatPanel};
mod switch;
pub use switch::switch;
mod icons;
pub use icons::install_icon_font;

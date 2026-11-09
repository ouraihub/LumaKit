//! 宿主启动时注册同一份图标字体，避免独立聊天窗口出现私用区方框。
use egui::{Button, FontData, FontDefinitions, FontFamily, FontId, Response, RichText, Ui, Vec2};

/// 将随组件分发的 Phosphor 字体加入宿主字体定义；在 `set_fonts` 前调用一次。
pub fn install_icon_font(fonts: &mut FontDefinitions) {
    fonts.font_data.insert(
        "phosphor".into(),
        FontData::from_static(include_bytes!("../assets/Phosphor.ttf")).into(),
    );
    let fallback = fonts.families.entry(FontFamily::Proportional).or_default();
    if !fallback.iter().any(|name| name == "phosphor") {
        fallback.insert(1.min(fallback.len()), "phosphor".into());
    }
}

pub(crate) fn button(glyph: &str) -> Button<'_> {
    Button::new(RichText::new(glyph).font(FontId::proportional(16.0)))
        .frame(false)
        .min_size(Vec2::splat(28.0))
}

pub(crate) fn action(ui: &mut Ui, enabled: bool, glyph: &str, label: &str) -> Response {
    ui.add_enabled(enabled, button(glyph))
        .on_hover_text(label)
        .accessible_name(label)
}

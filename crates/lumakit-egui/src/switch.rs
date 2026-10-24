//! 带标签的二态开关；只修改传入值，不持有产品配置或保存副作用。
use egui::{Response, Role, Sense, Stroke, StrokeKind, TextStyle, Ui, WidgetInfo, pos2, vec2};

/// 绘制滑动开关；点击标签、鼠标或聚焦后 Space/Enter 均通过同一交互切换。
///
/// 返回值的 changed 表示本次切换；禁用沿用 Ui 状态。多个实例由调用者用 push_id 隔离。
pub fn switch(ui: &mut Ui, checked: &mut bool, label: &str) -> Response {
    let height = 20.0;
    let track_width = 34.0;
    let font = TextStyle::Button.resolve(ui.style());
    // egui 0.37 布局入口需要拥有文本；沿用字体缓存，避免跨 atlas 重建持有失效的 galley。
    // 延后着色，让悬停、焦点和禁用状态使用本帧的交互语义色。
    let text = ui.fonts_mut(|fonts| fonts.layout_delayed_color(label.to_owned(), font, f32::INFINITY));
    let gap = ui.spacing().item_spacing.x;
    let (rect, mut response) = ui.allocate_exact_size(
        vec2(track_width + gap + text.size().x, ui.spacing().interact_size.y.max(24.0).max(text.size().y)), Sense::click());
    // clicked 已包含 Space、Enter 和无障碍激活，不再另处理键盘以免重复切换。
    if response.clicked() { *checked = !*checked; response.mark_changed(); }
    response.widget_info(|| WidgetInfo::selected(Role::Switch, ui.is_enabled(), *checked, label));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let track = egui::Rect::from_min_size(pos2(rect.left(), rect.center().y - height * 0.5), vec2(track_width, height));
        let fill = if *checked { ui.visuals().selection.bg_fill } else { visuals.bg_fill };
        ui.painter().rect(track, height * 0.5, fill, visuals.bg_stroke, StrokeKind::Inside);
        let radius = 8.0;
        let x = if *checked { track.right() - height * 0.5 } else { track.left() + height * 0.5 };
        let thumb = ui.visuals().selection.stroke.color;
        ui.painter().circle_filled(pos2(x, track.center().y), radius, thumb);
        ui.painter().galley(pos2(track.right() + gap, rect.center().y - text.size().y * 0.5), text, visuals.fg_stroke.color);
        if response.has_focus() {
            ui.painter().rect_stroke(track.expand(2.0), height * 0.5 + 2.0,
                Stroke::new(2.0, ui.visuals().selection.bg_fill), StrokeKind::Outside);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Key, Modifiers, PointerButton, RawInput, Rect};

    #[test]
    fn pointer_keyboard_disabled_and_accessible_state() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let mut checked = false;
        let mut draw = |events, enabled| {
            let mut response = None;
            let mut output = ctx.run_ui(RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 120.0))),
                events, ..Default::default()
            }, |ui| {
                response = Some(ui.push_id("switch", |ui| {
                    ui.add_enabled_ui(enabled, |ui| switch(ui, &mut checked, "启用")).inner
                }).inner);
            });
            output.textures_delta.clear();
            (response.expect("开关响应"), checked, output.platform_output.accesskit_update)
        };
        let (response, state, update) = draw(vec![], true);
        assert!(!state);
        assert!(response.rect.height() >= 24.0);
        let semantics = |update: Option<egui::accesskit::TreeUpdate>, checked, disabled| {
            let update = update.expect("无障碍树");
            let node = update.nodes.iter().map(|(_, node)| node)
                .find(|node| node.role() == egui::accesskit::Role::Switch).expect("开关节点");
            assert_eq!(node.label(), Some("启用"));
            assert_eq!(node.toggled(), Some(if checked {
                egui::accesskit::Toggled::True
            } else { egui::accesskit::Toggled::False }));
            assert_eq!(node.is_disabled(), disabled);
        };
        semantics(update, false, false);
        ctx.memory_mut(|memory| memory.request_focus(response.id));
        let key = |key, pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
        let id = response.id;
        let (response, state, update) = draw(vec![key(Key::Space, true)], true);
        assert!(state && response.changed());
        assert_eq!(response.id, id);
        semantics(update, true, false);
        let (response, state, _) = draw(vec![key(Key::Space, false)], true);
        assert!(state && !response.changed());
        let (response, state, _) = draw(vec![key(Key::Enter, true)], true);
        assert!(!state && response.changed());
        let (response, state, _) = draw(vec![key(Key::Enter, false)], true);
        assert!(!state && !response.changed());
        let (response, state, update) = draw(vec![key(Key::Space, true)], false);
        assert!(!state && !response.changed() && !response.enabled());
        semantics(update, false, true);
        draw(vec![key(Key::Space, false)], true);
        // 点击标签区域，而非仅验证胶囊本体可点击。
        let pos = pos2(response.rect.right() - 2.0, response.rect.center().y);
        let pointer = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        draw(vec![Event::PointerMoved(pos), pointer(true)], true);
        let (response, state, _) = draw(vec![pointer(false)], true);
        assert!(state && response.changed());
        draw(vec![pointer(true)], false);
        let (response, state, _) = draw(vec![pointer(false)], false);
        assert!(state && !response.changed());
        ctx.memory_mut(|memory| memory.request_focus(response.id));
        let (response, state, _) = draw(vec![key(Key::Space, true), key(Key::Enter, true)], true);
        assert!(!state && response.changed());
    }
}

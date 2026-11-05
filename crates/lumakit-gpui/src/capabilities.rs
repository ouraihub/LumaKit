//! Compact model capability indicators; all consumers pass effective runtime metadata.
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, tooltip::Tooltip};
use gpui_kit::{
    Div, InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement, Styled,
    div,
};
use lumakit_ai::model_config::ModelSettings;
use lumakit_design::{
    Roles,
    metrics::{radius, size, space},
};

/// Renders only declared support; missing/false capability metadata never lights an icon.
/// Badges are read-only and do not change model selection or request settings.
pub fn model_capability_badges(settings: &ModelSettings, r: Roles) -> Div {
    let model = settings.effective_model();
    let mut badges = div().flex().flex_none().items_center().gap(space::S2);
    for (visible, icon, title, foreground, background) in [
        (
            settings.supports_vision,
            IconName::Eye,
            "视觉输入",
            r.feedback_success,
            r.feedback_success_subtle,
        ),
        (
            settings.supports_audio,
            IconName::Ear,
            "音频输入",
            r.feedback_info,
            r.feedback_info_subtle,
        ),
        (
            settings.supports_video,
            IconName::Video,
            "视频输入",
            r.feedback_success,
            r.feedback_success_subtle,
        ),
        (
            model.supports_reasoning(),
            IconName::Brain,
            "推理",
            r.feedback_info,
            r.feedback_info_subtle,
        ),
        (
            settings.supports_tools == Some(true),
            IconName::Wrench,
            "工具调用",
            r.feedback_warning,
            r.feedback_warning_subtle,
        ),
        (
            settings.native_web_search,
            IconName::Globe,
            "原生联网",
            r.primary,
            r.primary_subtle,
        ),
    ] {
        if visible {
            badges = badges.child(
                div()
                    .id(title)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(space::S4)
                    .py(space::S2)
                    .rounded(radius::LG)
                    .bg(background)
                    .text_color(foreground)
                    .role(Role::Image)
                    .aria_label(title)
                    .tooltip(move |window, cx| Tooltip::new(title).build(window, cx))
                    .child(Icon::new(icon).size(size::ICON_SMALL))
                    .into_any_element(),
            );
        }
    }
    badges
}

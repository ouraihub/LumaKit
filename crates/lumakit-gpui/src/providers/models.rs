//! Model toolbar, groups and rows. Hosts supply actual model metadata and intent controls.
use crate::localization::text as tr;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::{AnyElement, Div, ParentElement, Styled, div, px};
use lumakit_design::{
    Roles,
    components::label,
    metrics::{radius, size, space, text},
};

/// Model section actions in the reference order, wrapping instead of obscuring controls.
pub fn models_toolbar(
    fold: AnyElement,
    search: Option<AnyElement>,
    filter: AnyElement,
    sync: AnyElement,
    add: AnyElement,
) -> Div {
    h_flex()
        .w_full()
        .min_w(px(0.))
        .flex_wrap()
        .gap(space::S4)
        .child(label(text::TITLE_SECTION, tr("模型")))
        .child(fold)
        .children(search.map(|search| {
            div()
                .w(lumakit_design::components::settings::INLINE_SEARCH)
                .max_w_full()
                .min_w(px(0.))
                .child(search)
        }))
        .child(filter)
        .child(div().flex_1())
        .child(sync)
        .child(add)
}

/// The one bordered model list shared by the two hosts.
pub fn model_list(rows: impl IntoIterator<Item = AnyElement>, r: &Roles) -> Div {
    v_flex()
        .w_full()
        .min_w(px(0.))
        .rounded(radius::LG)
        .border_1()
        .border_color(r.border_muted)
        .overflow_hidden()
        .children(rows)
}

/// Group header with an accessible host toggle and a remove action.
pub fn model_group(toggle: AnyElement, remove: AnyElement, first: bool, r: &Roles) -> Div {
    let row = h_flex()
        .min_w(px(0.))
        .min_h(size::SETTINGS_ROW)
        .px(space::S12)
        .gap(space::S8)
        .bg(r.container_elevated_bg)
        .child(toggle)
        .child(remove);
    if first {
        row
    } else {
        row.border_t_1().border_color(r.border_muted)
    }
}

/// Model content and trailing actions. Host controls retain their true selection semantics.
pub fn model_row(
    content: AnyElement,
    actions: impl IntoIterator<Item = AnyElement>,
    selected: bool,
    r: &Roles,
) -> Div {
    let row = h_flex()
        .min_w(px(0.))
        .min_h(size::SETTINGS_ROW)
        .px(space::S12)
        .gap(space::S8)
        .border_t_1()
        .border_color(r.border_muted)
        .child(content)
        .children(actions);
    if selected {
        row.bg(r.control_bg_selected)
    } else {
        row
    }
}

/// One discovered model, its type/selection metadata and add/remove action.
pub fn picker_row(content: AnyElement, action: AnyElement, selected: bool, r: &Roles) -> Div {
    let row = h_flex()
        .min_w(px(0.))
        .min_h(size::SETTINGS_ROW)
        .px(space::S8)
        .gap(space::S8)
        .border_b_1()
        .border_color(r.border_muted)
        .child(content)
        .child(action);
    if selected {
        row.bg(r.control_bg_selected)
    } else {
        row
    }
}

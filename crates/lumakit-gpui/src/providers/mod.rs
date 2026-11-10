//! Model-service presentation shared by GPUI hosts. Hosts own configuration, requests and secrets;
//! this module owns layout and accepts explicit semantic colors, controls and intent handlers.
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::{AnyElement, Div, ParentElement, SharedString, Styled, div};
use lumakit_design::{
    Roles,
    components::{label, settings},
    metrics::{size, space, text},
};

mod forms;
mod models;
mod rail;
pub use forms::*;
pub use models::*;
pub use rail::*;

/// Provider rail beside the detail on wide windows; stacked on narrow windows.
pub fn page(rail: AnyElement, detail: AnyElement, wide: bool) -> Div {
    let rail = div().flex_none().min_w(gpui_kit::px(0.)).child(rail);
    let rail = if wide {
        rail.w(size::LIST_COLUMN)
    } else {
        rail.w_full()
    };
    let body = div()
        .flex()
        .w_full()
        .min_w(gpui_kit::px(0.))
        .gap(space::S16)
        .child(rail)
        .child(detail);
    if wide {
        body.flex_row().items_start()
    } else {
        body.flex_col()
    }
}

/// Selected provider name, settings action and independent enable control.
pub fn detail_header(
    name: impl Into<SharedString>,
    settings: AnyElement,
    enabled: AnyElement,
) -> Div {
    h_flex()
        .min_w(gpui_kit::px(0.))
        .gap(space::S8)
        .child(
            label(text::HEADING_SM, name)
                .min_w(gpui_kit::px(0.))
                .truncate(),
        )
        .child(settings)
        .child(div().flex_1())
        .child(enabled)
}

/// Stable detail hierarchy, retained while asynchronous work is in flight.
pub fn detail(
    header: AnyElement,
    failure: Option<AnyElement>,
    key: AnyElement,
    strategy: AnyElement,
    route: AnyElement,
    models: AnyElement,
) -> Div {
    v_flex()
        .flex_1()
        .w_full()
        .min_w(gpui_kit::px(0.))
        .gap(space::S16)
        .child(header)
        .children(failure)
        .child(key)
        .child(strategy)
        .child(route)
        .child(models)
}

/// Key input or masked value with management and probe actions. No separate first-key save button.
/// Hosts submit Enter/Blur and retain the input until a successful write.
pub fn key_section(
    links: impl IntoIterator<Item = AnyElement>,
    value: AnyElement,
    manage: AnyElement,
    detect: AnyElement,
    result: Option<AnyElement>,
    r: &Roles,
) -> Div {
    v_flex()
        .min_w(gpui_kit::px(0.))
        .gap(space::S6)
        .child(settings::field_title("API 密钥", links, r))
        .child(
            h_flex()
                .min_w(gpui_kit::px(0.))
                .gap(space::S8)
                .flex_wrap()
                .child(
                    gpui_kit::div()
                        .flex_1()
                        .min_w(gpui_kit::px(0.))
                        .child(value),
                )
                .child(manage)
                .child(detect),
        )
        .children(result)
}

/// The actual host key-selection policy; presentation does not invent a shared retry policy.
pub fn strategy(
    value: impl Into<SharedString>,
    description: impl Into<SharedString>,
    r: &Roles,
) -> Div {
    v_flex()
        .gap(space::S4)
        .child(label(text::TITLE_COMPACT, "密钥使用方式").text_color(r.text_muted))
        .child(label(text::UI, value))
        .child(label(text::METADATA, description).text_color(r.text_subtle))
}

/// Route selector, editable base address, settings and an optional failed-write retry action.
pub fn route_section(
    add: Option<AnyElement>,
    picker: Option<AnyElement>,
    input: AnyElement,
    settings: AnyElement,
    retry: Option<AnyElement>,
    hint: Option<AnyElement>,
    r: &Roles,
) -> Div {
    v_flex()
        .min_w(gpui_kit::px(0.))
        .gap(space::S6)
        .child(settings::field_title("API 地址", add, r))
        .children(picker)
        .child(
            h_flex()
                .min_w(gpui_kit::px(0.))
                .w_full()
                .gap(space::S8)
                .child(
                    gpui_kit::div()
                        .flex_1()
                        .min_w(gpui_kit::px(0.))
                        .child(input),
                )
                .child(settings)
                .children(retry),
        )
        .children(hint)
}

/// Empty model list or filtered result, with the common dashed placeholder.
pub fn empty(message: impl Into<SharedString>, r: &Roles) -> AnyElement {
    settings::empty(message, r)
}

//! Common form and dialog content; hosts retain validation, transactions and close policy.
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use lumakit_design::{
    Roles,
    components::{label, settings},
    metrics::{space, text},
};

/// Shared caption/control pairing for every provider editor.
pub fn field(title: impl Into<SharedString>, control: impl IntoElement, r: &Roles) -> Div {
    settings::field(title, control, r)
}

/// Ordered form controls, shared by inline overlays and native dialog surfaces.
pub fn form_fields(fields: impl IntoIterator<Item = AnyElement>) -> Div {
    v_flex()
        .w_full()
        .min_w(px(0.))
        .gap(space::S16)
        .children(fields)
}

/// Fixed title and actions around a scrolling form; no persistence or secret ownership.
pub fn form_body(
    title: impl Into<SharedString>,
    body: AnyElement,
    footer: AnyElement,
    notice: Option<AnyElement>,
    r: &Roles,
) -> Div {
    v_flex()
        .w_full()
        .h_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .gap(space::S16)
        .child(
            label(text::TITLE_DIALOG, title)
                .flex_none()
                .text_color(r.text_default),
        )
        .child(
            div()
                .id("provider-form-scroll")
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
                .overflow_y_scroll()
                .child(body),
        )
        .children(notice)
        .child(div().flex_none().child(footer))
}

/// Destructive action on the left, cancel/save on the right, wrapping on short widths.
pub fn form_footer(
    destructive: Option<AnyElement>,
    cancel: AnyElement,
    save: Option<AnyElement>,
) -> Div {
    h_flex()
        .w_full()
        .flex_wrap()
        .gap(space::S8)
        .children(destructive)
        .child(div().flex_1())
        .child(cancel)
        .children(save)
}

/// Saved keys, host-specific label/value fields, security note and mutation actions.
pub fn key_manager(
    rows: impl IntoIterator<Item = AnyElement>,
    fields: impl IntoIterator<Item = AnyElement>,
    actions: AnyElement,
    note: AnyElement,
) -> Div {
    v_flex()
        .w_full()
        .min_w(px(0.))
        .gap(space::S8)
        .child(note)
        .child(v_flex().min_w(px(0.)).gap(space::S6).children(rows))
        .children(fields)
        .child(actions)
}

/// Compact label, masked value and submit entry, wrapping only when the window requires it.
pub fn key_entry(label: AnyElement, value: AnyElement, submit: AnyElement) -> Div {
    h_flex()
        .w_full()
        .min_w(px(0.))
        .flex_wrap()
        .gap(space::S6)
        .child(
            div()
                .w(lumakit_design::metrics::size::KEY_LABEL)
                .max_w_full()
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(lumakit_design::metrics::size::KEY_LABEL)
                .child(value),
        )
        .child(submit)
}

/// Content-sized dialog body for short forms such as key management.
pub fn compact_form_body(
    title: impl Into<SharedString>,
    body: AnyElement,
    footer: AnyElement,
    notice: Option<AnyElement>,
    r: &Roles,
) -> Div {
    v_flex()
        .w_full()
        .min_w(px(0.))
        .gap(space::S12)
        .child(
            label(text::TITLE_DIALOG, title)
                .flex_none()
                .text_color(r.text_default),
        )
        .child(
            div()
                .id("provider-compact-form-scroll")
                .min_h(px(0.))
                .min_w(px(0.))
                .max_h(lumakit_design::metrics::size::CONTENT_MAX / 2.)
                .overflow_y_scroll()
                .child(body),
        )
        .children(notice)
        .child(div().flex_none().child(footer))
}

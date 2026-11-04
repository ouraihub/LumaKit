//! Top-bar pieces: the status dot and the location breadcrumb.

use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, base::h_flex, div,
    prelude::FluentBuilder as _,
};
use gpui_kit::{assets::IconName, component::Icon};

use super::{Handler, label, roles};
use crate::metrics::{radius, size, space, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Success,
    Warning,
    Error,
    Info,
    Idle,
}

/// An 8 px status dot plus optional label (e.g. the top bar's connection state).
#[derive(IntoElement)]
pub struct StatusDot {
    status: Status,
    label: Option<SharedString>,
}

impl StatusDot {
    pub fn new(status: Status) -> Self {
        Self {
            status,
            label: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let color = match self.status {
            Status::Success => r.feedback_success,
            Status::Warning => r.feedback_warning,
            Status::Error => r.feedback_error,
            Status::Info => r.feedback_info,
            Status::Idle => r.text_subtle,
        };
        h_flex()
            .gap(space::S6)
            .child(
                div()
                    .size(size::STATUS_DOT)
                    .flex_none()
                    .rounded_full()
                    .bg(color),
            )
            .when_some(self.label, |d, text| {
                d.child(label(text::METADATA, text).text_color(r.text_muted))
            })
    }
}

/// A top-bar location selector: eyebrow caption above a value with a chevron.
#[derive(IntoElement)]
pub struct Breadcrumb {
    id: ElementId,
    eyebrow: SharedString,
    value: SharedString,
    icon: Option<IconName>,
    mono: bool,
    on_click: Option<Handler>,
}

impl Breadcrumb {
    pub fn new(
        id: impl Into<ElementId>,
        eyebrow: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            eyebrow: eyebrow.into(),
            value: value.into(),
            icon: None,
            mono: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Render the value in the code face (branch names).
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Breadcrumb {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let value_style = if self.mono {
            text::CODE_TEXT
        } else {
            text::ACTION
        };
        div()
            .id(self.id)
            .occlude()
            .flex()
            .flex_col()
            .justify_center()
            .px(space::S8)
            .h_full()
            .rounded(radius::SM)
            .cursor_pointer()
            .hover(|s| s.bg(r.control_bg_hovered))
            .when_some(self.on_click, |d, handler| d.on_click(handler))
            .child(label(text::CAPTION, self.eyebrow).text_color(r.text_subtle))
            .child(
                h_flex()
                    .gap(space::S4)
                    .when_some(self.icon, |d, icon| {
                        d.child(
                            Icon::new(icon)
                                .size(size::ICON_SMALL)
                                .text_color(r.text_muted),
                        )
                    })
                    .child(
                        label(value_style, self.value)
                            .text_color(r.text_default)
                            .truncate(),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(size::ICON_SMALL)
                            .text_color(r.text_subtle),
                    ),
            )
    }
}

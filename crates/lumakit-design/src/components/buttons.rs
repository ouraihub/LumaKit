//! Icon-only buttons: the header icon button and the activity-bar item.

use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_kit::{assets::IconName, component::Icon};

use super::{Handler, roles};
use crate::metrics::{radius, size};

/// A square icon button (24 px in headers). Disabled buttons paint muted and ignore clicks.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    selected: bool,
    disabled: bool,
    tooltip: Option<SharedString>,
    on_click: Option<Handler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            selected: false,
            disabled: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Accessible name, shown as the hover tooltip.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
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

impl RenderOnce for IconButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let color = if self.disabled {
            r.control_disabled_text
        } else if self.selected {
            r.primary
        } else {
            r.text_muted
        };
        let tooltip = self.tooltip.clone();
        div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .size(size::HEADER_CONTROL)
            .flex_none()
            .rounded(radius::SM)
            .text_color(color)
            .when(self.selected, |d| d.bg(r.control_bg_selected))
            .when(!self.disabled, |d| {
                d.cursor_pointer().hover(|s| s.bg(r.control_bg_hovered))
            })
            .when_some(tooltip, |d, tip| {
                d.aria_label(tip.clone()).tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
            })
            .when_some(self.on_click.filter(|_| !self.disabled), |d, handler| {
                d.on_click(handler)
            })
            .child(Icon::new(self.icon).size(size::ICON).text_color(color))
    }
}

/// One activity-bar button: `control-bg-selected` fill and a `primary` left bar when active.
#[derive(IntoElement)]
pub struct ActivityItem {
    id: ElementId,
    icon: IconName,
    tooltip: SharedString,
    active: bool,
    on_click: Option<Handler>,
}

impl ActivityItem {
    pub fn new(id: impl Into<ElementId>, icon: IconName, tooltip: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            tooltip: tooltip.into(),
            active: false,
            on_click: None,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
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

impl RenderOnce for ActivityItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let color = if self.active {
            r.text_default
        } else {
            r.text_muted
        };
        let tooltip = self.tooltip;
        div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .w(size::ACTIVITY_BAR)
            .h(size::ACTIVITY_BAR)
            .cursor_pointer()
            .text_color(color)
            .aria_label(tooltip.clone())
            .when(self.active, |d| d.bg(r.control_bg_selected))
            .when(!self.active, |d| d.hover(|s| s.bg(r.control_bg_hovered)))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            })
            .when_some(self.on_click, |d, handler| d.on_click(handler))
            .when(self.active, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(size::ACCENT_BAR)
                        .bg(r.primary),
                )
            })
            .child(Icon::new(self.icon).size(size::ICON).text_color(color))
    }
}

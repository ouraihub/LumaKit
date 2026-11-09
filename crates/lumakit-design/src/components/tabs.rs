//! One-of-many selectors: tabs, the tab strip and the segmented choice.

use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, FocusHandle, InteractiveElement, IntoElement,
    Orientation, ParentElement, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled,
    TestSupportExt as _, Toggled, Window, base::h_flex, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::{assets::IconName, component::Icon};

use super::{Handler, SelectHandler, label, roles};
use crate::metrics::{radius, size, space, text};

/// One tab of a [`TabStrip`]. Exposed as a focusable tab (Tab to reach it, Enter / Space to
/// activate) that reports whether it is selected; its close button is a separate, named button.
#[derive(IntoElement)]
pub struct Tab {
    id: ElementId,
    icon: Option<IconName>,
    label: SharedString,
    active: bool,
    on_click: Option<Handler>,
    on_close: Option<Handler>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon: None,
            label: label.into(),
            active: false,
            on_click: None,
            on_close: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
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

    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let color = if self.active {
            r.text_default
        } else {
            r.text_muted
        };
        let close_id = ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "close".into());
        // Kept across frames so focus survives the re-render an activation causes.
        let focus: FocusHandle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let close_label: SharedString = format!("关闭 {}", self.label).into();
        h_flex()
            .id(self.id)
            .test_support()
            .role(Role::Tab)
            .aria_label(self.label.clone())
            .aria_selected(self.active)
            .track_focus(&focus)
            .h_full()
            .gap(space::S6)
            .px(space::S8)
            .flex_none()
            .cursor_pointer()
            .border_r_1()
            .border_color(r.border_muted)
            .text_color(color)
            .when(self.active, |d| d.bg(r.container_workspace_bg))
            .when(!self.active, |d| d.hover(|s| s.bg(r.control_bg_hovered)))
            // Keyboard focus tints the tab (a border would shift its label).
            .focus_visible(|s| s.bg(r.primary_subtle))
            .when_some(self.on_click, |d, handler| d.on_click(handler))
            .when_some(self.icon, |d, icon| {
                d.child(Icon::new(icon).size(size::ICON_SMALL))
            })
            .child(label(text::ACTION, self.label))
            .when_some(self.on_close, |d, handler| {
                d.child(
                    div()
                        .id(close_id)
                        .test_support()
                        .role(Role::Button)
                        .aria_label(close_label)
                        .rounded(radius::XS)
                        .hover(|s| s.bg(r.control_bg_hovered))
                        // The tab itself is clickable: without this the click also selects the
                        // tab being closed.
                        .on_click(move |event, window, cx| {
                            cx.stop_propagation();
                            handler(event, window, cx);
                        })
                        .child(
                            Icon::new(IconName::X)
                                .size(size::ICON_SMALL)
                                .text_color(r.text_subtle),
                        ),
                )
            })
    }
}

/// The 28 px tab row with trailing actions. The tabs form a tab list; the actions sit outside it.
#[derive(IntoElement)]
pub struct TabStrip {
    tabs: Vec<AnyElement>,
    actions: Vec<AnyElement>,
}

impl TabStrip {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            actions: Vec::new(),
        }
    }

    pub fn tab(mut self, tab: Tab) -> Self {
        self.tabs.push(tab.into_any_element());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl Default for TabStrip {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for TabStrip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        h_flex()
            .h(size::TAB_STRIP)
            .flex_none()
            .bg(r.container_header_bg)
            .border_b_1()
            .border_color(r.border_muted)
            .child(
                h_flex()
                    .id("tab-list")
                    .role(Role::TabList)
                    .aria_orientation(Orientation::Horizontal)
                    .h_full()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .children(self.tabs),
            )
            .child(h_flex().gap(space::S2).px(space::S4).children(self.actions))
    }
}

/// A segmented choice (e.g. List / Tree). Exactly one option is active.
///
/// Exposed as a radio group: each option is a focusable radio button (Tab to reach it,
/// Enter / Space to pick it) that reports its checked state and position.
#[derive(IntoElement)]
pub struct Segment {
    id: ElementId,
    options: Vec<SharedString>,
    active: usize,
    label: Option<SharedString>,
    on_select: Option<SelectHandler>,
}

impl Segment {
    pub fn new(
        id: impl Into<ElementId>,
        options: impl IntoIterator<Item = impl Into<SharedString>>,
        active: usize,
    ) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().map(Into::into).collect(),
            active,
            label: None,
            on_select: None,
        }
    }

    /// The group's accessible name (e.g. the row title it sits in).
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Segment {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let handler = self.on_select.map(std::rc::Rc::new);
        let base = std::sync::Arc::new(self.id.clone());
        let count = self.options.len();
        h_flex()
            .id(self.id)
            .role(Role::RadioGroup)
            .aria_orientation(Orientation::Horizontal)
            .when_some(self.label, |d, label| d.aria_label(label))
            .h(size::HEADER_CONTROL)
            .p(space::S2)
            .gap(space::S2)
            .rounded(radius::SM)
            .bg(r.control_bg)
            .children(self.options.into_iter().enumerate().map(|(index, option)| {
                let active = index == self.active;
                let handler = handler.clone();
                let id = ElementId::NamedChild(base.clone(), index.to_string().into());
                // Kept across frames so focus survives the re-render a selection causes.
                let focus: FocusHandle = window
                    .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
                    .read(cx)
                    .clone();
                div()
                    .id(id)
                    .test_support()
                    .role(Role::RadioButton)
                    // Assistive technology reads either state; Kit's Radio states both too.
                    .aria_toggled(if active {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .aria_selected(active)
                    .aria_label(option.clone())
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(count)
                    .track_focus(&focus)
                    .h_full()
                    .flex()
                    .items_center()
                    .px(space::S6)
                    .rounded(radius::XS)
                    .cursor_pointer()
                    .text_color(if active { r.text_default } else { r.text_muted })
                    .when(active, |d| d.bg(r.control_bg_selected))
                    .when(!active, |d| d.hover(|s| s.bg(r.control_bg_hovered)))
                    // The border always exists so the focus ring never shifts the label.
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .focus_visible(|s| s.border_color(r.primary_muted))
                    // Picking the current option changes nothing, like a radio button.
                    .when_some(handler.filter(|_| !active), move |d, handler| {
                        d.on_click(move |_, window, cx| handler(&index, window, cx))
                    })
                    .child(label(text::ACTION, option))
            }))
    }
}

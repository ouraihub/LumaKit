//! Panel chrome: the panel header and the tree/list row.

use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, base::h_flex, div,
    prelude::FluentBuilder as _, px,
};
use gpui_kit::{assets::IconName, component::Icon};

use super::{Handler, label, roles};
use crate::metrics::{radius, size, space, text};

/// The 28 px header of a panel: a title on the left, action buttons on the right.
#[derive(IntoElement)]
pub struct PanelHeader {
    title: SharedString,
    actions: Vec<AnyElement>,
}

impl PanelHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            actions: Vec::new(),
        }
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl RenderOnce for PanelHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        h_flex()
            .h(size::PANEL_HEADER)
            .flex_none()
            .px(size::PANEL_PADDING)
            .gap(space::S4)
            .border_b_1()
            .border_color(r.border_muted)
            .child(
                label(text::TITLE_COMPACT, self.title)
                    .flex_1()
                    .text_color(r.text_default)
                    .truncate(),
            )
            .children(self.actions)
    }
}

/// One 22 px row of a tree or list: optional chevron, icon, label, trailing slot.
#[derive(IntoElement)]
pub struct TreeRow {
    id: ElementId,
    depth: usize,
    icon: Option<IconName>,
    label: SharedString,
    detail: Option<SharedString>,
    expanded: Option<bool>,
    selected: bool,
    trailing: Vec<AnyElement>,
    on_click: Option<Handler>,
}

impl TreeRow {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            depth: 0,
            icon: None,
            label: label.into(),
            detail: None,
            expanded: None,
            selected: false,
            trailing: Vec::new(),
            on_click: None,
        }
    }

    pub fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Secondary line (e.g. a workspace's branch), in `text-subtle`.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// `Some` shows a chevron (expandable row).
    pub fn expanded(mut self, expanded: Option<bool>) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
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

impl RenderOnce for TreeRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let name_color = if self.selected {
            r.primary
        } else {
            r.text_default
        };
        let indent = size::TREE_INDENT * self.depth as f32;
        let chevron = self.expanded.map(|open| {
            if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            }
        });
        let main = h_flex()
            .h(size::ROW)
            .gap(space::S4)
            .pl(indent + space::S4)
            .pr(space::S4)
            .child(
                div()
                    .w(size::ICON_SMALL)
                    .flex_none()
                    .when_some(chevron, |d, icon| {
                        d.child(
                            Icon::new(icon)
                                .size(size::ICON_SMALL)
                                .text_color(r.text_subtle),
                        )
                    }),
            )
            .when_some(self.icon, |d, icon| {
                d.child(Icon::new(icon).size(size::ICON).text_color(r.text_muted))
            })
            .child(
                label(text::UI, self.label)
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(name_color),
            )
            .children(self.trailing);
        let detail = self.detail.map(|detail| {
            let left = indent + space::S4 + size::ICON_SMALL + space::S4 + size::ICON + space::S4;
            label(text::METADATA, detail)
                .pl(left)
                .pb(space::S2)
                .truncate()
                .text_color(r.text_subtle)
        });
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .rounded(radius::SM)
            .cursor_pointer()
            .when(self.selected, |d| d.bg(r.control_bg_selected))
            .when(!self.selected, |d| d.hover(|s| s.bg(r.control_bg_hovered)))
            .when_some(self.on_click, |d, handler| d.on_click(handler))
            .child(main)
            .children(detail)
    }
}

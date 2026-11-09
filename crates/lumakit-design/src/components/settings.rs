//! Settings-page building blocks, extracted from ThinkRail Native's settings panels: the grouped
//! navigation column, groups, rows and fields, link labels, avatars, empty states, notices, the
//! dropdown select and the saved-key controls. Stateless: callers pass [`Roles`] and their own
//! handlers; nothing here owns settings state.

use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    assets::IconName,
    base::{h_flex, v_flex},
    component::{
        Icon, Sizable as _,
        button::{Button, ButtonVariants as _},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use super::label;
use crate::{
    Roles,
    metrics::{radius, size, space, text},
};

mod keys;
pub use keys::{KeyManager, KeyRow, delete_confirm, key_manage_button};

/// Width of the settings navigation column.
pub const NAV_WIDTH: gpui_kit::Pixels = px(192.);
/// Height of a navigation row and a section header in it.
pub const NAV_ROW: gpui_kit::Pixels = px(28.);
/// Side of an [`avatar`].
pub const AVATAR: gpui_kit::Pixels = px(24.);
/// Width of a compact inline search box (a list toolbar's).
pub const INLINE_SEARCH: gpui_kit::Pixels = px(160.);

/// The settings layout: the navigation column left, the scrolling page right.
pub fn shell(nav: impl IntoElement, page: impl IntoElement) -> gpui_kit::Div {
    h_flex()
        .size_full()
        .min_h(px(0.))
        .items_start()
        .child(nav)
        .child(content(page))
}

/// The narrow-window settings layout: a wrapping [`nav_row`] above the scrolling page.
pub fn stacked(nav: impl IntoElement, page: impl IntoElement) -> gpui_kit::Div {
    v_flex()
        .size_full()
        .min_h(px(0.))
        .child(nav)
        .child(content(page))
}

/// The scrolling page area both layouts share.
fn content(page: impl IntoElement) -> gpui_kit::Stateful<gpui_kit::Div> {
    v_flex()
        .id("settings-content")
        .flex_1()
        .h_full()
        .w_full()
        .min_w(px(0.))
        .min_h(px(0.))
        .overflow_y_scroll()
        .p(space::S16)
        .child(page)
}

/// The navigation column; fill it with [`nav_section`] and [`nav_item`].
pub fn nav(r: &Roles) -> gpui_kit::Div {
    v_flex()
        .w(NAV_WIDTH)
        .h_full()
        .flex_none()
        .gap(space::S4)
        .p(space::S12)
        .bg(r.container_elevated_bg)
        .border_r_1()
        .border_color(r.border_muted)
}

/// [`nav`] for [`stacked`]: the same items in a wrapping row with a rule below.
pub fn nav_row(r: &Roles) -> gpui_kit::Div {
    h_flex()
        .w_full()
        .flex_none()
        .flex_wrap()
        .gap(space::S4)
        .p(space::S8)
        .bg(r.container_elevated_bg)
        .border_b_1()
        .border_color(r.border_muted)
}

/// An uppercase section header in the navigation column. `first` drops the top gap.
pub fn nav_section(title: impl Into<SharedString>, first: bool, r: &Roles) -> AnyElement {
    label(text::LABEL_PILL, title)
        .h(NAV_ROW)
        .flex()
        .items_center()
        .when(!first, |d| d.mt(space::S8))
        .text_color(r.text_subtle)
        .into_any_element()
}

/// One navigation row: icon and title, the primary tint when `active`.
pub fn nav_item(
    id: impl Into<ElementId>,
    icon: IconName,
    title: impl Into<SharedString>,
    active: bool,
    r: &Roles,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let title = title.into();
    h_flex()
        .id(id)
        .h(NAV_ROW)
        .px(space::S8)
        .gap(space::S8)
        .rounded(radius::SM)
        .role(Role::Button)
        .aria_label(title.clone())
        .aria_selected(active)
        .when(active, |d| d.bg(r.primary_subtle))
        .when(!active, |d| {
            d.cursor_pointer().hover(|s| s.bg(r.control_bg_hovered))
        })
        .on_click(on_click)
        .child(
            Icon::new(icon)
                .size(size::ICON_SMALL)
                .text_color(if active { r.primary } else { r.text_muted }),
        )
        .child(label(text::UI, title).text_color(if active { r.primary } else { r.text_default }))
        .into_any_element()
}

/// A page title with an optional description, as the first child of a page.
pub fn page_header(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    r: &Roles,
) -> AnyElement {
    v_flex()
        .gap(space::S4)
        .child(label(text::HEADING_SM, title))
        .children(description.map(|d| label(text::METADATA, d).text_color(r.text_muted)))
        .into_any_element()
}

/// A titled group: an eyebrow title, an optional description, then its rows.
pub fn group(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    r: &Roles,
) -> gpui_kit::Div {
    v_flex()
        .child(label(text::EYEBROW, title).text_color(r.text_subtle))
        .children(description.map(|d| {
            label(text::METADATA, d)
                .text_color(r.text_muted)
                .pt(space::S4)
        }))
}

/// One setting: title (and description) left, control right, a rule below.
pub fn row(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    control: impl IntoElement,
    r: &Roles,
) -> AnyElement {
    h_flex()
        .gap(space::S16)
        .py(space::S12)
        .border_b_1()
        .border_color(r.border_muted)
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(space::S2)
                .child(label(text::UI, title))
                .children(description.map(|d| label(text::METADATA, d).text_color(r.text_subtle))),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// A labeled form field: a compact muted title above the control.
pub fn field(
    title: impl Into<SharedString>,
    control: impl IntoElement,
    r: &Roles,
) -> gpui_kit::Div {
    v_flex()
        .gap(space::S4)
        .child(label(text::TITLE_COMPACT, title).text_color(r.text_muted))
        .child(control)
}

/// A field title followed by inline link actions, as in `API 密钥 · 获取密钥`.
pub fn field_title(
    title: impl Into<SharedString>,
    links: impl IntoIterator<Item = AnyElement>,
    r: &Roles,
) -> gpui_kit::Div {
    h_flex()
        .gap(space::S8)
        .child(label(text::TITLE_COMPACT, title).text_color(r.text_muted))
        .children(links)
}

/// A small link-styled action for [`field_title`].
pub fn link(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Button {
    Button::new(id).link().xsmall().label(title)
}

/// A read-only value block: a compact title, the value, a subtle description.
pub fn value(
    title: impl Into<SharedString>,
    value: impl Into<SharedString>,
    description: Option<SharedString>,
    r: &Roles,
) -> AnyElement {
    v_flex()
        .gap(space::S4)
        .child(label(text::TITLE_COMPACT, title).text_color(r.text_muted))
        .child(label(text::UI, value))
        .children(description.map(|d| label(text::METADATA, d).text_color(r.text_subtle)))
        .into_any_element()
}

/// The request a setting will make, as secondary monospaced text (`GET …/items`).
pub fn request_line(content: impl Into<SharedString>, r: &Roles) -> AnyElement {
    label(text::CODE_SMALL, content)
        .text_color(r.text_subtle)
        .into_any_element()
}

/// A square with a name's first character, tinted with the primary color.
pub fn avatar(name: &str, r: &Roles) -> AnyElement {
    let initial: String = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    div()
        .size(AVATAR)
        .flex_none()
        .rounded(radius::MD)
        .bg(r.primary_subtle)
        .flex()
        .items_center()
        .justify_center()
        .child(label(text::EMPHASIS, initial).text_color(r.primary))
        .into_any_element()
}

/// A list's dashed empty state.
pub fn empty(message: impl Into<SharedString>, r: &Roles) -> AnyElement {
    div()
        .py(space::S24)
        .rounded(radius::MD)
        .border_1()
        .border_dashed()
        .border_color(r.border_default)
        .flex()
        .justify_center()
        .child(label(text::METADATA, message).text_color(r.text_subtle))
        .into_any_element()
}

/// A centered message for a page or pane with nothing selected.
pub fn placeholder(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    r: &Roles,
) -> AnyElement {
    v_flex()
        .flex_1()
        .items_center()
        .py(space::S40)
        .gap(space::S4)
        .child(label(text::TITLE_SECTION, title))
        .child(label(text::METADATA, description).text_color(r.text_subtle))
        .into_any_element()
}

/// How a [`notice`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Warning,
    Error,
}

/// A tinted information, warning or error line.
pub fn notice(content: impl Into<SharedString>, tone: Tone, r: &Roles) -> AnyElement {
    let (bg, fg, icon) = match tone {
        Tone::Info => (r.primary_subtle, r.primary, IconName::Info),
        Tone::Warning => (
            r.feedback_warning_subtle,
            r.feedback_warning,
            IconName::TriangleAlert,
        ),
        Tone::Error => (
            r.feedback_error_subtle,
            r.feedback_error,
            IconName::CircleAlert,
        ),
    };
    h_flex()
        .gap(space::S8)
        .p(space::S8)
        .rounded(radius::SM)
        .bg(bg)
        .child(Icon::new(icon).size(size::ICON_SMALL).text_color(fg))
        .child(
            label(text::METADATA, content)
                .flex_1()
                .min_w(px(0.))
                .text_color(r.text_default),
        )
        .into_any_element()
}

/// A 6 px status dot for a [`list_row`].
pub fn dot(color: impl Into<gpui_kit::Hsla>) -> AnyElement {
    div()
        .size(px(6.))
        .flex_none()
        .rounded_full()
        .bg(color.into())
        .into_any_element()
}

/// The `···` button that opens a row's menu; attach the menu with Kit's `dropdown_menu`.
pub fn row_menu_button(id: impl Into<ElementId>, tooltip: impl Into<SharedString>) -> Button {
    let tooltip = tooltip.into();
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .accessibility_label(tooltip.clone())
        .tooltip(tooltip)
}

/// The trigger of a [`select`]: the current `value` on the left, a chevron on the right, as in
/// ThinkRail's line dialog. `name` is what the choice is (e.g. 协议) and reaches assistive
/// technology as "name：value". Outlined; callers size it (`.w_full()`, `.small()`, `.ghost()`)
/// and disable it before passing it to [`select`].
pub fn select_button(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Button {
    let value: SharedString = value.into();
    Button::new(id)
        .outline()
        .accessibility_label(format!("{}：{}", name.into(), value))
        .child(
            h_flex()
                .w_full()
                .gap(space::S8)
                .child(label(text::UI, value))
                .child(div().flex_1())
                .child(Icon::new(IconName::ChevronDown).small()),
        )
}

/// A single choice as a dropdown: `trigger` (from [`select_button`]) opens a menu of `options`
/// with `current` checked; picking one calls `on_pick` with its value. Use it for any choice that
/// can grow (enums, user data); a row of buttons cannot hold many options.
pub fn select<T: Copy + PartialEq + 'static>(
    trigger: Button,
    current: Option<T>,
    options: impl IntoIterator<Item = (T, SharedString)>,
    on_pick: impl Fn(T, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let options: Vec<(T, SharedString)> = options.into_iter().collect();
    let on_pick = std::rc::Rc::new(on_pick);
    trigger
        .dropdown_menu(move |menu, _, _| {
            options.iter().fold(menu, |menu, (value, title)| {
                let (value, on_pick) = (*value, on_pick.clone());
                menu.item(
                    PopupMenuItem::new(title.clone())
                        .checked(current == Some(value))
                        .on_click(move |_, window, cx| on_pick(value, window, cx)),
                )
            })
        })
        .into_any_element()
}

/// Height of a [`list_row`].
pub const LIST_ROW: gpui_kit::Pixels = px(36.);

/// A selectable row of a list column: fill it with an [`avatar`], a name, a [`dot`] and a
/// [`row_menu_button`].
pub fn list_row(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    selected: bool,
    r: &Roles,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    h_flex()
        .id(id)
        .h(LIST_ROW)
        .px(space::S8)
        .gap(space::S8)
        .rounded(radius::MD)
        .role(Role::Button)
        .aria_label(name.into())
        .aria_selected(selected)
        .when(selected, |d| d.bg(r.control_bg_selected))
        .when(!selected, |d| {
            d.cursor_pointer().hover(|s| s.bg(r.control_bg_hovered))
        })
}

//! Maps [`Roles`] onto gpui-component's `ThemeColor`, so library controls (inputs, menus,
//! scrollbars, dialogs) paint from the same palette as LumaKit's own components.

use gpui_kit::{Hsla, Rgba, component::ThemeColor};

use crate::theme::Roles;

pub(crate) fn apply(c: &mut ThemeColor, r: &Roles) {
    let h = |color: Rgba| -> Hsla { color.into() };
    let text = h(r.text_default);

    c.background = h(r.container_workspace_bg);
    c.foreground = text;
    c.border = h(r.border_muted);
    c.input = h(r.control_border_default);
    c.ring = h(r.primary_muted);
    c.caret = h(r.primary);
    c.selection = h(r.selection_bg);
    c.overlay = h(r.overlay);
    c.window_border = h(r.border_muted);
    c.drag_border = h(r.primary);
    c.drop_target = h(r.primary_subtle);

    c.muted = h(r.control_bg);
    c.muted_foreground = h(r.text_muted);
    c.accent = h(r.control_bg_hovered);
    c.accent_foreground = text;
    c.secondary = h(r.control_bg);
    c.secondary_hover = h(r.control_bg_hovered);
    c.secondary_active = h(r.control_bg_selected);
    c.secondary_foreground = text;

    c.primary = h(r.control_primary_bg);
    c.primary_hover = h(r.control_primary_bg_hovered);
    c.primary_active = h(r.control_primary_bg_hovered);
    c.primary_foreground = h(r.control_primary_text);

    c.button = h(r.control_bg);
    c.button_hover = h(r.control_bg_hovered);
    c.button_active = h(r.control_bg_selected);
    c.button_foreground = text;
    c.button_primary = h(r.control_primary_bg);
    c.button_primary_hover = h(r.control_primary_bg_hovered);
    c.button_primary_active = h(r.control_primary_bg_hovered);
    c.button_primary_foreground = h(r.control_primary_text);
    c.button_secondary = h(r.control_bg);
    c.button_secondary_hover = h(r.control_bg_hovered);
    c.button_secondary_active = h(r.control_bg_selected);
    c.button_secondary_foreground = text;
    c.button_danger = h(r.control_destructive_bg);
    c.button_danger_hover = h(r.control_destructive_bg_hovered);
    c.button_danger_active = h(r.control_destructive_bg_hovered);
    c.button_danger_foreground = h(r.control_destructive_text);

    c.danger = h(r.feedback_error);
    c.danger_hover = h(r.control_destructive_bg_hovered);
    c.danger_active = h(r.control_destructive_bg_hovered);
    c.danger_foreground = h(r.control_destructive_text);
    c.success = h(r.feedback_success);
    c.success_hover = h(r.feedback_success);
    c.success_active = h(r.feedback_success);
    c.warning = h(r.feedback_warning);
    c.warning_hover = h(r.feedback_warning);
    c.warning_active = h(r.feedback_warning);
    c.info = h(r.feedback_info);
    c.info_hover = h(r.feedback_info);
    c.info_active = h(r.feedback_info);
    c.link = h(r.text_link);
    c.link_hover = h(r.text_link);
    c.link_active = h(r.text_link);

    c.popover = h(r.container_elevated_bg);
    c.popover_foreground = text;
    c.group_box = h(r.container_elevated_bg);
    c.group_box_foreground = text;

    c.list = h(r.container_sidebar_bg);
    c.list_hover = h(r.control_bg_hovered);
    c.list_active = h(r.control_bg_selected);
    c.list_active_border = h(r.primary);
    c.list_even = h(r.container_sidebar_bg);
    c.list_head = h(r.container_header_bg);
    c.table = h(r.container_workspace_bg);
    c.table_hover = h(r.control_bg_hovered);
    c.table_active = h(r.control_bg_selected);
    c.table_active_border = h(r.primary);
    c.table_even = h(r.container_workspace_bg);
    c.table_head = h(r.container_header_bg);
    c.table_head_foreground = h(r.text_muted);
    c.table_row_border = h(r.border_muted);

    c.sidebar = h(r.container_sidebar_bg);
    c.sidebar_foreground = text;
    c.sidebar_border = h(r.border_muted);
    c.sidebar_accent = h(r.control_bg_selected);
    c.sidebar_accent_foreground = h(r.primary);
    c.sidebar_primary = h(r.primary);
    c.sidebar_primary_foreground = h(r.text_on_primary);

    c.tab_bar = h(r.container_header_bg);
    c.tab = h(r.container_header_bg);
    c.tab_foreground = h(r.text_muted);
    c.tab_active = h(r.container_workspace_bg);
    c.tab_active_foreground = text;
    c.title_bar = h(r.container_header_bg);
    c.title_bar_border = h(r.border_muted);
    c.status_bar = h(r.container_header_bg);
    c.status_bar_border = h(r.border_muted);

    c.scrollbar = h(Rgba {
        a: 0.,
        ..r.container_workspace_bg
    });
    c.scrollbar_thumb = h(r.control_border_active);
    c.scrollbar_thumb_hover = h(r.text_subtle);
    c.skeleton = h(r.control_bg);
    c.progress_bar = h(r.primary);
    c.slider_bar = h(r.primary);
    c.slider_thumb = h(r.text_default);
    c.switch = h(r.control_border_active);
    c.switch_thumb = h(r.text_default);
}

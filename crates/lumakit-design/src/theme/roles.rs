//! The semantic roles the original derives from a palette (`apps/web/src/styles/colors.json`).
//! Views paint with [`Roles`] only; a palette value is never used directly.

use gpui_kit::Rgba;

use super::{Appearance, Palette};

/// The `colors.json` alpha ladder, as `color-mix(in srgb, X n%, transparent)` = alpha × n%.
mod alpha {
    pub const SUBTLE: f32 = 0.10;
    pub const WASH: f32 = 0.12;
    pub const SOFT: f32 = 0.20;
    pub const MUTED: f32 = 0.40;
    pub const STRONG: f32 = 0.60;
}

/// The semantic roles of `colors.json`, resolved for one palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roles {
    pub text_default: Rgba,
    pub text_muted: Rgba,
    pub text_subtle: Rgba,
    pub text_disabled: Rgba,
    pub text_on_primary: Rgba,
    pub text_link: Rgba,
    pub container_workspace_bg: Rgba,
    pub container_sidebar_bg: Rgba,
    pub container_terminal_bg: Rgba,
    pub container_header_bg: Rgba,
    pub container_content_bg: Rgba,
    pub container_elevated_bg: Rgba,
    pub control_bg: Rgba,
    pub control_bg_hovered: Rgba,
    pub control_bg_selected: Rgba,
    pub control_primary_bg: Rgba,
    pub control_primary_bg_hovered: Rgba,
    pub control_primary_text: Rgba,
    pub control_border_default: Rgba,
    pub control_border_active: Rgba,
    pub control_disabled_bg: Rgba,
    pub control_disabled_text: Rgba,
    pub control_disabled_border: Rgba,
    pub control_primary_disabled_bg: Rgba,
    pub control_primary_disabled_text: Rgba,
    pub control_destructive_bg: Rgba,
    pub control_soft_primary_bg_hovered: Rgba,
    pub control_destructive_bg_hovered: Rgba,
    pub control_destructive_text: Rgba,
    pub control_destructive_disabled_bg: Rgba,
    pub control_destructive_disabled_text: Rgba,
    pub border_default: Rgba,
    pub border_muted: Rgba,
    pub primary: Rgba,
    pub primary_subtle: Rgba,
    pub primary_soft: Rgba,
    pub primary_muted: Rgba,
    pub on_primary_soft: Rgba,
    pub feedback_info: Rgba,
    pub feedback_info_subtle: Rgba,
    pub feedback_info_muted: Rgba,
    pub feedback_success: Rgba,
    pub feedback_success_subtle: Rgba,
    pub feedback_success_muted: Rgba,
    pub feedback_error: Rgba,
    pub feedback_error_subtle: Rgba,
    pub feedback_error_muted: Rgba,
    pub feedback_warning: Rgba,
    pub feedback_warning_subtle: Rgba,
    pub feedback_warning_muted: Rgba,
    pub bubble_user_bg: Rgba,
    pub bubble_user_border: Rgba,
    /// `None` keeps the text's own color (`inherit`).
    pub selection_text: Option<Rgba>,
    pub selection_bg: Rgba,
    pub selection_bg_muted: Rgba,
    pub editor_selection_bg: Rgba,
    pub editor_selection_text: Option<Rgba>,
    pub editor_selection_highlight_bg: Rgba,
    pub editor_find_match_bg: Rgba,
    pub widget_shadow: Rgba,
    pub sunken: Rgba,
    pub overlay: Rgba,
}

impl Roles {
    pub fn derive(p: &Palette, appearance: Appearance) -> Self {
        let mix = |c: Rgba, f: f32| c.opacity(f);
        let black = |a: f32| Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a,
        };
        Self {
            text_default: p.text,
            text_muted: p.muted,
            text_subtle: p.hint,
            text_disabled: mix(p.text, alpha::STRONG),
            text_on_primary: p.on_accent,
            text_link: p.info,
            container_workspace_bg: p.background,
            container_sidebar_bg: p.sidebar,
            container_terminal_bg: p.sidebar,
            container_header_bg: p.header,
            container_content_bg: p.content,
            container_elevated_bg: p.elevated,
            control_bg: p.input,
            control_bg_hovered: p.hover,
            control_bg_selected: p.hover,
            control_primary_bg: p.accent_solid,
            control_primary_bg_hovered: p.accent_hover,
            control_primary_text: p.on_accent,
            control_border_default: p.border,
            control_border_active: p.border_strong,
            control_disabled_bg: mix(p.input, alpha::STRONG),
            control_disabled_text: mix(p.text, alpha::STRONG),
            control_disabled_border: mix(p.border, alpha::STRONG),
            control_primary_disabled_bg: mix(p.accent, alpha::STRONG),
            control_primary_disabled_text: mix(p.on_accent, alpha::STRONG),
            control_destructive_bg: p.danger,
            control_soft_primary_bg_hovered: mix(p.accent, alpha::WASH),
            control_destructive_bg_hovered: p.danger_hover,
            control_destructive_text: p.on_danger,
            control_destructive_disabled_bg: mix(p.danger, alpha::STRONG),
            control_destructive_disabled_text: mix(p.on_danger, alpha::STRONG),
            border_default: p.border_strong,
            border_muted: p.border,
            primary: p.accent,
            primary_subtle: mix(p.accent, alpha::SUBTLE),
            primary_soft: mix(p.accent, alpha::SOFT),
            primary_muted: mix(p.accent, alpha::MUTED),
            on_primary_soft: mix(p.on_accent, alpha::SOFT),
            feedback_info: p.info,
            feedback_info_subtle: mix(p.info, alpha::WASH),
            feedback_info_muted: mix(p.info, alpha::MUTED),
            feedback_success: p.success,
            feedback_success_subtle: mix(p.success, alpha::WASH),
            feedback_success_muted: mix(p.success, alpha::MUTED),
            feedback_error: p.danger,
            feedback_error_subtle: mix(p.danger, alpha::WASH),
            feedback_error_muted: mix(p.danger, alpha::MUTED),
            feedback_warning: p.warning,
            feedback_warning_subtle: mix(p.warning, alpha::WASH),
            feedback_warning_muted: mix(p.warning, alpha::MUTED),
            bubble_user_bg: mix(p.bubble_accent, alpha::SUBTLE),
            bubble_user_border: mix(p.bubble_accent, alpha::MUTED),
            selection_text: p.selection_foreground,
            selection_bg: p.selection,
            selection_bg_muted: mix(p.selection, alpha::MUTED),
            editor_selection_bg: p.editor_selection,
            editor_selection_text: p.editor_selection_foreground,
            editor_selection_highlight_bg: mix(p.editor_selection, alpha::SOFT),
            editor_find_match_bg: mix(p.warning, alpha::MUTED),
            widget_shadow: mix(p.text, alpha::SOFT),
            sunken: black(if appearance == Appearance::Dark {
                0.12
            } else {
                0.05
            }),
            overlay: black(0.5),
        }
    }
}

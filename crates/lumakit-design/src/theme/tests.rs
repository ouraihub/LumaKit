//! The bundled dark and light palettes and the roles derived from them, pinned to the original.

use gpui_kit::{Rgba, rgba};

use super::*;
use crate::{fonts::face_weight, metrics::text};

fn theme(appearance: Appearance) -> &'static LumaTheme {
    let id = match appearance {
        Appearance::Dark => "dark",
        Appearance::Light => "light",
    };
    ThemeCatalog::bundled().unwrap().get(id).unwrap()
}

#[track_caller]
fn same(actual: Rgba, expected: Rgba) {
    let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
    assert!(
        close(actual.r, expected.r)
            && close(actual.g, expected.g)
            && close(actual.b, expected.b)
            && close(actual.a, expected.a),
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn bundled_themes_parse_with_their_original_identity() {
    assert_eq!(theme(Appearance::Dark).id, "dark");
    assert_eq!(theme(Appearance::Light).id, "light");
    assert_eq!(theme(Appearance::Light).appearance, Appearance::Light);
}

#[test]
fn palette_values_are_the_original_hex_values() {
    let dark = &theme(Appearance::Dark).palette;
    same(dark.accent, rgba(0x8dff4fff));
    same(dark.background, rgba(0x09090bff));
    same(dark.selection, rgba(0x6ac8ff26));
    assert!(dark.selection_foreground.is_none());
    let light = &theme(Appearance::Light).palette;
    same(light.accent, rgba(0x2a7314ff));
    same(light.accent_solid, rgba(0x8dff4fff));
    same(light.text, rgba(0x27272aff));
    same(theme(Appearance::Light).ansi.colors()[15], rgba(0xa5a5a5ff));
}

#[test]
fn roles_follow_the_original_derivations() {
    let dark = &theme(Appearance::Dark).roles;
    same(dark.text_subtle, rgba(0x71717aff));
    same(dark.container_sidebar_bg, rgba(0x101013ff));
    same(dark.container_terminal_bg, rgba(0x101013ff));
    same(dark.border_default, rgba(0x3f3f46ff));
    same(dark.border_muted, rgba(0x27272aff));
    same(dark.control_bg_selected, rgba(0x27272aff));
    same(dark.control_primary_bg, rgba(0x8dff4fff));
    // color-mix(in srgb, X n%, transparent) keeps the hue and scales alpha.
    same(
        dark.text_disabled,
        Rgba {
            a: 0.6,
            ..rgba(0xf4f4f5ff)
        },
    );
    same(
        dark.primary_subtle,
        Rgba {
            a: 0.1,
            ..rgba(0x8dff4fff)
        },
    );
    same(
        dark.feedback_error_subtle,
        Rgba {
            a: 0.12,
            ..rgba(0xff4b75ff)
        },
    );
    same(
        dark.bubble_user_border,
        Rgba {
            a: 0.4,
            ..rgba(0x8dff4fff)
        },
    );
    same(
        dark.selection_bg_muted,
        Rgba {
            a: 0.4 * (0x26 as f32 / 255.),
            ..rgba(0x6ac8ffff)
        },
    );
    same(
        dark.sunken,
        Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.12,
        },
    );

    let light = &theme(Appearance::Light).roles;
    // Light keeps the bright solid primary fill but tints from its own darker accent.
    same(light.control_primary_bg, rgba(0x8dff4fff));
    same(
        light.primary_soft,
        Rgba {
            a: 0.2,
            ..rgba(0x2a7314ff)
        },
    );
    same(
        light.sunken,
        Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.05,
        },
    );
}

#[test]
fn type_styles_render_with_the_nearest_shipped_face() {
    assert_eq!(text::UI.weight.0, 370.);
    assert_eq!(face_weight(&text::UI).0, 400.);
    assert_eq!(face_weight(&text::BUTTON).0, 500.);
    assert_eq!(face_weight(&text::HEADING_SM).0, 600.);
    assert_eq!(face_weight(&text::CODE_TEXT).0, 400.);
}

#[test]
fn an_accent_override_reaches_every_accent_role_and_keeps_text_on_it_readable() {
    let light = theme(Appearance::Light);
    let vermilion = rgba(0xae3224ff);
    let custom = light.with_accent(vermilion);
    for role in [custom.roles.primary, custom.roles.control_primary_bg] { same(role, vermilion); }
    same(custom.roles.primary_subtle, vermilion.opacity(0.10));
    same(custom.roles.control_primary_text, rgba(0xffffffff));
    assert_ne!(custom.roles.control_primary_bg_hovered, vermilion);
    // A light accent on a dark theme gets black text.
    let dark = theme(Appearance::Dark).with_accent(rgba(0xff9c89ff));
    same(dark.roles.control_primary_text, rgba(0x000000ff));
    // Everything not derived from the accent is unchanged.
    same(custom.roles.container_workspace_bg, light.roles.container_workspace_bg);
    same(custom.roles.feedback_error, light.roles.feedback_error);
}

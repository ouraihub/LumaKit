//! Type styles under font settings: defaults unchanged, code size delta, weight offset snapping.

use super::*;

fn resolve_with(settings: Typography, style: &TextStyle) -> ResolvedStyle {
    Active::new(settings).resolve(style)
}

#[test]
fn defaults_render_the_bundled_faces_at_the_original_sizes() {
    let ui = resolve_with(Typography::default(), &text::UI);
    assert_eq!(ui.family.as_ref(), fonts::INTERFACE);
    assert_eq!((ui.size, ui.weight), (px(14.), FontWeight(400.)));
    assert_eq!(ui.line_height, px(14.) * 1.3);
    // The bundled faces have no CJK glyphs; Chinese text names its fallbacks explicitly.
    let cjk = FontFallbacks::from_fonts(CJK_FALLBACKS.map(str::to_owned).to_vec());
    assert_eq!((ui.fallbacks, ui.features), (Some(cjk), None));
    let code = resolve_with(Typography::default(), &text::CODE_TEXT);
    assert_eq!(code.family.as_ref(), fonts::CODE);
    assert_eq!((code.size, code.weight), (px(13.), FontWeight(400.)));
    let brand = resolve_with(Typography::default(), &text::BRAND_HERO);
    assert_eq!(brand.family.as_ref(), fonts::BRAND);
}

#[test]
fn density_scales_every_style_and_its_line_height() {
    let compact = Typography { scale: 13. / 14., ..Typography::default() };
    let ui = resolve_with(compact.clone(), &text::UI);
    assert_eq!(ui.size, px(13.));
    assert_eq!(ui.line_height, px(13.) * 1.3);
    let code = resolve_with(compact, &text::CODE_TEXT);
    assert_eq!(code.size, px(13.) * (13. / 14.));
}

#[test]
fn code_styles_keep_their_distance_from_the_code_size() {
    let settings = Typography {
        code_size: px(16.),
        ..Typography::default()
    };
    let size = |style| resolve_with(settings.clone(), style).size;
    assert_eq!(size(&text::CODE_TEXT), px(16.));
    assert_eq!(size(&text::CODE_DOCUMENT), px(16.));
    assert_eq!(size(&text::CODE_SMALL), px(14.));
    assert_eq!(size(&text::UI), px(14.));
    let line = resolve_with(settings.clone(), &text::CODE_TEXT).line_height;
    assert_eq!(line, px(16.) * 1.4);
}

#[test]
fn the_weight_offset_snaps_for_bundled_faces_and_is_raw_for_named_ones() {
    let offset = |weight_offset, family: &str| Typography {
        weight_offset,
        interface_family: family.to_owned(),
        ..Typography::default()
    };
    let weight = |settings, style| resolve_with(settings, style).weight.0;
    // UI is 370: +30 → 400, +90 → 460 → 500, -70 → 300, +130 → 500 → 600 (nearest; ties go up).
    assert_eq!(weight(offset(30., fonts::INTERFACE), &text::UI), 400.);
    assert_eq!(weight(offset(90., fonts::INTERFACE), &text::UI), 500.);
    assert_eq!(weight(offset(-70., fonts::INTERFACE), &text::UI), 300.);
    assert_eq!(weight(offset(130., fonts::INTERFACE), &text::UI), 500.);
    assert_eq!(
        weight(offset(130., fonts::INTERFACE), &text::HEADING_SM),
        600.
    );
    // A chosen family gets the exact weight.
    assert_eq!(weight(offset(30., "Segoe UI"), &text::UI), 400.);
    assert_eq!(weight(offset(90., "Segoe UI"), &text::UI), 460.);
    assert_eq!(weight(offset(0., "Segoe UI"), &text::UI), 370.);
    // Code follows the offset over JetBrains Mono's 400 / 500; brand never moves.
    assert_eq!(
        weight(offset(60., fonts::INTERFACE), &text::CODE_TEXT),
        500.
    );
    assert_eq!(
        weight(offset(130., fonts::INTERFACE), &text::BRAND_HERO),
        400.
    );
}

#[test]
fn chosen_families_carry_their_fallbacks() {
    let settings = Typography {
        code_family: "Cascadia Code".into(),
        code_fallbacks: vec!["Consolas".into(), fonts::CODE.into()],
        ..Typography::default()
    };
    let code = resolve_with(settings, &text::CODE_SMALL);
    assert_eq!(code.family.as_ref(), "Cascadia Code");
    let fallbacks = code.fallbacks.unwrap();
    assert_eq!(fallbacks.fallback_list(), ["Consolas", fonts::CODE]);
    assert_eq!(code.weight, FontWeight(400.));
}

#[test]
fn code_styles_turn_ligatures_off_unless_enabled() {
    let off = resolve_with(Typography::default(), &text::CODE_TEXT);
    let tags = off.features.unwrap();
    assert_eq!(tags.is_calt_enabled(), Some(false));
    assert!(tags.tag_value_list().contains(&("liga".to_owned(), 0)));
    let on = resolve_with(
        Typography {
            ligatures: true,
            ..Typography::default()
        },
        &text::CODE_TEXT,
    );
    assert_eq!(on.features, Some(FontFeatures::default()));
    assert_eq!(
        resolve_with(Typography::default(), &text::UI).features,
        None
    );
}

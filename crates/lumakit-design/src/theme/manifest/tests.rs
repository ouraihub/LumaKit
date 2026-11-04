//! Strict v2 parsing, against the original schema.ts rules and the bundled files.

use gpui_kit::rgba;
use serde_json::{Value, json};

use super::*;
use crate::theme::{Contrast, ThemeCatalog, catalog::DEFAULT_THEME};

const DARK: &str = include_str!("../../../themes/dark.theme.json");
const MINIMAL_DARK: &str = include_str!("../../../themes/minimal-dark.theme.json");

fn value(json: &str) -> Value {
    serde_json::from_str(json).unwrap()
}

fn parse(value: &Value) -> Result<LumaTheme, ManifestError> {
    LumaTheme::parse(&value.to_string())
}

#[track_caller]
fn issues(value: &Value) -> Vec<String> {
    let error = parse(value).unwrap_err();
    assert_eq!(error.kind, ManifestErrorKind::InvalidTheme);
    error.issues
}

#[test]
fn every_bundled_file_parses_with_its_identity() {
    let catalog = ThemeCatalog::bundled().unwrap();
    let identity: Vec<_> = catalog
        .list()
        .iter()
        .map(|t| {
            (
                t.id.as_str(),
                t.label.as_str(),
                t.order,
                t.appearance,
                t.contrast,
            )
        })
        .collect();
    use {Appearance::*, Contrast::*};
    assert_eq!(
        identity,
        [
            (DEFAULT_THEME, "Dark", 10, Dark, Normal),
            ("light", "Light", 20, Light, Normal),
            ("minimal-dark", "Minimal Dark", 30, Dark, Normal),
            ("minimal-light", "Minimal Light", 40, Light, Normal),
            ("high-contrast-dark", "High Contrast Dark", 50, Dark, High),
            (
                "high-contrast-light",
                "High Contrast Light",
                60,
                Light,
                High
            ),
            ("warm-dark", "Warm Dark", 70, Dark, Normal),
            ("warm-light", "Warm Light", 80, Light, Normal),
        ]
    );
    let dark = catalog.get("dark").unwrap();
    assert_eq!(dark.syntax.keyword, rgba(0xff7b72ff));
}

#[test]
fn unknown_keys_are_rejected_at_every_level() {
    let mut v = value(DARK);
    v["extra"] = json!(1);
    v["colors"]["accentish"] = json!("#000000");
    v["ansi"]["orange"] = json!("#000000");
    v["syntax"]["macro"] = json!("#000000");
    assert_eq!(
        issues(&v),
        [
            "theme.extra is not allowed",
            "theme.colors.accentish is not allowed",
            "theme.ansi.orange is not allowed",
            "theme.syntax.macro is not allowed",
        ]
    );
}

#[test]
fn colors_must_be_canonical_lowercase_hex() {
    for bad in ["#ABCDEF", "#abc", "abcdef", "#abcdeg", "#abcdef1", ""] {
        let mut v = value(DARK);
        v["colors"]["accent"] = json!(bad);
        assert_eq!(
            issues(&v),
            ["theme.colors.accent must be a canonical lowercase #rrggbb or #rrggbbaa color"],
            "{bad}"
        );
    }
    let mut v = value(DARK);
    v["colors"]["selection"] = json!("#6ac8ff26");
    v["ansi"]["red"] = json!("#FF0000");
    assert_eq!(
        issues(&v),
        ["theme.ansi.red must be a canonical lowercase #rrggbb or #rrggbbaa color"]
    );
}

#[test]
fn null_is_allowed_only_for_the_selection_foregrounds() {
    let mut v = value(DARK);
    v["colors"]["selectionForeground"] = Value::Null;
    v["colors"]["editorSelectionForeground"] = json!("#ffffff");
    let theme = parse(&v).unwrap();
    assert_eq!(theme.palette.selection_foreground, None);
    assert_eq!(
        theme.palette.editor_selection_foreground,
        Some(rgba(0xffffffff))
    );
    v["colors"]["accent"] = Value::Null;
    v["colors"]["selectionForeground"] = json!(3);
    assert_eq!(
        issues(&v),
        [
            "theme.colors.accent must be a canonical lowercase #rrggbb or #rrggbbaa color",
            "theme.colors.selectionForeground must be a canonical lowercase #rrggbb or #rrggbbaa color or null",
        ]
    );
    let mut v = value(DARK);
    v["colors"]
        .as_object_mut()
        .unwrap()
        .remove("selectionForeground");
    assert_eq!(issues(&v).len(), 1);
}

#[test]
fn missing_destructive_keys_derive_to_the_original_values() {
    for json in [DARK, MINIMAL_DARK] {
        let source = parse(&value(json)).unwrap();
        let mut legacy = value(json);
        let colors = legacy["colors"].as_object_mut().unwrap();
        colors.remove("dangerHover");
        colors.remove("onDanger");
        let derived = parse(&legacy).unwrap();
        assert_eq!(
            derived.palette.danger_hover, source.palette.danger_hover,
            "{}",
            source.id
        );
        let p = derived.palette;
        assert!(
            p.on_danger == p.text || p.on_danger == p.background,
            "{}",
            source.id
        );
    }
    // dark: danger #ff4b75 → hover ×0.88 = #e04267; text #f4f4f5 vs background #09090b over the
    // two fills: the dark background reads better.
    let mut v = value(DARK);
    let colors = v["colors"].as_object_mut().unwrap();
    colors.remove("dangerHover");
    colors.remove("onDanger");
    let p = parse(&v).unwrap().palette;
    assert_eq!(p.danger_hover, rgba(0xe04267ff));
    assert_eq!(p.on_danger, rgba(0x09090bff));
    // The alpha channel is kept.
    v["colors"]["danger"] = json!("#ff4b7580");
    assert_eq!(parse(&v).unwrap().palette.danger_hover, rgba(0xe0426780));
}

#[test]
fn root_fields_are_checked() {
    let mut v = value(DARK);
    v["schemaVersion"] = json!(1);
    v["id"] = json!("Dark");
    v["label"] = json!("  ");
    v["order"] = json!(1.5);
    v["appearance"] = json!("dim");
    v["contrast"] = json!("low");
    v["colors"] = json!([]);
    v["ansi"] = Value::Null;
    v.as_object_mut().unwrap().remove("syntax");
    assert_eq!(
        issues(&v),
        [
            "theme.schemaVersion must be 2",
            "theme.id must be a lowercase slug (1-64 characters; letters, numbers, ., _, -)",
            "theme.label must be a non-empty string",
            "theme.order must be an integer",
            r#"theme.appearance must be "light" or "dark""#,
            r#"theme.contrast must be "normal" or "high""#,
            "theme.colors must be an object",
            "theme.ansi must be an object",
            "theme.syntax must be an object",
        ]
    );
    assert_eq!(issues(&json!([])), ["theme manifest must be an object"]);
    let mut v = value(DARK);
    v.as_object_mut().unwrap().remove("$schema");
    v["order"] = json!(-3.0);
    assert_eq!(parse(&v).unwrap().order, -3);
}

#[test]
fn slugs_follow_the_original_pattern() {
    assert!(is_theme_slug("a"));
    assert!(is_theme_slug(&format!("a{}", "b".repeat(63))));
    assert!(!is_theme_slug(&format!("a{}", "b".repeat(64))));
    assert!(is_theme_slug("0.x_y-z"));
    assert!(!is_theme_slug(".a"));
    assert!(!is_theme_slug(""));
    assert!(!is_theme_slug("a b"));
}

#[test]
fn json_nested_deeper_than_32_levels_is_invalid_json() {
    let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let deep = LumaTheme::parse(&nested(33)).unwrap_err();
    assert_eq!(deep.kind, ManifestErrorKind::InvalidJson);
    assert_eq!(deep.issues, ["nested deeper than 32 levels"]);
    let limit = LumaTheme::parse(&nested(MAX_NESTING)).unwrap_err();
    assert_eq!(limit.kind, ManifestErrorKind::InvalidTheme);
    // Brackets inside strings do not count.
    let mut v = value(DARK);
    v["label"] = json!("[[[[\\\"{{{{".repeat(10));
    assert!(parse(&v).is_ok());
    let broken = LumaTheme::parse("{").unwrap_err();
    assert_eq!(broken.kind, ManifestErrorKind::InvalidJson);
    assert!(!broken.detail().is_empty());
}

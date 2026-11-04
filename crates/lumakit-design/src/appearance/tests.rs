use super::*;
use gpui_kit::WindowAppearance;

fn catalog() -> &'static ThemeCatalog { ThemeCatalog::bundled().unwrap() }

#[test]
fn default_is_the_catalog_default_fixed_with_the_theme_accent() {
    let p = Preferences::default();
    assert_eq!((p.theme_id.as_str(), p.follow_system, p.accent, p.density), (DEFAULT_THEME, false, Accent::Theme, Density::Comfortable));
    assert_eq!(p.resolve(catalog(), WindowAppearance::Light).id, DEFAULT_THEME, "a fixed theme ignores the window");
}

#[test]
fn serialized_schema_is_stable_and_strict() {
    let p = Preferences { theme_id: "warm-light".into(), accent: Accent::Cyan, density: Density::Compact, ..Default::default() };
    let value = serde_json::to_value(&p).unwrap();
    assert_eq!(value, serde_json::json!({ "theme_id": "warm-light", "follow_system": false, "light_theme": "light",
        "dark_theme": "dark", "accent": "Cyan", "density": "Compact" }));
    assert!(serde_json::from_value::<Preferences>(serde_json::json!({ "theme_id": "dark" })).is_err(), "a partial record is corrupt, not a default");
}

#[test]
fn accent_overrides_only_when_chosen() {
    let base = Preferences { theme_id: "warm-light".into(), ..Default::default() };
    let theme_default = base.resolve(catalog(), WindowAppearance::Light);
    assert_eq!(theme_default.palette, catalog().get("warm-light").unwrap().palette);
    let blue = Preferences { accent: Accent::Blue, ..base.clone() }.resolve(catalog(), WindowAppearance::Light);
    assert_eq!(blue.palette.accent, ACCENT_PRESETS[1].light);
    assert_eq!(blue.palette.background, theme_default.palette.background, "accent never changes the page");
    let dark = Preferences { theme_id: "dark".into(), accent: Accent::Blue, ..base }.resolve(catalog(), WindowAppearance::Light);
    assert_eq!(dark.palette.accent, ACCENT_PRESETS[1].dark, "a dark theme takes the preset's dark color");
    let ids: Vec<_> = ACCENTS.iter().filter_map(|(accent, _)| accent.preset().map(|p| p.id)).collect();
    assert_eq!(ids, ["red", "blue", "green", "yellow", "cyan", "purple", "gray"]);
}

#[test]
fn compact_density_scales_type_down() {
    assert_eq!(Density::Comfortable.scale(), 1.);
    assert!((Density::Compact.scale() * 14. - 13.).abs() < 1e-4);
}

#[test]
fn follow_system_picks_the_slot_of_the_window_appearance() {
    let p = Preferences { theme_id: "minimal-dark".into(), follow_system: true, light_theme: "warm-light".into(), dark_theme: "warm-dark".into(), ..Default::default() };
    assert_eq!(p.resolve(catalog(), WindowAppearance::Light).id, "warm-light");
    assert_eq!(p.resolve(catalog(), WindowAppearance::VibrantLight).id, "warm-light");
    assert_eq!(p.resolve(catalog(), WindowAppearance::Dark).id, "warm-dark");
    assert_eq!(p.resolve(catalog(), WindowAppearance::VibrantDark).id, "warm-dark");
    let fixed = Preferences { follow_system: false, ..p };
    assert_eq!(fixed.resolve(catalog(), WindowAppearance::Light).id, "minimal-dark");
}

#[test]
fn follow_toggle_pairs_the_family_and_keeps_the_shown_theme() {
    for (fixed, light, dark) in [("warm-dark", "warm-light", "warm-dark"), ("minimal-light", "minimal-light", "minimal-dark"),
        ("dark", "light", "dark"), ("high-contrast-light", "high-contrast-light", "high-contrast-dark")] {
        let on = Preferences { theme_id: fixed.into(), ..Default::default() }.set_follow(catalog(), true, WindowAppearance::Light);
        assert!(on.follow_system);
        assert_eq!((on.light_theme.as_str(), on.dark_theme.as_str()), (light, dark), "{fixed}");
    }
    let following = Preferences { follow_system: true, light_theme: "warm-light".into(), dark_theme: "minimal-dark".into(), ..Default::default() };
    let off = following.set_follow(catalog(), false, WindowAppearance::Dark);
    assert!(!off.follow_system);
    assert_eq!(off.theme_id, "minimal-dark");
    let light = catalog().get("light").unwrap();
    let picked = following.pick_theme(light);
    assert_eq!((picked.light_theme.as_str(), picked.dark_theme.as_str()), ("light", "minimal-dark"));
    assert!(picked.is_selected(light) && picked.is_selected(catalog().get("minimal-dark").unwrap()));
    assert_eq!(Preferences::default().pick_theme(light).theme_id, "light");
}

#[test]
fn every_bundled_theme_has_a_chinese_name() {
    for theme in catalog().list() { assert_ne!(theme_name(theme), theme.label.as_str(), "{} needs a name", theme.id); }
}

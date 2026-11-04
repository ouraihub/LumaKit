//! Catalog ordering, user themes and every resolution rule, against the original's runtime.ts and
//! AppearanceSettings.tsx behavior.

use serde_json::Value;

use super::*;

fn bundled() -> &'static ThemeCatalog {
    ThemeCatalog::bundled().unwrap()
}

/// A valid manifest (the dark palette) with the given identity.
fn manifest(id: &str, label: &str, order: i64, appearance: &str, contrast: &str) -> String {
    let mut value: Value = serde_json::from_str(BUNDLED[0].1).unwrap();
    value["id"] = id.into();
    value["label"] = label.into();
    value["order"] = order.into();
    value["appearance"] = appearance.into();
    value["contrast"] = contrast.into();
    value.to_string()
}

fn with_users(files: &[(&str, &str)]) -> ThemeCatalog {
    bundled().with_user_themes(
        files
            .iter()
            .map(|(name, json)| (name.to_string(), Ok(json.to_string()))),
    )
}

fn ids(catalog: &ThemeCatalog) -> Vec<&str> {
    catalog.list().iter().map(|t| t.id.as_str()).collect()
}

fn id(theme: Option<&Arc<LumaTheme>>) -> Option<&str> {
    theme.map(|t| t.id.as_str())
}

fn system(
    light: &'static str,
    dark: &'static str,
    appearance: Appearance,
) -> SystemThemes<'static> {
    SystemThemes {
        light,
        dark,
        appearance,
    }
}

#[test]
fn bundled_themes_list_the_default_first_then_by_order() {
    assert_eq!(
        ids(bundled()),
        [
            "dark",
            "light",
            "minimal-dark",
            "minimal-light",
            "high-contrast-dark",
            "high-contrast-light",
            "warm-dark",
            "warm-light"
        ]
    );
    assert!(bundled().user_issues().is_empty());
}

#[test]
fn user_themes_sort_by_order_label_id_but_never_before_the_default() {
    let early = manifest("early", "Early", 1, "dark", "normal");
    let zed = manifest("zed", "Zed", 15, "light", "normal");
    let ant = manifest("ant", "Ant", 15, "light", "normal");
    let catalog = with_users(&[("a", &early), ("b", &zed), ("c", &ant)]);
    assert_eq!(
        &ids(&catalog)[..5],
        ["dark", "user.early", "user.ant", "user.zed", "light"]
    );
}

#[test]
fn user_ids_get_the_user_prefix() {
    let dark = manifest("dark", "Mine", 70, "dark", "normal");
    let catalog = with_users(&[("dark.theme.json", &dark)]);
    assert_eq!(id(catalog.get("user.dark")), Some("user.dark"));
    assert_eq!(catalog.get("dark").map(|t| t.label.as_str()), Some("Dark"));
    assert!(catalog.user_issues().is_empty());
}

#[test]
fn a_prefixed_id_longer_than_64_is_reported() {
    let fits = "a".repeat(59);
    let long = "b".repeat(60);
    let catalog = with_users(&[
        ("fits", &manifest(&fits, "Fits", 70, "dark", "normal")),
        ("long", &manifest(&long, "Long", 70, "dark", "normal")),
    ]);
    assert!(catalog.get(&format!("user.{fits}")).is_some());
    assert_eq!(
        catalog.user_issues(),
        [UserThemeIssue {
            file_name: "long".into(),
            kind: UserThemeIssueKind::IdTooLong,
            detail: Some(long),
        }]
    );
}

#[test]
fn a_duplicate_id_keeps_the_first_file() {
    let first = manifest("ocean", "First", 70, "dark", "normal");
    let second = manifest("ocean", "Second", 70, "dark", "normal");
    let catalog = with_users(&[("a.theme.json", &first), ("b.theme.json", &second)]);
    assert_eq!(
        catalog.get("user.ocean").map(|t| t.label.as_str()),
        Some("First")
    );
    assert_eq!(
        catalog.user_issues(),
        [UserThemeIssue {
            file_name: "b.theme.json".into(),
            kind: UserThemeIssueKind::DuplicateId,
            detail: Some("ocean".into()),
        }]
    );
}

#[test]
fn file_problems_are_reported_per_file_without_blocking_the_others() {
    let good = manifest("good", "Good", 70, "dark", "normal");
    let catalog = bundled().with_user_themes([
        ("big".to_owned(), Err(FileIssue::TooLarge)),
        ("extra".to_owned(), Err(FileIssue::TooMany)),
        (
            "locked".to_owned(),
            Err(FileIssue::Unreadable("denied".into())),
        ),
        (
            "silent".to_owned(),
            Err(FileIssue::Unreadable(String::new())),
        ),
        ("broken".to_owned(), Ok("{".to_owned())),
        ("wrong".to_owned(), Ok(r#"{"schemaVersion":2}"#.to_owned())),
        ("good".to_owned(), Ok(good)),
    ]);
    let kinds: Vec<_> = catalog
        .user_issues()
        .iter()
        .map(|i| (i.file_name.as_str(), i.kind, i.detail.is_some()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("big", UserThemeIssueKind::TooLarge, false),
            ("extra", UserThemeIssueKind::TooMany, false),
            ("locked", UserThemeIssueKind::Unreadable, true),
            ("silent", UserThemeIssueKind::Unreadable, false),
            ("broken", UserThemeIssueKind::InvalidJson, true),
            ("wrong", UserThemeIssueKind::InvalidTheme, true),
        ]
    );
    let wrong = catalog.user_issues()[5].detail.as_deref().unwrap();
    assert!(
        wrong.starts_with("theme.id must be a lowercase slug"),
        "{wrong}"
    );
    assert!(
        wrong.contains("; theme.label must be a non-empty string"),
        "{wrong}"
    );
    assert!(catalog.get("user.good").is_some());
}

#[test]
fn fixed_mode_resolves_the_theme_or_falls_back_to_the_default() {
    let exact = bundled().resolve("minimal-light", None);
    assert_eq!(
        (exact.theme.id.as_str(), exact.fallback),
        ("minimal-light", false)
    );
    let missing = bundled().resolve("user.gone", None);
    assert_eq!(missing.theme.id, "dark");
    assert_eq!(missing.requested_id, "user.gone");
    assert!(missing.fallback);
    let renamed = bundled().resolve("high-contrast", None);
    assert_eq!(
        (renamed.theme.id.as_str(), renamed.fallback),
        ("high-contrast-dark", false)
    );
}

#[test]
fn system_mode_takes_the_slot_of_the_current_appearance() {
    let pair = |appearance| system("minimal-light", "high-contrast-dark", appearance);
    let light = bundled().resolve("dark", Some(pair(Appearance::Light)));
    assert_eq!(
        (light.theme.id.as_str(), light.fallback),
        ("minimal-light", false)
    );
    let dark = bundled().resolve("dark", Some(pair(Appearance::Dark)));
    assert_eq!(
        (dark.theme.id.as_str(), dark.requested_id.as_str()),
        ("high-contrast-dark", "high-contrast-dark")
    );
}

#[test]
fn a_missing_or_wrong_appearance_slot_falls_back_to_a_bundled_theme() {
    // A user light theme ordered before every bundled one is never the fallback.
    let first = manifest("first", "First", 1, "light", "normal");
    let catalog = with_users(&[("first", &first)]);
    let missing = catalog.resolve("dark", Some(system("user.gone", "dark", Appearance::Light)));
    assert_eq!(
        (missing.theme.id.as_str(), missing.fallback),
        ("light", true)
    );
    assert_eq!(missing.requested_id, "user.gone");
    let wrong = catalog.resolve(
        "dark",
        Some(system("minimal-dark", "dark", Appearance::Light)),
    );
    assert_eq!((wrong.theme.id.as_str(), wrong.fallback), ("light", true));
    let user = catalog.resolve(
        "dark",
        Some(system("user.first", "dark", Appearance::Light)),
    );
    assert_eq!(
        (user.theme.id.as_str(), user.fallback),
        ("user.first", false)
    );
    let dark = catalog.resolve("dark", Some(system("light", "light", Appearance::Dark)));
    assert_eq!((dark.theme.id.as_str(), dark.fallback), ("dark", true));
}

#[test]
fn the_derived_pair_keeps_the_fixed_theme_and_its_contrast() {
    let pair = |light: &str, dark: &str| ThemePair {
        light: light.into(),
        dark: dark.into(),
    };
    assert_eq!(
        bundled().derive_pair("minimal-dark"),
        pair("light", "minimal-dark")
    );
    assert_eq!(
        bundled().derive_pair("minimal-light"),
        pair("minimal-light", "dark")
    );
    assert_eq!(
        bundled().derive_pair("high-contrast-light"),
        pair("high-contrast-light", "high-contrast-dark")
    );
    assert_eq!(bundled().derive_pair("user.gone"), pair("light", "dark"));
    // The other slot is bundled even when a user theme would sort first.
    let first = manifest("first", "First", 1, "light", "high");
    let catalog = with_users(&[("first", &first)]);
    assert_eq!(
        catalog.derive_pair("high-contrast-dark"),
        pair("high-contrast-light", "high-contrast-dark")
    );
}

#[test]
fn counterpart_matches_the_family_then_a_family_less_theme() {
    let c = bundled();
    let get = |id: &str| c.get(id).unwrap();
    let counterpart = |from: &str, appearance, contrast| {
        id(c.counterpart(get(from), appearance, contrast)).map(str::to_owned)
    };
    let some = |id: &str| Some(id.to_owned());
    use {Appearance::*, Contrast::*};
    assert_eq!(
        counterpart("minimal-dark", Light, Normal),
        some("minimal-light")
    );
    assert_eq!(counterpart("dark", Dark, High), some("high-contrast-dark"));
    assert_eq!(
        counterpart("high-contrast-dark", Dark, Normal),
        some("dark")
    );
    assert_eq!(
        counterpart("high-contrast-light", Light, Normal),
        some("light")
    );
    assert_eq!(
        counterpart("minimal-dark", Dark, High),
        some("high-contrast-dark")
    );
    assert_eq!(
        counterpart("light", Light, High),
        some("high-contrast-light")
    );
}

#[test]
fn a_user_family_finds_its_own_counterpart() {
    let night = manifest("ocean-dark", "Ocean Dark", 70, "dark", "normal");
    let day = manifest("ocean-light", "Ocean Light", 71, "light", "normal");
    let lonely = manifest("forest-dark", "Forest", 72, "dark", "normal");
    let c = with_users(&[("a", &night), ("b", &day), ("c", &lonely)]);
    let from = |id: &str| c.get(id).unwrap();
    assert_eq!(
        id(c.appearance_target(from("user.ocean-dark"), Appearance::Light)),
        Some("user.ocean-light")
    );
    assert_eq!(
        id(c.appearance_target(from("user.forest-dark"), Appearance::Light)),
        Some("light")
    );
}

#[test]
fn appearance_target_keeps_a_theme_that_already_matches() {
    let c = bundled();
    let from = |id: &str| c.get(id).unwrap();
    assert_eq!(
        id(c.appearance_target(from("light"), Appearance::Light)),
        Some("light")
    );
    assert_eq!(
        id(c.appearance_target(from("minimal-dark"), Appearance::Light)),
        Some("minimal-light")
    );
    assert_eq!(
        id(c.appearance_target(from("high-contrast-light"), Appearance::Dark)),
        Some("high-contrast-dark")
    );
}

#[test]
fn family_keys_follow_the_original_regexes() {
    assert_eq!(family::key("minimal-dark"), "minimal");
    assert_eq!(family::key("minimal-light"), "minimal");
    assert_eq!(family::key("dark"), "");
    assert_eq!(family::key("high-contrast-light"), "");
    assert_eq!(family::key("high-contrast"), "");
    assert_eq!(family::key("user.ocean-dark"), "user.ocean");
    assert_eq!(family::key("xdark"), "xdark");
    assert_eq!(family::key("high-contrast-minimal-dark"), "minimal");
}

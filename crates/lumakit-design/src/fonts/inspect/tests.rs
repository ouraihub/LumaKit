//! Font file inspection over the bundled TTFs, a collection header and garbage.

use super::*;

const GEIST_LIGHT: &[u8] = include_bytes!("../../../assets/fonts/Geist-Light.ttf");
const GEIST_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Geist-SemiBold.ttf");
const MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
const MONO_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
const ORBITRON: &[u8] = include_bytes!("../../../assets/fonts/Orbitron-Variable.ttf");

fn face(family: &str, monospace: bool, weight: u16) -> Result<FaceInfo, InspectError> {
    Ok(FaceInfo {
        family: family.to_owned(),
        monospace,
        weight,
    })
}

#[test]
fn bundled_faces_report_family_spacing_and_weight() {
    assert_eq!(inspect(GEIST_LIGHT), face("Geist", false, 300));
    assert_eq!(inspect(GEIST_SEMIBOLD), face("Geist", false, 600));
    assert_eq!(inspect(MONO_REGULAR), face("JetBrains Mono", true, 400));
    assert_eq!(inspect(MONO_MEDIUM), face("JetBrains Mono", true, 500));
}

#[test]
fn a_variable_font_reports_its_default_weight() {
    let orbitron = inspect(ORBITRON).unwrap();
    assert_eq!(
        (orbitron.family.as_str(), orbitron.monospace),
        ("Orbitron", false)
    );
    assert_eq!(orbitron.weight, 400);
}

#[test]
fn collections_are_rejected() {
    let mut ttc = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x01".to_vec();
    ttc.extend_from_slice(&[0; 16]);
    assert_eq!(inspect(&ttc), Err(InspectError::Collection));
}

#[test]
fn garbage_is_invalid() {
    assert!(matches!(
        inspect(b"not a font at all"),
        Err(InspectError::Invalid(_))
    ));
    assert!(matches!(inspect(&[]), Err(InspectError::Invalid(_))));
    // A valid header with its tables cut off.
    assert!(matches!(
        inspect(&MONO_REGULAR[..64]),
        Err(InspectError::Invalid(_))
    ));
}

#[test]
fn family_names_follow_the_core_rule() {
    assert!(is_font_family_name("JetBrains Mono"));
    assert!(!is_font_family_name(" Padded"));
    assert!(!is_font_family_name("Semi;colon"));
    assert!(!is_font_family_name(&"x".repeat(129)));
    assert_eq!(MAC_ROMAN_HIGH.chars().count(), 128);
}

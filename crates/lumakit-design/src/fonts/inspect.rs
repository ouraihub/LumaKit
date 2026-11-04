//! Reads a user font file's family name, monospace flag and weight with `ttf-parser` (A13),
//! following the original's `packages/server/src/fonts/sfnt.ts` for TrueType / OpenType files.
//! Collections (`ttcf`) are rejected: DirectWrite registration takes single faces only here.

#[cfg(test)]
mod tests;

use std::fmt;

use ttf_parser::{Face, FaceParsingError, PlatformId, Tag, name::Name, name_id};

/// What the settings page shows for one font file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceInfo {
    pub family: String,
    pub monospace: bool,
    /// OS/2 `usWeightClass`, or a variable font's default `wght`; 400 when absent or invalid.
    pub weight: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectError {
    /// A `.ttc` / `.otc` font collection.
    Collection,
    /// Not a usable TrueType / OpenType font; the English detail says why.
    Invalid(String),
}

impl fmt::Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collection => f.write_str("the file is a font collection"),
            Self::Invalid(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for InspectError {}

const FAMILY_NAME_IDS: [u16; 2] = [name_id::TYPOGRAPHIC_FAMILY, name_id::FAMILY];
/// OS/2 PANOSE `bFamilyType` / `bProportion` offsets and the "Latin text, monospaced" values.
const OS2_PANOSE_FAMILY: usize = 32;
const OS2_PANOSE_PROPORTION: usize = 35;
const PANOSE_LATIN_TEXT: u8 = 2;
const PANOSE_MONOSPACED: u8 = 9;
const DEFAULT_WEIGHT: u16 = 400;
const FAMILY_NAME_MAX: usize = 128;
/// Mac OS Roman bytes 0x80–0xFF.
const MAC_ROMAN_HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø\
¿¡¬√ƒ≈∆«»…\u{a0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{f8ff}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";

/// The same rule as core's `is_font_family_name` (ui does not depend on core).
fn is_font_family_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= FAMILY_NAME_MAX
        && name.trim() == name
        && !name.contains(['"', '\\', ';', '{', '}'])
        && !name.chars().any(char::is_control)
}

/// Windows Unicode US-English first, then other Windows languages, Unicode, Mac Roman English.
fn rank(name: &Name<'_>) -> Option<u8> {
    match name.platform_id {
        PlatformId::Windows if matches!(name.encoding_id, 0 | 1 | 10) => {
            Some(if name.language_id == 0x409 { 0 } else { 1 })
        }
        PlatformId::Unicode => Some(2),
        PlatformId::Macintosh if name.encoding_id == 0 => {
            Some(if name.language_id == 0 { 3 } else { 4 })
        }
        _ => None,
    }
}

fn decode(name: &Name<'_>) -> String {
    let text: String = if name.platform_id == PlatformId::Macintosh {
        name.name
            .iter()
            .map(|&byte| match byte {
                0..0x80 => char::from(byte),
                _ => MAC_ROMAN_HIGH
                    .chars()
                    .nth(usize::from(byte - 0x80))
                    .unwrap_or(char::REPLACEMENT_CHARACTER),
            })
            .collect()
    } else {
        let (pairs, _) = name.name.as_chunks::<2>();
        let units = pairs.iter().map(|&pair| u16::from_be_bytes(pair));
        char::decode_utf16(units)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    };
    let printable: String = text
        .chars()
        .filter(|&c| c >= ' ' && c != '\u{7f}')
        .collect();
    printable
        .trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
        .to_owned()
}

/// The first non-empty name among `ids`, best-ranked record first (original `bestName`).
fn best_name(face: &Face<'_>, ids: &[u16]) -> String {
    for &id in ids {
        let mut candidates: Vec<(u8, Name<'_>)> = face
            .names()
            .into_iter()
            .filter(|name| name.name_id == id)
            .filter_map(|name| Some((rank(&name)?, name)))
            .collect();
        candidates.sort_by_key(|(rank, _)| *rank);
        if let Some(text) = candidates
            .iter()
            .map(|(_, name)| decode(name))
            .find(|text| !text.is_empty())
        {
            return text;
        }
    }
    String::new()
}

fn css_weight(value: f32) -> u16 {
    // Clamped to 1–1000 first, so the cast is exact.
    value.round().clamp(1., 1000.) as u16
}

/// Inspects one font file.
pub fn inspect(bytes: &[u8]) -> Result<FaceInfo, InspectError> {
    if bytes.starts_with(b"ttcf") {
        return Err(InspectError::Collection);
    }
    let face = Face::parse(bytes, 0).map_err(|error| {
        InspectError::Invalid(match error {
            FaceParsingError::UnknownMagic => {
                "the file is not a TrueType or OpenType font".to_owned()
            }
            other => format!("the font is malformed: {other}"),
        })
    })?;
    if face.tables().name.is_none() {
        return Err(InspectError::Invalid(
            "the font has no name table".to_owned(),
        ));
    }
    let family = best_name(&face, &FAMILY_NAME_IDS);
    if family.is_empty() {
        return Err(InspectError::Invalid("the font names no family".to_owned()));
    }
    if !is_font_family_name(&family) {
        return Err(InspectError::Invalid(format!(
            "the family name \"{family}\" cannot be used"
        )));
    }
    let os2 = face.raw_face().table(Tag::from_bytes(b"OS/2"));
    let panose_monospace = os2.is_some_and(|os2| {
        os2.get(OS2_PANOSE_FAMILY) == Some(&PANOSE_LATIN_TEXT)
            && os2.get(OS2_PANOSE_PROPORTION) == Some(&PANOSE_MONOSPACED)
    });
    let class = face.weight().to_number();
    let class = if (1..=1000).contains(&class) {
        class
    } else {
        DEFAULT_WEIGHT
    };
    let weight = face
        .variation_axes()
        .into_iter()
        .find(|axis| axis.tag == Tag::from_bytes(b"wght"))
        .map_or(class, |axis| css_weight(axis.def_value));
    Ok(FaceInfo {
        family,
        monospace: face.is_monospaced() || panose_monospace,
        weight,
    })
}

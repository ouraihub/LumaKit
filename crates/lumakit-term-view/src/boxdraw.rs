//! Geometric box-drawing for U+2500..U+259F.
//!
//! PoC-1 §4 chose geometry over font fallback so neighbouring lines never
//! leave a seam. This module exposes `is_boxdraw(ch)` so the painter can
//! detect the range and defer actual geometry to a follow-up task once the
//! view has a real window host. For T1.18 we only identify the range so the
//! text pipeline knows to skip these glyphs.

/// `true` when `ch` is one of the Unicode box-drawing characters
/// (U+2500..U+259F) that PoC-1 requires geometric rendering for.
pub fn is_boxdraw(ch: char) -> bool {
    matches!(ch, '\u{2500}'..='\u{259F}')
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn range_matches_block_and_box() {
        assert!(is_boxdraw('\u{2500}'));
        assert!(is_boxdraw('\u{259F}'));
        assert!(is_boxdraw('\u{2580}'));
        assert!(!is_boxdraw('a'));
        assert!(!is_boxdraw('\u{24FF}'));
    }
}

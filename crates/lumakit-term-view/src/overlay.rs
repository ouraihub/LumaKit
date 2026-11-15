//! Overlay data the host feeds the painter: search hits, keyword highlight
//! bands, ghost text. The view never reads product state; everything comes
//! through this interface so lumakit-term-view stays product-neutral.

use gpui_kit::Hsla;

/// Cell-space inclusive range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    /// Starting row within the current viewport.
    pub start_row: usize,
    /// Starting column.
    pub start_col: usize,
    /// Ending row.
    pub end_row: usize,
    /// Ending column.
    pub end_col: usize,
}

/// A highlighted band drawn beneath text.
#[derive(Debug, Clone, Copy)]
pub struct HighlightBand {
    /// Range the band covers.
    pub range: CellRange,
    /// Fill color.
    pub color: Hsla,
}

/// Ghost text drawn at a grid position (04 §18 shell suggest).
#[derive(Debug, Clone)]
pub struct GhostText {
    /// Starting row within the current viewport.
    pub row: usize,
    /// Starting column.
    pub col: usize,
    /// Text to render.
    pub text: String,
    /// Foreground color (usually fg at reduced alpha).
    pub color: Hsla,
}

/// All overlays the host passes to the painter in one call.
#[derive(Debug, Default, Clone)]
pub struct Overlays {
    /// Search-result highlights.
    pub search: Vec<HighlightBand>,
    /// Keyword highlights.
    pub keywords: Vec<HighlightBand>,
    /// Ghost text suggestions.
    pub ghosts: Vec<GhostText>,
}

impl Overlays {
    /// `true` when nothing is to be drawn.
    pub fn is_empty(&self) -> bool {
        self.search.is_empty() && self.keywords.is_empty() && self.ghosts.is_empty()
    }
}

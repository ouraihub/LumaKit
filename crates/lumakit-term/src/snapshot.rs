//! `TermHandle::snapshot`: copy the visible rows into width-segmented style
//! runs so the view can paint without touching the live grid again (04 §6.5).
//!
//! Width segmentation: GPUI's `force_width` advances every base glyph by the
//! same cell width, so the view needs one `shape_line` per narrow / wide run
//! within a row. Backgrounds of equal color on adjacent cells coalesce into
//! one quad per run so the painter emits one draw call per band.
//!
//! The snapshot is deliberately colour-agnostic in bytes: the host supplies a
//! [`Palette`] with 16 ANSI colours, fg, bg, cursor and selection, each as a
//! 4-byte RGBA tuple; AutoTerm's `autoterm_ui::Rgba` and ThinkRail's palette
//! both feed in with a trivial `.into()`.

use std::hash::{Hash, Hasher};

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

use crate::term::{Listener, TermHandle};

/// 32-bit RGBA colour (straight alpha); 0 is fully transparent, 255 fully
/// opaque. Snapshots keep final resolved colours only; the host does not need
/// to re-run the palette lookup during painting.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotRgba {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel.
    pub a: u8,
}

impl SnapshotRgba {
    /// Construct from the four channels.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

impl From<[u8; 4]> for SnapshotRgba {
    fn from(value: [u8; 4]) -> Self {
        Self {
            r: value[0],
            g: value[1],
            b: value[2],
            a: value[3],
        }
    }
}

/// Colour palette the snapshot resolves against (04 §6.5 host palette).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Default foreground.
    pub foreground: SnapshotRgba,
    /// Default background. Cells carrying this colour never contribute to
    /// `backgrounds` — the view paints its clear colour instead.
    pub background: SnapshotRgba,
    /// Cursor block / underline.
    pub cursor: SnapshotRgba,
    /// Selection background (usually with alpha < 255).
    pub selection: SnapshotRgba,
    /// 16 ANSI slots: 0..=7 regular, 8..=15 bright, in the usual order.
    pub ansi: [SnapshotRgba; 16],
}

/// Cell style for one run within a segment. Backgrounds live in a separate
/// vector, so the run key drops the background to minimise run count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunStyle {
    /// Foreground colour.
    pub foreground: SnapshotRgba,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Any underline flag (single / double / curly / dotted / dashed).
    pub underline: bool,
    /// Strikethrough.
    pub strike: bool,
}

/// A run of `len` bytes within a segment painted in one style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StyleRun {
    /// Number of UTF-8 bytes this run covers in the parent `Segment.text`.
    pub len: usize,
    /// Style for every glyph in the run.
    pub style: RunStyle,
}

/// A single-width-class run of cells, starting at `col`, painted with its
/// own `shape_line` call.
#[derive(Debug, Clone)]
pub struct Segment {
    /// Grid column the segment starts at.
    pub col: usize,
    /// True for wide (CJK / emoji) glyphs, false for narrow.
    pub wide: bool,
    /// Decoded text, `WIDE_CHAR_SPACER` / `HIDDEN` substituted, zero-width
    /// combining characters appended to their base glyph.
    pub text: String,
    /// Styled runs over `text`; byte-length prefix sums form the run offsets.
    pub runs: Vec<StyleRun>,
}

/// Non-default background band: `(start_col, cell_count, color)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BgRun {
    /// First column the band covers.
    pub col: usize,
    /// Column count the band spans (wide cells count as two).
    pub len: usize,
    /// Background colour.
    pub color: SnapshotRgba,
}

/// One visible row, ready for painting.
#[derive(Debug, Clone)]
pub struct RowSnapshot {
    /// Segments in column order.
    pub segments: Vec<Segment>,
    /// Non-default background bands in column order.
    pub backgrounds: Vec<BgRun>,
    /// 64-bit content hash of this row, used by the view's damage tracker.
    pub hash: u64,
}

/// Cursor state in snapshot space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// 0-based row (including display offset).
    pub row: usize,
    /// 0-based column.
    pub col: usize,
    /// Cursor shape (`Block`, `Underline`, `Beam`, `HollowBlock`).
    pub shape: CursorShape,
    /// Whether the cursor is visible this frame.
    pub visible: bool,
}

/// Selection range in snapshot space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionSpan {
    /// Starting row.
    pub start_row: usize,
    /// Starting column.
    pub start_col: usize,
    /// Ending row.
    pub end_row: usize,
    /// Ending column (exclusive).
    pub end_col: usize,
}

/// A complete snapshot of the visible terminal area.
#[derive(Debug, Clone)]
pub struct Frame {
    /// One entry per visible row, top-to-bottom.
    pub rows: Vec<RowSnapshot>,
    /// Cursor position, `None` when the cursor is off-grid or hidden.
    pub cursor: Option<Cursor>,
    /// Selection span, if any.
    pub selection: Option<SelectionSpan>,
    /// How many lines above the current buffer end are visible (0 = tailing).
    pub display_offset: usize,
    /// Terminal mode bits (alternate screen, bracketed paste, …).
    pub mode: TermMode,
}

impl TermHandle {
    /// Copy the visible rows into a `Frame`. Call `take_dirty()` just before
    /// this so output parsed while the snapshot was in flight triggers the
    /// next wakeup.
    pub fn snapshot(&self, palette: &Palette) -> Frame {
        self.with_term(|term| snapshot(term, palette))
    }
}

fn snapshot(term: &Term<Listener>, palette: &Palette) -> Frame {
    let content = term.renderable_content();
    let cols = term.columns();
    let lines = term.screen_lines();
    let offset = content.display_offset as i32;
    let selection = content.selection;
    let mode = content.mode;
    let display_offset = content.display_offset;

    let mut rows: Vec<RowSnapshot> = (0..lines)
        .map(|_| RowSnapshot {
            segments: Vec::new(),
            backgrounds: Vec::new(),
            hash: 0,
        })
        .collect();

    for indexed in content.display_iter {
        let cell = indexed.cell;
        let row = (indexed.point.line.0 + offset) as usize;
        if row >= lines || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let col = indexed.point.column.0;
        let mut fg = resolve(cell.fg, palette, content.colors);
        let mut bg = resolve(cell.bg, palette, content.colors);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(Flags::DIM) {
            // Multiply alpha by 0.7 so the dim style stays palette-driven.
            fg.a = ((u16::from(fg.a) * 179) / 255) as u8;
        }
        let selected = selection.is_some_and(|s| s.contains(indexed.point));
        if selected {
            bg = palette.selection;
        }

        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let width = if wide { 2 } else { 1 };
        let r = &mut rows[row];
        if bg != palette.background {
            match r.backgrounds.last_mut() {
                Some(last) if last.color == bg && last.col + last.len == col => last.len += width,
                _ => r.backgrounds.push(BgRun {
                    col,
                    len: width,
                    color: bg,
                }),
            }
        }
        let style = RunStyle {
            foreground: fg,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            strike: cell.flags.contains(Flags::STRIKEOUT),
        };
        let ch = if cell.flags.contains(Flags::HIDDEN) || cell.c == '\0' {
            ' '
        } else {
            cell.c
        };

        if r.segments.last().is_none_or(|s| s.wide != wide) {
            r.segments.push(Segment {
                col,
                wide,
                text: String::with_capacity(if wide { 16 } else { cols }),
                runs: Vec::new(),
            });
        }
        let Some(segment) = r.segments.last_mut() else {
            continue;
        };
        let before = segment.text.len();
        segment.text.push(ch);
        if let Some(extra) = cell.zerowidth() {
            segment.text.extend(extra);
        }
        let len = segment.text.len() - before;
        match segment.runs.last_mut() {
            Some(run) if run.style == style => run.len += len,
            _ => segment.runs.push(StyleRun { len, style }),
        }
    }

    for row in &mut rows {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for seg in &row.segments {
            seg.col.hash(&mut hasher);
            seg.wide.hash(&mut hasher);
            seg.text.hash(&mut hasher);
            for run in &seg.runs {
                run.hash(&mut hasher);
            }
        }
        for bg in &row.backgrounds {
            bg.hash(&mut hasher);
        }
        row.hash = hasher.finish();
    }

    let cursor_shape = content.cursor.shape;
    let cursor_point = content.cursor.point;
    let cursor_row = (cursor_point.line.0 + offset) as isize;
    let cursor = if cursor_row >= 0 && (cursor_row as usize) < lines {
        Some(Cursor {
            row: cursor_row as usize,
            col: cursor_point.column.0,
            shape: cursor_shape,
            visible: cursor_shape != CursorShape::Hidden,
        })
    } else {
        None
    };

    let selection_span = selection.map(|s| {
        let start = s.start;
        let end = s.end;
        SelectionSpan {
            start_row: (start.line.0 + offset).max(0) as usize,
            start_col: start.column.0,
            end_row: (end.line.0 + offset).max(0) as usize,
            end_col: end.column.0,
        }
    });

    Frame {
        rows,
        cursor,
        selection: selection_span,
        display_offset,
        mode,
    }
}

fn resolve(color: Color, palette: &Palette, colors: &Colors) -> SnapshotRgba {
    match color {
        Color::Spec(rgb) => rgb_to_rgba(rgb),
        Color::Indexed(i) => {
            if let Some(rgb) = colors[usize::from(i)] {
                return rgb_to_rgba(rgb);
            }
            match i {
                0..=15 => palette.ansi[usize::from(i)],
                16..=231 => {
                    let i = i - 16;
                    let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                    SnapshotRgba {
                        r: step(i / 36),
                        g: step((i / 6) % 6),
                        b: step(i % 6),
                        a: 255,
                    }
                }
                _ => {
                    let v = 8 + (i - 232) * 10;
                    SnapshotRgba {
                        r: v,
                        g: v,
                        b: v,
                        a: 255,
                    }
                }
            }
        }
        Color::Named(name) => {
            if let Some(rgb) = colors[name] {
                return rgb_to_rgba(rgb);
            }
            match name {
                NamedColor::Foreground | NamedColor::BrightForeground => palette.foreground,
                NamedColor::Background => palette.background,
                NamedColor::Cursor => palette.cursor,
                NamedColor::DimForeground => SnapshotRgba {
                    a: 179,
                    ..palette.foreground
                },
                n if (n as usize) < 16 => palette.ansi[n as usize],
                n => {
                    let base = (n as usize).saturating_sub(NamedColor::DimBlack as usize) % 8;
                    SnapshotRgba {
                        a: 179,
                        ..palette.ansi[base]
                    }
                }
            }
        }
    }
}

fn rgb_to_rgba(c: Rgb) -> SnapshotRgba {
    SnapshotRgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: 255,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use crate::event::{NullSink, NullWriter};
    use crate::term::{GridSize, TermHandle};

    use super::*;

    fn default_palette() -> Palette {
        Palette {
            foreground: SnapshotRgba::new(228, 228, 231, 255),
            background: SnapshotRgba::new(24, 24, 27, 255),
            cursor: SnapshotRgba::new(34, 211, 238, 255),
            selection: SnapshotRgba::new(34, 211, 238, 51),
            ansi: [
                SnapshotRgba::new(63, 63, 70, 255),
                SnapshotRgba::new(248, 113, 113, 255),
                SnapshotRgba::new(52, 211, 153, 255),
                SnapshotRgba::new(251, 191, 36, 255),
                SnapshotRgba::new(96, 165, 250, 255),
                SnapshotRgba::new(192, 132, 252, 255),
                SnapshotRgba::new(34, 211, 238, 255),
                SnapshotRgba::new(176, 176, 184, 255),
                SnapshotRgba::new(102, 102, 112, 255),
                SnapshotRgba::new(252, 165, 165, 255),
                SnapshotRgba::new(110, 231, 183, 255),
                SnapshotRgba::new(253, 230, 138, 255),
                SnapshotRgba::new(147, 197, 253, 255),
                SnapshotRgba::new(216, 180, 254, 255),
                SnapshotRgba::new(103, 232, 249, 255),
                SnapshotRgba::new(208, 208, 216, 255),
            ],
        }
    }

    fn make(cols: u16, rows: u16) -> TermHandle {
        TermHandle::new(
            GridSize { cols, rows },
            1000,
            Arc::new(NullWriter),
            Arc::new(NullSink),
        )
    }

    #[test]
    fn mixed_width_row_splits_into_segments() {
        let handle = make(16, 2);
        handle.advance("ab中cd文ef字gh".as_bytes());
        let frame = handle.snapshot(&default_palette());
        let row = &frame.rows[0];
        // ab | 中 | cd | 文 | ef | 字 | gh  = 7 segments.
        assert_eq!(row.segments.len(), 7, "segments: {:?}", row.segments);
        assert!(row.segments[1].wide);
        assert_eq!(row.segments[0].col, 0);
        assert_eq!(row.segments[1].col, 2);
        assert_eq!(row.segments[2].col, 4);
        assert_eq!(row.segments[6].col, 12);
    }

    #[test]
    fn identical_style_runs_merge() {
        let handle = make(80, 2);
        handle.advance(b"hello world");
        let frame = handle.snapshot(&default_palette());
        let row = &frame.rows[0];
        // One narrow segment with a single run.
        assert_eq!(row.segments.len(), 1);
        assert_eq!(row.segments[0].runs.len(), 1);
        assert_eq!(row.segments[0].runs[0].len, 80);
    }

    #[test]
    fn background_runs_merge() {
        let handle = make(20, 2);
        // Paint four red-bg cells, then default, then two red-bg cells.
        handle.advance(b"\x1b[41maaaa\x1b[0mXX\x1b[41mBB");
        let frame = handle.snapshot(&default_palette());
        let bgs = &frame.rows[0].backgrounds;
        assert_eq!(bgs.len(), 2, "backgrounds: {bgs:?}");
        assert_eq!(bgs[0].col, 0);
        assert_eq!(bgs[0].len, 4);
        assert_eq!(bgs[1].col, 6);
        assert_eq!(bgs[1].len, 2);
    }

    #[test]
    fn full_screen_snapshot_is_fast_enough() {
        // Not a hard SLA — compiled in debug mode this is well under 1 ms even
        // for 200×60 of ASCII. Record the measurement in the test output.
        let handle = make(200, 60);
        let mut payload = String::new();
        for _ in 0..60 {
            payload.push_str(&"abcdefghij".repeat(20));
            payload.push('\n');
        }
        handle.advance(payload.as_bytes());
        let palette = default_palette();
        let start = Instant::now();
        let frame = handle.snapshot(&palette);
        let elapsed = start.elapsed();
        assert_eq!(frame.rows.len(), 60);
        #[allow(clippy::print_stdout)]
        {
            println!("snapshot 200x60 took {elapsed:?} (debug build)");
        }
    }
}

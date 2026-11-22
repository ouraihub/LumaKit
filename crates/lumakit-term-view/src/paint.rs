//! Painter: given a `Frame` snapshot, fills the background band, shapes one
//! `shape_line` per segment, and overlays cursor + composition. Ported from
//! the PoC-1 implementation with the palette types promoted to
//! `lumakit_term::SnapshotRgba`.

use gpui_kit::{
    App, Bounds, Font, FontStyle, FontWeight, Hsla, Pixels, SharedString, StrikethroughStyle,
    TextAlign, TextRun, UnderlineStyle, Window, fill, point, px, size,
};
use lumakit_term::{CursorShape, Frame, Palette, SnapshotRgba};

use crate::overlay::Overlays;
use crate::palette::{to_hsla, to_rgba};

/// Pixel-level metrics the host hands the painter.
#[derive(Debug, Clone)]
pub struct Metrics {
    /// Width of one cell in logical pixels.
    pub cell_width: f32,
    /// Height of one row in logical pixels.
    pub line_height: f32,
    /// Font size in pixels.
    pub font_size: Pixels,
    /// Base font to use for shaped segments.
    pub font: Font,
}

/// Paint one snapshot. Returns the number of `shape_line` calls made (used
/// by diagnostics / tests).
#[allow(clippy::too_many_arguments)]
pub fn paint_frame(
    frame: &Frame,
    bounds: Bounds<Pixels>,
    m: &Metrics,
    palette: &Palette,
    composition: &str,
    focused: bool,
    overlays: &Overlays,
    window: &mut Window,
    cx: &mut App,
) -> usize {
    window.paint_quad(fill(bounds, to_rgba(palette.background)));

    let cell = |row: usize, col: usize| {
        bounds.origin
            + point(
                px(col as f32 * m.cell_width),
                px(row as f32 * m.line_height),
            )
    };

    // 1. Non-default backgrounds.
    for (row, snap) in frame.rows.iter().enumerate() {
        for bg in &snap.backgrounds {
            window.paint_quad(fill(
                Bounds::new(
                    cell(row, bg.col),
                    size(px(bg.len as f32 * m.cell_width), px(m.line_height)),
                ),
                to_rgba(bg.color),
            ));
        }
    }

    // 2. Keyword / search highlight bands (drawn underneath glyphs).
    for band in overlays.keywords.iter().chain(overlays.search.iter()) {
        let origin = cell(band.range.start_row, band.range.start_col);
        let width = px(
            (band.range.end_col.saturating_sub(band.range.start_col) as f32 + 1.0) * m.cell_width,
        );
        window.paint_quad(fill(
            Bounds::new(origin, size(width, px(m.line_height))),
            band.color,
        ));
    }

    // A block cursor sits under the glyph so the character stays readable;
    // beam and underline are thin and drawn on top (step 5). A hidden cursor
    // (DECTCEM off) paints nothing.
    let cursor = frame.cursor.filter(|c| c.row < frame.rows.len());
    let cursor_color = to_rgba(if focused {
        palette.cursor
    } else {
        SnapshotRgba {
            a: palette.cursor.a / 2,
            ..palette.cursor
        }
    });
    if let Some(c) = cursor
        && c.visible
        && !matches!(c.shape, CursorShape::Beam | CursorShape::Underline)
    {
        window.paint_quad(fill(
            Bounds::new(
                cell(c.row, c.col),
                size(px(m.cell_width), px(m.line_height)),
            ),
            cursor_color,
        ));
    }

    // 3. Shaped segments.
    let mut shaped = 0;
    for (row, snap) in frame.rows.iter().enumerate() {
        for seg in &snap.segments {
            let decorated = seg
                .runs
                .iter()
                .any(|run| run.style.underline || run.style.strike);
            if !decorated && seg.text.trim_end().is_empty() {
                continue;
            }
            let runs: Vec<TextRun> = seg
                .runs
                .iter()
                .map(|run| make_run(run.len, run.style, m))
                .collect();
            let advance = if seg.wide { 2.0 } else { 1.0 } * m.cell_width;
            let line = window.text_system().shape_line(
                SharedString::from(seg.text.clone()),
                m.font_size,
                &runs,
                Some(px(advance)),
            );
            shaped += 1;
            let _ = line.paint(
                cell(row, seg.col),
                px(m.line_height),
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }

    // 4. Ghost text.
    for ghost in &overlays.ghosts {
        let run = TextRun {
            len: ghost.text.len(),
            font: m.font.clone(),
            color: ghost.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            SharedString::from(ghost.text.clone()),
            m.font_size,
            &[run],
            None,
        );
        shaped += 1;
        let _ = line.paint(
            cell(ghost.row, ghost.col),
            px(m.line_height),
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    // 5. Thin cursor + IME composition; the composition still shows where
    // text will land when the cursor is hidden.
    if let Some(c) = cursor {
        let origin = cell(c.row, c.col);
        let thin = match c.shape {
            CursorShape::Beam => Some(Bounds::new(origin, size(px(2.0), px(m.line_height)))),
            CursorShape::Underline => Some(Bounds::new(
                point(origin.x, origin.y + px(m.line_height - 2.0)),
                size(px(m.cell_width), px(2.0)),
            )),
            _ => None,
        };
        if let Some(quad) = thin
            && c.visible
        {
            window.paint_quad(fill(quad, cursor_color));
        }
        if !composition.is_empty() {
            let run = TextRun {
                len: composition.len(),
                font: m.font.clone(),
                color: to_hsla(palette.foreground),
                background_color: Some(to_hsla(palette.background)),
                underline: Some(UnderlineStyle {
                    color: None,
                    thickness: px(1.0),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                SharedString::from(composition.to_owned()),
                m.font_size,
                &[run],
                None,
            );
            let _ = line.paint(origin, px(m.line_height), TextAlign::Left, None, window, cx);
        }
    }

    shaped
}

fn make_run(len: usize, style: lumakit_term::RunStyle, m: &Metrics) -> TextRun {
    let mut font = m.font.clone();
    if style.bold {
        font.weight = FontWeight::BOLD;
    }
    if style.italic {
        font.style = FontStyle::Italic;
    }
    let fg: Hsla = to_hsla(style.foreground);
    TextRun {
        len,
        font,
        color: fg,
        background_color: None,
        underline: style.underline.then(|| UnderlineStyle {
            color: Some(fg),
            thickness: px(1.0),
            wavy: false,
        }),
        strikethrough: style.strike.then(|| StrikethroughStyle {
            color: Some(fg),
            thickness: px(1.0),
        }),
    }
}

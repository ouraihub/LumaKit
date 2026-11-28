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
    /// Font for Private Use Area glyphs (Nerd Font icons). Platform
    /// fallback chains only search installed fonts, so a bundled symbols
    /// font must be named explicitly; `None` leaves PUA text to `font`.
    pub symbols: Option<Font>,
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
            let runs = split_symbols(&seg.text, &seg.runs, m);
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

/// U+E000–U+F8FF and the supplementary private-use planes.
fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFD)
}

/// Text runs for one segment; with a symbols font, each style run is cut
/// where text enters or leaves the Private Use Area.
fn split_symbols(text: &str, runs: &[lumakit_term::StyleRun], m: &Metrics) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len());
    let mut start = 0;
    for run in runs {
        let end = (start + run.len).min(text.len());
        let Some(symbols) = &m.symbols else {
            out.push(make_run(run.len, run.style, m));
            start = end;
            continue;
        };
        let mut piece_start = start;
        let mut piece_pua = None;
        for (offset, c) in text[start..end].char_indices() {
            let pua = is_private_use(c);
            if piece_pua.is_some_and(|p| p != pua) {
                let at = start + offset;
                out.push(styled(
                    at - piece_start,
                    run.style,
                    piece_pua == Some(true),
                    symbols,
                    m,
                ));
                piece_start = at;
            }
            piece_pua = Some(pua);
        }
        if end > piece_start {
            out.push(styled(
                end - piece_start,
                run.style,
                piece_pua == Some(true),
                symbols,
                m,
            ));
        }
        start = end;
    }
    out
}

fn styled(
    len: usize,
    style: lumakit_term::RunStyle,
    symbol: bool,
    symbols: &Font,
    m: &Metrics,
) -> TextRun {
    let mut run = make_run(len, style, m);
    if symbol {
        run.font = Font {
            weight: run.font.weight,
            style: run.font.style,
            ..symbols.clone()
        };
    }
    run
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn font(family: &str) -> Font {
        Font {
            family: family.to_owned().into(),
            features: Default::default(),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
            fallbacks: None,
        }
    }

    fn metrics(symbols: Option<Font>) -> Metrics {
        Metrics {
            cell_width: 8.0,
            line_height: 16.0,
            font_size: px(14.0),
            font: font("Mono"),
            symbols,
        }
    }

    #[test]
    fn private_use_glyphs_get_the_symbols_font() {
        let text = "a\u{e0a0}b";
        let runs = [lumakit_term::StyleRun {
            len: text.len(),
            style: lumakit_term::RunStyle {
                foreground: SnapshotRgba::new(255, 255, 255, 255),
                bold: false,
                italic: false,
                underline: false,
                strike: false,
            },
        }];
        let out = split_symbols(text, &runs, &metrics(Some(font("Symbols"))));
        let families: Vec<_> = out
            .iter()
            .map(|r| (r.len, r.font.family.to_string()))
            .collect();
        assert_eq!(
            families,
            [
                (1, "Mono".into()),
                (3, "Symbols".into()),
                (1, "Mono".into())
            ]
        );
        let plain = split_symbols(text, &runs, &metrics(None));
        assert_eq!(plain.len(), 1);
    }
}

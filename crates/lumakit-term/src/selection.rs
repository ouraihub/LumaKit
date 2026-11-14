//! Selection helpers on top of alacritty's native selection (04 §3.3).
//!
//! The terminal already tracks a mouse-driven selection via
//! `Term::selection`; this module wraps it so hosts hit one API for the four
//! common modes (character, block, semantic word, line) and get the UTF-8
//! text back with wide-char and soft-wrap handling done by the grid.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};

use crate::term::{Listener, TermHandle};

/// Kind of selection the caller wants (maps 1-1 to
/// `alacritty_terminal::selection::SelectionType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// Normal character-wise selection.
    Simple,
    /// Rectangular block selection.
    Block,
    /// Word selection (double-click / `shift+click`).
    Semantic,
    /// Whole lines (triple-click).
    Lines,
}

impl From<SelectionKind> for SelectionType {
    fn from(kind: SelectionKind) -> Self {
        match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Block => SelectionType::Block,
            SelectionKind::Semantic => SelectionType::Semantic,
            SelectionKind::Lines => SelectionType::Lines,
        }
    }
}

/// Grid coordinate used by the selection API; `line` is 0-based from the top
/// of the visible area (negative values address scrollback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPoint {
    /// Signed row offset (as alacritty expects).
    pub line: i32,
    /// Column (0-based).
    pub col: usize,
    /// Which side of the cell the cursor was on when the drag started.
    pub side: GridSide,
}

/// Which side of a cell the pointer was pressed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridSide {
    /// Left half.
    Left,
    /// Right half.
    Right,
}

impl From<GridSide> for Side {
    fn from(side: GridSide) -> Self {
        match side {
            GridSide::Left => Side::Left,
            GridSide::Right => Side::Right,
        }
    }
}

fn to_point(point: GridPoint, screen_lines: usize) -> Point {
    // alacritty treats `Line` as grid-relative where `0` is the top of the
    // visible area and negative values refer to scrollback.
    let _ = screen_lines;
    Point {
        line: Line(point.line),
        column: Column(point.col),
    }
}

impl TermHandle {
    /// Start a new selection anchored at `anchor`.
    pub fn start_selection(&self, kind: SelectionKind, anchor: GridPoint) {
        self.with_term_mut(|term| {
            let screen_lines = term.screen_lines();
            let selection = Selection::new(
                kind.into(),
                to_point(anchor, screen_lines),
                anchor.side.into(),
            );
            term.selection = Some(selection);
        });
    }

    /// Drag the current selection to `cursor`. No-op if nothing is selected.
    pub fn update_selection(&self, cursor: GridPoint) {
        self.with_term_mut(|term| {
            let screen_lines = term.screen_lines();
            if let Some(selection) = term.selection.as_mut() {
                selection.update(to_point(cursor, screen_lines), cursor.side.into());
            }
        });
    }

    /// Clear the current selection.
    pub fn clear_selection(&self) {
        self.with_term_mut(|term| {
            term.selection = None;
        });
    }

    /// Return the selected text as UTF-8; `None` if nothing is selected or
    /// the selection is empty. alacritty handles wide characters and soft
    /// wraps.
    pub fn selection_to_string(&self) -> Option<String> {
        self.with_term(Listener::read_selection)
    }
}

impl Listener {
    fn read_selection<T>(term: &alacritty_terminal::Term<T>) -> Option<String>
    where
        T: alacritty_terminal::event::EventListener,
    {
        term.selection_to_string()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::event::{NullSink, NullWriter};
    use crate::term::{GridSize, TermHandle};

    fn handle(cols: u16, rows: u16) -> TermHandle {
        TermHandle::new(
            GridSize { cols, rows },
            100,
            Arc::new(NullWriter),
            Arc::new(NullSink),
        )
    }

    #[test]
    fn wide_char_selection_includes_both_cells() {
        let handle = handle(32, 2);
        handle.advance("ab中cd文ef字gh".as_bytes());
        handle.start_selection(
            SelectionKind::Simple,
            GridPoint {
                line: 0,
                col: 0,
                side: GridSide::Left,
            },
        );
        handle.update_selection(GridPoint {
            line: 0,
            col: 13,
            side: GridSide::Right,
        });
        let selected = handle.selection_to_string().expect("selection");
        assert!(selected.contains('中'));
        assert!(selected.contains('文'));
        assert!(selected.contains('字'));
    }

    #[test]
    fn line_selection_returns_whole_line() {
        let handle = handle(40, 2);
        handle.advance(b"hello world!\r\n");
        handle.start_selection(
            SelectionKind::Lines,
            GridPoint {
                line: 0,
                col: 0,
                side: GridSide::Left,
            },
        );
        handle.update_selection(GridPoint {
            line: 0,
            col: 5,
            side: GridSide::Right,
        });
        let text = handle.selection_to_string().expect("line selection");
        assert!(text.contains("hello world!"));
    }

    #[test]
    fn clear_removes_selection() {
        let handle = handle(10, 2);
        handle.advance(b"abc");
        handle.start_selection(
            SelectionKind::Simple,
            GridPoint {
                line: 0,
                col: 0,
                side: GridSide::Left,
            },
        );
        handle.update_selection(GridPoint {
            line: 0,
            col: 2,
            side: GridSide::Right,
        });
        assert!(handle.selection_to_string().is_some());
        handle.clear_selection();
        assert!(handle.selection_to_string().is_none());
    }
}

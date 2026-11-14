//! Terminal search (04 §3.3 `search`).
//!
//! alacritty's `RegexSearch` uses smart case by default: a pattern with any
//! uppercase character is treated as case-sensitive, otherwise as
//! case-insensitive. We expose that behaviour as an option: when the host
//! wants strict matching it sets `case_sensitive`, which prepends `(?-i)` to
//! the pattern.
//!
//! Whole-word matching post-filters the raw matches by inspecting the two
//! adjacent cells, because `regex_automata`'s hybrid DFA does not implement
//! `\b` across the whole grid in the way the user expects.

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Column, Line, Point};
use alacritty_terminal::term::search::RegexSearch;

use crate::term::{Listener, TermHandle};

/// Search option flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// Force case-sensitive matching (opt out of alacritty's smart case).
    pub case_sensitive: bool,
    /// Keep only matches whose adjacent cells are non-word characters.
    pub whole_word: bool,
}

/// One hit, inclusive range of grid points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Starting row offset (negative = scrollback).
    pub start_line: i32,
    /// Starting column.
    pub start_col: usize,
    /// Ending row offset.
    pub end_line: i32,
    /// Ending column.
    pub end_col: usize,
}

/// Compile `pattern` into an alacritty regex. `None` when the pattern does
/// not compile.
pub fn compile(pattern: &str, options: SearchOptions) -> Option<RegexSearch> {
    let effective = if options.case_sensitive {
        format!("(?-i){pattern}")
    } else {
        pattern.to_owned()
    };
    RegexSearch::new(&effective).ok()
}

impl TermHandle {
    /// Return every regex hit between the oldest scrollback row and the
    /// bottom of the screen. Hits are returned in reading order; whole-word
    /// mode filters on adjacent non-word cells.
    pub fn search_all(&self, pattern: &str, options: SearchOptions) -> Vec<Match> {
        let Some(mut regex) = compile(pattern, options) else {
            return Vec::new();
        };
        self.with_term(|term| gather(term, &mut regex, options))
    }
}

fn gather(term: &Term<Listener>, regex: &mut RegexSearch, options: SearchOptions) -> Vec<Match> {
    let rows = term.screen_lines();
    let cols = term.columns();
    if cols == 0 {
        return Vec::new();
    }
    let history = term.history_size();
    let mut matches = Vec::new();
    let end = Point::new(Line((rows as i32).saturating_sub(1)), Column(cols - 1));
    let start = Point::new(Line(-(history as i32)), Column(0));
    let mut cursor = start;
    while let Some(hit) = term.regex_search_right(regex, cursor, end) {
        let start_point = *hit.start();
        let end_point = *hit.end();
        let keep = !options.whole_word || whole_word_ok(term, start_point, end_point);
        if keep {
            matches.push(Match {
                start_line: start_point.line.0,
                start_col: start_point.column.0,
                end_line: end_point.line.0,
                end_col: end_point.column.0,
            });
        }
        let next = end_point.add(term, Boundary::Grid, 1);
        if next == end_point {
            break;
        }
        cursor = next;
    }
    matches
}

fn whole_word_ok(term: &Term<Listener>, start: Point, end: Point) -> bool {
    let before = start.sub(term, Boundary::Grid, 1);
    let after = end.add(term, Boundary::Grid, 1);
    let is_word = |p: Point| -> bool {
        if p == start || p == end {
            return false;
        }
        let cell = &term.grid()[p];
        let ch = cell.c;
        ch.is_alphanumeric() || ch == '_'
    };
    !is_word(before) && !is_word(after)
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
    fn plain_matches_are_found() {
        let handle = handle(80, 4);
        handle.advance(b"hello world hello world");
        let matches = handle.search_all("hello", SearchOptions::default());
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn smart_case_matches_regardless_of_case() {
        // Default behaviour: case-insensitive when the pattern has no
        // uppercase letters, matching alacritty's smart case.
        let handle = handle(80, 4);
        handle.advance(b"Hello HELLO hello");
        let smart = handle.search_all("hello", SearchOptions::default());
        assert_eq!(smart.len(), 3);
        let strict = handle.search_all(
            "hello",
            SearchOptions {
                case_sensitive: true,
                whole_word: false,
            },
        );
        assert_eq!(strict.len(), 1);
    }

    #[test]
    fn whole_word_filters_substrings() {
        let handle = handle(80, 4);
        handle.advance(b"cat caterpillar scatter cat");
        let whole = handle.search_all(
            "cat",
            SearchOptions {
                case_sensitive: false,
                whole_word: true,
            },
        );
        assert_eq!(whole.len(), 2, "got: {whole:?}");
        let any = handle.search_all("cat", SearchOptions::default());
        assert!(any.len() >= 3, "got: {any:?}");
    }

    #[test]
    fn regex_supports_character_classes() {
        let handle = handle(80, 4);
        handle.advance(b"abc 123 def 4567");
        let digits = handle.search_all("[0-9]+", SearchOptions::default());
        assert_eq!(digits.len(), 2);
    }

    #[test]
    fn empty_pattern_returns_no_matches() {
        let handle = handle(80, 4);
        handle.advance(b"anything");
        assert!(
            handle
                .search_all("(((", SearchOptions::default())
                .is_empty()
        );
    }
}

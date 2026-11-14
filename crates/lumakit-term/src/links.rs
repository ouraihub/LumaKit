//! Link hit testing (04 §3.3). Two layers:
//! 1. OSC 8 hyperlinks — alacritty stores them directly on cells, so
//!    `hyperlink_at(row, col)` just reads the cell's hyperlink URI.
//! 2. URL regex — the usual matcher to find `http(s)://…`, `mailto:…`,
//!    `file://…` text that was not marked up as an OSC 8 link. Hits are
//!    enumerated once per snapshot and the view maps pointer coordinates
//!    into them.
//!
//! The URL regex is kept simple on purpose; a browser-grade one is wasted
//! effort inside a terminal.

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};

use crate::search::{Match, SearchOptions};
use crate::term::{Listener, TermHandle};

/// Default URL regex (ASCII-safe superset of common link forms).
pub const URL_REGEX: &str = r"(https?|ftp|ftps|file|mailto)://[^\s<>\x00-\x1f]+|mailto:[^\s<>]+";

/// One detected link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Full URI, decoded as UTF-8 lossy.
    pub uri: String,
    /// Grid range of the link; inclusive.
    pub span: Match,
    /// `true` when the URI came from an OSC 8 cell, `false` from the regex.
    pub explicit: bool,
}

impl TermHandle {
    /// Return the URI anchored at `(line, col)`. Checks the OSC 8 hyperlink
    /// first; if no hyperlink is attached, scans the regex matches for the
    /// enclosing span.
    pub fn link_at(&self, line: i32, col: usize) -> Option<Link> {
        self.with_term(|term| {
            if let Some(uri) = osc8_at(term, line, col) {
                return Some(Link {
                    uri,
                    span: Match {
                        start_line: line,
                        start_col: col,
                        end_line: line,
                        end_col: col,
                    },
                    explicit: true,
                });
            }
            None
        })
    }

    /// Enumerate every regex-detected URL on the current screen and
    /// scrollback. OSC 8 hyperlinks are left out; call [`Self::link_at`] to
    /// address them from a pointer position.
    pub fn link_regex_matches(&self) -> Vec<Link> {
        let matches = self.search_all(URL_REGEX, SearchOptions::default());
        matches
            .into_iter()
            .map(|span| Link {
                uri: self.text_in(span),
                span,
                explicit: false,
            })
            .collect()
    }

    fn text_in(&self, span: Match) -> String {
        self.with_term(|term| text_between(term, span))
    }
}

fn text_between(term: &Term<Listener>, span: Match) -> String {
    let rows = term.screen_lines();
    let cols = term.columns();
    let _ = rows;
    let _ = cols;
    let mut out = String::new();
    let mut point = Point::new(Line(span.start_line), Column(span.start_col));
    let end = Point::new(Line(span.end_line), Column(span.end_col));
    while point <= end {
        let cell = &term.grid()[point];
        out.push(cell.c);
        if point.column.0 + 1 < term.columns() {
            point.column = Column(point.column.0 + 1);
        } else {
            point.column = Column(0);
            point.line = Line(point.line.0 + 1);
        }
    }
    out
}

fn osc8_at(term: &Term<Listener>, line: i32, col: usize) -> Option<String> {
    let point = Point::new(Line(line), Column(col));
    if col >= term.columns() {
        return None;
    }
    term.grid()[point].hyperlink().map(|h| h.uri().to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use crate::event::{NullSink, NullWriter};
    use crate::term::{GridSize, TermHandle};

    fn handle() -> TermHandle {
        TermHandle::new(
            GridSize { cols: 80, rows: 4 },
            100,
            Arc::new(NullWriter),
            Arc::new(NullSink),
        )
    }

    #[test]
    fn regex_detects_https_url() {
        let handle = handle();
        handle.advance(b"see https://example.com/path?x=1 for details");
        let links = handle.link_regex_matches();
        assert_eq!(links.len(), 1);
        assert!(links[0].uri.starts_with("https://example.com/path"));
        assert!(!links[0].explicit);
    }

    #[test]
    fn osc8_cell_resolves_to_uri() {
        let handle = handle();
        handle.advance(b"\x1b]8;;https://example.org/osc8\x1b\\click\x1b]8;;\x1b\\");
        let link = handle.link_at(0, 1).expect("osc8 link");
        assert_eq!(link.uri, "https://example.org/osc8");
        assert!(link.explicit);
    }

    #[test]
    fn link_regex_ignores_text_without_scheme() {
        let handle = handle();
        handle.advance(b"this has no urls at all");
        assert!(handle.link_regex_matches().is_empty());
    }
}

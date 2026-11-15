//! Shared terminal core for GUI apps built on alacritty_terminal.
//!
//! `TermHandle` owns a `Term<Listener>` behind a `FairMutex`, so the parsing
//! thread and the UI thread share one ticket and neither can starve the other.
//! `advance(bytes)` parses incoming PTY output; only the false→true edge of
//! `dirty` emits a `TermEvent::Wakeup`, so a burst of output costs at most one
//! wakeup per snapshot. Hosts hand it an `EventSink` that receives title,
//! bell, clipboard-store, cwd, OSC 133 / 9;4 events. Replies to DA / DSR
//! queries flow back through the host-supplied `PtyWriter`.
//!
//! Snapshot, OSC pre-parse, selection, search, link hit testing and
//! absolute-line/timestamp history live in sibling modules; the handle
//! keeps them wired to the same lock.

#![warn(missing_docs)]

mod event;
mod lines;
mod links;
mod osc;
mod search;
mod selection;
mod snapshot;
mod term;

pub use alacritty_terminal::term::TermMode;
pub use alacritty_terminal::vte::ansi::CursorShape;
pub use event::{
    ChannelSink, EventSink, NullSink, NullWriter, OscProgress, Prompt133, PromptKind, PtyWriter,
    TermEvent,
};
pub use lines::{LineHistory, LineStamp};
pub use links::{Link, URL_REGEX};
pub use osc::OscPreParser;
pub use search::{Match, SearchOptions, compile as compile_search_regex};
pub use selection::{GridPoint, GridSide, SelectionKind};
pub use snapshot::{
    BgRun, Cursor, Frame, Palette, RowSnapshot, RunStyle, Segment, SelectionSpan, SnapshotRgba,
    StyleRun,
};
pub use term::{GridSize, TermHandle};

//! The `TermHandle`: one lock around an alacritty `Term`, a `Processor`, and
//! the dirty flag that gates wakeups.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, test::TermSize};
use alacritty_terminal::vte::ansi::Processor;

use crate::event::{EventSink, PtyWriter, TermEvent};

/// Terminal grid dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    /// Column count (>=1).
    pub cols: u16,
    /// Row count (>=1).
    pub rows: u16,
}

/// Owned handle around a locked alacritty `Term` and its VTE parser.
pub struct TermHandle {
    inner: Arc<Inner>,
}

struct Inner {
    term: FairMutex<Term<Listener>>,
    parser: FairMutex<Processor>,
    dirty: AtomicBool,
    writer: Arc<dyn PtyWriter>,
    sink: Arc<dyn EventSink>,
}

impl std::fmt::Debug for TermHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermHandle").finish_non_exhaustive()
    }
}

impl TermHandle {
    /// Build a handle with `size.rows × size.cols` cells and `scrollback`
    /// rows of history.
    pub fn new(
        size: GridSize,
        scrollback: usize,
        writer: Arc<dyn PtyWriter>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let listener = Listener {
            writer: writer.clone(),
            sink: sink.clone(),
        };
        let term = Term::new(
            config,
            &TermSize::new(usize::from(size.cols), usize::from(size.rows)),
            listener,
        );
        Self {
            inner: Arc::new(Inner {
                term: FairMutex::new(term),
                parser: FairMutex::new(Processor::new()),
                dirty: AtomicBool::new(false),
                writer,
                sink,
            }),
        }
    }

    /// Parse incoming bytes into the grid. Marks the terminal dirty; only the
    /// false→true transition emits a `TermEvent::Wakeup`, so a steady stream
    /// of output costs one wakeup per painted frame (`take_dirty` clears the
    /// flag).
    pub fn advance(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut parser = self.inner.parser.lock();
        let mut term = self.inner.term.lock();
        parser.advance(&mut *term, bytes);
        drop(term);
        drop(parser);
        if !self.inner.dirty.swap(true, Ordering::AcqRel) {
            self.inner.sink.send(TermEvent::Wakeup);
        }
    }

    /// Clear the dirty flag; returns the previous value. Call this right
    /// before snapshotting so output parsed afterwards wakes the view again.
    pub fn take_dirty(&self) -> bool {
        self.inner.dirty.swap(false, Ordering::AcqRel)
    }

    /// Current grid size reported by the term.
    pub fn size(&self) -> GridSize {
        let term = self.inner.term.lock();
        GridSize {
            cols: term.columns() as u16,
            rows: term.screen_lines() as u16,
        }
    }

    /// Resize the terminal to `size`. The host is still responsible for
    /// resizing the PTY master.
    pub fn resize(&self, size: GridSize) {
        let mut term = self.inner.term.lock();
        let cols = usize::from(size.cols);
        let rows = usize::from(size.rows);
        if term.columns() != cols || term.screen_lines() != rows {
            term.resize(TermSize::new(cols, rows));
        }
        drop(term);
        if !self.inner.dirty.swap(true, Ordering::AcqRel) {
            self.inner.sink.send(TermEvent::Wakeup);
        }
    }

    /// Change the scrollback-history limit (04 §16 `terminal.scrollback`).
    pub fn set_scrollback(&self, lines: usize) {
        let mut term = self.inner.term.lock();
        term.set_options(Config {
            scrolling_history: lines,
            ..Config::default()
        });
    }

    /// Lock the underlying term for the duration of `f`. Snapshot, selection
    /// and search modules in later T1.09–T1.12 steps use this; callers must
    /// keep the critical section short so the reader thread does not starve.
    pub fn with_term<R>(&self, f: impl FnOnce(&Term<Listener>) -> R) -> R {
        let term = self.inner.term.lock();
        f(&term)
    }

    /// Mutable variant of [`Self::with_term`] for operations that update the
    /// grid (search, selection drag).
    pub fn with_term_mut<R>(&self, f: impl FnOnce(&mut Term<Listener>) -> R) -> R {
        let mut term = self.inner.term.lock();
        f(&mut term)
    }

    /// Clone of the host-supplied writer, for reply paths that live in
    /// sibling modules.
    pub fn writer(&self) -> Arc<dyn PtyWriter> {
        self.inner.writer.clone()
    }
}

/// Alacritty listener that forwards PtyWrite replies to the host writer and
/// turns the other events into [`TermEvent`]s the sink receives.
#[derive(Clone)]
pub struct Listener {
    writer: Arc<dyn PtyWriter>,
    sink: Arc<dyn EventSink>,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener").finish_non_exhaustive()
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.writer.write(text.as_bytes()),
            Event::Title(title) => self.sink.send(TermEvent::Title(title)),
            Event::Bell => self.sink.send(TermEvent::Bell),
            Event::ClipboardStore(target, text) => {
                let letter = match target {
                    alacritty_terminal::term::ClipboardType::Clipboard => 'c',
                    alacritty_terminal::term::ClipboardType::Selection => 's',
                };
                self.sink.send(TermEvent::ClipboardStore {
                    target: Some(letter),
                    text,
                });
            }
            Event::ResetTitle => self.sink.send(TermEvent::Title(String::new())),
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use crate::event::{ChannelSink, NullWriter, PtyWriter, TermEvent};

    use super::*;

    #[derive(Default)]
    struct RecordWriter {
        buf: Mutex<Vec<u8>>,
    }

    impl PtyWriter for RecordWriter {
        fn write(&self, bytes: &[u8]) {
            if let Ok(mut guard) = self.buf.lock() {
                guard.extend_from_slice(bytes);
            }
        }
    }

    fn handle_with(writer: Arc<dyn PtyWriter>) -> (TermHandle, async_channel::Receiver<TermEvent>) {
        let (sink, rx) = ChannelSink::bounded(256);
        let handle = TermHandle::new(
            GridSize { cols: 80, rows: 24 },
            1000,
            writer,
            Arc::new(sink),
        );
        (handle, rx)
    }

    #[test]
    fn one_wakeup_per_burst_until_take_dirty() {
        let (handle, rx) = handle_with(Arc::new(NullWriter));
        for _ in 0..100 {
            handle.advance(b"hello ");
        }
        let mut wakeups = 0;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, TermEvent::Wakeup) {
                wakeups += 1;
            }
        }
        assert_eq!(wakeups, 1, "burst produced {wakeups} wakeups, expected 1");
        assert!(handle.take_dirty(), "dirty should have been set by advance");
        // Next burst wakes once again.
        handle.advance(b"again");
        let mut after = 0;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, TermEvent::Wakeup) {
                after += 1;
            }
        }
        assert_eq!(after, 1, "post-clear burst produced {after} wakeups");
    }

    #[test]
    fn resize_changes_grid_size_and_wakes() {
        let (handle, rx) = handle_with(Arc::new(NullWriter));
        handle.resize(GridSize {
            cols: 120,
            rows: 40,
        });
        assert_eq!(
            handle.size(),
            GridSize {
                cols: 120,
                rows: 40
            }
        );
        let mut saw_wakeup = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, TermEvent::Wakeup) {
                saw_wakeup = true;
            }
        }
        assert!(saw_wakeup);
    }

    #[test]
    fn da_query_is_answered_through_writer() {
        let writer = Arc::new(RecordWriter::default());
        let (handle, _rx) = handle_with(writer.clone());
        // CSI c = primary device attributes query.
        handle.advance(b"\x1b[c");
        // Alacritty's primary DA reply is `CSI ? 6 2 ; ...`; just make sure
        // *something* was written back.
        let captured = writer.buf.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(!captured.is_empty(), "DA query produced no reply");
        assert!(
            captured.starts_with(b"\x1b["),
            "unexpected DA reply: {captured:?}"
        );
    }

    #[test]
    fn dsr_cursor_position_is_answered() {
        let writer = Arc::new(RecordWriter::default());
        let (handle, _rx) = handle_with(writer.clone());
        handle.advance(b"\x1b[6n");
        let captured = writer.buf.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(
            captured.starts_with(b"\x1b[") && captured.ends_with(b"R"),
            "unexpected DSR reply: {captured:?}"
        );
    }

    #[test]
    fn empty_advance_does_not_wake() {
        let (handle, rx) = handle_with(Arc::new(NullWriter));
        handle.advance(b"");
        assert!(rx.try_recv().is_err());
        assert!(!handle.take_dirty());
    }
}

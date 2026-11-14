//! Events the handle surfaces to the host. The host decides whether to
//! forward them into its own domain event type (AutoTerm converts them into
//! `TerminalEvent`; ThinkRail wraps them into its session stream).

use std::fmt;
use std::sync::Arc;

/// A single terminal event. Non-exhaustive: callers must `match _ => {}` so
/// new variants in minor releases do not break downstream matches.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// Window title changed (OSC 0 / 2).
    Title(String),
    /// Hardware bell / BEL byte.
    Bell,
    /// Grid is dirty (bytes arrived); host should schedule a snapshot.
    Wakeup,
    /// OSC 52 clipboard write request.
    ClipboardStore {
        /// Selection target: `c` (clipboard), `s` (selection) or empty.
        target: Option<char>,
        /// Decoded clipboard payload (UTF-8 lossy).
        text: String,
    },
    /// OSC 7: process current working directory.
    Cwd(String),
    /// OSC 133 prompt marker (`A` start, `B` end, `C` output start,
    /// `D[;exit_code]` output end).
    Prompt(Prompt133),
    /// OSC 9;4 taskbar progress.
    Progress(OscProgress),
    /// Mode change (alternate screen, bracketed paste, mouse, …).
    ModeChange(super::TermMode),
}

/// Prompt marker payload, derived from OSC 133.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt133 {
    /// `A`: prompt start.
    PromptStart,
    /// `B`: prompt end / input start.
    PromptEnd,
    /// `C`: output start.
    OutputStart,
    /// `D[;exit_code]`: output end.
    OutputEnd {
        /// Exit code reported by the shell, when present.
        exit_code: Option<i32>,
    },
}

/// Legacy alias for the prompt-marker kinds; kept for host code migrating off
/// ad-hoc constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// `A` prompt start.
    PromptStart,
    /// `B` prompt end.
    PromptEnd,
    /// `C` output start.
    OutputStart,
    /// `D` output end.
    OutputEnd,
}

/// OSC 9;4 progress state. `state` 0 clears, 1 normal, 2 error, 3 indeterminate, 4 paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OscProgress {
    /// Progress state code.
    pub state: u8,
    /// Progress value 0-100 (if the sequence included one).
    pub value: Option<u8>,
}

/// A sink that receives [`TermEvent`]s. The host decides whether to forward
/// synchronously (channels), re-emit into its own event bus, or drop.
pub trait EventSink: Send + Sync + 'static {
    /// Deliver one event; the method is called from the parsing thread while
    /// the term lock is held, so handlers must not block or re-enter the
    /// handle.
    fn send(&self, event: TermEvent);
}

/// Sink that forwards events to an async channel; convenience for common hosts.
#[allow(dead_code)]
#[derive(Clone)]
pub struct ChannelSink {
    sender: async_channel::Sender<TermEvent>,
}

impl fmt::Debug for ChannelSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelSink").finish_non_exhaustive()
    }
}

#[allow(dead_code)]
impl ChannelSink {
    /// Create a bounded channel (`capacity` messages) and return the sink and
    /// its receiver side. Callers that cannot afford drops should poll the
    /// receiver on every frame.
    pub fn bounded(capacity: usize) -> (Self, async_channel::Receiver<TermEvent>) {
        let (sender, receiver) = async_channel::bounded(capacity);
        (Self { sender }, receiver)
    }
}

impl EventSink for ChannelSink {
    fn send(&self, event: TermEvent) {
        let _ = self.sender.try_send(event);
    }
}

/// A sink that silently drops every event. Useful in tests where only the
/// `Wakeup` edge matters and wiring a channel would be noise.
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub struct NullSink;
impl EventSink for NullSink {
    fn send(&self, _: TermEvent) {}
}

/// Writer the handle calls to flush DA / DSR / clipboard replies back to the
/// PTY. Hosts usually wrap a locked `Box<dyn Write>` around the master side.
pub trait PtyWriter: Send + Sync + 'static {
    /// Write all of `bytes` to the PTY; errors are swallowed because the
    /// handle has no way to surface them.
    fn write(&self, bytes: &[u8]);
}

/// A no-op writer, for tests that do not exercise replies.
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub struct NullWriter;
impl PtyWriter for NullWriter {
    fn write(&self, _: &[u8]) {}
}

/// Blanket impl so hosts can hand a bare `Arc<T>` where `T: PtyWriter`.
impl<T: PtyWriter + ?Sized> PtyWriter for Arc<T> {
    fn write(&self, bytes: &[u8]) {
        (**self).write(bytes)
    }
}

impl<T: EventSink + ?Sized> EventSink for Arc<T> {
    fn send(&self, event: TermEvent) {
        (**self).send(event)
    }
}

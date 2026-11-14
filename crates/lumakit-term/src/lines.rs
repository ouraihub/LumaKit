//! Absolute line numbers and timestamps (03 §14.2). alacritty's grid tracks a
//! growing line index but the terminal view needs a monotonic row counter
//! plus the wall-clock time each row first appeared, so the UI can render a
//! gutter with both. We keep the mapping outside alacritty because its grid
//! prunes history by row index, not by absolute number.
//!
//! The host feeds `on_line_appended()` each time a new row enters the
//! terminal (the pty reader or the `TermHandle::advance` callback) and
//! `on_scrollback_trim(n)` when the scrollback cap evicts the oldest `n`
//! rows. `current_screen()` returns the absolute numbers currently visible.

use std::collections::VecDeque;
use std::time::SystemTime;

/// One recorded row: absolute line number and first-write timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineStamp {
    /// Monotonically increasing line number (0-based).
    pub abs: u64,
    /// Wall-clock time the row first appeared.
    pub appeared_at: SystemTime,
}

/// Rolling map of absolute line numbers to timestamps, bounded by the
/// terminal's scrollback cap. Trimming the oldest rows keeps the structure
/// `O(cap)` regardless of session length.
#[derive(Debug, Clone)]
pub struct LineHistory {
    /// Oldest-first queue of stamps; indices stay aligned with the alacritty
    /// grid even after `on_scrollback_trim`.
    stamps: VecDeque<LineStamp>,
    /// Next absolute line number to assign.
    next_abs: u64,
    /// Scrollback cap in rows; `on_line_appended` evicts the oldest entry
    /// once the queue exceeds it.
    cap: usize,
    /// Rows above the alt screen are frozen: entries do not change while the
    /// terminal is on the alt screen.
    frozen: Option<FrozenSnapshot>,
}

#[derive(Debug, Clone)]
struct FrozenSnapshot {
    stamps: VecDeque<LineStamp>,
    next_abs: u64,
}

impl LineHistory {
    /// Create an empty history with the given scrollback cap (number of rows
    /// to keep, including the visible screen).
    pub fn new(cap: usize) -> Self {
        Self {
            stamps: VecDeque::with_capacity(cap.min(16_384)),
            next_abs: 0,
            cap,
            frozen: None,
        }
    }

    /// Replace the scrollback cap. Rows above the new cap are evicted in
    /// age order.
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap;
        self.trim();
    }

    /// Record one newly appended row; returns its absolute line number.
    pub fn on_line_appended(&mut self, appeared_at: SystemTime) -> u64 {
        let abs = self.next_abs;
        self.next_abs = self.next_abs.saturating_add(1);
        self.stamps.push_back(LineStamp { abs, appeared_at });
        self.trim();
        abs
    }

    /// Trim the oldest `count` rows (called when alacritty evicts them).
    pub fn on_scrollback_trim(&mut self, count: usize) {
        for _ in 0..count.min(self.stamps.len()) {
            self.stamps.pop_front();
        }
    }

    /// Switched to the alternate screen: snapshot the main screen so entering
    /// and leaving the alt screen does not disturb its absolute numbers.
    pub fn enter_alt_screen(&mut self) {
        if self.frozen.is_none() {
            self.frozen = Some(FrozenSnapshot {
                stamps: self.stamps.clone(),
                next_abs: self.next_abs,
            });
        }
    }

    /// Left the alternate screen: restore the frozen main-screen mapping.
    pub fn leave_alt_screen(&mut self) {
        if let Some(frozen) = self.frozen.take() {
            self.stamps = frozen.stamps;
            self.next_abs = frozen.next_abs;
        }
    }

    /// Absolute line number of the first retained row.
    pub fn first_abs(&self) -> Option<u64> {
        self.stamps.front().map(|s| s.abs)
    }

    /// Absolute line number of the last retained row.
    pub fn last_abs(&self) -> Option<u64> {
        self.stamps.back().map(|s| s.abs)
    }

    /// Number of retained rows.
    pub fn len(&self) -> usize {
        self.stamps.len()
    }

    /// `true` when no rows have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.stamps.is_empty()
    }

    /// Stamp for the given absolute line, or `None` if it has been evicted.
    pub fn get(&self, abs: u64) -> Option<LineStamp> {
        let first = self.stamps.front()?.abs;
        let idx = abs.checked_sub(first)? as usize;
        self.stamps.get(idx).copied()
    }

    /// Visible screen's absolute numbers. `screen_lines` is the live screen
    /// height; the newest `screen_lines` entries are returned oldest-first.
    pub fn current_screen(&self, screen_lines: usize) -> Vec<LineStamp> {
        let len = self.stamps.len();
        let start = len.saturating_sub(screen_lines);
        self.stamps.range(start..).copied().collect()
    }

    fn trim(&mut self) {
        while self.stamps.len() > self.cap {
            self.stamps.pop_front();
        }
    }
}

impl Default for LineHistory {
    fn default() -> Self {
        Self::new(10_000)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn sample() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    #[test]
    fn appended_rows_get_sequential_absolute_numbers() {
        let mut history = LineHistory::new(10);
        for i in 0..5 {
            let abs = history.on_line_appended(sample() + Duration::from_secs(i));
            assert_eq!(abs, i);
        }
        assert_eq!(history.first_abs(), Some(0));
        assert_eq!(history.last_abs(), Some(4));
    }

    #[test]
    fn exceeding_cap_evicts_oldest() {
        let mut history = LineHistory::new(3);
        for _ in 0..6 {
            history.on_line_appended(sample());
        }
        assert_eq!(history.len(), 3);
        assert_eq!(history.first_abs(), Some(3));
        assert_eq!(history.last_abs(), Some(5));
    }

    #[test]
    fn explicit_trim_removes_oldest() {
        let mut history = LineHistory::new(100);
        for _ in 0..10 {
            history.on_line_appended(sample());
        }
        history.on_scrollback_trim(4);
        assert_eq!(history.len(), 6);
        assert_eq!(history.first_abs(), Some(4));
    }

    #[test]
    fn alt_screen_round_trip_preserves_numbers() {
        let mut history = LineHistory::new(100);
        for _ in 0..5 {
            history.on_line_appended(sample());
        }
        history.enter_alt_screen();
        for _ in 0..3 {
            history.on_line_appended(sample());
        }
        history.leave_alt_screen();
        assert_eq!(history.len(), 5, "alt screen edits leaked into main screen");
        assert_eq!(history.last_abs(), Some(4));
    }

    #[test]
    fn get_resolves_absolute_number_into_timestamp() {
        let mut history = LineHistory::new(10);
        let base = sample();
        for i in 0..5 {
            history.on_line_appended(base + Duration::from_secs(i));
        }
        let stamp = history.get(2).unwrap();
        assert_eq!(stamp.appeared_at, base + Duration::from_secs(2));
        assert!(history.get(999).is_none());
    }

    #[test]
    fn current_screen_returns_tail() {
        let mut history = LineHistory::new(100);
        for _ in 0..10 {
            history.on_line_appended(sample());
        }
        let screen = history.current_screen(4);
        assert_eq!(screen.len(), 4);
        assert_eq!(screen.first().unwrap().abs, 6);
        assert_eq!(screen.last().unwrap().abs, 9);
    }

    #[test]
    fn set_cap_shrinks_history() {
        let mut history = LineHistory::new(100);
        for _ in 0..20 {
            history.on_line_appended(sample());
        }
        history.set_cap(5);
        assert_eq!(history.len(), 5);
        assert_eq!(history.first_abs(), Some(15));
    }
}

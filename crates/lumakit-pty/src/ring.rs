//! Byte ring buffer for raw PTY output.
//!
//! Replaces autorail's "replay + offset" history (which grew without bound
//! until an explicit reset) with a fixed-capacity circular buffer. On
//! overflow the oldest bytes are evicted. 128 KiB is the recommended minimum
//! for AutoTerm (one screenful of 200×60 characters + headroom).

use std::collections::VecDeque;

/// FIFO byte buffer with a hard capacity. `push` always succeeds; the oldest
/// bytes are dropped to make room.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    data: VecDeque<u8>,
    capacity: usize,
}

impl RingBuffer {
    /// Create a buffer that holds at most `capacity` bytes.
    pub fn new(capacity: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(capacity.min(1 << 24)),
            capacity,
        }
    }

    /// Append `bytes`. Oldest bytes are evicted if the buffer would exceed
    /// its capacity.
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.capacity {
            self.data.clear();
            let start = bytes.len() - self.capacity;
            self.data.extend(&bytes[start..]);
            return;
        }
        let overflow = (self.data.len() + bytes.len()).saturating_sub(self.capacity);
        for _ in 0..overflow {
            self.data.pop_front();
        }
        self.data.extend(bytes);
    }

    /// Current byte count.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// `true` when empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Hard capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Change the cap. Oldest bytes are evicted if the new cap is smaller.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        while self.data.len() > capacity {
            self.data.pop_front();
        }
    }

    /// Clone the current contents oldest-first. Allocates; call sparingly.
    pub fn snapshot(&self) -> Vec<u8> {
        self.data.iter().copied().collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn push_within_capacity_keeps_everything() {
        let mut ring = RingBuffer::new(16);
        ring.push(b"hello");
        ring.push(b" world");
        assert_eq!(ring.snapshot(), b"hello world");
    }

    #[test]
    fn overflow_keeps_newest() {
        let mut ring = RingBuffer::new(8);
        ring.push(b"0123456789abcdef");
        assert_eq!(ring.snapshot(), b"89abcdef");
    }

    #[test]
    fn multi_push_overflow() {
        let mut ring = RingBuffer::new(5);
        ring.push(b"abc");
        ring.push(b"def");
        ring.push(b"gh");
        assert_eq!(ring.snapshot(), b"defgh");
    }

    #[test]
    fn set_capacity_trims_oldest() {
        let mut ring = RingBuffer::new(10);
        ring.push(b"abcdefghij");
        ring.set_capacity(4);
        assert_eq!(ring.snapshot(), b"ghij");
    }
}

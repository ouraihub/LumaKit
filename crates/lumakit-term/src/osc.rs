//! OSC pre-parser: scans PTY output bytes for OSC 7 (cwd), OSC 133 (A/B/C/D
//! prompt markers) and OSC 9;4 (taskbar progress) before handing them to the
//! alacritty parser verbatim. The state machine handles cross-block splits
//! and both terminators (BEL `0x07` and ST `\x1b\\`); malformed sequences
//! reset the scanner without panicking or discarding bytes.
//!
//! This layer never deletes bytes from the stream: alacritty still owns the
//! canonical semantics for every OSC it recognises. The pre-parse exists so
//! AutoTerm can surface prompt markers / cwd / progress as events without
//! having to read them off the live grid.

use crate::event::{EventSink, OscProgress, Prompt133, TermEvent};

/// Streaming OSC scanner. One instance per PTY; feed every byte with
/// [`OscPreParser::feed`].
#[derive(Debug, Default)]
pub struct OscPreParser {
    state: State,
    /// Accumulated payload between `OSC` and the terminator; capped at 4 KiB
    /// so a runaway sequence cannot grow unbounded.
    payload: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum State {
    /// Normal passthrough.
    #[default]
    Text,
    /// Saw `\x1b`, waiting for `]` or any other byte (which resets).
    Escape,
    /// Inside an `OSC … ST / BEL` payload.
    Osc,
    /// Inside an OSC payload, just consumed the ESC of a possible `ESC \` ST.
    OscEsc,
}

const MAX_PAYLOAD: usize = 4 * 1024;

impl OscPreParser {
    /// Create an empty scanner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed `bytes` into the scanner, emitting events through `sink` as
    /// complete OSC sequences arrive. Bytes are not modified or dropped.
    pub fn feed(&mut self, bytes: &[u8], sink: &dyn EventSink) {
        for &byte in bytes {
            match self.state {
                State::Text => {
                    if byte == 0x1b {
                        self.state = State::Escape;
                    }
                }
                State::Escape => match byte {
                    b']' => {
                        self.payload.clear();
                        self.state = State::Osc;
                    }
                    0x1b => self.state = State::Escape,
                    _ => self.state = State::Text,
                },
                State::Osc => match byte {
                    0x07 => self.finish(sink),
                    0x1b => self.state = State::OscEsc,
                    _ => {
                        if self.payload.len() < MAX_PAYLOAD {
                            self.payload.push(byte);
                        }
                    }
                },
                State::OscEsc => match byte {
                    b'\\' => self.finish(sink),
                    0x1b => self.state = State::OscEsc,
                    other => {
                        if self.payload.len() + 2 < MAX_PAYLOAD {
                            self.payload.push(0x1b);
                            self.payload.push(other);
                        }
                        self.state = State::Osc;
                    }
                },
            }
        }
    }

    fn finish(&mut self, sink: &dyn EventSink) {
        if let Some(event) = parse_payload(&self.payload) {
            sink.send(event);
        }
        self.payload.clear();
        self.state = State::Text;
    }
}

fn parse_payload(payload: &[u8]) -> Option<TermEvent> {
    let text = std::str::from_utf8(payload).ok()?;
    let (head, rest) = text.split_once(';').unwrap_or((text, ""));
    match head {
        "7" => Some(TermEvent::Cwd(decode_cwd(rest))),
        "133" => parse_133(rest),
        "9" => rest.strip_prefix("4;").and_then(parse_progress),
        _ => None,
    }
}

fn parse_133(rest: &str) -> Option<TermEvent> {
    let mut parts = rest.splitn(2, ';');
    let kind = parts.next()?;
    let args = parts.next().unwrap_or("");
    let prompt = match kind {
        "A" => Prompt133::PromptStart,
        "B" => Prompt133::PromptEnd,
        "C" => Prompt133::OutputStart,
        "D" => {
            let exit_code = args
                .split(';')
                .find_map(|segment| segment.strip_prefix("code=").or(Some(segment)))
                .and_then(|value| value.parse::<i32>().ok());
            Prompt133::OutputEnd { exit_code }
        }
        _ => return None,
    };
    Some(TermEvent::Prompt(prompt))
}

fn parse_progress(rest: &str) -> Option<TermEvent> {
    let mut parts = rest.split(';');
    let state: u8 = parts.next()?.parse().ok()?;
    let value = parts.next().and_then(|v| v.parse::<u8>().ok());
    Some(TermEvent::Progress(OscProgress { state, value }))
}

/// Decodes `file://host/path` → the raw path portion. Percent-encoded bytes
/// are turned back into UTF-8 bytes; invalid sequences leave the `%` and the
/// following two bytes verbatim.
fn decode_cwd(raw: &str) -> String {
    let path = raw.strip_prefix("file://").map_or(raw, |rest| {
        // Drop the host component: everything up to the next `/` (inclusive).
        rest.find('/').map_or(rest, |slash| &rest[slash..])
    });
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = from_hex(bytes[i + 1]);
            let lo = from_hex(bytes[i + 2]);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::event::EventSink;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<TermEvent>>);

    impl EventSink for Recorder {
        fn send(&self, event: TermEvent) {
            if let Ok(mut guard) = self.0.lock() {
                guard.push(event);
            }
        }
    }

    fn feed_and_collect(chunks: &[&[u8]]) -> Vec<TermEvent> {
        let sink = Recorder::default();
        let mut parser = OscPreParser::new();
        for chunk in chunks {
            parser.feed(chunk, &sink);
        }
        sink.0.into_inner().unwrap_or_default()
    }

    #[test]
    fn osc_7_bel_terminator() {
        let events = feed_and_collect(&[b"\x1b]7;file://host/home/me\x07"]);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], TermEvent::Cwd(path) if path == "/home/me"));
    }

    #[test]
    fn osc_7_st_terminator() {
        let events = feed_and_collect(&[b"\x1b]7;file:///tmp\x1b\\"]);
        assert!(matches!(&events[0], TermEvent::Cwd(path) if path == "/tmp"));
    }

    #[test]
    fn osc_7_percent_decoding() {
        let events = feed_and_collect(&[b"\x1b]7;file:///home/me/%E6%B5%8B%E8%AF%95\x07"]);
        if let TermEvent::Cwd(path) = &events[0] {
            assert_eq!(path, "/home/me/测试");
        } else {
            panic!("no cwd event: {events:?}");
        }
    }

    #[test]
    fn osc_133_split_across_two_chunks() {
        let events = feed_and_collect(&[b"\x1b]133", b";A\x07"]);
        assert!(matches!(
            events.as_slice(),
            [TermEvent::Prompt(Prompt133::PromptStart)]
        ));
    }

    #[test]
    fn osc_133_split_across_three_chunks() {
        let events = feed_and_collect(&[b"\x1b", b"]133;", b"D;0\x07"]);
        assert!(matches!(
            events.as_slice(),
            [TermEvent::Prompt(Prompt133::OutputEnd {
                exit_code: Some(0)
            })]
        ));
    }

    #[test]
    fn osc_9_4_progress() {
        let events = feed_and_collect(&[b"\x1b]9;4;1;42\x07"]);
        if let TermEvent::Progress(p) = &events[0] {
            assert_eq!(p.state, 1);
            assert_eq!(p.value, Some(42));
        } else {
            panic!("no progress event: {events:?}");
        }
    }

    #[test]
    fn malformed_osc_does_not_panic_and_recovers() {
        // Bogus OSC number, then a real OSC 7.
        let events = feed_and_collect(&[b"\x1b]bogus;garbage\x07", b"\x1b]7;file:///a\x07"]);
        assert!(matches!(&events[0], TermEvent::Cwd(path) if path == "/a"));
    }

    #[test]
    fn bytes_pass_through_in_parser_memory_only() {
        // The pre-parser never removes bytes; the host still passes the
        // original stream to alacritty. This test mirrors that by asserting
        // `feed` does not mutate the input buffer.
        let payload = b"\x1b]7;file:///tmp\x07".to_vec();
        let copy = payload.clone();
        let sink = Recorder::default();
        let mut parser = OscPreParser::new();
        parser.feed(&payload, &sink);
        assert_eq!(payload, copy);
    }
}

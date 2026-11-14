//! Streaming decoder that stitches multibyte characters across read
//! boundaries (03 §14.2). UTF-8 input is passed straight through so the
//! common case stays zero-copy; other encodings go through `encoding_rs`
//! with a residue buffer for partial sequences at the end of a chunk.

use encoding_rs::{Decoder as RsDecoder, Encoding, UTF_8};
use thiserror::Error;

/// Decoder errors.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// Encoding label is not recognised by `encoding_rs`.
    #[error("unknown encoding: {0}")]
    UnknownEncoding(String),
}

/// Streaming decoder. `feed` returns UTF-8 output derived from `bytes`.
/// Partial sequences at the tail are buffered until the next call.
#[derive(Debug)]
pub struct Decoder {
    encoding: &'static Encoding,
    inner: Option<RsDecoder>,
    /// Residue bytes left over from the previous `feed` (encodings where
    /// sequences can span reads).
    residue: Vec<u8>,
}

impl Decoder {
    /// Create a decoder for `label` (IANA / WHATWG).
    ///
    /// # Errors
    /// Returns [`DecodeError::UnknownEncoding`] when `label` is unknown.
    pub fn new(label: &str) -> Result<Self, DecodeError> {
        let encoding = Encoding::for_label(label.as_bytes())
            .ok_or_else(|| DecodeError::UnknownEncoding(label.to_owned()))?;
        Ok(Self {
            encoding,
            inner: if encoding == UTF_8 {
                None
            } else {
                Some(encoding.new_decoder())
            },
            residue: Vec::new(),
        })
    }

    /// UTF-8 decoder (zero-copy pass-through).
    pub fn utf8() -> Self {
        Self {
            encoding: UTF_8,
            inner: None,
            residue: Vec::new(),
        }
    }

    /// Encoding label.
    pub fn label(&self) -> &'static str {
        self.encoding.name()
    }
    /// Feed another chunk; returns freshly decoded UTF-8 text.
    pub fn feed(&mut self, bytes: &[u8]) -> String {
        match &mut self.inner {
            None => {
                // UTF-8 fast path. Append to residue only if the chunk ends
                // mid-sequence; otherwise emit directly.
                self.residue.extend_from_slice(bytes);
                let residue = std::mem::take(&mut self.residue);
                let (text, tail) = split_at_utf8_boundary(&residue);
                let out = text.to_owned();
                self.residue = tail.to_vec();
                out
            }
            Some(decoder) => {
                // encoding_rs owns its own partial-sequence state when we
                // pass `last=false`. Allocate a safe worst-case output
                // buffer so the decoder never returns `OutputFull`.
                let mut out = String::new();
                out.reserve(
                    decoder
                        .max_utf8_buffer_length(bytes.len())
                        .unwrap_or(bytes.len() * 4 + 4),
                );
                let (_result, _read, _rep) = decoder.decode_to_string(bytes, &mut out, false);
                out
            }
        }
    }

    /// Flush any remaining residue; called when the stream ends.
    pub fn finish(mut self) -> String {
        if let Some(mut decoder) = self.inner.take() {
            let mut out = String::new();
            out.reserve(decoder.max_utf8_buffer_length(0).unwrap_or(16));
            let (_r, _read, _rep) = decoder.decode_to_string(b"", &mut out, true);
            out
        } else {
            let text = String::from_utf8_lossy(&self.residue).into_owned();
            self.residue.clear();
            text
        }
    }
}
/// Split `bytes` into the longest valid UTF-8 prefix and the trailing
/// partial sequence. A partial sequence is at most 3 bytes long.
fn split_at_utf8_boundary(bytes: &[u8]) -> (&str, &[u8]) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text, &[]),
        Err(error) => {
            let valid_up_to = error.valid_up_to();
            match error.error_len() {
                Some(_) => {
                    // Genuine invalid sequence; feed the prefix as-is and
                    // keep the rest as residue so the next chunk may contain
                    // the completion. `from_utf8_lossy` would eat the error.
                    let (head, tail) = bytes.split_at(valid_up_to);
                    (std::str::from_utf8(head).unwrap_or(""), tail)
                }
                None => {
                    let (head, tail) = bytes.split_at(valid_up_to);
                    (std::str::from_utf8(head).unwrap_or(""), tail)
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_mid_sequence_preserves_byte_integrity() {
        // "中" in UTF-8 is E4 B8 AD.
        let mut decoder = Decoder::utf8();
        let part_a = decoder.feed(&[0xe4, 0xb8]);
        assert_eq!(part_a, "");
        let part_b = decoder.feed(&[0xad, b'a']);
        assert_eq!(part_b, "中a");
    }

    #[test]
    fn gbk_split_mid_sequence_stitches_correctly() {
        // "中文" in GBK: D6 D0 CE C4.
        let mut decoder = Decoder::new("gbk").expect("gbk supported");
        let a = decoder.feed(&[0xd6]);
        let b = decoder.feed(&[0xd0, 0xce]);
        let c = decoder.feed(&[0xc4]);
        let joined = format!("{a}{b}{c}");
        assert_eq!(joined, "中文");
    }
    #[test]
    fn shift_jis_split_decodes() {
        // "日本" in Shift_JIS: 93 FA 96 7B.
        let mut decoder = Decoder::new("shift_jis").expect("shift_jis supported");
        let a = decoder.feed(&[0x93]);
        let b = decoder.feed(&[0xfa, 0x96]);
        let c = decoder.feed(&[0x7b]);
        let d = decoder.finish();
        let joined = format!("{a}{b}{c}{d}");
        assert_eq!(joined, "日本");
    }

    #[test]
    fn unknown_encoding_errors() {
        assert!(Decoder::new("not-a-real-encoding").is_err());
    }

    #[test]
    fn utf8_passthrough_simple() {
        let mut decoder = Decoder::utf8();
        assert_eq!(decoder.feed(b"hello"), "hello");
    }
}

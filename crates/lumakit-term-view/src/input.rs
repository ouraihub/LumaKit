//! Keyboard → VT byte sequences and IME composition helpers.
//!
//! Ported from autorail `panels/terminal/input.rs`. Printable text comes
//! through the platform IME; `encode_key` deliberately returns `None` for
//! bare printable keys so the byte is never sent twice.

use std::ops::Range;

use gpui_kit::Keystroke;

/// Byte offset of a UTF-16 `offset` in `text` (clamped to the end).
pub fn utf16_byte(text: &str, offset: usize) -> usize {
    let mut count = 0;
    for (byte, ch) in text.char_indices() {
        if count + ch.len_utf16() > offset {
            return byte;
        }
        count += ch.len_utf16();
    }
    text.len()
}

/// Replaces a UTF-16 `range` of the composition with `text` (`None`
/// replaces it all).
pub fn replace_composition(previous: &str, range: Option<Range<usize>>, text: &str) -> String {
    let Some(range) = range else {
        return text.to_owned();
    };
    let start = utf16_byte(previous, range.start);
    let end = utf16_byte(previous, range.end).max(start);
    let mut result = String::with_capacity(start + text.len() + previous.len() - end);
    result.push_str(&previous[..start]);
    result.push_str(text);
    result.push_str(&previous[end..]);
    result
}

/// Encode a key as the VT byte sequence the terminal expects.
/// Printable text returns `None`: it arrives through the platform IME.
pub fn encode_key(key: &Keystroke, application_cursor: bool) -> Option<String> {
    let m = key.modifiers;
    if m.platform {
        return None;
    }
    let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let cursor = match key.key.as_str() {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(code) = cursor {
        return Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{code}")
        } else {
            format!("\x1b{}{code}", if application_cursor { 'O' } else { '[' })
        });
    }
    let tilde = match key.key.as_str() {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(code) = tilde {
        return Some(if modifier > 1 {
            format!("\x1b[{code};{modifier}~")
        } else {
            format!("\x1b[{code}~")
        });
    }
    let function = match key.key.as_str() {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(code) = function {
        return Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{code}")
        } else {
            format!("\x1bO{code}")
        });
    }
    let mut text: String = match key.key.as_str() {
        "enter" => "\r".into(),
        "escape" => "\x1b".into(),
        "backspace" => if m.control { "\x08" } else { "\x7f" }.into(),
        "tab" => if m.shift { "\x1b[Z" } else { "\t" }.into(),
        "space" if m.control => "\0".into(),
        name if m.control && name.len() == 1 => {
            let byte = name.as_bytes()[0].to_ascii_uppercase();
            if (b'@'..=b'_').contains(&byte) {
                ((byte & 0x1f) as char).to_string()
            } else if byte == b'?' {
                "\x7f".into()
            } else {
                return None;
            }
        }
        _ if m.alt => key.key_char.clone().unwrap_or_else(|| key.key.clone()),
        _ => return None,
    };
    if m.alt {
        text.insert(0, '\x1b');
    }
    Some(text)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn key(spec: &'static str) -> Keystroke {
        Keystroke::parse(spec).unwrap()
    }

    #[test]
    fn control_cursor_and_function_keys() {
        assert_eq!(encode_key(&key("ctrl-c"), false).as_deref(), Some("\x03"));
        assert_eq!(encode_key(&key("up"), true).as_deref(), Some("\x1bOA"));
        assert_eq!(encode_key(&key("up"), false).as_deref(), Some("\x1b[A"));
        assert_eq!(
            encode_key(&key("ctrl-right"), false).as_deref(),
            Some("\x1b[1;5C")
        );
        assert_eq!(
            encode_key(&key("shift-tab"), false).as_deref(),
            Some("\x1b[Z")
        );
        assert_eq!(encode_key(&key("f5"), false).as_deref(), Some("\x1b[15~"));
        assert_eq!(encode_key(&key("alt-b"), false).as_deref(), Some("\x1bb"));
        assert_eq!(
            encode_key(&key("a"), false),
            None,
            "printable text comes from the IME"
        );
    }

    #[test]
    fn composition_edits_use_utf16_offsets() {
        assert_eq!(utf16_byte("a😀b", 3), 5);
        assert_eq!(replace_composition("中😀文", Some(1..3), "华"), "中华文");
        assert_eq!(replace_composition("abc", None, "x"), "x");
    }
}

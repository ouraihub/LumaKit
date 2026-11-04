//! The three color blocks of a v2 manifest (`colors`, `ansi`, `syntax`): canonical hex parsing,
//! the original's `dangerHover` / `onDanger` derivation (WCAG contrast), and the block structs.

use gpui_kit::{Rgba, rgba};
use serde_json::{Map, Value};

use crate::theme::{Ansi, Palette, Syntax};

const COLOR_KEYS: [&str; 27] = [
    "accent",
    "accentHover",
    "accentSolid",
    "onAccent",
    "bubbleAccent",
    "background",
    "header",
    "content",
    "sidebar",
    "input",
    "elevated",
    "hover",
    "border",
    "borderStrong",
    "text",
    "muted",
    "hint",
    "selection",
    "selectionForeground",
    "editorSelection",
    "editorSelectionForeground",
    "info",
    "success",
    "danger",
    "dangerHover",
    "onDanger",
    "warning",
];
const NULLABLE_KEYS: [&str; 2] = ["selectionForeground", "editorSelectionForeground"];
const DERIVED_KEYS: [&str; 2] = ["dangerHover", "onDanger"];
pub(super) const ANSI_KEYS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
];
pub(super) const SYNTAX_KEYS: [&str; 21] = [
    "foreground",
    "comment",
    "commentDoc",
    "keyword",
    "string",
    "number",
    "regexp",
    "annotation",
    "tag",
    "attributeName",
    "attributeValue",
    "property",
    "function",
    "type",
    "variable",
    "constant",
    "operator",
    "punctuation",
    "inserted",
    "deleted",
    "changed",
];
const DANGER_HOVER_SCALE: f64 = 0.88;

/// A color as its four bytes; a `#rrggbb` color is opaque.
type Rgba8 = [u8; 4];
/// One `colors` key: `None` missing or invalid, `Some(None)` an allowed `null`.
type Slot = Option<Option<Rgba8>>;

fn hex(value: &Value) -> Option<Rgba8> {
    let digits = value.as_str()?.strip_prefix('#')?;
    let canonical = matches!(digits.len(), 6 | 8)
        && digits
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !canonical {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    Some(if digits.len() == 6 {
        (value << 8 | 0xff).to_be_bytes()
    } else {
        value.to_be_bytes()
    })
}

fn color(c: Rgba8) -> Rgba {
    rgba(u32::from_be_bytes(c))
}

fn scale_rgb([r, g, b, a]: Rgba8, factor: f64) -> Rgba8 {
    // A u8 scaled by < 1 rounds back into u8 range.
    let channel = |v: u8| (f64::from(v) * factor).round() as u8;
    [channel(r), channel(g), channel(b), a]
}

fn luminance([r, g, b, _]: Rgba8) -> f64 {
    let linear = |v: u8| {
        let v = f64::from(v) / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast(a: Rgba8, b: Rgba8) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The label with the best worst-case contrast over `fills`; the earlier one wins a tie.
fn most_readable(labels: &[Rgba8], fills: [Rgba8; 2]) -> Option<Rgba8> {
    let worst = |label: Rgba8| {
        fills
            .map(|fill| contrast(label, fill))
            .into_iter()
            .fold(f64::INFINITY, f64::min)
    };
    labels.iter().copied().fold(None, |best, label| match best {
        Some(best) if worst(label) <= worst(best) => Some(best),
        _ => Some(label),
    })
}

pub(super) fn note_unexpected_keys(
    map: &Map<String, Value>,
    allowed: &[&str],
    path: &str,
    issues: &mut Vec<String>,
) {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            issues.push(format!("{path}.{key} is not allowed"));
        }
    }
}

fn object<'a>(
    value: Option<&'a Value>,
    path: &str,
    issues: &mut Vec<String>,
) -> Option<&'a Map<String, Value>> {
    let map = value.and_then(Value::as_object);
    if map.is_none() {
        issues.push(format!("{path} must be an object"));
    }
    map
}

/// An `ansi` or `syntax` block: every key a canonical color, nothing else.
pub(super) fn block<const N: usize>(
    value: Option<&Value>,
    keys: &[&str; N],
    path: &str,
    issues: &mut Vec<String>,
) -> Option<[Rgba; N]> {
    let map = object(value, path, issues)?;
    note_unexpected_keys(map, keys, path, issues);
    let colors = keys.map(|key| {
        let parsed = map.get(key).and_then(hex);
        if parsed.is_none() {
            issues.push(format!(
                "{path}.{key} must be a canonical lowercase #rrggbb or #rrggbbaa color"
            ));
        }
        parsed
    });
    colors
        .iter()
        .all(Option::is_some)
        .then(|| colors.map(|c| c.map_or(rgba(0), color)))
}

/// The `colors` block; the derived keys are filled in when the file omits them.
pub(super) fn palette(value: Option<&Value>, issues: &mut Vec<String>) -> Option<Palette> {
    let start = issues.len();
    let map = object(value, "theme.colors", issues)?;
    note_unexpected_keys(map, &COLOR_KEYS, "theme.colors", issues);
    let mut slots: [Slot; 27] = COLOR_KEYS.map(|key| {
        let raw = map.get(key);
        let nullable = NULLABLE_KEYS.contains(&key);
        if nullable && raw.is_some_and(Value::is_null) {
            return Some(None);
        }
        if raw.is_none() && DERIVED_KEYS.contains(&key) {
            return None;
        }
        let parsed = raw.and_then(hex);
        if parsed.is_none() {
            let or_null = if nullable { " or null" } else { "" };
            issues.push(format!(
                "theme.colors.{key} must be a canonical lowercase #rrggbb or #rrggbbaa color{or_null}"
            ));
        }
        parsed.map(Some)
    });
    let index = |key: &str| COLOR_KEYS.iter().position(|k| *k == key);
    let get = |slots: &[Slot; 27], key: &str| index(key).and_then(|i| slots[i]).flatten();
    let set = |slots: &mut [Slot; 27], key: &str, c: Rgba8| {
        if let Some(i) = index(key) {
            slots[i] = Some(Some(c));
        }
    };
    if let Some(danger) = get(&slots, "danger") {
        let hover = get(&slots, "dangerHover").unwrap_or(scale_rgb(danger, DANGER_HOVER_SCALE));
        set(&mut slots, "dangerHover", hover);
        let labels: Vec<Rgba8> = [get(&slots, "text"), get(&slots, "background")]
            .into_iter()
            .flatten()
            .collect();
        if get(&slots, "onDanger").is_none()
            && let Some(label) = most_readable(&labels, [danger, hover])
        {
            set(&mut slots, "onDanger", label);
        }
    }
    if issues.len() > start {
        return None;
    }
    let c = |key: &str| get(&slots, key).map_or(rgba(0), color);
    let n = |key: &str| get(&slots, key).map(color);
    Some(Palette {
        accent: c("accent"),
        accent_hover: c("accentHover"),
        accent_solid: c("accentSolid"),
        on_accent: c("onAccent"),
        bubble_accent: c("bubbleAccent"),
        background: c("background"),
        header: c("header"),
        content: c("content"),
        sidebar: c("sidebar"),
        input: c("input"),
        elevated: c("elevated"),
        hover: c("hover"),
        border: c("border"),
        border_strong: c("borderStrong"),
        text: c("text"),
        muted: c("muted"),
        hint: c("hint"),
        selection: c("selection"),
        selection_foreground: n("selectionForeground"),
        editor_selection: c("editorSelection"),
        editor_selection_foreground: n("editorSelectionForeground"),
        info: c("info"),
        success: c("success"),
        danger: c("danger"),
        danger_hover: c("dangerHover"),
        on_danger: c("onDanger"),
        warning: c("warning"),
    })
}

pub(super) fn ansi(colors: [Rgba; 16]) -> Ansi {
    let [
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    ] = colors;
    Ansi {
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    }
}

pub(super) fn syntax(colors: [Rgba; 21]) -> Syntax {
    let [
        foreground,
        comment,
        comment_doc,
        keyword,
        string,
        number,
        regexp,
        annotation,
        tag,
        attribute_name,
        attribute_value,
        property,
        function,
        type_,
        variable,
        constant,
        operator,
        punctuation,
        inserted,
        deleted,
        changed,
    ] = colors;
    Syntax {
        foreground,
        comment,
        comment_doc,
        keyword,
        string,
        number,
        regexp,
        annotation,
        tag,
        attribute_name,
        attribute_value,
        property,
        function,
        type_,
        variable,
        constant,
        operator,
        punctuation,
        inserted,
        deleted,
        changed,
    }
}

//! Strict parsing of one theme file in the original's v2 format (`apps/web/src/themes/schema.ts`,
//! nesting cap from `packages/server/src/themes`): every key is checked, unknown keys anywhere
//! are rejected, colors are canonical lowercase hex, and `dangerHover` / `onDanger` are derived
//! when a file omits them. Issue strings are the original's English ones, shown verbatim.

mod colors;
#[cfg(test)]
mod tests;

use std::fmt;

use serde_json::Value;

use self::colors::{ANSI_KEYS, SYNTAX_KEYS, ansi, block, note_unexpected_keys, palette, syntax};
use super::{Appearance, Contrast, Roles, LumaTheme};

/// JSON nested deeper than this is rejected before parsing (a manifest needs 2).
pub const MAX_NESTING: usize = 32;

const ROOT_KEYS: [&str; 10] = [
    "$schema",
    "schemaVersion",
    "id",
    "label",
    "order",
    "appearance",
    "contrast",
    "colors",
    "ansi",
    "syntax",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestErrorKind {
    /// Not JSON, or nested deeper than [`MAX_NESTING`].
    InvalidJson,
    /// JSON, but not a valid v2 manifest.
    InvalidTheme,
}

/// Why a theme file was rejected; `issues` are the original's English detail strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub kind: ManifestErrorKind,
    pub issues: Vec<String>,
}

impl ManifestError {
    fn json(detail: String) -> Self {
        Self {
            kind: ManifestErrorKind::InvalidJson,
            issues: vec![detail],
        }
    }

    /// The issues as one line, joined like the original's user-theme detail.
    pub fn detail(&self) -> String {
        self.issues.join("; ")
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail())
    }
}

impl std::error::Error for ManifestError {}

/// The original's `THEME_ID`: `^[a-z0-9][a-z0-9._-]{0,63}$`.
pub(crate) fn is_theme_slug(id: &str) -> bool {
    let Some((first, rest)) = id.as_bytes().split_first() else {
        return false;
    };
    id.len() <= 64
        && matches!(first, b'a'..=b'z' | b'0'..=b'9')
        && rest
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

/// The deepest `{`/`[` nesting outside strings (original `nestingDepth`).
fn nesting_depth(text: &str) -> usize {
    let (mut depth, mut deepest, mut in_string, mut escaped) = (0usize, 0usize, false, false);
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    deepest = deepest.max(depth);
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    deepest
}

/// `Number.isInteger`: an integral JSON number (a huge one saturates).
fn integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().filter(|f| f.fract() == 0.).map(|f| f as i64))
}

impl LumaTheme {
    /// Parses one v2 theme file. The id is the manifest's own; the catalog prefixes user ids.
    pub fn parse(json: &str) -> Result<Self, ManifestError> {
        if nesting_depth(json) > MAX_NESTING {
            return Err(ManifestError::json(format!(
                "nested deeper than {MAX_NESTING} levels"
            )));
        }
        let value: Value =
            serde_json::from_str(json).map_err(|e| ManifestError::json(e.to_string()))?;
        let mut issues = Vec::new();
        let Some(root) = value.as_object() else {
            issues.push("theme manifest must be an object".to_owned());
            return Err(ManifestError {
                kind: ManifestErrorKind::InvalidTheme,
                issues,
            });
        };
        note_unexpected_keys(root, &ROOT_KEYS, "theme", &mut issues);
        if root.get("schemaVersion").and_then(Value::as_f64) != Some(2.) {
            issues.push("theme.schemaVersion must be 2".to_owned());
        }
        let id = root
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_theme_slug(id));
        if id.is_none() {
            issues.push(
                "theme.id must be a lowercase slug (1-64 characters; letters, numbers, ., _, -)"
                    .to_owned(),
            );
        }
        let label = root
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.trim().is_empty());
        if label.is_none() {
            issues.push("theme.label must be a non-empty string".to_owned());
        }
        let order = root.get("order").and_then(integer);
        if order.is_none() {
            issues.push("theme.order must be an integer".to_owned());
        }
        let appearance = match root.get("appearance").and_then(Value::as_str) {
            Some("light") => Some(Appearance::Light),
            Some("dark") => Some(Appearance::Dark),
            _ => None,
        };
        if appearance.is_none() {
            issues.push(r#"theme.appearance must be "light" or "dark""#.to_owned());
        }
        let contrast = match root.get("contrast").and_then(Value::as_str) {
            Some("normal") => Some(Contrast::Normal),
            Some("high") => Some(Contrast::High),
            _ => None,
        };
        if contrast.is_none() {
            issues.push(r#"theme.contrast must be "normal" or "high""#.to_owned());
        }
        let colors = palette(root.get("colors"), &mut issues);
        let ansi_colors = block(root.get("ansi"), &ANSI_KEYS, "theme.ansi", &mut issues);
        let syntax_colors = block(
            root.get("syntax"),
            &SYNTAX_KEYS,
            "theme.syntax",
            &mut issues,
        );
        let parts = (id, label, order, appearance, contrast);
        match (parts, colors, ansi_colors, syntax_colors) {
            (
                (Some(id), Some(label), Some(order), Some(appearance), Some(contrast)),
                Some(palette),
                Some(ansi_colors),
                Some(syntax_colors),
            ) if issues.is_empty() => Ok(Self {
                id: id.to_owned(),
                label: label.to_owned(),
                order,
                appearance,
                contrast,
                roles: Roles::derive(&palette, appearance),
                palette,
                ansi: ansi(ansi_colors),
                syntax: syntax(syntax_colors),
            }),
            _ => Err(ManifestError {
                kind: ManifestErrorKind::InvalidTheme,
                issues,
            }),
        }
    }
}

//! A theme file's color blocks as values: the `colors` palette, the 16 ANSI colors and the
//! syntax colors (validated and kept, though nothing renders syntax yet). Built by `manifest`.

use gpui_kit::Rgba;

/// A theme's `colors` block. `danger_hover` and `on_danger` are always present here: a file that
/// omits them gets the original's derived values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_solid: Rgba,
    pub on_accent: Rgba,
    pub bubble_accent: Rgba,
    pub background: Rgba,
    pub header: Rgba,
    pub content: Rgba,
    pub sidebar: Rgba,
    pub input: Rgba,
    pub elevated: Rgba,
    pub hover: Rgba,
    pub border: Rgba,
    pub border_strong: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub hint: Rgba,
    pub selection: Rgba,
    /// `None` keeps the selected text's own color.
    pub selection_foreground: Option<Rgba>,
    pub editor_selection: Rgba,
    pub editor_selection_foreground: Option<Rgba>,
    pub info: Rgba,
    pub success: Rgba,
    pub danger: Rgba,
    pub danger_hover: Rgba,
    pub on_danger: Rgba,
    pub warning: Rgba,
}

/// A theme's `ansi` block, in SGR order 0–15.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ansi {
    pub black: Rgba,
    pub red: Rgba,
    pub green: Rgba,
    pub yellow: Rgba,
    pub blue: Rgba,
    pub magenta: Rgba,
    pub cyan: Rgba,
    pub white: Rgba,
    pub bright_black: Rgba,
    pub bright_red: Rgba,
    pub bright_green: Rgba,
    pub bright_yellow: Rgba,
    pub bright_blue: Rgba,
    pub bright_magenta: Rgba,
    pub bright_cyan: Rgba,
    pub bright_white: Rgba,
}

impl Ansi {
    pub fn colors(&self) -> [Rgba; 16] {
        [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
            self.bright_black,
            self.bright_red,
            self.bright_green,
            self.bright_yellow,
            self.bright_blue,
            self.bright_magenta,
            self.bright_cyan,
            self.bright_white,
        ]
    }
}

/// A theme's `syntax` block (original `SYNTAX_COLOR_KEYS`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Syntax {
    pub foreground: Rgba,
    pub comment: Rgba,
    pub comment_doc: Rgba,
    pub keyword: Rgba,
    pub string: Rgba,
    pub number: Rgba,
    pub regexp: Rgba,
    pub annotation: Rgba,
    pub tag: Rgba,
    pub attribute_name: Rgba,
    pub attribute_value: Rgba,
    pub property: Rgba,
    pub function: Rgba,
    pub type_: Rgba,
    pub variable: Rgba,
    pub constant: Rgba,
    pub operator: Rgba,
    pub punctuation: Rgba,
    pub inserted: Rgba,
    pub deleted: Rgba,
    pub changed: Rgba,
}

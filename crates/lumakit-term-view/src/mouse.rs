//! Mouse encoding (xterm 1000 / 1002 / 1006) and grid-coordinate utilities.
//!
//! Ported from PoC-1 + autorail terminal panel. The caller passes a
//! `TermMode` so we pick the right encoding: 1006 (SGR) when the mode bit
//! is set, 1000 (basic) otherwise. Wheel-in-alt-screen events convert to
//! up/down arrow byte sequences per terminal convention.

use lumakit_term::TermMode;

use crate::paint::Metrics;

/// Position in logical pixels relative to the view origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewPoint {
    /// Horizontal offset in logical pixels.
    pub x: f32,
    /// Vertical offset in logical pixels.
    pub y: f32,
}

/// Mouse button identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary (left) button.
    Left,
    /// Middle button.
    Middle,
    /// Secondary (right) button.
    Right,
    /// Wheel up.
    WheelUp,
    /// Wheel down.
    WheelDown,
}

/// Modifier bits used by mouse encodings.
#[derive(Debug, Default, Clone, Copy)]
pub struct Modifiers {
    /// Shift held.
    pub shift: bool,
    /// Alt / Meta held.
    pub alt: bool,
    /// Control held.
    pub control: bool,
}

/// Cell coordinates computed from a `ViewPoint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// Zero-based column.
    pub col: usize,
    /// Zero-based row within the visible area.
    pub row: usize,
}

/// Convert `point` into a `Cell`. Rows and columns are clamped to the
/// visible grid.
pub fn hit_test(
    point: ViewPoint,
    metrics: &Metrics,
    visible_cols: usize,
    visible_rows: usize,
) -> Cell {
    let col = ((point.x / metrics.cell_width).max(0.0).floor() as usize)
        .min(visible_cols.saturating_sub(1));
    let row = ((point.y / metrics.line_height).max(0.0).floor() as usize)
        .min(visible_rows.saturating_sub(1));
    Cell { col, row }
}

/// Classify a pointer within a cell: left or right half.
pub fn side(point: ViewPoint, metrics: &Metrics) -> Side {
    let fraction = (point.x / metrics.cell_width).fract();
    if fraction < 0.5 {
        Side::Left
    } else {
        Side::Right
    }
}

/// Side of a cell the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Left half.
    Left,
    /// Right half.
    Right,
}

/// Encode a mouse press for the terminal. `None` when the terminal is not
/// in a mouse-reporting mode.
pub fn encode_press(
    button: MouseButton,
    cell: Cell,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<String> {
    if !reports_buttons(mode) && !reports_motion(mode) {
        return None;
    }
    let base = button_code(button);
    let code = modifier_bits(modifiers) | base;
    Some(if mode.contains(TermMode::SGR_MOUSE) {
        format!("\x1b[<{};{};{}M", code, cell.col + 1, cell.row + 1)
    } else {
        encode_basic(code, cell, 'M')
    })
}

/// Encode a wheel event. When the alt screen is active, this instead
/// returns the arrow-key sequence so programs that do not opt into mouse
/// reporting still scroll.
pub fn encode_wheel(
    direction: WheelDirection,
    cell: Cell,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<String> {
    if mode.contains(TermMode::ALT_SCREEN) && !reports_buttons(mode) {
        return Some(match direction {
            WheelDirection::Up => "\x1b[A".into(),
            WheelDirection::Down => "\x1b[B".into(),
        });
    }
    let button = match direction {
        WheelDirection::Up => MouseButton::WheelUp,
        WheelDirection::Down => MouseButton::WheelDown,
    };
    encode_press(button, cell, modifiers, mode)
}

/// Wheel direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelDirection {
    /// Scroll up.
    Up,
    /// Scroll down.
    Down,
}

fn button_code(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    }
}

fn modifier_bits(modifiers: Modifiers) -> u8 {
    let mut bits = 0;
    if modifiers.shift {
        bits |= 4;
    }
    if modifiers.alt {
        bits |= 8;
    }
    if modifiers.control {
        bits |= 16;
    }
    bits
}

fn reports_buttons(mode: TermMode) -> bool {
    mode.contains(TermMode::MOUSE_REPORT_CLICK) || mode.contains(TermMode::MOUSE_DRAG)
}

fn reports_motion(mode: TermMode) -> bool {
    mode.contains(TermMode::MOUSE_MOTION) || mode.contains(TermMode::MOUSE_DRAG)
}

fn encode_basic(code: u8, cell: Cell, terminator: char) -> String {
    // Legacy xterm encoding; coordinates are 1-based and offset by 32.
    let col = (cell.col + 1).min(223) + 32;
    let row = (cell.row + 1).min(223) + 32;
    let code_byte = code + 32;
    format!(
        "\x1b[M{}{}{}",
        code_byte as char, col as u8 as char, row as u8 as char
    )
    .replace("{terminator}", &terminator.to_string())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics {
            cell_width: 10.0,
            line_height: 20.0,
            font_size: gpui_kit::px(14.0),
            font: gpui_kit::Font {
                family: "JetBrains Mono".into(),
                features: Default::default(),
                weight: gpui_kit::FontWeight::NORMAL,
                style: gpui_kit::FontStyle::Normal,
                fallbacks: None,
            },
        }
    }

    #[test]
    fn hit_test_clamps_to_grid() {
        let m = metrics();
        let cell = hit_test(ViewPoint { x: 55.0, y: 42.0 }, &m, 10, 5);
        assert_eq!(cell, Cell { col: 5, row: 2 });
        let far = hit_test(
            ViewPoint {
                x: 9999.0,
                y: 9999.0,
            },
            &m,
            10,
            5,
        );
        assert_eq!(far, Cell { col: 9, row: 4 });
    }

    #[test]
    fn sgr_mouse_press_includes_coordinates() {
        let mode = TermMode::SGR_MOUSE | TermMode::MOUSE_REPORT_CLICK;
        let encoded = encode_press(
            MouseButton::Left,
            Cell { col: 3, row: 7 },
            Modifiers::default(),
            mode,
        )
        .unwrap();
        assert!(encoded.starts_with("\x1b[<0;4;8"));
    }

    #[test]
    fn wheel_on_alt_screen_sends_arrow_keys() {
        let mode = TermMode::ALT_SCREEN;
        let encoded = encode_wheel(
            WheelDirection::Up,
            Cell { col: 0, row: 0 },
            Modifiers::default(),
            mode,
        )
        .unwrap();
        assert_eq!(encoded, "\x1b[A");
    }
}

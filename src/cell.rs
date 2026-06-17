//! A single character cell: the atom of the terminal grid.

/// A color a cell can carry. `Default` means "the terminal's default fg/bg".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Default,
    /// 0–15 = the 16 ANSI colors, 16–255 = the xterm 256-color cube/greys.
    Indexed(u8),
    /// 24-bit truecolor.
    Rgb(u8, u8, u8),
}

/// Text attribute bit flags (bold, underline, …). Stored packed in a `u8`.
pub mod attr {
    pub const BOLD: u8 = 1 << 0;
    pub const ITALIC: u8 = 1 << 1;
    pub const UNDERLINE: u8 = 1 << 2;
    pub const REVERSE: u8 = 1 << 3;
}

/// One screen cell: the glyph plus how to paint it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: 0,
        }
    }
}

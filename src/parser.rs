//! Bridges the `vte` tokenizer to our [`Grid`]. `vte` does the hard work of
//! turning a raw byte stream into clean callbacks (print this char, execute
//! this control byte, dispatch this CSI sequence); we give those callbacks
//! meaning by mutating the grid.

use vte::{Params, Perform};

use crate::cell::{attr, Color};
use crate::grid::Grid;

/// Holds a mutable borrow of the grid for the duration of one `advance()` call.
pub struct Performer<'a> {
    pub grid: &'a mut Grid,
}

impl Perform for Performer<'_> {
    /// A printable character arrived.
    fn print(&mut self, c: char) {
        self.grid.print(c);
    }

    /// A C0 control byte (newline, tab, …).
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.grid.line_feed(), // LF, vertical tab, form feed
            b'\r' => self.grid.carriage_return(),
            b'\t' => self.grid.tab(),
            0x08 => self.grid.backspace(),
            _ => {}
        }
    }

    /// A complete CSI sequence: `ESC [ params... action`.
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // Private sequences (DEC modes) are marked with a leading `?`, e.g.
        // `\x1b[?25h`. We handle cursor show/hide; other modes are ignored.
        if intermediates.first() == Some(&b'?') {
            let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
            if mode == 25 {
                match action {
                    'h' => self.grid.set_cursor_visible(true),
                    'l' => self.grid.set_cursor_visible(false),
                    _ => {}
                }
            }
            return;
        }

        // First sub-param of the nth group, treating 0/absent as `default`.
        let nth = |i: usize, default: usize| -> usize {
            match params.iter().nth(i) {
                Some(p) if p[0] != 0 => p[0] as usize,
                _ => default,
            }
        };

        match action {
            'A' => self.grid.move_up(nth(0, 1)),
            'B' => self.grid.move_down(nth(0, 1)),
            'C' => self.grid.move_right(nth(0, 1)),
            'D' => self.grid.move_left(nth(0, 1)),
            // CUP/HVP are 1-based on the wire; convert to our 0-based grid.
            'H' | 'f' => self.grid.move_to(nth(0, 1) - 1, nth(1, 1) - 1),
            'J' => self.grid.erase_in_display(nth(0, 0) as u16),
            'K' => self.grid.erase_in_line(nth(0, 0) as u16),
            'm' => self.sgr(params),
            _ => {} // unhandled CSI (modes, scroll regions, …) — added in later milestones
        }
    }
}

impl Performer<'_> {
    /// SGR — Select Graphic Rendition: update the pen's colors and attributes.
    /// Handles resets, the basic attrs, the 16 ANSI colors, and the extended
    /// `38;5;n` / `38;2;r;g;b` (and colon-subparam) forms.
    fn sgr(&mut self, params: &Params) {
        // Flatten into groups so we can look across params for extended colors.
        let groups: Vec<Vec<u16>> = params.iter().map(|p| p.to_vec()).collect();
        if groups.is_empty() {
            self.reset_pen();
            return;
        }

        let pen = &mut self.grid.pen;
        let mut i = 0;
        while i < groups.len() {
            let g = &groups[i];
            match g[0] {
                0 => {
                    pen.fg = Color::Default;
                    pen.bg = Color::Default;
                    pen.attrs = 0;
                }
                1 => pen.attrs |= attr::BOLD,
                3 => pen.attrs |= attr::ITALIC,
                4 => pen.attrs |= attr::UNDERLINE,
                7 => pen.attrs |= attr::REVERSE,
                22 => pen.attrs &= !attr::BOLD,
                23 => pen.attrs &= !attr::ITALIC,
                24 => pen.attrs &= !attr::UNDERLINE,
                27 => pen.attrs &= !attr::REVERSE,
                30..=37 => pen.fg = Color::Indexed((g[0] - 30) as u8),
                39 => pen.fg = Color::Default,
                40..=47 => pen.bg = Color::Indexed((g[0] - 40) as u8),
                49 => pen.bg = Color::Default,
                90..=97 => pen.fg = Color::Indexed((g[0] - 90 + 8) as u8),
                100..=107 => pen.bg = Color::Indexed((g[0] - 100 + 8) as u8),
                38 => {
                    if let Some(c) = extended_color(g, &groups, &mut i) {
                        pen.fg = c;
                    }
                }
                48 => {
                    if let Some(c) = extended_color(g, &groups, &mut i) {
                        pen.bg = c;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn reset_pen(&mut self) {
        self.grid.pen.fg = Color::Default;
        self.grid.pen.bg = Color::Default;
        self.grid.pen.attrs = 0;
    }
}

/// Decode `38`/`48` extended color. Two encodings:
///   - colon sub-params in one group: `[38, 5, n]` or `[38, 2, r, g, b]`
///   - semicolon-separated groups:    `38 ; 5 ; n` (advances `i` past them)
fn extended_color(group: &[u16], groups: &[Vec<u16>], i: &mut usize) -> Option<Color> {
    // Colon form: everything is inside this one group.
    if group.len() >= 3 && group[1] == 5 {
        return Some(Color::Indexed(group[2] as u8));
    }
    if group.len() >= 5 && group[1] == 2 {
        return Some(Color::Rgb(group[2] as u8, group[3] as u8, group[4] as u8));
    }
    // Semicolon form: read the following groups and advance the cursor.
    let mode = groups.get(*i + 1)?[0];
    if mode == 5 {
        let n = groups.get(*i + 2)?[0];
        *i += 2;
        Some(Color::Indexed(n as u8))
    } else if mode == 2 {
        let r = groups.get(*i + 2)?[0];
        let gr = groups.get(*i + 3)?[0];
        let b = groups.get(*i + 4)?[0];
        *i += 4;
        Some(Color::Rgb(r as u8, gr as u8, b as u8))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vte::Parser;

    /// Feed raw bytes through a real `vte::Parser` into a fresh grid.
    fn run(bytes: &[u8], rows: usize, cols: usize) -> Grid {
        let mut grid = Grid::new(rows, cols);
        let mut parser = Parser::new();
        let mut perf = Performer { grid: &mut grid };
        parser.advance(&mut perf, bytes);
        grid
    }

    #[test]
    fn plain_text_lands_in_grid() {
        let g = run(b"hi", 1, 8);
        assert_eq!(g.cell(0, 0).ch, 'h');
        assert_eq!(g.cell(0, 1).ch, 'i');
    }

    #[test]
    fn cup_moves_cursor_absolute() {
        // ESC[2;3H -> row 2, col 3 (1-based) == (1,2) 0-based.
        let g = run(b"\x1b[2;3H", 5, 5);
        assert_eq!((g.cursor_row, g.cursor_col), (1, 2));
    }

    #[test]
    fn sgr_sets_color_and_resets() {
        // red 'a', reset, plain 'b'
        let g = run(b"\x1b[31ma\x1b[0mb", 1, 4);
        assert_eq!(g.cell(0, 0).fg, Color::Indexed(1));
        assert_eq!(g.cell(0, 1).fg, Color::Default);
    }

    #[test]
    fn sgr_truecolor() {
        let g = run(b"\x1b[38;2;10;20;30mx", 1, 4);
        assert_eq!(g.cell(0, 0).fg, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn erase_display_clears_screen() {
        let g = run(b"abc\x1b[2J", 1, 3);
        assert_eq!(g.to_text(), "   ");
    }

    #[test]
    fn carriage_return_overwrites() {
        let g = run(b"hello\rH", 1, 5);
        assert_eq!(g.to_text(), "Hello");
    }

    #[test]
    fn private_mode_hides_and_shows_cursor() {
        let g = run(b"\x1b[?25l", 1, 3);
        assert!(!g.cursor_visible());
        let g = run(b"\x1b[?25l\x1b[?25h", 1, 3);
        assert!(g.cursor_visible());
    }
}

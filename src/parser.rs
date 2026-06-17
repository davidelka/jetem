//! Bridges the `vte` tokenizer to our [`Grid`]. `vte` does the hard work of
//! turning a raw byte stream into clean callbacks (print this char, execute
//! this control byte, dispatch this CSI sequence); we give those callbacks
//! meaning by mutating the grid.

use vte::{Params, Perform};

use crate::cell::{attr, Color};
use crate::screen::Screen;

/// Holds a mutable borrow of the screen for the duration of one `advance()`
/// call. All output is routed to the screen's currently-active buffer, so an
/// app can flip to the alternate screen mid-chunk and keep drawing.
pub struct Performer<'a> {
    pub screen: &'a mut Screen,
}

impl Perform for Performer<'_> {
    /// A printable character arrived.
    fn print(&mut self, c: char) {
        self.screen.active_mut().print(c);
    }

    /// A C0 control byte (newline, tab, …).
    fn execute(&mut self, byte: u8) {
        let g = self.screen.active_mut();
        match byte {
            b'\n' | 0x0b | 0x0c => g.line_feed(), // LF, vertical tab, form feed
            b'\r' => g.carriage_return(),
            b'\t' => g.tab(),
            0x08 => g.backspace(),
            _ => {}
        }
    }

    /// A complete CSI sequence: `ESC [ params... action`.
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // Private sequences (DEC modes) are marked with a leading `?`, e.g.
        // `\x1b[?25h`. We handle cursor show/hide and the alternate screen;
        // other modes are ignored.
        if intermediates.first() == Some(&b'?') {
            let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
            let set = action == 'h';
            match mode {
                25 => self.screen.active_mut().set_cursor_visible(set),
                // 47 / 1047 / 1049 all switch to/from the alternate screen.
                47 | 1047 | 1049 => {
                    if set {
                        self.screen.enter_alt();
                    } else {
                        self.screen.leave_alt();
                    }
                }
                _ => {}
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

        // Inline `active_mut()` per arm (rather than binding it once) so the
        // `'m'` arm can take `&mut self` for `sgr` without a borrow conflict.
        match action {
            'A' => self.screen.active_mut().move_up(nth(0, 1)),
            'B' => self.screen.active_mut().move_down(nth(0, 1)),
            'C' => self.screen.active_mut().move_right(nth(0, 1)),
            'D' => self.screen.active_mut().move_left(nth(0, 1)),
            // CUP/HVP are 1-based on the wire; convert to our 0-based grid.
            'H' | 'f' => self.screen.active_mut().move_to(nth(0, 1) - 1, nth(1, 1) - 1),
            'J' => self.screen.active_mut().erase_in_display(nth(0, 0) as u16),
            'K' => self.screen.active_mut().erase_in_line(nth(0, 0) as u16),
            'm' => self.sgr(params),
            _ => {} // unhandled CSI (scroll regions, …) — added in later milestones
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

        let pen = &mut self.screen.active_mut().pen;
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
        let pen = &mut self.screen.active_mut().pen;
        pen.fg = Color::Default;
        pen.bg = Color::Default;
        pen.attrs = 0;
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
    use crate::screen::Screen;
    use vte::Parser;

    /// Feed raw bytes through a real `vte::Parser` into a fresh screen.
    fn run(bytes: &[u8], rows: usize, cols: usize) -> Screen {
        let mut screen = Screen::new(rows, cols);
        let mut parser = Parser::new();
        let mut perf = Performer { screen: &mut screen };
        parser.advance(&mut perf, bytes);
        screen
    }

    #[test]
    fn plain_text_lands_in_grid() {
        let s = run(b"hi", 1, 8);
        assert_eq!(s.active().cell(0, 0).ch, 'h');
        assert_eq!(s.active().cell(0, 1).ch, 'i');
    }

    #[test]
    fn cup_moves_cursor_absolute() {
        // ESC[2;3H -> row 2, col 3 (1-based) == (1,2) 0-based.
        let s = run(b"\x1b[2;3H", 5, 5);
        let g = s.active();
        assert_eq!((g.cursor_row, g.cursor_col), (1, 2));
    }

    #[test]
    fn sgr_sets_color_and_resets() {
        // red 'a', reset, plain 'b'
        let s = run(b"\x1b[31ma\x1b[0mb", 1, 4);
        assert_eq!(s.active().cell(0, 0).fg, Color::Indexed(1));
        assert_eq!(s.active().cell(0, 1).fg, Color::Default);
    }

    #[test]
    fn sgr_truecolor() {
        let s = run(b"\x1b[38;2;10;20;30mx", 1, 4);
        assert_eq!(s.active().cell(0, 0).fg, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn erase_display_clears_screen() {
        let s = run(b"abc\x1b[2J", 1, 3);
        assert_eq!(s.active().to_text(), "   ");
    }

    #[test]
    fn carriage_return_overwrites() {
        let s = run(b"hello\rH", 1, 5);
        assert_eq!(s.active().to_text(), "Hello");
    }

    #[test]
    fn private_mode_hides_and_shows_cursor() {
        let s = run(b"\x1b[?25l", 1, 3);
        assert!(!s.active().cursor_visible());
        let s = run(b"\x1b[?25l\x1b[?25h", 1, 3);
        assert!(s.active().cursor_visible());
    }

    #[test]
    fn alt_screen_switch_preserves_primary() {
        // Write to primary, enter alt and draw, leave alt -> primary restored.
        let s = run(b"AB\x1b[?1049hZZ\x1b[?1049l", 1, 4);
        assert_eq!(s.active().cell(0, 0).ch, 'A');
        assert_eq!(s.active().cell(0, 1).ch, 'B');
    }
}

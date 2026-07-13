//! Bridges the `vte` tokenizer to our [`Grid`]. `vte` does the hard work of
//! turning a raw byte stream into clean callbacks (print this char, execute
//! this control byte, dispatch this CSI sequence); we give those callbacks
//! meaning by mutating the grid.

use vte::{Params, Perform};

use crate::block::BlockTracker;
use crate::cell::{attr, Color};
use crate::screen::{MouseTracking, Screen};

/// Holds a mutable borrow of the screen (and the block tracker) for the duration
/// of one `advance()` call. Output is routed to the screen's active buffer; OSC
/// 133 marks drive the tracker so we capture command blocks.
pub struct Performer<'a> {
    pub screen: &'a mut Screen,
    pub blocks: &'a mut BlockTracker,
}

impl Perform for Performer<'_> {
    /// A printable character arrived.
    fn print(&mut self, c: char) {
        self.screen.active_mut().print(c);
        self.blocks.feed_output(c);
    }

    /// OSC strings: we care about 133 (semantic prompts) and 7 (cwd).
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        match params.first().copied() {
            Some(b"133") => match params.get(1).copied() {
                Some(b"A") => self.blocks.prompt_start(),
                Some(b"B") => {
                    let g = self.screen.active();
                    self.blocks.command_start(g.cursor_row, g.cursor_col);
                }
                Some(b"C") => match params.get(2).and_then(|p| crate::block::decode_command(p)) {
                    // The shell sent the exact command (base64) — robust.
                    Some(cmd) => self.blocks.output_start_with_command(cmd),
                    // Fallback: read the command off the grid.
                    None => self.blocks.output_start(self.screen.active()),
                },
                Some(b"D") => {
                    let code = params
                        .get(2)
                        .and_then(|p| std::str::from_utf8(p).ok())
                        .and_then(|s| s.parse::<i32>().ok());
                    self.blocks.command_end(code);
                }
                _ => {}
            },
            Some(b"7") => {
                if let Some(uri) = params.get(1) {
                    self.blocks.set_cwd_from_uri(uri);
                }
            }
            _ => {}
        }
    }

    /// A C0 control byte (newline, tab, …).
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => {
                self.screen.active_mut().line_feed(); // LF, vertical tab, form feed
                self.blocks.feed_output('\n');
            }
            b'\r' => self.screen.active_mut().carriage_return(),
            b'\t' => self.screen.active_mut().tab(),
            0x08 => self.screen.active_mut().backspace(),
            _ => {}
        }
    }

    /// A bare escape sequence: `ESC` then a final byte (no `[`). We handle the
    /// save/restore-cursor pair; charset/keypad designations are safe no-ops.
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => self.screen.active_mut().save_cursor(),   // DECSC
            b'8' => self.screen.active_mut().restore_cursor(), // DECRC
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
                // Mouse tracking level (`?1000/1002/1003`). Setting any of them
                // selects that level; resetting returns to `Off`. A program sets
                // exactly one, so last-writer-wins matches real usage. (The legacy
                // X10 `?9` and the `?1005/1015` encodings are intentionally skipped
                // — obsolete; every modern app uses 1000/1002/1003 + SGR 1006.)
                1000 | 1002 | 1003 => {
                    let level = match (set, mode) {
                        (false, _) => MouseTracking::Off,
                        (true, 1000) => MouseTracking::Normal,
                        (true, 1002) => MouseTracking::ButtonEvent,
                        (true, _) => MouseTracking::AnyEvent,
                    };
                    self.screen.modes_mut().mouse = level;
                }
                1006 => self.screen.modes_mut().mouse_sgr = set,
                2004 => self.screen.modes_mut().bracketed_paste = set,
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
            'B' | 'e' => self.screen.active_mut().move_down(nth(0, 1)),
            'C' | 'a' => self.screen.active_mut().move_right(nth(0, 1)),
            'D' => self.screen.active_mut().move_left(nth(0, 1)),
            'E' => self.screen.active_mut().cursor_next_line(nth(0, 1)),
            'F' => self.screen.active_mut().cursor_prev_line(nth(0, 1)),
            // CHA / VPA — absolute column / row (1-based on the wire).
            'G' | '`' => self.screen.active_mut().move_to_col(nth(0, 1) - 1),
            'd' => self.screen.active_mut().move_to_row(nth(0, 1) - 1),
            // CUP/HVP are 1-based on the wire; convert to our 0-based grid.
            'H' | 'f' => self.screen.active_mut().move_to(nth(0, 1) - 1, nth(1, 1) - 1),
            'J' => self.screen.active_mut().erase_in_display(nth(0, 0) as u16),
            'K' => self.screen.active_mut().erase_in_line(nth(0, 0) as u16),
            // In-line editing (default count 1).
            '@' => self.screen.active_mut().insert_chars(nth(0, 1)),
            'P' => self.screen.active_mut().delete_chars(nth(0, 1)),
            'X' => self.screen.active_mut().erase_chars(nth(0, 1)),
            // Save / restore cursor (ANSI.SYS form of DECSC/DECRC).
            's' => self.screen.active_mut().save_cursor(),
            'u' => self.screen.active_mut().restore_cursor(),
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
                2 => pen.attrs |= attr::DIM,
                3 => pen.attrs |= attr::ITALIC,
                4 => pen.attrs |= attr::UNDERLINE,
                5 => pen.attrs |= attr::BLINK,
                7 => pen.attrs |= attr::REVERSE,
                8 => pen.attrs |= attr::HIDDEN,
                9 => pen.attrs |= attr::STRIKETHROUGH,
                // 22 = "normal intensity" clears both bold and dim.
                22 => pen.attrs &= !(attr::BOLD | attr::DIM),
                23 => pen.attrs &= !attr::ITALIC,
                24 => pen.attrs &= !attr::UNDERLINE,
                25 => pen.attrs &= !attr::BLINK,
                27 => pen.attrs &= !attr::REVERSE,
                28 => pen.attrs &= !attr::HIDDEN,
                29 => pen.attrs &= !attr::STRIKETHROUGH,
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
        let mut blocks = BlockTracker::new_in_memory();
        let mut parser = Parser::new();
        let mut perf = Performer {
            screen: &mut screen,
            blocks: &mut blocks,
        };
        parser.advance(&mut perf, bytes);
        screen
    }

    /// Like `run`, but returns the block tracker for OSC 133 tests.
    fn run_blocks(bytes: &[u8], rows: usize, cols: usize) -> BlockTracker {
        let mut screen = Screen::new(rows, cols);
        let mut blocks = BlockTracker::new_in_memory();
        let mut parser = Parser::new();
        let mut perf = Performer {
            screen: &mut screen,
            blocks: &mut blocks,
        };
        parser.advance(&mut perf, bytes);
        blocks
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
    fn cursor_absolute_column_move() {
        // Print "abcdef", CHA `\x1b[3G` -> column 3 (1-based) = 0-based col 2,
        // then "xy" overwrites "cd".
        let s = run(b"abcdef\x1b[3Gxy", 1, 6);
        let r: String = (0..6).map(|c| s.active().cell(0, c).ch).collect();
        assert_eq!(r, "abxyef");
    }

    #[test]
    fn delete_and_erase_chars() {
        // DCH: "abcdef", cursor to col 1, delete 2 -> "adef".
        let s = run(b"abcdef\x1b[2G\x1b[2P", 1, 6);
        let r: String = (0..6).map(|c| s.active().cell(0, c).ch).collect();
        assert_eq!(r, "adef  ");
        // ECH: blank 3 from col 2 in place.
        let s = run(b"abcdef\x1b[3G\x1b[3X", 1, 6);
        let r: String = (0..6).map(|c| s.active().cell(0, c).ch).collect();
        assert_eq!(r, "ab   f");
    }

    #[test]
    fn save_restore_cursor_both_forms() {
        // DECSC/DECRC via ESC 7 / ESC 8: save at col 0, move + type, restore, type.
        let s = run(b"\x1b7XYZ\x1b8A", 1, 6);
        // After restore, 'A' overwrites col 0 -> "AYZ".
        assert_eq!(s.active().cell(0, 0).ch, 'A');
        assert_eq!(s.active().cell(0, 1).ch, 'Y');
        // CSI s / u form.
        let s = run(b"\x1b[shi\x1b[uJ", 1, 6);
        assert_eq!(s.active().cell(0, 0).ch, 'J');
    }

    #[test]
    fn sgr_new_attributes() {
        use crate::cell::attr;
        let s = run(b"\x1b[2;4;9mx", 1, 4); // dim + underline + strikethrough
        let a = s.active().cell(0, 0).attrs;
        assert!(a & attr::DIM != 0);
        assert!(a & attr::UNDERLINE != 0);
        assert!(a & attr::STRIKETHROUGH != 0);
        // 22 clears bold AND dim together; 24/29 clear their own.
        let s = run(b"\x1b[1;2mx\x1b[22my", 1, 4);
        assert_eq!(s.active().cell(0, 1).attrs & (attr::BOLD | attr::DIM), 0);
    }

    #[test]
    fn mouse_and_paste_modes_toggle() {
        // Tracking level follows the last ?1000/1002/1003 set; ?...l clears to Off.
        assert_eq!(run(b"\x1b[?1000h", 1, 3).modes().mouse, MouseTracking::Normal);
        assert_eq!(run(b"\x1b[?1002h", 1, 3).modes().mouse, MouseTracking::ButtonEvent);
        assert_eq!(run(b"\x1b[?1003h", 1, 3).modes().mouse, MouseTracking::AnyEvent);
        assert_eq!(run(b"\x1b[?1000h\x1b[?1000l", 1, 3).modes().mouse, MouseTracking::Off);
        // SGR encoding and bracketed paste are independent bools.
        assert!(run(b"\x1b[?1006h", 1, 3).modes().mouse_sgr);
        assert!(run(b"\x1b[?2004h", 1, 3).modes().bracketed_paste);
        assert!(!run(b"\x1b[?2004h\x1b[?2004l", 1, 3).modes().bracketed_paste);
    }

    #[test]
    fn alt_screen_switch_preserves_primary() {
        // Write to primary, enter alt and draw, leave alt -> primary restored.
        let s = run(b"AB\x1b[?1049hZZ\x1b[?1049l", 1, 4);
        assert_eq!(s.active().cell(0, 0).ch, 'A');
        assert_eq!(s.active().cell(0, 1).ch, 'B');
    }

    #[test]
    fn osc133_captures_command_block() {
        // A (prompt) | B (mark at 0,0) | "ls" typed | C (capture "ls") |
        // "out" output | D;0 (close, exit 0).
        let bytes = b"\x1b]133;A\x07\x1b]133;B\x07ls\x1b]133;C\x07out\x1b]133;D;0\x07";
        let blocks = run_blocks(bytes, 4, 20);
        let b = blocks.last().expect("a block was captured");
        assert_eq!(b.command, "ls");
        assert_eq!(b.output, "out");
        assert_eq!(b.exit_code, Some(0));
    }

    #[test]
    fn osc133_explicit_command_from_payload() {
        // C carries base64("git push"); the captured command must be exact,
        // independent of whatever is on the grid.
        let bytes = b"PROMPT$ \x1b]133;C;Z2l0IHB1c2g=\x07out\x1b]133;D;0\x07";
        let blocks = run_blocks(bytes, 4, 40);
        assert_eq!(blocks.last().unwrap().command, "git push");
    }

    #[test]
    fn osc133_command_excludes_prompt() {
        // A prompt is printed before the B mark; only the typed command
        // (between B and C) must be captured, not the prompt text.
        let bytes = b"\x1b]133;A\x07PROMPT$ \x1b]133;B\x07ls -l\x1b]133;C\x07out\x1b]133;D;0\x07";
        let blocks = run_blocks(bytes, 4, 40);
        assert_eq!(blocks.last().unwrap().command, "ls -l");
    }

    #[test]
    fn osc133_captures_nonzero_exit() {
        let bytes = b"\x1b]133;B\x07false\x1b]133;C\x07\x1b]133;D;1\x07";
        let blocks = run_blocks(bytes, 4, 20);
        assert_eq!(blocks.last().unwrap().exit_code, Some(1));
        assert_eq!(blocks.last().unwrap().command, "false");
    }
}

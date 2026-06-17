//! The grid: a rows×cols matrix of [`Cell`]s plus a cursor and the current
//! "pen" (the fg/bg/attrs applied to newly printed characters). This is the
//! in-memory model of what's on screen — the parser mutates it, the renderer
//! reads it.

use crate::cell::{Cell, Color};

pub struct Grid {
    pub rows: usize,
    pub cols: usize,
    /// Row-major: index = row * cols + col.
    cells: Vec<Cell>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    /// Deferred-wrap flag: set when the last column was just filled. The cursor
    /// stays put until the *next* printable char, which then wraps. This is how
    /// real terminals avoid scrolling early on a perfectly-full line.
    wrap_pending: bool,
    /// Template cell carrying the active fg/bg/attrs (set by SGR sequences).
    pub pen: Cell,
}

impl Grid {
    pub fn new(rows: usize, cols: usize) -> Self {
        Grid {
            rows,
            cols,
            cells: vec![Cell::default(); rows * cols],
            cursor_row: 0,
            cursor_col: 0,
            wrap_pending: false,
            pen: Cell::default(),
        }
    }

    fn index(&self, row: usize, col: usize) -> usize {
        row * self.cols + col
    }

    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[self.index(row, col)]
    }

    /// A blank cell that inherits the pen's background (erasing fills with the
    /// current bg color, matching real terminals).
    fn blank(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: self.pen.bg,
            attrs: 0,
        }
    }

    // --- printing & control characters -----------------------------------

    /// Write `ch` at the cursor using the current pen, then advance the cursor
    /// (wrapping to the next line at the right edge).
    pub fn print(&mut self, ch: char) {
        // A pending wrap from the previous char takes effect now.
        if self.wrap_pending {
            self.cursor_col = 0;
            self.line_feed();
            self.wrap_pending = false;
        }
        let idx = self.index(self.cursor_row, self.cursor_col);
        self.cells[idx] = Cell { ch, ..self.pen };
        // At the right edge, defer the wrap instead of moving now.
        if self.cursor_col + 1 >= self.cols {
            self.wrap_pending = true;
        } else {
            self.cursor_col += 1;
        }
    }

    /// `\n` — move down one row, scrolling the screen if at the bottom.
    pub fn line_feed(&mut self) {
        self.wrap_pending = false;
        if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        } else {
            self.scroll_up();
        }
    }

    /// `\r` — return to column 0.
    pub fn carriage_return(&mut self) {
        self.wrap_pending = false;
        self.cursor_col = 0;
    }

    /// `\t` — advance to the next 8-column tab stop.
    pub fn tab(&mut self) {
        let next = ((self.cursor_col / 8) + 1) * 8;
        self.cursor_col = next.min(self.cols - 1);
    }

    /// Backspace — move the cursor left one column (no erase).
    pub fn backspace(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    // --- cursor movement (CSI A/B/C/D, H) --------------------------------

    pub fn move_up(&mut self, n: usize) {
        self.wrap_pending = false;
        self.cursor_row = self.cursor_row.saturating_sub(n);
    }
    pub fn move_down(&mut self, n: usize) {
        self.wrap_pending = false;
        self.cursor_row = (self.cursor_row + n).min(self.rows - 1);
    }
    pub fn move_left(&mut self, n: usize) {
        self.wrap_pending = false;
        self.cursor_col = self.cursor_col.saturating_sub(n);
    }
    pub fn move_right(&mut self, n: usize) {
        self.wrap_pending = false;
        self.cursor_col = (self.cursor_col + n).min(self.cols - 1);
    }

    /// CUP — absolute move; inputs are 0-based and clamped to the grid.
    pub fn move_to(&mut self, row: usize, col: usize) {
        self.wrap_pending = false;
        self.cursor_row = row.min(self.rows - 1);
        self.cursor_col = col.min(self.cols - 1);
    }

    // --- erasing (CSI J / K) ---------------------------------------------

    /// EL — erase in line. mode 0: cursor→end, 1: start→cursor, 2: whole line.
    pub fn erase_in_line(&mut self, mode: u16) {
        let row = self.cursor_row;
        let (start, end) = match mode {
            1 => (0, self.cursor_col + 1),
            2 => (0, self.cols),
            _ => (self.cursor_col, self.cols),
        };
        let blank = self.blank();
        for col in start..end {
            let idx = self.index(row, col);
            self.cells[idx] = blank;
        }
    }

    /// ED — erase in display. mode 0: cursor→end, 1: start→cursor, 2: all.
    pub fn erase_in_display(&mut self, mode: u16) {
        let blank = self.blank();
        let cursor = self.index(self.cursor_row, self.cursor_col);
        let (start, end) = match mode {
            1 => (0, cursor + 1),
            2 | 3 => (0, self.cells.len()),
            _ => (cursor, self.cells.len()),
        };
        for cell in &mut self.cells[start..end] {
            *cell = blank;
        }
    }

    /// Drop the top row, shift everything up, and blank the new bottom row.
    fn scroll_up(&mut self) {
        let blank = self.blank();
        self.cells.drain(0..self.cols);
        self.cells.resize(self.rows * self.cols, blank);
    }

    // --- debugging / headless rendering ----------------------------------

    /// Render the grid as plain text (one line per row), for headless dumps.
    pub fn to_text(&self) -> String {
        let mut s = String::with_capacity(self.rows * (self.cols + 1));
        for row in 0..self.rows {
            for col in 0..self.cols {
                s.push(self.cell(row, col).ch);
            }
            if row + 1 < self.rows {
                s.push('\n');
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_advances_and_wraps() {
        let mut g = Grid::new(3, 4);
        for ch in "abcde".chars() {
            g.print(ch);
        }
        // "abcd" fills row 0, "e" wraps to row 1 col 0.
        assert_eq!(g.cell(0, 0).ch, 'a');
        assert_eq!(g.cell(0, 3).ch, 'd');
        assert_eq!(g.cell(1, 0).ch, 'e');
        assert_eq!((g.cursor_row, g.cursor_col), (1, 1));
    }

    #[test]
    fn carriage_return_and_line_feed() {
        let mut g = Grid::new(3, 8);
        for ch in "hi".chars() {
            g.print(ch);
        }
        g.carriage_return();
        g.line_feed();
        assert_eq!((g.cursor_row, g.cursor_col), (1, 0));
    }

    #[test]
    fn move_to_clamps() {
        let mut g = Grid::new(3, 4);
        g.move_to(99, 99);
        assert_eq!((g.cursor_row, g.cursor_col), (2, 3));
    }

    #[test]
    fn erase_line_to_end() {
        let mut g = Grid::new(1, 5);
        for ch in "hello".chars() {
            g.print(ch);
        }
        g.move_to(0, 2);
        g.erase_in_line(0); // erase from col 2 to end
        assert_eq!(g.to_text(), "he   ");
    }

    #[test]
    fn scroll_on_overflow() {
        let mut g = Grid::new(2, 3);
        // Fill both rows, then one more line feed scrolls row 0 away.
        for ch in "abcdef".chars() {
            g.print(ch);
        }
        g.carriage_return();
        g.line_feed(); // at bottom -> scroll
        assert_eq!(g.cell(0, 0).ch, 'd'); // old row 1 became row 0
    }
}

//! The grid: a rows×cols matrix of [`Cell`]s plus a cursor and the current
//! "pen" (the fg/bg/attrs applied to newly printed characters). This is the
//! in-memory model of what's on screen — the parser mutates it, the renderer
//! reads it.

use std::collections::VecDeque;

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
    /// Whether the cursor should be drawn (toggled by DECTCEM `?25 h/l`).
    cursor_visible: bool,
    /// Lines that have scrolled off the top, oldest at the front.
    scrollback: VecDeque<Vec<Cell>>,
    max_scrollback: usize,
    /// How many lines we're scrolled up from the live screen (0 = at bottom).
    view_offset: usize,
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
            cursor_visible: true,
            scrollback: VecDeque::new(),
            max_scrollback: 1000,
            view_offset: 0,
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

    /// Archive the top row into scrollback, shift everything up, and blank the
    /// new bottom row.
    fn scroll_up(&mut self) {
        let blank = self.blank();
        let top: Vec<Cell> = self.cells[0..self.cols].to_vec();
        self.scrollback.push_back(top);
        while self.scrollback.len() > self.max_scrollback {
            self.scrollback.pop_front();
        }
        // If the user is scrolled up, follow the content so their viewport stays
        // anchored on the same lines as new output pushes in.
        if self.view_offset > 0 {
            self.view_offset = (self.view_offset + 1).min(self.scrollback.len());
        }
        self.cells.drain(0..self.cols);
        self.cells.resize(self.rows * self.cols, blank);
    }

    // --- scrollback & cursor visibility ----------------------------------

    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    /// Cap scrollback length; 0 disables it (used for the alternate screen).
    pub fn set_max_scrollback(&mut self, max: usize) {
        self.max_scrollback = max;
        while self.scrollback.len() > self.max_scrollback {
            self.scrollback.pop_front();
        }
    }

    /// Blank every cell and reset the cursor/pen/view to a fresh state (used
    /// when entering the alternate screen).
    pub fn clear(&mut self) {
        self.cells.iter_mut().for_each(|c| *c = Cell::default());
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.wrap_pending = false;
        self.pen = Cell::default();
        self.view_offset = 0;
    }
    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }
    pub fn view_offset(&self) -> usize {
        self.view_offset
    }

    /// Scroll the viewport: positive = up into history, negative = back down.
    pub fn scroll_view(&mut self, delta: isize) {
        let max = self.scrollback.len() as isize;
        let next = (self.view_offset as isize + delta).clamp(0, max);
        self.view_offset = next as usize;
    }

    /// Snap back to the live screen (called on keystrokes).
    pub fn reset_view(&mut self) {
        self.view_offset = 0;
    }

    /// The cell shown at screen position (row, col), accounting for how far the
    /// viewport is scrolled into scrollback. Returned by value (Cell is Copy) so
    /// out-of-range columns in mixed-width scrollback yield a blank safely.
    pub fn visible_cell(&self, row: usize, col: usize) -> Cell {
        if row < self.view_offset {
            let sb = self.scrollback.len() - self.view_offset + row;
            self.scrollback[sb].get(col).copied().unwrap_or_default()
        } else {
            let live = row - self.view_offset;
            self.cells[live * self.cols + col]
        }
    }

    /// Resize the grid to `new_rows` × `new_cols`, preserving existing content
    /// anchored top-left and clamping the cursor. No reflow (re-wrapping) yet —
    /// the shell redraws its prompt on SIGWINCH, which refreshes the live line.
    pub fn resize(&mut self, new_rows: usize, new_cols: usize) {
        let new_rows = new_rows.max(1);
        let new_cols = new_cols.max(1);
        if new_rows == self.rows && new_cols == self.cols {
            return;
        }

        let blank = self.blank();
        let mut next = vec![blank; new_rows * new_cols];
        let copy_rows = self.rows.min(new_rows);
        let copy_cols = self.cols.min(new_cols);
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                next[r * new_cols + c] = self.cells[r * self.cols + c];
            }
        }

        self.cells = next;
        self.rows = new_rows;
        self.cols = new_cols;
        self.cursor_row = self.cursor_row.min(new_rows - 1);
        self.cursor_col = self.cursor_col.min(new_cols - 1);
        self.wrap_pending = false;
        self.view_offset = 0;
    }

    // --- debugging / headless rendering ----------------------------------

    /// The text from `from` to `to` (inclusive of `from`, exclusive of `to`),
    /// reading across rows. Used to capture a typed command between OSC marks.
    pub fn text_between(&self, from: (usize, usize), to: (usize, usize)) -> String {
        let (r0, c0) = from;
        let (r1, c1) = to;
        if r1 < r0 || r0 >= self.rows {
            return String::new();
        }
        let mut s = String::new();
        if r0 == r1 {
            for c in c0..c1.min(self.cols) {
                s.push(self.cell(r0, c).ch);
            }
        } else {
            for c in c0..self.cols {
                s.push(self.cell(r0, c).ch);
            }
            for r in (r0 + 1)..r1.min(self.rows) {
                s.push('\n');
                for c in 0..self.cols {
                    s.push(self.cell(r, c).ch);
                }
            }
            if r1 < self.rows {
                s.push('\n');
                for c in 0..c1.min(self.cols) {
                    s.push(self.cell(r1, c).ch);
                }
            }
        }
        s.trim_end().to_string()
    }

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

    /// Push three rows through a 2-row grid; the first row lands in scrollback,
    /// and scrolling the view up reveals it on screen row 0.
    #[test]
    fn scrollback_archives_and_views() {
        let mut g = Grid::new(2, 3);
        for line in ["aaa", "bbb", "ccc"] {
            for ch in line.chars() {
                g.print(ch);
            }
            g.carriage_return();
            g.line_feed();
        }
        // "aaa" and "bbb" scrolled into scrollback; live screen shows "ccc".
        assert_eq!(g.view_offset(), 0);
        let live0: String = (0..3).map(|c| g.visible_cell(0, c).ch).collect();
        assert_eq!(live0, "ccc");

        // Up one line: the most-recent archived line "bbb" appears on top,
        // pushing "ccc" down to row 1.
        g.scroll_view(1);
        assert_eq!(g.view_offset(), 1);
        let row0: String = (0..3).map(|c| g.visible_cell(0, c).ch).collect();
        let row1: String = (0..3).map(|c| g.visible_cell(1, c).ch).collect();
        assert_eq!((row0.as_str(), row1.as_str()), ("bbb", "ccc"));

        // Up another line reveals the oldest line "aaa" at the top.
        g.scroll_view(1);
        assert_eq!(g.view_offset(), 2);
        let row0: String = (0..3).map(|c| g.visible_cell(0, c).ch).collect();
        assert_eq!(row0, "aaa");
    }

    #[test]
    fn scroll_view_clamps() {
        let mut g = Grid::new(2, 3);
        g.scroll_view(10); // nothing in scrollback yet
        assert_eq!(g.view_offset(), 0);
        g.scroll_view(-10);
        assert_eq!(g.view_offset(), 0);
    }

    #[test]
    fn cursor_visibility_toggles() {
        let mut g = Grid::new(2, 2);
        assert!(g.cursor_visible());
        g.set_cursor_visible(false);
        assert!(!g.cursor_visible());
    }

    #[test]
    fn resize_grow_preserves_content() {
        let mut g = Grid::new(2, 3);
        for ch in "abc".chars() {
            g.print(ch);
        }
        g.resize(4, 5); // grow both dimensions
        assert_eq!(g.rows, 4);
        assert_eq!(g.cols, 5);
        let row0: String = (0..3).map(|c| g.cell(0, c).ch).collect();
        assert_eq!(row0, "abc"); // top-left content kept
    }

    #[test]
    fn resize_shrink_clamps_cursor() {
        let mut g = Grid::new(5, 5);
        g.move_to(4, 4);
        g.resize(2, 2);
        assert_eq!((g.cursor_row, g.cursor_col), (1, 1)); // clamped into bounds
    }

    #[test]
    fn visible_cell_safe_after_width_change() {
        // Archive a wide line, then shrink width; reading the old line must not panic.
        let mut g = Grid::new(2, 6);
        for ch in "wwwwww".chars() {
            g.print(ch);
        }
        g.carriage_return();
        g.line_feed();
        for ch in "xxxxxx".chars() {
            g.print(ch);
        }
        g.carriage_return();
        g.line_feed(); // "wwwwww" -> scrollback (width 6)
        g.resize(2, 3); // now narrower than the archived line
        g.scroll_view(1);
        // Reading columns 0..3 of the archived wide line is safe and correct.
        let row0: String = (0..3).map(|c| g.visible_cell(0, c).ch).collect();
        assert_eq!(row0, "www");
    }
}

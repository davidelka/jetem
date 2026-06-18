//! Text selection over a pane's visible cells. Linear (reading-order) selection
//! from an `anchor` cell to a `head` cell; extraction reads through
//! `Grid::visible_cell` so it works over scrollback too.

use crate::grid::Grid;
use crate::layout::PaneId;

#[derive(Clone, Copy)]
pub struct Selection {
    pub pane: PaneId,
    /// Where the drag started (screen row, col).
    pub anchor: (usize, usize),
    /// Where the cursor is now (screen row, col).
    pub head: (usize, usize),
}

impl Selection {
    pub fn new(pane: PaneId, pos: (usize, usize)) -> Self {
        Self {
            pane,
            anchor: pos,
            head: pos,
        }
    }

    pub fn set_head(&mut self, pos: (usize, usize)) {
        self.head = pos;
    }

    /// (start, end) in reading order (start <= end).
    fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Is (row, col) within the selection? Inclusive of both ends.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let (s, e) = self.normalized();
        (row, col) >= s && (row, col) <= e
    }

    /// The selected text, with trailing spaces trimmed per line and a newline
    /// between rows.
    pub fn text(&self, grid: &Grid) -> String {
        let (s, e) = self.normalized();
        let last_col = grid.cols.saturating_sub(1);
        let mut out = String::new();
        for row in s.0..=e.0 {
            if row >= grid.rows {
                break;
            }
            let c0 = if row == s.0 { s.1 } else { 0 };
            let c1 = if row == e.0 { e.1 } else { last_col };
            let mut line = String::new();
            for col in c0..=c1.min(last_col) {
                line.push(grid.visible_cell(row, col).ch);
            }
            out.push_str(line.trim_end());
            if row != e.0 {
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_single_line() {
        let mut g = Grid::new(2, 10);
        for ch in "hello".chars() {
            g.print(ch);
        }
        let sel = Selection {
            pane: 0,
            anchor: (0, 0),
            head: (0, 4),
        };
        assert_eq!(sel.text(&g), "hello");
    }

    #[test]
    fn text_multi_line_trims_trailing() {
        let mut g = Grid::new(3, 5);
        for ch in "ab".chars() {
            g.print(ch);
        }
        g.carriage_return();
        g.line_feed();
        for ch in "cd".chars() {
            g.print(ch);
        }
        let sel = Selection {
            pane: 0,
            anchor: (0, 0),
            head: (1, 1),
        };
        assert_eq!(sel.text(&g), "ab\ncd");
    }

    #[test]
    fn normalizes_reversed_drag() {
        let sel = Selection {
            pane: 0,
            anchor: (1, 1),
            head: (0, 0),
        };
        assert!(sel.contains(0, 0));
        assert!(sel.contains(1, 1));
        assert!(!sel.contains(1, 2));
    }
}

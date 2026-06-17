//! A terminal's two screen buffers. The **primary** buffer is the normal
//! scrolling screen (with scrollback) — your shell. The **alternate** buffer is
//! a separate, scrollback-less canvas that full-screen apps (vim, less, htop)
//! switch to via `\x1b[?1049h` and switch back from via `\x1b[?1049l`, leaving
//! the primary buffer (and your scrollback) untouched so it restores on exit.
//!
//! This is core VT protocol, per terminal. It lives *inside* one terminal; the
//! pane/plugin/compositor layer sits above it (see CLAUDE.md, protocol-vs-policy).

use crate::grid::Grid;

pub struct Screen {
    primary: Grid,
    alt: Grid,
    alt_active: bool,
}

impl Screen {
    pub fn new(rows: usize, cols: usize) -> Self {
        let primary = Grid::new(rows, cols);
        let mut alt = Grid::new(rows, cols);
        alt.set_max_scrollback(0); // the alternate screen has no scrollback
        Self {
            primary,
            alt,
            alt_active: false,
        }
    }

    /// The buffer currently shown / written to.
    pub fn active(&self) -> &Grid {
        if self.alt_active {
            &self.alt
        } else {
            &self.primary
        }
    }

    pub fn active_mut(&mut self) -> &mut Grid {
        if self.alt_active {
            &mut self.alt
        } else {
            &mut self.primary
        }
    }

    /// Switch to a fresh alternate screen (the primary is left intact).
    pub fn enter_alt(&mut self) {
        if !self.alt_active {
            self.alt.clear();
            self.alt_active = true;
        }
    }

    /// Switch back to the primary screen, which restores automatically since it
    /// was never modified while alt was active.
    pub fn leave_alt(&mut self) {
        self.alt_active = false;
    }

    /// Resize both buffers so a window resize is consistent regardless of which
    /// is active (e.g. SIGWINCH while inside vim).
    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.primary.resize(rows, cols);
        self.alt.resize(rows, cols);
    }

    pub fn rows(&self) -> usize {
        self.active().rows
    }
    pub fn cols(&self) -> usize {
        self.active().cols
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_line(g: &mut Grid, s: &str) {
        for ch in s.chars() {
            g.print(ch);
        }
    }

    #[test]
    fn alt_preserves_primary() {
        let mut screen = Screen::new(2, 5);
        type_line(screen.active_mut(), "shell");

        screen.enter_alt();
        // Alt starts blank; drawing here doesn't touch the primary.
        type_line(screen.active_mut(), "vimUI");
        let alt0: String = (0..5).map(|c| screen.active().cell(0, c).ch).collect();
        assert_eq!(alt0, "vimUI");

        screen.leave_alt();
        let prim0: String = (0..5).map(|c| screen.active().cell(0, c).ch).collect();
        assert_eq!(prim0, "shell"); // primary restored intact
    }

    #[test]
    fn alt_has_no_scrollback() {
        let mut screen = Screen::new(2, 3);
        screen.enter_alt();
        // Scroll the alt buffer past its height.
        for line in ["aaa", "bbb", "ccc", "ddd"] {
            type_line(screen.active_mut(), line);
            screen.active_mut().carriage_return();
            screen.active_mut().line_feed();
        }
        // No history accumulated: scrolling up is a no-op.
        screen.active_mut().scroll_view(5);
        assert_eq!(screen.active().view_offset(), 0);
    }

    #[test]
    fn resize_affects_both_buffers() {
        let mut screen = Screen::new(2, 3);
        screen.resize(4, 6);
        assert_eq!((screen.rows(), screen.cols()), (4, 6));
        screen.enter_alt();
        assert_eq!((screen.rows(), screen.cols()), (4, 6)); // alt resized too
    }
}

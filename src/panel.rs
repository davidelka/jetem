//! A modal, scrollable, read-only text panel — for longer content like AI
//! answers (the toast is too cramped). Word-wraps the body to its width and
//! scrolls with the arrow/page keys. Drawn with the `render` UI primitives.

use crate::font::Font;
use crate::pane::Rect;
use crate::render;

const MAX_COLS: usize = 76;
const MAX_ROWS: usize = 22;

const PANEL_BG: (u8, u8, u8) = (24, 26, 34);
const TITLE: (u8, u8, u8) = (120, 180, 250);
const TEXT: (u8, u8, u8) = (210, 210, 220);
const HINT: (u8, u8, u8) = (120, 120, 135);
const BORDER: u32 = 0x00_5a_9c_e6;

pub struct TextPanel {
    title: String,
    /// Word-wrapped body lines.
    lines: Vec<String>,
    /// Index of the first visible line.
    scroll: usize,
    cols: usize,
}

impl TextPanel {
    pub fn new(title: String, body: &str, max_cols: usize) -> Self {
        let cols = MAX_COLS.min(max_cols.max(10));
        Self {
            title,
            lines: wrap(body, cols),
            scroll: 0,
            cols,
        }
    }

    fn visible_rows(&self) -> usize {
        MAX_ROWS.min(self.lines.len()).max(1)
    }

    pub fn scroll(&mut self, delta: isize) {
        let max = self.lines.len().saturating_sub(self.visible_rows()) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max.max(0)) as usize;
    }

    pub fn page(&self) -> isize {
        self.visible_rows() as isize - 1
    }

    /// Draw the panel centered in the window.
    pub fn draw(&self, buf: &mut [u32], w: usize, h: usize, font: &mut Font) {
        let (cw, ch) = (font.cell_w, font.cell_h);
        let pad = 10;
        let rows = self.visible_rows();
        let panel_w = self.cols * cw + pad * 2;
        // title row + body rows + footer row
        let panel_h = (rows + 2) * ch + pad * 2;
        let px = w.saturating_sub(panel_w) / 2;
        let py = h.saturating_sub(panel_h) / 3;
        let panel = Rect::new(px, py, panel_w, panel_h);

        render::fill(buf, w, h, panel, PANEL_BG);
        render::draw_border(buf, w, h, panel, BORDER, 1);

        let tx = px + pad;
        let mut ty = py + pad;

        // Title.
        render::draw_text(buf, w, h, font, tx, ty, &self.title, TITLE, Some(PANEL_BG));
        ty += ch + ch / 2;

        // Body.
        for line in self.lines.iter().skip(self.scroll).take(rows) {
            render::draw_text(buf, w, h, font, tx, ty, line, TEXT, Some(PANEL_BG));
            ty += ch;
        }

        // Footer hint.
        let more = self.lines.len() > self.scroll + rows || self.scroll > 0;
        let hint = if more {
            "↑/↓ PgUp/PgDn scroll · Esc close"
        } else {
            "Esc close"
        };
        render::draw_text(buf, w, h, font, tx, py + panel_h - pad - ch, hint, HINT, Some(PANEL_BG));
    }
}

/// Greedy word-wrap to `cols` columns, preserving existing newlines and hard-
/// splitting any word longer than the width.
fn wrap(body: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    for raw in body.split('\n') {
        let mut line = String::new();
        for word in raw.split(' ') {
            let wlen = word.chars().count();
            if line.is_empty() {
                line.push_str(word);
            } else if line.chars().count() + 1 + wlen <= cols {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line.push_str(word);
            }
            // A single word wider than the panel: hard-split it.
            while line.chars().count() > cols {
                let head: String = line.chars().take(cols).collect();
                out.push(head);
                line = line.chars().skip(cols).collect();
            }
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_lines_and_keeps_newlines() {
        let lines = wrap("hello world foo\nbar", 9);
        // "hello world foo" wraps at 9 cols -> "hello", "world foo"; then "bar".
        assert_eq!(lines, vec!["hello", "world foo", "bar"]);
    }

    #[test]
    fn hard_splits_overlong_word() {
        let lines = wrap("abcdefghij", 4);
        assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn scroll_clamps() {
        let mut p = TextPanel::new("t".into(), "a\nb\nc", 40);
        p.scroll(-5);
        assert_eq!(p.scroll, 0);
        p.scroll(100);
        assert!(p.scroll <= p.lines.len());
    }
}

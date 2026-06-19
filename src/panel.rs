//! A modal text panel — for longer content like AI answers (the toast is too
//! cramped). Word-wraps the body, scrolls, supports mouse text selection
//! (mark + copy), and an optional interactive input line for chat. Drawn with
//! the `render` UI primitives.

use crate::font::Font;
use crate::pane::Rect;
use crate::render;

const MAX_COLS: usize = 76;
const MAX_ROWS: usize = 22;
const PAD: usize = 10;

const PANEL_BG: (u8, u8, u8) = (24, 26, 34);
const TITLE: (u8, u8, u8) = (120, 180, 250);
const TEXT: (u8, u8, u8) = (210, 210, 220);
const HINT: (u8, u8, u8) = (120, 120, 135);
const SEL_BG: (u8, u8, u8) = (50, 82, 122);
const INPUT_FG: (u8, u8, u8) = (235, 235, 245);
const BORDER: u32 = 0x00_5a_9c_e6;

/// Cached geometry, computed the same way for drawing and hit-testing.
struct Geo {
    rect: Rect,
    content_x: usize,
    content_y: usize,
    cw: usize,
    ch: usize,
    rows: usize,
}

pub struct TextPanel {
    title: String,
    lines: Vec<String>,
    scroll: usize,
    cols: usize,
    // selection over wrapped lines: (line, col) char indices
    anchor: Option<(usize, usize)>,
    head: (usize, usize),
    // interactive chat
    pub interactive: bool,
    pub owner: usize, // PluginId that opened the panel
    input: String,
}

impl TextPanel {
    pub fn new(title: String, body: &str, max_cols: usize, interactive: bool, owner: usize) -> Self {
        let cols = MAX_COLS.min(max_cols.max(10));
        Self {
            title,
            lines: wrap(body, cols),
            scroll: 0,
            cols,
            anchor: None,
            head: (0, 0),
            interactive,
            owner,
            input: String::new(),
        }
    }

    fn visible_rows(&self) -> usize {
        MAX_ROWS.min(self.lines.len()).max(1)
    }

    pub fn page(&self) -> isize {
        self.visible_rows() as isize - 1
    }

    pub fn scroll(&mut self, delta: isize) {
        let max = self.lines.len().saturating_sub(self.visible_rows()) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max.max(0)) as usize;
    }

    // --- interactive input ------------------------------------------------

    pub fn on_char(&mut self, c: char) {
        self.input.push(c);
    }
    pub fn on_backspace(&mut self) {
        self.input.pop();
    }
    /// Take the typed input (non-empty), clearing it.
    pub fn take_input(&mut self) -> Option<String> {
        if self.input.trim().is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.input))
        }
    }

    // --- selection --------------------------------------------------------

    fn geo(&self, w: usize, h: usize, font: &Font) -> Geo {
        let (cw, ch) = (font.cell_w, font.cell_h);
        let rows = self.visible_rows();
        let extra = if self.interactive { 1 } else { 0 };
        let panel_w = self.cols * cw + PAD * 2;
        let panel_h = (rows + 2 + extra) * ch + PAD * 2;
        let px = w.saturating_sub(panel_w) / 2;
        let py = h.saturating_sub(panel_h) / 3;
        Geo {
            rect: Rect::new(px, py, panel_w, panel_h),
            content_x: px + PAD,
            content_y: py + PAD + ch + ch / 2, // below the title
            cw,
            ch,
            rows,
        }
    }

    /// Map a pixel to a (line, col) within the body, if it's over the text.
    pub fn cell_at(&self, px: f64, py: f64, w: usize, h: usize, font: &Font) -> Option<(usize, usize)> {
        let g = self.geo(w, h, font);
        let (px, py) = (px as usize, py as usize);
        if px < g.content_x || py < g.content_y {
            return None;
        }
        let row = (py - g.content_y) / g.ch;
        if row >= g.rows {
            return None;
        }
        let line = self.scroll + row;
        if line >= self.lines.len() {
            return None;
        }
        let col = ((px - g.content_x) / g.cw).min(self.lines[line].chars().count());
        Some((line, col))
    }

    pub fn begin_select(&mut self, pos: (usize, usize)) {
        self.anchor = Some(pos);
        self.head = pos;
    }
    pub fn extend_select(&mut self, pos: (usize, usize)) {
        self.head = pos;
    }

    fn normalized(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        Some(if a <= self.head { (a, self.head) } else { (self.head, a) })
    }

    /// Selected text, or the whole body if nothing is selected.
    pub fn copy_text(&self) -> String {
        match self.normalized() {
            Some((s, e)) if s != e => {
                let mut out = String::new();
                for li in s.0..=e.0.min(self.lines.len().saturating_sub(1)) {
                    let chars: Vec<char> = self.lines[li].chars().collect();
                    let c0 = if li == s.0 { s.1 } else { 0 }.min(chars.len());
                    let c1 = if li == e.0 { e.1 } else { chars.len() }.min(chars.len());
                    out.extend(&chars[c0..c1.max(c0)]);
                    if li != e.0 {
                        out.push('\n');
                    }
                }
                out
            }
            _ => self.lines.join("\n"),
        }
    }

    /// Selected (start, end) columns for a given body line, if highlighted.
    fn sel_cols(&self, line_idx: usize, line_len: usize) -> Option<(usize, usize)> {
        let (s, e) = self.normalized()?;
        if s == e || line_idx < s.0 || line_idx > e.0 {
            return None;
        }
        let c0 = if line_idx == s.0 { s.1 } else { 0 };
        let c1 = if line_idx == e.0 { e.1 } else { line_len };
        Some((c0.min(line_len), c1.min(line_len)))
    }

    // --- drawing ----------------------------------------------------------

    pub fn draw(&self, buf: &mut [u32], w: usize, h: usize, font: &mut Font) {
        let g = self.geo(w, h, font);
        render::fill(buf, w, h, g.rect, PANEL_BG);
        render::draw_border(buf, w, h, g.rect, BORDER, 1);

        render::draw_text(buf, w, h, font, g.content_x, g.rect.y + PAD, &self.title, TITLE, Some(PANEL_BG));

        for row in 0..g.rows {
            let line_idx = self.scroll + row;
            if line_idx >= self.lines.len() {
                break;
            }
            let line = &self.lines[line_idx];
            let y = g.content_y + row * g.ch;
            if let Some((c0, c1)) = self.sel_cols(line_idx, line.chars().count()) {
                let hx = g.content_x + c0 * g.cw;
                let hw = (c1 - c0) * g.cw;
                render::fill(buf, w, h, Rect::new(hx, y, hw, g.ch), SEL_BG);
            }
            render::draw_text(buf, w, h, font, g.content_x, y, line, TEXT, None);
        }

        let footer_y = g.rect.y + g.rect.h - PAD - g.ch;
        if self.interactive {
            let prompt = format!("> {}", self.input);
            render::draw_text(buf, w, h, font, g.content_x, footer_y, &prompt, INPUT_FG, Some(PANEL_BG));
        } else {
            render::draw_text(
                buf, w, h, font, g.content_x, footer_y,
                "drag to select · Ctrl-Shift-C copy · ↑/↓ scroll · Esc close",
                HINT, Some(PANEL_BG),
            );
        }
    }
}

/// Greedy word-wrap to `cols`, preserving newlines and hard-splitting overlong words.
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

    fn panel(body: &str) -> TextPanel {
        TextPanel::new("t".into(), body, 40, false, 0)
    }

    #[test]
    fn wraps_long_lines_and_keeps_newlines() {
        assert_eq!(wrap("hello world foo\nbar", 9), vec!["hello", "world foo", "bar"]);
    }

    #[test]
    fn hard_splits_overlong_word() {
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn copy_selection_within_line() {
        let mut p = panel("hello world");
        p.begin_select((0, 0));
        p.extend_select((0, 5));
        assert_eq!(p.copy_text(), "hello");
    }

    #[test]
    fn copy_all_when_no_selection() {
        let p = panel("a\nb");
        assert_eq!(p.copy_text(), "a\nb");
    }

    #[test]
    fn copy_selection_across_lines() {
        let mut p = panel("foo\nbar\nbaz");
        p.begin_select((0, 1));
        p.extend_select((2, 2));
        assert_eq!(p.copy_text(), "oo\nbar\nba");
    }
}

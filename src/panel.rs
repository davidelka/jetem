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
const HEADER_FG: (u8, u8, u8) = (150, 200, 255); // table header text
const HEADER_BG: (u8, u8, u8) = (38, 44, 60); // table header band
const STRIPE_BG: (u8, u8, u8) = (30, 33, 43); // zebra-stripe for odd body rows
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
    // when set, the panel renders as a table (header band + zebra rows); the raw
    // data is kept for TSV copy. `lines` holds the rendered, fixed-width rows.
    table: Option<TableMeta>,
}

/// Raw table data, kept for TSV copy and header/zebra styling in `draw`.
struct TableMeta {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
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
            table: None,
        }
    }

    /// Build a panel that renders a table: a header row (accent band) over
    /// aligned, zebra-striped body rows. Read-only (non-interactive). Columns are
    /// sized to their widest cell, then shrunk widest-first and truncated with `…`
    /// to fit `max_cols`.
    pub fn new_table(
        title: String,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        max_cols: usize,
        owner: usize,
    ) -> Self {
        let cap = MAX_COLS.min(max_cols.max(10));
        let (lines, width) = render_table(&headers, &rows, cap);
        Self {
            title,
            lines,
            scroll: 0,
            cols: width.max(10),
            anchor: None,
            head: (0, 0),
            interactive: false,
            owner,
            input: String::new(),
            table: Some(TableMeta { headers, rows }),
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
            // No selection: a table copies as TSV; plain text copies its lines.
            _ => match &self.table {
                Some(t) => {
                    let mut out = t.headers.join("\t");
                    for r in &t.rows {
                        out.push('\n');
                        out.push_str(&r.join("\t"));
                    }
                    out
                }
                None => self.lines.join("\n"),
            },
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

        let table = self.table.is_some();
        for row in 0..g.rows {
            let line_idx = self.scroll + row;
            if line_idx >= self.lines.len() {
                break;
            }
            let line = &self.lines[line_idx];
            let y = g.content_y + row * g.ch;
            // Table styling: the header row (line 0) gets an accent band; even
            // body rows get a subtle zebra stripe.
            if table {
                let full = Rect::new(g.content_x, y, self.cols * g.cw, g.ch);
                if line_idx == 0 {
                    render::fill(buf, w, h, full, HEADER_BG);
                } else if line_idx % 2 == 0 {
                    render::fill(buf, w, h, full, STRIPE_BG);
                }
            }
            if let Some((c0, c1)) = self.sel_cols(line_idx, line.chars().count()) {
                let hx = g.content_x + c0 * g.cw;
                let hw = (c1 - c0) * g.cw;
                render::fill(buf, w, h, Rect::new(hx, y, hw, g.ch), SEL_BG);
            }
            let fg = if table && line_idx == 0 { HEADER_FG } else { TEXT };
            render::draw_text(buf, w, h, font, g.content_x, y, line, fg, None);
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

/// Render a table to fixed-width display lines (header first, then rows) and
/// return them with the total table width in columns. Columns are sized to the
/// widest cell, then shrunk widest-first and truncated with `…` to fit `cols`.
fn render_table(headers: &[String], rows: &[Vec<String>], cols: usize) -> (Vec<String>, usize) {
    const SEP: usize = 2; // spaces between columns
    const MINW: usize = 3; // never shrink a column below this
    let ncols = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);

    let mut widths = vec![0usize; ncols];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = widths[i].max(h.chars().count());
    }
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            if i < ncols {
                widths[i] = widths[i].max(c.chars().count());
            }
        }
    }

    // Shrink the widest column until the table fits the budget.
    let sep_total = SEP * (ncols - 1);
    let budget = cols.saturating_sub(sep_total).max(ncols * MINW);
    while widths.iter().sum::<usize>() > budget {
        let mi = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .map(|(i, _)| i)
            .unwrap();
        if widths[mi] <= MINW {
            break;
        }
        widths[mi] -= 1;
    }

    let fmt = |cells: &[String]| -> String {
        let mut s = String::new();
        for i in 0..ncols {
            if i > 0 {
                s.push_str("  ");
            }
            s.push_str(&pad_trunc(cells.get(i).map(String::as_str).unwrap_or(""), widths[i]));
        }
        s
    };
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(fmt(headers));
    for r in rows {
        lines.push(fmt(r));
    }
    (lines, widths.iter().sum::<usize>() + sep_total)
}

/// Left-justify `s` to `w` columns, truncating with a trailing `…` if too long.
fn pad_trunc(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n > w {
        if w == 0 {
            return String::new();
        }
        let mut t: String = s.chars().take(w - 1).collect();
        t.push('…');
        t
    } else {
        let mut t = String::from(s);
        t.extend(std::iter::repeat(' ').take(w - n));
        t
    }
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

    #[test]
    fn table_renders_header_then_rows() {
        let p = TextPanel::new_table(
            "t".into(),
            vec!["NAME".into(), "AGE".into()],
            vec![
                vec!["alice".into(), "30".into()],
                vec!["bob".into(), "7".into()],
            ],
            80,
            0,
        );
        assert_eq!(p.lines.len(), 3); // header + 2 rows
        assert!(p.lines[0].starts_with("NAME"));
        assert!(p.lines[1].starts_with("alice"));
    }

    #[test]
    fn table_copies_as_tsv() {
        let p = TextPanel::new_table(
            "t".into(),
            vec!["a".into(), "b".into()],
            vec![vec!["1".into(), "2".into()]],
            80,
            0,
        );
        assert_eq!(p.copy_text(), "a\tb\n1\t2");
    }

    #[test]
    fn table_truncates_overlong_cell() {
        let (lines, _w) = render_table(&["h".to_string()], &[vec!["abcdefghij".to_string()]], 5);
        assert_eq!(lines[1].chars().count(), 5);
        assert!(lines[1].ends_with('…'));
    }
}

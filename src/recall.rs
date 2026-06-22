//! The command-recall overlay: a searchable list of past commands, drawn over
//! the panes. The first custom-drawn UI (not a terminal grid) — the prototype
//! for future plugin widgets. Filtering logic here is pure and unit-tested;
//! drawing uses the `render::draw_text` primitive.

use crate::block::{load_history, Block};
use crate::font::Font;
use crate::pane::Rect;
use crate::render;
use crate::theme::Theme;

const VISIBLE_ROWS: usize = 12;
const MAX_COLS: usize = 70;

pub struct Recall {
    /// History, most-recent-first and deduped by command.
    all: Vec<Block>,
    query: String,
    /// Indices into `all` matching the query.
    results: Vec<usize>,
    selected: usize,
    /// Index of the first visible result row.
    scroll: usize,
}

impl Recall {
    /// Open over on-disk history plus this session's in-memory blocks (so it
    /// works even if the file was just deleted or hasn't been flushed).
    pub fn open(session: Vec<Block>) -> Self {
        let mut blocks = load_history();
        blocks.extend(session); // session blocks are the most recent
        blocks.reverse(); // most recent first
        let mut seen = std::collections::HashSet::new();
        let mut all = Vec::new();
        for b in blocks {
            if seen.insert(b.command.clone()) {
                all.push(b);
            }
        }
        let mut r = Self {
            all,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            scroll: 0,
        };
        r.refilter();
        r
    }

    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        self.results = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, b)| q.is_empty() || b.command.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn on_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn on_backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn move_sel(&mut self, delta: isize) {
        if self.results.is_empty() {
            return;
        }
        let n = self.results.len() as isize;
        self.selected = (self.selected as isize + delta).clamp(0, n - 1) as usize;
        // Keep the selection within the visible window.
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + VISIBLE_ROWS {
            self.scroll = self.selected + 1 - VISIBLE_ROWS;
        }
    }

    pub fn selected_command(&self) -> Option<String> {
        self.results
            .get(self.selected)
            .map(|&i| self.all[i].command.clone())
    }

    /// The output captured for the selected block (for "copy block output").
    pub fn selected_output(&self) -> Option<String> {
        self.results
            .get(self.selected)
            .map(|&i| self.all[i].output.clone())
    }

    /// Draw the overlay centered near the top of the window.
    pub fn draw(&self, buf: &mut [u32], w: usize, h: usize, font: &mut Font, theme: &Theme) {
        let r = &theme.recall;
        let (cw, ch) = (font.cell_w, font.cell_h);
        let pad = 8;
        let cols = MAX_COLS.min((w / cw).saturating_sub(4)).max(10);
        let rows_shown = VISIBLE_ROWS.min(self.results.len()).max(1);
        let panel_w = cols * cw + pad * 2;
        let panel_h = (rows_shown + 1) * ch + pad * 2;
        let px = w.saturating_sub(panel_w) / 2;
        let py = h / 6;
        let panel = Rect::new(px, py, panel_w, panel_h);

        render::fill(buf, w, h, panel, r.bg.rgb());
        render::draw_border(buf, w, h, panel, theme.ui.focus_border.packed(), 1);

        let tx = px + pad;
        let mut ty = py + pad;

        // Query line.
        let prompt = format!("> {}", self.query);
        render::draw_text(buf, w, h, font, tx, ty, &truncate(&prompt, cols), r.text.rgb(), Some(r.bg.rgb()));
        ty += ch;

        // Result rows.
        if self.results.is_empty() {
            render::draw_text(buf, w, h, font, tx, ty, "(no matches)", r.dim.rgb(), Some(r.bg.rgb()));
            return;
        }
        for k in 0..rows_shown {
            let ridx = self.scroll + k;
            if ridx >= self.results.len() {
                break;
            }
            let block = &self.all[self.results[ridx]];
            let selected = ridx == self.selected;
            let (fg, bg) = if selected { (r.sel_fg.rgb(), r.sel_bg.rgb()) } else { (r.text.rgb(), r.bg.rgb()) };
            if selected {
                render::fill(buf, w, h, Rect::new(px, ty, panel_w, ch), r.sel_bg.rgb());
            }
            let line = truncate(&block.command, cols);
            render::draw_text(buf, w, h, font, tx, ty, &line, fg, Some(bg));
            ty += ch;
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(cmd: &str) -> Block {
        Block {
            command: cmd.to_string(),
            output: String::new(),
            exit_code: Some(0),
            cwd: None,
            started_at_ms: 0,
        }
    }

    fn recall_with(cmds: &[&str]) -> Recall {
        // Build directly (bypass disk): emulate open()'s dedup over reversed history.
        let mut seen = std::collections::HashSet::new();
        let mut all = Vec::new();
        for c in cmds.iter().rev() {
            if seen.insert(c.to_string()) {
                all.push(block(c));
            }
        }
        let mut r = Recall {
            all,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            scroll: 0,
        };
        r.refilter();
        r
    }

    #[test]
    fn dedup_keeps_most_recent_order() {
        // history oldest->newest: git, ls, git  => recent-first deduped: git, ls
        let r = recall_with(&["git", "ls", "git"]);
        assert_eq!(r.all.len(), 2);
        assert_eq!(r.all[0].command, "git");
        assert_eq!(r.all[1].command, "ls");
    }

    #[test]
    fn substring_filter_is_case_insensitive() {
        let mut r = recall_with(&["GIT status", "ls", "cargo build"]);
        for c in "git".chars() {
            r.on_char(c);
        }
        assert_eq!(r.selected_command().as_deref(), Some("GIT status"));
        assert_eq!(r.results.len(), 1);
    }

    #[test]
    fn selection_clamps_at_ends() {
        let mut r = recall_with(&["a", "b", "c"]);
        r.move_sel(-5);
        assert_eq!(r.selected, 0);
        r.move_sel(99);
        assert_eq!(r.selected, r.results.len() - 1);
    }
}

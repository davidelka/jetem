//! Scrollback text search: an incremental, `less`/`vim`-style `/` search over a
//! terminal's history + live screen. Like the recall overlay, it reads in-process
//! grid state (scrollback isn't exposed to plugins), so it lives in core.
//!
//! The matching logic here is pure and unit-tested; the window drives it (feeds
//! the line list, routes keys) and `render` paints the highlights.

/// One match: an absolute line index (see `Grid::all_lines_text`), the starting
/// column, and the match length in chars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

pub struct Search {
    query: String,
    matches: Vec<Match>,
    /// Index into `matches` of the "current" (brightly highlighted) one.
    current: usize,
}

impl Search {
    pub fn new() -> Self {
        Self { query: String::new(), matches: Vec::new(), current: 0 }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// (current-1-based, total) — for the `/foo (2/7)` status line. `(0, 0)` when
    /// there are no matches.
    pub fn counts(&self) -> (usize, usize) {
        if self.matches.is_empty() {
            (0, 0)
        } else {
            (self.current + 1, self.matches.len())
        }
    }

    pub fn on_char(&mut self, c: char, lines: &[String]) {
        self.query.push(c);
        self.refilter(lines);
    }

    pub fn on_backspace(&mut self, lines: &[String]) {
        self.query.pop();
        self.refilter(lines);
    }

    /// Recompute all matches for the current query (case-insensitive substring),
    /// keeping the current selection on the nearest still-valid match.
    fn refilter(&mut self, lines: &[String]) {
        let anchor = self.current_match().map(|m| m.line);
        self.matches.clear();
        let q = self.query.to_lowercase();
        if !q.is_empty() {
            for (line_idx, line) in lines.iter().enumerate() {
                let hay = line.to_lowercase();
                // All (possibly overlapping-free) occurrences on this line.
                let mut from = 0;
                while let Some(rel) = hay[from..].find(&q) {
                    let col = char_col(line, from + rel);
                    self.matches.push(Match { line: line_idx, col, len: q.chars().count() });
                    from += rel + q.len();
                }
            }
        }
        // Re-anchor the selection near where it was, so typing more doesn't jump
        // the viewport all the way back to the top.
        self.current = match anchor {
            Some(a) => self.matches.iter().position(|m| m.line >= a).unwrap_or(0),
            None => 0,
        };
    }

    pub fn current_match(&self) -> Option<Match> {
        self.matches.get(self.current).copied()
    }

    /// Advance the selection by `delta` matches, wrapping around. Returns the new
    /// current match (so the caller can scroll it into view).
    pub fn step(&mut self, delta: isize) -> Option<Match> {
        if self.matches.is_empty() {
            return None;
        }
        let n = self.matches.len() as isize;
        self.current = (((self.current as isize + delta) % n + n) % n) as usize;
        self.current_match()
    }

    /// Is the cell at (`line`, `col`) inside any match? Returns whether it's a
    /// match at all and whether it's the *current* match (painted brighter).
    pub fn hit(&self, line: usize, col: usize) -> Option<bool> {
        let cur = self.current_match();
        self.matches
            .iter()
            .find(|m| m.line == line && col >= m.col && col < m.col + m.len)
            .map(|m| cur == Some(*m))
    }
}

/// Convert a byte offset within `s` to a column (char) index — matches are found
/// on `to_lowercase()` byte strings but painted per visible char cell.
fn char_col(s: &str, byte_off: usize) -> usize {
    s[..byte_off].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<String> {
        ["error: file not found", "compiling file.rs", "done"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn finds_all_case_insensitive() {
        let mut s = Search::new();
        for c in "FILE".chars() {
            s.on_char(c, &lines());
        }
        assert_eq!(s.counts(), (1, 2)); // "file" on lines 0 and 1
        assert_eq!(s.current_match(), Some(Match { line: 0, col: 7, len: 4 }));
    }

    #[test]
    fn multiple_hits_on_one_line() {
        let ls = vec!["ababab".to_string()];
        let mut s = Search::new();
        s.on_char('a', &ls);
        s.on_char('b', &ls);
        assert_eq!(s.counts(), (1, 3)); // "ab" at cols 0, 2, 4
    }

    #[test]
    fn step_wraps_both_ways() {
        let mut s = Search::new();
        for c in "file".chars() {
            s.on_char(c, &lines());
        }
        assert_eq!(s.step(1).map(|m| m.line), Some(1)); // 0 -> 1
        assert_eq!(s.step(1).map(|m| m.line), Some(0)); // wrap 1 -> 0
        assert_eq!(s.step(-1).map(|m| m.line), Some(1)); // wrap back 0 -> 1
    }

    #[test]
    fn hit_flags_current_match() {
        let mut s = Search::new();
        for c in "file".chars() {
            s.on_char(c, &lines());
        }
        // Current is the first match (line 0). Its cells are the "bright" hit.
        assert_eq!(s.hit(0, 7), Some(true));
        assert_eq!(s.hit(0, 10), Some(true));
        assert_eq!(s.hit(0, 11), None); // just past "file"
        assert_eq!(s.hit(1, 10), Some(false)); // a match, but not current
    }

    #[test]
    fn empty_query_has_no_matches() {
        let mut s = Search::new();
        s.on_char('x', &lines());
        s.on_backspace(&lines());
        assert_eq!(s.counts(), (0, 0));
        assert_eq!(s.current_match(), None);
    }
}

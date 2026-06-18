//! Command blocks: turning the byte stream into structured records of
//! "what ran and what it produced", driven by OSC 133 semantic-prompt marks.
//!
//! Lifecycle the shell emits (see shell-integration.zsh):
//!   OSC 133;A  prompt start
//!   OSC 133;B  command start (cursor is where you type) — we mark the position
//!   OSC 133;C  output start  — we read the command from the grid, open a block
//!   OSC 133;D;<code>  command end — we close the block with its exit code
//!
//! Finished blocks are appended to a JSONL history file for later search.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::grid::Grid;

/// Cap output captured per block so a `yes`-style flood can't grow unbounded.
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub cwd: Option<String>,
    pub started_at_ms: u64,
}

/// A block being accumulated between OSC 133;C and ;D.
struct OpenBlock {
    command: String,
    output: String,
    cwd: Option<String>,
    started_at_ms: u64,
    truncated: bool,
}

pub struct BlockTracker {
    /// Cursor position recorded at the `B` mark, used to read the command text.
    command_mark: Option<(usize, usize)>,
    /// Latest cwd reported via OSC 7.
    cwd: Option<String>,
    open: Option<OpenBlock>,
    /// Blocks finished this session (kept for the future recall UI).
    history: Vec<Block>,
    /// Append-only persistence; `None` in tests / if the file can't be opened.
    store: Option<File>,
}

impl BlockTracker {
    /// Open (or create) the shared history file under the XDG data dir.
    pub fn new() -> Self {
        let store = history_path().and_then(|path| {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            OpenOptions::new().create(true).append(true).open(path).ok()
        });
        Self {
            command_mark: None,
            cwd: None,
            open: None,
            history: Vec::new(),
            store,
        }
    }

    /// A tracker that never persists — for unit tests.
    pub fn new_in_memory() -> Self {
        Self {
            command_mark: None,
            cwd: None,
            open: None,
            history: Vec::new(),
            store: None,
        }
    }

    pub fn history(&self) -> &[Block] {
        &self.history
    }

    pub fn last(&self) -> Option<&Block> {
        self.history.last()
    }

    // --- OSC 133 marks ---------------------------------------------------

    pub fn prompt_start(&mut self) {
        self.command_mark = None;
    }

    /// `B`: remember where the typed command will begin.
    pub fn command_start(&mut self, row: usize, col: usize) {
        self.command_mark = Some((row, col));
    }

    /// `C`: read the command text from `grid` (between the B mark and the
    /// cursor) and open a block to collect output.
    pub fn output_start(&mut self, grid: &Grid) {
        let command = match self.command_mark.take() {
            Some(mark) => grid.text_between(mark, (grid.cursor_row, grid.cursor_col)),
            None => String::new(),
        };
        self.open = Some(OpenBlock {
            command,
            output: String::new(),
            cwd: self.cwd.clone(),
            started_at_ms: now_ms(),
            truncated: false,
        });
    }

    /// `D;<code>`: close the open block and persist it.
    pub fn command_end(&mut self, exit_code: Option<i32>) {
        let Some(open) = self.open.take() else { return };
        let block = Block {
            command: open.command.trim().to_string(),
            output: open.output,
            exit_code,
            cwd: open.cwd,
            started_at_ms: open.started_at_ms,
        };
        if let Some(file) = &mut self.store {
            if let Ok(line) = serde_json::to_string(&block) {
                let _ = writeln!(file, "{line}");
            }
        }
        self.history.push(block);
    }

    /// A printed char arrived; if a block is open, capture it as output.
    pub fn feed_output(&mut self, c: char) {
        if let Some(open) = &mut self.open {
            if open.output.len() < MAX_OUTPUT {
                open.output.push(c);
            } else {
                open.truncated = true;
            }
        }
    }

    /// OSC 7: `file://host/path` — record the working directory.
    pub fn set_cwd_from_uri(&mut self, uri: &[u8]) {
        if let Ok(s) = std::str::from_utf8(uri) {
            // Strip the scheme + host, keep the path.
            let path = s
                .strip_prefix("file://")
                .map(|rest| rest.splitn(2, '/').nth(1).map(|p| format!("/{p}")))
                .flatten();
            self.cwd = path.or_else(|| Some(s.to_string()));
        }
    }
}

/// Load every block from the on-disk history file (oldest first). Malformed
/// lines are skipped. Returns an empty vec if there's no history yet.
pub fn load_history() -> Vec<Block> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Block>(line).ok())
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn history_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))?;
    Some(base.join("terminal").join("history.jsonl"))
}

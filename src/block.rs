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

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
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
    /// Blocks finished this session (kept for the recall UI).
    history: Vec<Block>,
    /// Blocks finished since the last `drain_completed` (for event emission).
    completed: Vec<Block>,
    /// Path of the history file; `None` in tests. We open it per-write (rather
    /// than holding a handle) so deleting the file mid-session just recreates it.
    store: Option<PathBuf>,
}

impl BlockTracker {
    /// Prepare the shared history file path under the XDG data dir.
    pub fn new() -> Self {
        let store = history_path();
        if let Some(path) = &store {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
        }
        Self {
            command_mark: None,
            cwd: None,
            open: None,
            history: Vec::new(),
            completed: Vec::new(),
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
            completed: Vec::new(),
            store: None,
        }
    }

    pub fn history(&self) -> &[Block] {
        &self.history
    }

    /// Take the blocks finished since the last call (for event emission).
    pub fn drain_completed(&mut self) -> Vec<Block> {
        std::mem::take(&mut self.completed)
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

    /// `C` with no explicit command: read the command text from `grid` (between
    /// the B mark and the cursor). Fallback for integrations that don't send the
    /// command — fragile with themed prompts, hence the explicit path below.
    pub fn output_start(&mut self, grid: &Grid) {
        let command = match self.command_mark.take() {
            Some(mark) => grid.text_between(mark, (grid.cursor_row, grid.cursor_col)),
            None => String::new(),
        };
        self.open_block(command);
    }

    /// `C` with the exact command supplied by the shell (preferred — robust
    /// regardless of prompt theme).
    pub fn output_start_with_command(&mut self, command: String) {
        self.command_mark = None;
        self.open_block(command);
    }

    fn open_block(&mut self, command: String) {
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
        if let Some(path) = &self.store {
            if let Ok(line) = serde_json::to_string(&block) {
                // Open per-write so the file is recreated if it was deleted.
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
        self.completed.push(block.clone());
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

/// Decode a base64 OSC payload into a command string (the shell sends the exact
/// command base64-encoded so arbitrary characters survive the OSC channel).
pub fn decode_command(payload: &[u8]) -> Option<String> {
    String::from_utf8(base64_decode(payload)?).ok()
}

/// Minimal standard-base64 decoder (no external dependency).
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in input {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        buf = (buf << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
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
    Some(base.join("jetem").join("history.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_returns_each_completed_block_once() {
        let mut t = BlockTracker::new_in_memory();
        t.output_start_with_command("ls".into());
        t.command_end(Some(0));
        assert_eq!(t.drain_completed().len(), 1);
        // Drained once: empty on the second call.
        assert_eq!(t.drain_completed().len(), 0);
    }

    #[test]
    fn base64_decodes_command() {
        assert_eq!(decode_command(b"Z2l0IHB1c2g=").as_deref(), Some("git push"));
    }
}

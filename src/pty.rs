//! PTY plumbing: spawn a shell attached to the slave end of a pseudo-terminal
//! and hand back the master end's reader/writer so the rest of the app can talk
//! to the shell as if it were a real terminal.

use std::io::{Read, Write};
use std::path::PathBuf;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Our shell integration, embedded so it ships inside the binary.
const INTEGRATION: &str = include_str!("../shell-integration.zsh");

/// Owns the master end of a PTY and the spawned shell child.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Pty {
    /// Spawn `shell` (e.g. `/usr/bin/zsh`) on a fresh PTY sized `rows`x`cols`.
    pub fn spawn(shell: &str, rows: u16, cols: u16) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Launch the shell with stdio wired to the slave end.
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        // Auto-inject our shell integration (OSC 133) for zsh via ZDOTDIR.
        if let Some((zdotdir, user_zdotdir)) = zsh_integration(shell) {
            cmd.env("ZDOTDIR", zdotdir);
            cmd.env("USER_ZDOTDIR", user_zdotdir);
        }
        let child = pair.slave.spawn_command(cmd)?;

        // Drop the slave handle: once the child exits, the master read side will
        // see EOF instead of blocking forever on a still-open slave fd.
        drop(pair.slave);

        Ok(Self {
            master: pair.master,
            child,
        })
    }

    /// A blocking reader over the shell's output (clone — safe to move to a thread).
    pub fn reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Ok(self.master.try_clone_reader()?)
    }

    /// A writer into the shell's input (our keystrokes go here).
    pub fn writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        Ok(self.master.take_writer()?)
    }

    /// Tell the kernel the new grid size; the shell receives SIGWINCH so
    /// full-screen programs (vim, htop) re-layout. Used from M6 onward.
    pub fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Has the shell exited? Returns its exit status if so.
    pub fn try_wait(&mut self) -> anyhow::Result<Option<portable_pty::ExitStatus>> {
        Ok(self.child.try_wait()?)
    }
}

/// If `shell` is zsh, build a temp `ZDOTDIR` whose rc files source the user's
/// real config and then our integration, and return `(ZDOTDIR, USER_ZDOTDIR)`
/// to set in the child env. The temp `.zshrc` restores `ZDOTDIR` afterward so
/// the user's session behaves normally. Returns `None` for non-zsh shells or on
/// any filesystem error (the user can still `source` the snippet manually).
fn zsh_integration(shell: &str) -> Option<(PathBuf, String)> {
    let is_zsh = std::path::Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "zsh");
    if !is_zsh {
        return None;
    }

    let user_zdotdir =
        std::env::var("ZDOTDIR").unwrap_or_else(|_| std::env::var("HOME").unwrap_or_default());

    let dir = std::env::temp_dir().join(format!("terminal-zdotdir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;

    let integration = dir.join("integration.zsh");
    std::fs::write(&integration, INTEGRATION).ok()?;

    // Forward the user's real rc files (looked up via $USER_ZDOTDIR), then our
    // integration, then restore ZDOTDIR for the rest of the session.
    let fwd = |name: &str| format!("[[ -f \"$USER_ZDOTDIR/{name}\" ]] && source \"$USER_ZDOTDIR/{name}\"\n");
    std::fs::write(dir.join(".zshenv"), fwd(".zshenv")).ok()?;
    std::fs::write(dir.join(".zprofile"), fwd(".zprofile")).ok()?;
    std::fs::write(
        dir.join(".zlogin"),
        format!("{}ZDOTDIR=\"$USER_ZDOTDIR\"\n", fwd(".zlogin")),
    )
    .ok()?;
    std::fs::write(
        dir.join(".zshrc"),
        format!(
            "{}source \"{}\"\nZDOTDIR=\"$USER_ZDOTDIR\"\n",
            fwd(".zshrc"),
            integration.display()
        ),
    )
    .ok()?;

    Some((dir, user_zdotdir))
}

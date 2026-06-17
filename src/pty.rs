//! PTY plumbing: spawn a shell attached to the slave end of a pseudo-terminal
//! and hand back the master end's reader/writer so the rest of the app can talk
//! to the shell as if it were a real terminal.

use std::io::{Read, Write};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

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

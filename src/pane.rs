//! A pane: one rectangular region of the window backed by a content source.
//!
//! Today the only content source is a terminal (a PTY + its `Screen` + a reader
//! thread). This is the `Surface` seam: later, other pane kinds (plugin-drawn
//! widgets, etc.) implement the same "occupy a rect, take input, resize" shape
//! without the compositor (M8b) needing to know the difference.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use vte::Parser;
use winit::event_loop::EventLoopProxy;

use crate::block::BlockTracker;
use crate::layout::PaneId;
use crate::parser::Performer;
use crate::pty::Pty;
use crate::screen::Screen;
use crate::window::UserEvent;

/// A pixel rectangle within the window framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Rect { x, y, w, h }
    }
}

/// A terminal occupying a rect: owns its PTY, its shared `Screen` (fed by a
/// background reader thread), and the writer for keystrokes.
pub struct TerminalPane {
    pty: Pty,
    screen: Arc<Mutex<Screen>>,
    /// Command blocks captured from this shell's OSC 133 marks.
    blocks: Arc<Mutex<BlockTracker>>,
    writer: Box<dyn Write + Send>,
    rect: Rect,
}

impl TerminalPane {
    /// Spawn `shell` sized to `rect` (in pixels, divided by the cell size). The
    /// reader thread wakes the event loop via `proxy` whenever output arrives.
    pub fn spawn(
        id: PaneId,
        shell: &str,
        rect: Rect,
        cell_w: usize,
        cell_h: usize,
        proxy: EventLoopProxy<UserEvent>,
    ) -> anyhow::Result<Self> {
        let cols = (rect.w / cell_w).max(1);
        let rows = (rect.h / cell_h).max(1);

        let pty = Pty::spawn(shell, rows as u16, cols as u16)?;
        let screen = Arc::new(Mutex::new(Screen::new(rows, cols)));
        let blocks = Arc::new(Mutex::new(BlockTracker::new()));

        let mut reader = pty.reader()?;
        {
            let screen = screen.clone();
            let blocks = blocks.clone();
            thread::spawn(move || {
                let mut vte = Parser::new();
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let finished = {
                                // Always lock screen before blocks (consistent order).
                                let mut s = screen.lock().unwrap();
                                let mut b = blocks.lock().unwrap();
                                let mut perf = Performer {
                                    screen: &mut s,
                                    blocks: &mut b,
                                };
                                vte.advance(&mut perf, &buf[..n]);
                                b.drain_completed()
                            };
                            for block in finished {
                                let _ = proxy.send_event(UserEvent::Block { pane: id, block });
                            }
                            let _ = proxy.send_event(UserEvent::Redraw);
                        }
                    }
                }
            });
        }

        let writer = pty.writer()?;
        Ok(Self {
            pty,
            screen,
            blocks,
            writer,
            rect,
        })
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn screen(&self) -> &Arc<Mutex<Screen>> {
        &self.screen
    }

    pub fn blocks(&self) -> &Arc<Mutex<BlockTracker>> {
        &self.blocks
    }

    /// Forward keystrokes to the shell.
    pub fn write_input(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Move/resize the pane: recompute rows/cols and tell both the grid model
    /// and the PTY (SIGWINCH) about the new size.
    pub fn resize_to(&mut self, rect: Rect, cell_w: usize, cell_h: usize) {
        self.rect = rect;
        let cols = (rect.w / cell_w).max(1);
        let rows = (rect.h / cell_h).max(1);
        {
            let mut s = self.screen.lock().unwrap();
            if s.rows() != rows || s.cols() != cols {
                s.resize(rows, cols);
            }
        }
        let _ = self.pty.resize(rows as u16, cols as u16);
    }
}

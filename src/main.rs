//! M4 — Interactive terminal.
//!
//! The shell's output is parsed into a shared [`Grid`] by a background reader
//! thread, and a winit event loop paints that grid into a real window via a
//! software framebuffer (softbuffer) with glyphs we rasterize ourselves
//! (fontdue). Keyboard input in the window is encoded to bytes and written back
//! to the PTY, so it's a real, usable terminal (fixed 80×24 until M6).

mod cell;
mod font;
mod grid;
mod parser;
mod pty;
mod render;
mod window;

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;

use grid::Grid;
use parser::Performer;
use pty::Pty;
use vte::Parser;
use winit::event_loop::EventLoop;
use window::{App, UserEvent};

const ROWS: u16 = 24;
const COLS: u16 = 80;
const FONT_PATH: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";
const FONT_PX: f32 = 16.0;

fn main() -> anyhow::Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let pty = Pty::spawn(&shell, ROWS, COLS)?;

    let grid = Arc::new(Mutex::new(Grid::new(ROWS as usize, COLS as usize)));
    let font = font::Font::load(FONT_PATH, FONT_PX)?;

    // The event loop carries our custom `UserEvent` so the reader thread can
    // wake it to repaint.
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    // Reader thread: shell output -> parse into the shared grid -> wake the UI.
    let mut reader = pty.reader()?;
    {
        let grid = grid.clone();
        thread::spawn(move || {
            let mut vte = Parser::new();
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        {
                            let mut g = grid.lock().unwrap();
                            let mut perf = Performer { grid: &mut g };
                            vte.advance(&mut perf, &buf[..n]);
                        }
                        // Ignore the error if the window has already closed.
                        let _ = proxy.send_event(UserEvent::Redraw);
                    }
                }
            }
        });
    }

    let writer = pty.writer()?;
    let mut app = App::new(grid, font, pty, writer);
    event_loop.run_app(&mut app)?;
    Ok(())
}

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
mod pane;
mod parser;
mod pty;
mod render;
mod screen;
mod window;

use pane::{Rect, TerminalPane};
use winit::event_loop::EventLoop;
use window::{App, UserEvent};

const ROWS: u16 = 24;
const COLS: u16 = 80;
const FONT_PATH: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";
const FONT_PX: f32 = 16.0;

fn main() -> anyhow::Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let font = font::Font::load(FONT_PATH, FONT_PX)?;

    // The event loop carries our custom `UserEvent` so pane reader threads can
    // wake it to repaint.
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    // One full-window terminal pane to start; M8b adds splits.
    let rect = Rect::new(0, 0, COLS as usize * font.cell_w, ROWS as usize * font.cell_h);
    let pane = TerminalPane::spawn(&shell, rect, font.cell_w, font.cell_h, proxy)?;

    let mut app = App::new(pane, font);
    event_loop.run_app(&mut app)?;
    Ok(())
}

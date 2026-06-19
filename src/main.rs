//! M4 — Interactive terminal.
//!
//! The shell's output is parsed into a shared [`Grid`] by a background reader
//! thread, and a winit event loop paints that grid into a real window via a
//! software framebuffer (softbuffer) with glyphs we rasterize ourselves
//! (fontdue). Keyboard input in the window is encoded to bytes and written back
//! to the PTY, so it's a real, usable terminal (fixed 80×24 until M6).

mod block;
mod cell;
mod config;
mod font;
mod grid;
mod layout;
mod pane;
mod parser;
mod plugin;
mod pty;
mod recall;
mod render;
mod screen;
mod selection;
mod window;

use std::collections::HashMap;

use pane::Rect;
use plugin::Plugin;
use winit::event_loop::EventLoop;
use window::{App, UserEvent};

const ROWS: u16 = 24;
const COLS: u16 = 80;
const FONT_PATH: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";
const FONT_PX: f32 = 16.0;

fn main() -> anyhow::Result<()> {
    // Log panics (incl. from the main event loop) to a file for debugging.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/terminal-panic.log")
        {
            let _ = writeln!(f, "{info}");
        }
        default_hook(info);
    }));

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let font = font::Font::load(FONT_PATH, FONT_PX)?;

    // The event loop carries our custom `UserEvent` so pane reader threads can
    // wake it to repaint.
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    // Spawn configured plugins (explicit opt-in via ~/.config/terminal/plugins.toml).
    let mut plugins = HashMap::new();
    for (id, pc) in config::load().plugin.iter().enumerate() {
        if let Some(p) = Plugin::spawn(id, &pc.command, proxy.clone()) {
            plugins.insert(id, p);
        } else {
            eprintln!("[terminal] failed to spawn plugin: {}", pc.command);
        }
    }

    // Start with one full-window pane; Ctrl-A splits create more.
    let rect = Rect::new(0, 0, COLS as usize * font.cell_w, ROWS as usize * font.cell_h);
    let mut app = App::new(font, proxy, shell, rect, plugins)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}

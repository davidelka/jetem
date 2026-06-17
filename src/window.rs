//! The windowing layer: a winit `ApplicationHandler` that owns the window and
//! softbuffer surface, and repaints the shared grid whenever the reader thread
//! signals new output (via a `UserEvent::Redraw` woken through the event loop).

use std::io::Write;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::font::Font;
use crate::grid::Grid;
use crate::render;

/// Wakes the event loop from the reader thread: "the grid changed, repaint."
#[derive(Debug)]
pub enum UserEvent {
    Redraw,
}

pub struct App {
    grid: Arc<Mutex<Grid>>,
    font: Font,
    /// Our keystrokes go here -> PTY -> shell.
    writer: Box<dyn Write + Send>,
    /// Latest modifier state (Ctrl/Shift/…), updated on `ModifiersChanged`.
    mods: ModifiersState,
    /// Pixel size of the window, derived from the grid + font cell size.
    win_w: u32,
    win_h: u32,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    // Kept alive for as long as the surface uses it.
    _context: Option<softbuffer::Context<Arc<Window>>>,
}

impl App {
    pub fn new(grid: Arc<Mutex<Grid>>, font: Font, writer: Box<dyn Write + Send>) -> Self {
        let (rows, cols) = {
            let g = grid.lock().unwrap();
            (g.rows, g.cols)
        };
        let win_w = (cols * font.cell_w) as u32;
        let win_h = (rows * font.cell_h) as u32;
        Self {
            grid,
            font,
            writer,
            mods: ModifiersState::empty(),
            win_w,
            win_h,
            window: None,
            surface: None,
            _context: None,
        }
    }

    fn redraw(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        surface
            .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
            .unwrap();

        let mut buffer = surface.buffer_mut().unwrap();
        {
            let grid = self.grid.lock().unwrap();
            render::paint(&mut buffer, w as usize, h as usize, &grid, &mut self.font);
        }
        buffer.present().unwrap();
    }
}

impl ApplicationHandler<UserEvent> for App {
    /// Called once the platform is ready: create the window + surface.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // already created (resumed can fire more than once)
        }
        let attrs = Window::default_attributes()
            .with_title("terminal")
            .with_inner_size(PhysicalSize::new(self.win_w, self.win_h))
            .with_resizable(false); // fixed size until M6 handles reflow

        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let context = softbuffer::Context::new(window.clone()).unwrap();
        let surface = softbuffer::Surface::new(&context, window.clone()).unwrap();

        self.window = Some(window);
        self._context = Some(context);
        self.surface = Some(surface);
        self.redraw();
    }

    /// The reader thread signalled new output — ask the window to repaint.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as isize,
                    MouseScrollDelta::PixelDelta(p) => (p.y / self.font.cell_h as f64) as isize,
                };
                if lines != 0 {
                    self.grid.lock().unwrap().scroll_view(lines);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                // Shift+PageUp/Down scroll the viewport locally — they are a
                // terminal feature, not bytes for the shell.
                if self.mods.shift_key() {
                    if let Key::Named(key @ (NamedKey::PageUp | NamedKey::PageDown)) =
                        event.logical_key
                    {
                        {
                            let mut g = self.grid.lock().unwrap();
                            let page = g.rows as isize - 1;
                            g.scroll_view(if key == NamedKey::PageUp { page } else { -page });
                        }
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                }
                // Any other key snaps back to the live screen, then is sent on.
                // The shell echoes printable chars back, so we never echo locally.
                self.grid.lock().unwrap().reset_view();
                if let Some(bytes) = encode_key(&event, self.mods) {
                    let _ = self.writer.write_all(&bytes);
                    let _ = self.writer.flush();
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// Translate a key press into the bytes a terminal sends to the shell.
fn encode_key(event: &KeyEvent, mods: ModifiersState) -> Option<Vec<u8>> {
    // Ctrl + letter -> control code (Ctrl+C = 0x03, Ctrl+D = 0x04, …).
    if mods.control_key() {
        if let Key::Character(s) = &event.logical_key {
            let c = s.chars().next()?.to_ascii_lowercase();
            if c.is_ascii_alphabetic() {
                return Some(vec![(c as u8) & 0x1f]);
            }
        }
    }

    match &event.logical_key {
        Key::Named(named) => match named {
            NamedKey::Enter => Some(vec![b'\r']), // terminals send CR, not LF
            NamedKey::Backspace => Some(vec![0x7f]), // DEL, what readline expects
            NamedKey::Tab => Some(vec![b'\t']),
            NamedKey::Escape => Some(vec![0x1b]),
            NamedKey::Space => Some(vec![b' ']),
            NamedKey::ArrowUp => Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => Some(b"\x1b[D".to_vec()),
            NamedKey::Home => Some(b"\x1b[H".to_vec()),
            NamedKey::End => Some(b"\x1b[F".to_vec()),
            _ => None,
        },
        // Printable text (already layout/shift-resolved by winit).
        Key::Character(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

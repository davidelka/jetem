//! The windowing layer: a winit `ApplicationHandler` that owns the window and
//! softbuffer surface, and repaints the shared grid whenever the reader thread
//! signals new output (via a `UserEvent::Redraw` woken through the event loop).

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::font::Font;
use crate::layout::{Layout, PaneId, SplitDir};
use crate::pane::{Rect, TerminalPane};
use crate::render;

/// Wakes the event loop from a pane reader thread: "output changed, repaint."
#[derive(Debug)]
pub enum UserEvent {
    Redraw,
}

/// Pixels of divider left between panes, and the focus-border thickness.
const GAP: usize = 2;
const BORDER: usize = 1;

#[derive(Clone, Copy)]
enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

pub struct App {
    panes: HashMap<PaneId, TerminalPane>,
    layout: Layout,
    focused: PaneId,
    next_id: PaneId,
    /// Cloned per new pane so its reader thread can wake the loop.
    proxy: EventLoopProxy<UserEvent>,
    shell: String,
    font: Font,
    mods: ModifiersState,
    /// True after the Ctrl-A prefix, until the next (command) key.
    pending_prefix: bool,
    win_w: u32,
    win_h: u32,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    _context: Option<softbuffer::Context<Arc<Window>>>,
}

impl App {
    pub fn new(
        font: Font,
        proxy: EventLoopProxy<UserEvent>,
        shell: String,
        initial: Rect,
    ) -> anyhow::Result<Self> {
        let first =
            TerminalPane::spawn(&shell, initial, font.cell_w, font.cell_h, proxy.clone())?;
        let id: PaneId = 0;
        let mut panes = HashMap::new();
        panes.insert(id, first);
        Ok(Self {
            panes,
            layout: Layout::Leaf(id),
            focused: id,
            next_id: 1,
            proxy,
            shell,
            font,
            mods: ModifiersState::empty(),
            pending_prefix: false,
            win_w: initial.w as u32,
            win_h: initial.h as u32,
            window: None,
            surface: None,
            _context: None,
        })
    }

    /// Recompute every pane's rect from the layout and resize each (grid + PTY).
    fn relayout(&mut self) {
        let area = Rect::new(0, 0, self.win_w as usize, self.win_h as usize);
        let mut rects = Vec::new();
        self.layout.compute_rects(area, GAP, &mut rects);
        for (id, r) in rects {
            if let Some(p) = self.panes.get_mut(&id) {
                p.resize_to(r, self.font.cell_w, self.font.cell_h);
            }
        }
    }

    /// Split the focused pane, spawning a fresh shell in the new half.
    fn split(&mut self, dir: SplitDir) {
        let new_id = self.next_id;
        let tmp = Rect::new(0, 0, self.win_w as usize, self.win_h as usize);
        let pane = match TerminalPane::spawn(
            &self.shell,
            tmp,
            self.font.cell_w,
            self.font.cell_h,
            self.proxy.clone(),
        ) {
            Ok(p) => p,
            Err(_) => return,
        };
        self.next_id += 1;
        self.panes.insert(new_id, pane);
        let layout = std::mem::replace(&mut self.layout, Layout::Leaf(new_id));
        self.layout = layout.split(self.focused, dir, new_id);
        self.focused = new_id;
        self.relayout();
    }

    /// Close the focused pane (dropping it hangs up its shell). Exits the app
    /// when the last pane closes.
    fn close_focused(&mut self, event_loop: &ActiveEventLoop) {
        let layout = std::mem::replace(&mut self.layout, Layout::Leaf(self.focused));
        match layout.remove(self.focused) {
            None => {
                event_loop.exit();
                return;
            }
            Some(l) => self.layout = l,
        }
        self.panes.remove(&self.focused);
        // Focus the first remaining leaf.
        let mut rects = Vec::new();
        let area = Rect::new(0, 0, self.win_w as usize, self.win_h as usize);
        self.layout.compute_rects(area, GAP, &mut rects);
        if let Some((id, _)) = rects.first() {
            self.focused = *id;
        }
        self.relayout();
    }

    /// Move focus to the nearest pane in `dir` (by rect center distance).
    fn focus_dir(&mut self, dir: FocusDir) {
        let cur = match self.panes.get(&self.focused) {
            Some(p) => p.rect(),
            None => return,
        };
        let (cx, cy) = (cur.x + cur.w / 2, cur.y + cur.h / 2);
        let mut best = None;
        let mut best_d = usize::MAX;
        for (id, p) in &self.panes {
            if *id == self.focused {
                continue;
            }
            let r = p.rect();
            let (px, py) = (r.x + r.w / 2, r.y + r.h / 2);
            let ok = match dir {
                FocusDir::Left => px < cx,
                FocusDir::Right => px > cx,
                FocusDir::Up => py < cy,
                FocusDir::Down => py > cy,
            };
            if !ok {
                continue;
            }
            let d = cx.abs_diff(px).pow(2) + cy.abs_diff(py).pow(2);
            if d < best_d {
                best_d = d;
                best = Some(*id);
            }
        }
        if let Some(id) = best {
            self.focused = id;
        }
    }

    /// Handle the key pressed right after the Ctrl-A prefix.
    fn handle_prefix_command(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        match &event.logical_key {
            Key::Character(s) => match s.as_str() {
                "|" | "v" => self.split(SplitDir::LeftRight),
                "-" | "s" => self.split(SplitDir::TopBottom),
                "x" => self.close_focused(event_loop),
                "h" => self.focus_dir(FocusDir::Left),
                "l" => self.focus_dir(FocusDir::Right),
                "k" => self.focus_dir(FocusDir::Up),
                "j" => self.focus_dir(FocusDir::Down),
                // Ctrl-A then a -> send a literal Ctrl-A to the shell.
                "a" => {
                    if let Some(p) = self.panes.get_mut(&self.focused) {
                        p.write_input(&[0x01]);
                    }
                }
                _ => {}
            },
            Key::Named(n) => match n {
                NamedKey::ArrowLeft => self.focus_dir(FocusDir::Left),
                NamedKey::ArrowRight => self.focus_dir(FocusDir::Right),
                NamedKey::ArrowUp => self.focus_dir(FocusDir::Up),
                NamedKey::ArrowDown => self.focus_dir(FocusDir::Down),
                _ => {}
            },
            _ => {}
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
        buffer.fill(render::DIVIDER); // gaps between panes show through
        for (id, pane) in &self.panes {
            let rect = pane.rect();
            let focused = *id == self.focused;
            let screen = pane.screen().lock().unwrap();
            render::paint(
                &mut buffer,
                w as usize,
                h as usize,
                rect,
                screen.active(),
                &mut self.font,
                focused,
            );
        }
        if self.panes.len() > 1 {
            if let Some(p) = self.panes.get(&self.focused) {
                let rect = p.rect();
                render::draw_border(&mut buffer, w as usize, h as usize, rect, render::FOCUS_BORDER, BORDER);
            }
        }
        buffer.present().unwrap();
    }

    /// Window resized: relayout all panes (each resizes its grid + PTY).
    fn on_resize(&mut self, px_w: u32, px_h: u32) {
        if px_w == 0 || px_h == 0 {
            return; // ignore minimize / degenerate sizes
        }
        self.win_w = px_w;
        self.win_h = px_h;
        self.relayout();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
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
            .with_inner_size(PhysicalSize::new(self.win_w, self.win_h));

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
            WindowEvent::Resized(size) => self.on_resize(size.width, size.height),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as isize,
                    MouseScrollDelta::PixelDelta(p) => (p.y / self.font.cell_h as f64) as isize,
                };
                if lines != 0 {
                    if let Some(p) = self.panes.get(&self.focused) {
                        p.screen().lock().unwrap().active_mut().scroll_view(lines);
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                // The key right after Ctrl-A is a multiplexer command. Ignore
                // bare modifier presses (e.g. the Shift needed to type `|`) so
                // they don't consume the prefix before the real command key.
                if self.pending_prefix {
                    if is_modifier_key(&event) {
                        return;
                    }
                    self.pending_prefix = false;
                    self.handle_prefix_command(event_loop, &event);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if is_prefix(&event, self.mods) {
                    self.pending_prefix = true;
                    return; // swallow the prefix itself
                }
                // Shift+PageUp/Down scroll the focused pane's viewport locally.
                if self.mods.shift_key() {
                    if let Key::Named(key @ (NamedKey::PageUp | NamedKey::PageDown)) =
                        event.logical_key
                    {
                        if let Some(p) = self.panes.get(&self.focused) {
                            let mut s = p.screen().lock().unwrap();
                            let page = s.rows() as isize - 1;
                            s.active_mut()
                                .scroll_view(if key == NamedKey::PageUp { page } else { -page });
                        }
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                }
                // Any other key snaps the focused pane to the bottom and is sent
                // on. The shell echoes printable chars back, so no local echo.
                if let Some(p) = self.panes.get(&self.focused) {
                    p.screen().lock().unwrap().active_mut().reset_view();
                }
                if let Some(bytes) = encode_key(&event, self.mods) {
                    if let Some(p) = self.panes.get_mut(&self.focused) {
                        p.write_input(&bytes);
                    }
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

/// Ctrl-A is the multiplexer prefix (like tmux's Ctrl-B).
fn is_prefix(event: &KeyEvent, mods: ModifiersState) -> bool {
    mods.control_key()
        && matches!(&event.logical_key, Key::Character(s) if s.eq_ignore_ascii_case("a"))
}

/// A bare modifier keypress (Shift/Ctrl/Alt/…), which must not consume a pending
/// prefix — we wait for the actual command key that the modifier produces.
fn is_modifier_key(event: &KeyEvent) -> bool {
    matches!(
        &event.logical_key,
        Key::Named(
            NamedKey::Shift
                | NamedKey::Control
                | NamedKey::Alt
                | NamedKey::Super
                | NamedKey::Meta
                | NamedKey::Hyper
        )
    )
}

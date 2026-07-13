//! The windowing layer: a winit `ApplicationHandler` that owns the window and
//! softbuffer surface, and repaints the shared grid whenever the reader thread
//! signals new output (via a `UserEvent::Redraw` woken through the event loop).

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::ansi;
use crate::block::Block;
use crate::font::Font;
use crate::layout::{Layout, PaneId, SplitDir};
use crate::pane::{Rect, TerminalPane};
use crate::panel::{TextPanel, TreeNode};
use crate::keys::{self, CoreAction, KeyConfig};
use crate::plugin::{Action, Plugin, PluginId, PluginInbound, Registry};
use crate::recall::Recall;
use crate::render;
use crate::screen::MouseTracking;
use crate::search::Search;
use crate::selection::Selection;
use crate::theme::Theme;

/// Events delivered to the winit loop from background threads.
#[derive(Debug)]
pub enum UserEvent {
    /// A pane's output changed — repaint.
    Redraw,
    /// A message arrived from plugin `id`.
    Plugin { id: PluginId, msg: PluginInbound },
    /// A command block finished in `pane` — emit a `command_end` event.
    Block { pane: PaneId, block: Block },
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
    /// Running plugins and what they've registered.
    plugins: HashMap<PluginId, Plugin>,
    registry: Registry,
    font: Font,
    /// The color theme (loaded once at startup).
    theme: Theme,
    mods: ModifiersState,
    /// True after the Ctrl-A prefix, until the next (command) key.
    pending_prefix: bool,
    /// The command-recall overlay, when open (captures input + drawn on top).
    overlay: Option<Recall>,
    /// Scrollback text search (`Ctrl-A /`), when active: captures input, tints
    /// matches in the focused pane, and shows a prompt bar. Searches `focused`.
    search: Option<Search>,
    /// A modal text panel (e.g. an AI answer), when open.
    panel: Option<TextPanel>,
    /// A transient toast message from `host/notify` and when it was shown.
    toast: Option<(String, Instant)>,
    /// Active text selection (mouse drag), if any.
    selection: Option<Selection>,
    /// True while the left button is held (extending a selection).
    selecting: bool,
    /// While a program has mouse tracking on, the button currently held down
    /// (0=left, 1=middle, 2=right) so we can report drag motion. `None` = no
    /// button pressed.
    mouse_held: Option<u8>,
    /// Latest mouse position in pixels.
    cursor_px: (f64, f64),
    /// System clipboard; `None` if it failed to initialize.
    clipboard: Option<arboard::Clipboard>,
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
        plugins: HashMap<PluginId, Plugin>,
        theme: Theme,
        keys: KeyConfig,
    ) -> anyhow::Result<Self> {
        let id: PaneId = 0;
        let first =
            TerminalPane::spawn(id, &shell, initial, font.cell_w, font.cell_h, proxy.clone())?;
        let mut panes = HashMap::new();
        panes.insert(id, first);
        // Kick off the handshake with every plugin.
        for p in plugins.values() {
            p.initialize();
        }
        Ok(Self {
            panes,
            layout: Layout::Leaf(id),
            focused: id,
            next_id: 1,
            proxy,
            shell,
            plugins,
            registry: Registry::new(&keys),
            font,
            theme,
            mods: ModifiersState::empty(),
            pending_prefix: false,
            overlay: None,
            search: None,
            panel: None,
            toast: None,
            selection: None,
            selecting: false,
            mouse_held: None,
            cursor_px: (0.0, 0.0),
            clipboard: arboard::Clipboard::new().ok(),
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
            new_id,
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

    /// Handle the key pressed right after the prefix. Both core actions (recall,
    /// search, literal) and plugin commands (mux split/focus/close, etc.) resolve
    /// through the one unified binding table — the user's `keys.toml` can rebind
    /// either.
    fn handle_prefix_command(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        let Some(chord) = keys::event_prefixed_chord(event) else {
            return;
        };
        self.dispatch_chord(&chord, event_loop);
    }

    /// Look a canonical chord up in the registry and run whatever it's bound to.
    /// Returns whether the chord was bound (so global-key handling can fall through).
    fn dispatch_chord(&mut self, chord: &str, event_loop: &ActiveEventLoop) -> bool {
        match self.registry.action_for_chord(chord).cloned() {
            Some(Action::Core(action)) => {
                self.dispatch_core_action(action, event_loop);
                true
            }
            Some(Action::Plugin { command, plugin }) => {
                if let Some(p) = self.plugins.get(&plugin) {
                    p.invoke(&command);
                }
                true
            }
            None => false,
        }
    }

    /// Run a built-in action. These stay in core because they touch in-process
    /// state (grid, blocks, clipboard) a plugin can't reach.
    fn dispatch_core_action(&mut self, action: CoreAction, _event_loop: &ActiveEventLoop) {
        match action {
            CoreAction::Recall => {
                let mut session = Vec::new();
                for p in self.panes.values() {
                    session.extend(p.blocks().lock().unwrap().history().iter().cloned());
                }
                self.overlay = Some(Recall::open(session));
            }
            CoreAction::Search => self.search = Some(Search::new()),
            CoreAction::LiteralPrefix => {
                if let Some(p) = self.panes.get_mut(&self.focused) {
                    p.write_input(&[ansi::CTRL_A]);
                }
            }
            CoreAction::Copy => self.copy_contextual(),
            CoreAction::Paste => self.paste(),
            CoreAction::ScrollUp | CoreAction::ScrollDown => {
                if let Some(p) = self.panes.get(&self.focused) {
                    let mut s = p.screen().lock().unwrap();
                    let page = s.rows() as isize - 1;
                    let delta = if matches!(action, CoreAction::ScrollUp) { page } else { -page };
                    s.active_mut().scroll_view(delta);
                }
            }
        }
    }

    /// Copy from whatever's focused: an open panel's selection, then the recall
    /// overlay's block output, else the mouse text selection.
    fn copy_contextual(&mut self) {
        if let Some(p) = &self.panel {
            let text = p.copy_text();
            self.set_clipboard(text);
        } else if let Some(o) = &self.overlay {
            if let Some(text) = o.selected_output() {
                self.set_clipboard(text);
            }
        } else {
            self.copy_selection();
        }
    }

    /// Route a key to the open recall overlay. Enter inserts the selected
    /// command into the focused pane (without running it); Esc closes.
    fn handle_overlay_key(&mut self, event: &KeyEvent) {
        let mut close = false;
        let mut insert = None;
        if let Some(o) = &mut self.overlay {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => close = true,
                Key::Named(NamedKey::Enter) => {
                    insert = o.selected_command();
                    close = true;
                }
                Key::Named(NamedKey::Backspace) => o.on_backspace(),
                Key::Named(NamedKey::ArrowUp) => o.move_sel(-1),
                Key::Named(NamedKey::ArrowDown) => o.move_sel(1),
                Key::Character(s) => {
                    for c in s.chars() {
                        o.on_char(c);
                    }
                }
                _ => {}
            }
        }
        if let Some(cmd) = insert {
            if let Some(p) = self.panes.get_mut(&self.focused) {
                p.write_input(cmd.as_bytes());
            }
        }
        if close {
            self.overlay = None;
        }
    }

    /// Route a key to the active scrollback search. Typing refilters and jumps to
    /// the nearest match; Enter/↓ and ↑ cycle matches; Esc closes (leaving the
    /// view where the last match is, `less`-style).
    fn handle_search_key(&mut self, event: &KeyEvent) {
        // The line list comes from the focused pane's grid; hold it briefly.
        let lines: Vec<String> = match self.panes.get(&self.focused) {
            Some(p) => p.screen().lock().unwrap().active().all_lines_text(),
            None => return,
        };
        let mut jump = None; // absolute line to scroll into view
        let mut close = false;
        if let Some(s) = &mut self.search {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => close = true,
                Key::Named(NamedKey::Enter) | Key::Named(NamedKey::ArrowDown) => {
                    jump = s.step(1).map(|m| m.line);
                }
                Key::Named(NamedKey::ArrowUp) => jump = s.step(-1).map(|m| m.line),
                Key::Named(NamedKey::Backspace) => {
                    s.on_backspace(&lines);
                    jump = s.current_match().map(|m| m.line);
                }
                Key::Character(ch) => {
                    for c in ch.chars() {
                        s.on_char(c, &lines);
                    }
                    jump = s.current_match().map(|m| m.line);
                }
                _ => {}
            }
        }
        if let Some(abs) = jump {
            if let Some(p) = self.panes.get(&self.focused) {
                p.screen().lock().unwrap().active_mut().scroll_to_line(abs);
            }
        }
        if close {
            self.search = None;
        }
    }

    /// Map a pixel position to (pane, row, col) of the pane under it.
    fn cell_at(&self, px: f64, py: f64) -> Option<(PaneId, usize, usize)> {
        for (id, p) in &self.panes {
            let r = p.rect();
            if px >= r.x as f64
                && px < (r.x + r.w) as f64
                && py >= r.y as f64
                && py < (r.y + r.h) as f64
            {
                let col = ((px - r.x as f64) / self.font.cell_w as f64) as usize;
                let row = ((py - r.y as f64) / self.font.cell_h as f64) as usize;
                let (rows, cols) = {
                    let s = p.screen().lock().unwrap();
                    (s.rows(), s.cols())
                };
                return Some((
                    *id,
                    row.min(rows.saturating_sub(1)),
                    col.min(cols.saturating_sub(1)),
                ));
            }
        }
        None
    }

    /// Put text on the system clipboard (no-op if empty or unavailable).
    fn set_clipboard(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if let Some(cb) = &mut self.clipboard {
            let _ = cb.set_text(text);
        }
    }

    /// Copy the current mouse selection to the system clipboard.
    fn copy_selection(&mut self) {
        let text = match &self.selection {
            Some(sel) => match self.panes.get(&sel.pane) {
                Some(p) => {
                    let s = p.screen().lock().unwrap();
                    sel.text(s.active())
                }
                None => return,
            },
            None => return,
        };
        self.set_clipboard(text);
    }

    /// Paste the clipboard into the focused pane. If the program has bracketed
    /// paste on (`?2004h` — set by vim, most shells' line editors, etc.), wrap the
    /// text in `ESC[200~ … ESC[201~` so it's treated as one inert block instead of
    /// typed keystrokes (no auto-run, no vim auto-indent cascade).
    fn paste(&mut self) {
        let text = match &mut self.clipboard {
            Some(cb) => cb.get_text().unwrap_or_default(),
            None => return,
        };
        if text.is_empty() {
            return;
        }
        let bracketed = self
            .panes
            .get(&self.focused)
            .map(|p| p.screen().lock().unwrap().modes().bracketed_paste)
            .unwrap_or(false);
        let bytes = wrap_paste(&text, bracketed);
        if let Some(p) = self.panes.get_mut(&self.focused) {
            p.write_input(&bytes);
        }
    }

    /// If a program in the pane under the pointer has mouse tracking on, encode
    /// this event and send it to that pane's PTY; return whether it was consumed.
    /// Holding **Shift** bypasses reporting so you can always select text locally
    /// (matching xterm). Motion is only reported at the level the program asked
    /// for: a drag needs ButtonEvent (`?1002`), free motion needs AnyEvent (`?1003`).
    fn report_mouse(&mut self, kind: MouseKind, button: u8, px: f64, py: f64) -> bool {
        if self.mods.shift_key() {
            return false;
        }
        let Some((pane, row, col)) = self.cell_at(px, py) else {
            return false;
        };
        let modes = match self.panes.get(&pane) {
            Some(p) => p.screen().lock().unwrap().modes(),
            None => return false,
        };
        if modes.mouse == MouseTracking::Off {
            return false;
        }
        if matches!(kind, MouseKind::Motion) {
            let ok = match modes.mouse {
                MouseTracking::AnyEvent => true,
                MouseTracking::ButtonEvent => button != 3, // only while dragging
                _ => false,
            };
            if !ok {
                return false;
            }
        }
        let bytes = encode_mouse(kind, button, col, row, modes.mouse_sgr, self.mods);
        if let Some(p) = self.panes.get_mut(&pane) {
            p.write_input(&bytes);
        }
        true
    }

    /// Handle a message from a plugin (runs on the main thread).
    fn handle_plugin_message(
        &mut self,
        event_loop: &ActiveEventLoop,
        id: PluginId,
        msg: PluginInbound,
    ) {
        match msg {
            PluginInbound::Manifest(m) => {
                if let Some(pv) = m.protocol_version {
                    if pv != crate::plugin::PROTOCOL_VERSION {
                        eprintln!(
                            "[jetem] plugin '{}' targets protocol v{pv}, host speaks v{} — may misbehave",
                            m.name,
                            crate::plugin::PROTOCOL_VERSION
                        );
                    }
                }
                // Record the plugin's name (used to prefix its host/log output).
                if let Some(p) = self.plugins.get_mut(&id) {
                    p.name = m.name.clone();
                }
                self.registry.apply_manifest(id, &m);
            }
            PluginInbound::HostAction {
                id: req_id,
                method,
                params,
            } => {
                // `host/getTheme` is a *query*: reply with the current theme as
                // JSON (rather than the usual {ok} acknowledgement) so a plugin can
                // read live colors — e.g. to compute the true opposite background.
                if method == "host/getTheme" {
                    let theme = serde_json::to_value(&self.theme).unwrap_or(Value::Null);
                    if let Some(p) = self.plugins.get(&id) {
                        p.reply_value(req_id, json!({ "theme": theme }));
                    }
                    return;
                }
                let ok = self.apply_host_action(event_loop, id, &method, &params);
                if let Some(p) = self.plugins.get(&id) {
                    p.reply(req_id, ok);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            PluginInbound::Closed => {
                self.plugins.remove(&id);
            }
        }
    }

    /// Send an event notification to every plugin subscribed to it.
    fn dispatch_event(&self, name: &str, params: Value) {
        if let Some(subs) = self.registry.events.get(name) {
            for pid in subs {
                if let Some(p) = self.plugins.get(pid) {
                    p.event(name, params.clone());
                }
            }
        }
    }

    /// Perform a `host/*` action requested by a plugin. Reuses existing actions.
    fn apply_host_action(
        &mut self,
        event_loop: &ActiveEventLoop,
        plugin_id: PluginId,
        method: &str,
        params: &serde_json::Value,
    ) -> bool {
        match method {
            "host/splitPane" => {
                let dir = match params.get("dir").and_then(|d| d.as_str()) {
                    Some("topbottom") => SplitDir::TopBottom,
                    _ => SplitDir::LeftRight,
                };
                self.split(dir);
                true
            }
            "host/focusPane" => {
                let dir = match params.get("dir").and_then(|d| d.as_str()) {
                    Some("left") => FocusDir::Left,
                    Some("right") => FocusDir::Right,
                    Some("up") => FocusDir::Up,
                    Some("down") => FocusDir::Down,
                    _ => return false,
                };
                self.focus_dir(dir);
                true
            }
            "host/closePane" => {
                self.close_focused(event_loop);
                true
            }
            "host/writeToFocusedPane" => {
                if let Some(text) = params.get("text").and_then(|t| t.as_str()) {
                    if let Some(p) = self.panes.get_mut(&self.focused) {
                        p.write_input(text.as_bytes());
                    }
                    return true;
                }
                false
            }
            "host/log" => {
                if let Some(text) = params.get("text").and_then(|t| t.as_str()) {
                    let level = params.get("level").and_then(|l| l.as_str()).unwrap_or("info");
                    let name = self
                        .plugins
                        .get(&plugin_id)
                        .map(|p| p.name.as_str())
                        .filter(|n| !n.is_empty())
                        .unwrap_or("plugin");
                    eprintln!("[{name}/{level}] {text}");
                    return true;
                }
                false
            }
            "host/notify" => {
                if let Some(text) = params.get("text").and_then(|t| t.as_str()) {
                    self.toast = Some((text.to_string(), Instant::now()));
                    return true;
                }
                false
            }
            "host/showPanel" => {
                let title = params
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let body = params.get("body").and_then(|b| b.as_str()).unwrap_or("");
                let interactive = params.get("input").and_then(|i| i.as_bool()).unwrap_or(false);
                let cols = self.win_w as usize / self.font.cell_w.max(1);
                self.panel = Some(TextPanel::new(title, body, cols, interactive, plugin_id));
                true
            }
            "host/showTable" => {
                let title = params
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let headers = json_str_row(params.get("headers"));
                let rows = params
                    .get("rows")
                    .and_then(|r| r.as_array())
                    .map(|a| a.iter().map(|row| json_str_row(Some(row))).collect())
                    .unwrap_or_default();
                let cols = self.win_w as usize / self.font.cell_w.max(1);
                self.panel = Some(TextPanel::new_table(title, headers, rows, cols, plugin_id));
                true
            }
            "host/showTree" => {
                let title = params
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut nodes = Vec::new();
                if let Some(tree) = params.get("tree") {
                    flatten_tree(tree, 0, &mut nodes);
                }
                let cols = self.win_w as usize / self.font.cell_w.max(1);
                self.panel = Some(TextPanel::new_tree(title, nodes, cols, plugin_id));
                true
            }
            "host/closePanel" => {
                self.panel = None;
                true
            }
            "host/setTheme" => {
                // Runtime, in-memory theme change (not persisted). A `preset` swaps
                // the whole theme; a `patch` deep-merges a partial onto the current
                // one. The event-loop requests a redraw after every host action, so
                // the new colors take effect on the next paint without a restart.
                if let Some(name) = params.get("preset").and_then(|p| p.as_str()) {
                    match Theme::preset(name) {
                        Some(t) => self.theme = t,
                        None => eprintln!("[host/setTheme] unknown preset {name:?}"),
                    }
                }
                if let Some(patch) = params.get("patch") {
                    self.theme = self.theme.patched(patch);
                }
                true
            }
            _ => false, // unknown action
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
        buffer.fill(self.theme.ui.divider.packed()); // gaps between panes show through
        for (id, pane) in &self.panes {
            let rect = pane.rect();
            let focused = *id == self.focused;
            let sel = self.selection.as_ref().filter(|s| s.pane == *id);
            // Search highlights only apply to the focused pane it's searching.
            let search = if focused { self.search.as_ref() } else { None };
            let screen = pane.screen().lock().unwrap();
            render::paint(
                &mut buffer,
                w as usize,
                h as usize,
                rect,
                screen.active(),
                &mut self.font,
                focused,
                sel,
                search,
                &self.theme,
            );
        }
        if self.panes.len() > 1 {
            if let Some(p) = self.panes.get(&self.focused) {
                let rect = p.rect();
                render::draw_border(&mut buffer, w as usize, h as usize, rect, self.theme.ui.focus_border.packed(), BORDER);
            }
        }
        // The recall overlay draws on top of everything.
        if let Some(overlay) = &self.overlay {
            overlay.draw(&mut buffer, w as usize, h as usize, &mut self.font, &self.theme);
        }
        // A modal text panel (e.g. an AI answer) draws above the overlay.
        if let Some(panel) = &self.panel {
            panel.draw(&mut buffer, w as usize, h as usize, &mut self.font, &self.theme);
        }
        // Scrollback-search prompt bar along the bottom: `/query   (2/7)`.
        if let Some(search) = &self.search {
            let (cw, ch) = (self.font.cell_w, self.font.cell_h.max(1));
            let y = (h as usize).saturating_sub(ch);
            let bar = Rect::new(0, y, w as usize, ch);
            render::fill(&mut buffer, w as usize, h as usize, bar, self.theme.panel.bg.rgb());
            let (cur, total) = search.counts();
            let count = if total == 0 { "(no matches)".to_string() } else { format!("({cur}/{total})") };
            let text = format!("/{}   {}", search.query(), count);
            render::draw_text(&mut buffer, w as usize, h as usize, &mut self.font, cw, y, &text, self.theme.search.prompt.rgb(), None);
        }
        // A transient toast (from host/notify) along the bottom for a few
        // seconds. Multi-line answers (e.g. from the AI plugin) render as a
        // stack of lines sized to fit. Suppressed while the search bar is up —
        // both anchor to the bottom, so a lingering toast would hide the search
        // prompt and make it look like input is stuck.
        if let Some((text, at)) = self.toast.as_ref().filter(|_| self.search.is_none()) {
            if at.elapsed().as_secs() < 10 {
                let (cw, ch) = (self.font.cell_w, self.font.cell_h);
                let lines: Vec<&str> = text.lines().collect();
                let n = lines.len().max(1);
                let bar_h = n * ch + 8;
                let y0 = (h as usize).saturating_sub(bar_h);
                let bar = Rect::new(0, y0, w as usize, bar_h);
                render::fill(&mut buffer, w as usize, h as usize, bar, (40, 44, 56));
                for (i, line) in lines.iter().enumerate() {
                    render::draw_text(
                        &mut buffer,
                        w as usize,
                        h as usize,
                        &mut self.font,
                        cw,
                        y0 + 4 + i * ch,
                        line,
                        (235, 235, 245),
                        None,
                    );
                }
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
            .with_title("jetem")
            .with_inner_size(PhysicalSize::new(self.win_w, self.win_h));

        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let context = softbuffer::Context::new(window.clone()).unwrap();
        let surface = softbuffer::Surface::new(&context, window.clone()).unwrap();

        self.window = Some(window);
        self._context = Some(context);
        self.surface = Some(surface);
        self.redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Redraw => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            UserEvent::Plugin { id, msg } => self.handle_plugin_message(event_loop, id, msg),
            UserEvent::Block { pane, block } => {
                let params = json!({
                    "pane": pane,
                    "command": block.command,
                    "exit_code": block.exit_code,
                    "cwd": block.cwd,
                    "output": block.output,
                });
                self.dispatch_event("command_end", params);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(size) => self.on_resize(size.width, size.height),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_px = (position.x, position.y);
                // If a program grabbed the mouse, report motion first: a drag (a
                // held button) at ButtonEvent level, or any motion at AnyEvent.
                // `3` is the "no button" code used when hovering under AnyEvent.
                let btn = self.mouse_held.unwrap_or(3);
                if self.report_mouse(MouseKind::Motion, btn, position.x, position.y) {
                    return;
                }
                if !self.selecting {
                    return;
                }
                // While a panel is open, the drag selects panel text.
                if self.panel.is_some() {
                    let (wp, hp, px, py) = (self.win_w as usize, self.win_h as usize, position.x, position.y);
                    let cell = self.panel.as_ref().and_then(|p| p.cell_at(px, py, wp, hp, &self.font));
                    if let (Some(pos), Some(p)) = (cell, self.panel.as_mut()) {
                        p.extend_select(pos);
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                    return;
                }
                if let (Some((pane, row, col)), Some(sel)) =
                    (self.cell_at(position.x, position.y), self.selection.as_mut())
                {
                    if pane == sel.pane {
                        sel.set_head((row, col));
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (px, py) = self.cursor_px;
                // Map to the wire button code; ignore back/forward/extra buttons.
                let code = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    _ => return,
                };
                let pressed = state == ElementState::Pressed;
                // A tracking program gets the click (unless Shift bypasses it).
                let kind = if pressed { MouseKind::Press } else { MouseKind::Release };
                if self.report_mouse(kind, code, px, py) {
                    self.mouse_held = if pressed { Some(code) } else { None };
                    return;
                }
                // Otherwise: local text selection, left button only (as before).
                if code == 0 {
                    match state {
                        ElementState::Pressed => {
                            self.selecting = true;
                            if self.panel.is_some() {
                                let (wp, hp) = (self.win_w as usize, self.win_h as usize);
                                let cell = self.panel.as_ref().and_then(|p| p.cell_at(px, py, wp, hp, &self.font));
                                if let (Some(pos), Some(p)) = (cell, self.panel.as_mut()) {
                                    p.begin_select(pos);
                                }
                            } else {
                                self.selection = self
                                    .cell_at(px, py)
                                    .map(|(pane, row, col)| Selection::new(pane, (row, col)));
                            }
                        }
                        ElementState::Released => self.selecting = false,
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (px, py) = self.cursor_px;
                let (dir, lines) = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y, (y * 3.0) as isize),
                    MouseScrollDelta::PixelDelta(p) => {
                        (p.y as f32, (p.y / self.font.cell_h as f64) as isize)
                    }
                };
                // A tracking program gets wheel-up (64) / wheel-down (65).
                if dir != 0.0 {
                    let kind = if dir > 0.0 { MouseKind::WheelUp } else { MouseKind::WheelDown };
                    if self.report_mouse(kind, 0, px, py) {
                        return;
                    }
                }
                // Otherwise scroll our local scrollback viewport (as before).
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
                // A bare modifier press (Ctrl/Shift/Alt/…) is never a command and
                // must not disturb an active selection — otherwise pressing Ctrl to
                // begin Ctrl-Shift-C would clear the marks before Shift+C arrives
                // (the fall-through below treats any other key as "typing").
                if is_modifier_key(&event) {
                    return;
                }
                // The canonical global chord for this key (e.g. "ctrl+shift+c").
                // Resolved once and reused for copy/paste, the prefix, and other
                // global bindings below.
                let gchord = keys::event_global_chord(&event, self.mods);
                // Copy/Paste run before the modal captures so they work even while
                // a panel or the recall overlay is open. Bound via the same table,
                // so `keys.toml` can remap them.
                if let Some(c) = &gchord {
                    if let Some(Action::Core(a)) = self.registry.action_for_chord(c).cloned() {
                        if matches!(a, CoreAction::Copy | CoreAction::Paste) {
                            self.dispatch_core_action(a, event_loop);
                            return;
                        }
                    }
                }
                // A modal panel captures all input while it's open.
                if self.panel.is_some() {
                    let mut close = false;
                    let mut submit: Option<(usize, String)> = None;
                    if let Some(panel) = &mut self.panel {
                        match &event.logical_key {
                            Key::Named(NamedKey::Escape) => close = true,
                            // Foldable-tree navigation (a tree panel is non-interactive).
                            Key::Named(NamedKey::ArrowUp) if panel.is_tree() => panel.tree_move(-1),
                            Key::Named(NamedKey::ArrowDown) if panel.is_tree() => panel.tree_move(1),
                            Key::Named(NamedKey::ArrowRight) if panel.is_tree() => panel.tree_set_collapsed(false),
                            Key::Named(NamedKey::ArrowLeft) if panel.is_tree() => panel.tree_set_collapsed(true),
                            Key::Named(NamedKey::Enter) if panel.is_tree() => panel.tree_toggle(),
                            Key::Named(NamedKey::Space) if panel.is_tree() => panel.tree_toggle(),
                            Key::Named(NamedKey::Enter) if panel.interactive => {
                                if let Some(text) = panel.take_input() {
                                    submit = Some((panel.owner, text));
                                }
                            }
                            Key::Named(NamedKey::Backspace) if panel.interactive => panel.on_backspace(),
                            Key::Named(NamedKey::Space) if panel.interactive => panel.on_char(' '),
                            Key::Named(NamedKey::ArrowUp) => panel.scroll(-1),
                            Key::Named(NamedKey::ArrowDown) => panel.scroll(1),
                            Key::Named(NamedKey::PageUp) => {
                                let d = panel.page();
                                panel.scroll(-d);
                            }
                            Key::Named(NamedKey::PageDown) => {
                                let d = panel.page();
                                panel.scroll(d);
                            }
                            Key::Character(s) => {
                                if panel.interactive {
                                    for c in s.chars() {
                                        panel.on_char(c);
                                    }
                                } else if s == "q" {
                                    close = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    // Send a submitted follow-up to the owning plugin.
                    if let Some((owner, text)) = submit {
                        if let Some(p) = self.plugins.get(&owner) {
                            p.event("panelInput", json!({ "text": text }));
                        }
                    }
                    if close {
                        self.panel = None;
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                // The recall overlay captures all input while it's open.
                if self.overlay.is_some() {
                    self.handle_overlay_key(&event);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                // Scrollback search captures input while active.
                if self.search.is_some() {
                    self.handle_search_key(&event);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
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
                if gchord.as_deref() == Some(self.registry.prefix.as_str()) {
                    self.pending_prefix = true;
                    return; // swallow the prefix itself
                }
                // Any other configured global chord (scrollback scroll, or a
                // plugin's global binding) fires here in normal mode.
                if let Some(c) = gchord {
                    if self.dispatch_chord(&c, event_loop) {
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                }
                // Any other key snaps the focused pane to the bottom and is sent
                // on. The shell echoes printable chars back, so no local echo.
                self.selection = None; // typing clears the highlight
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
            NamedKey::Enter => Some(vec![ansi::CR]),
            NamedKey::Backspace => Some(vec![ansi::BACKSPACE]),
            NamedKey::Tab => Some(vec![ansi::TAB]),
            NamedKey::Escape => Some(vec![ansi::ESC]),
            NamedKey::Space => Some(vec![ansi::SPACE]),
            NamedKey::ArrowUp => Some(ansi::CURSOR_UP.as_bytes().to_vec()),
            NamedKey::ArrowDown => Some(ansi::CURSOR_DOWN.as_bytes().to_vec()),
            NamedKey::ArrowRight => Some(ansi::CURSOR_RIGHT.as_bytes().to_vec()),
            NamedKey::ArrowLeft => Some(ansi::CURSOR_LEFT.as_bytes().to_vec()),
            NamedKey::Home => Some(ansi::HOME.as_bytes().to_vec()),
            NamedKey::End => Some(ansi::END.as_bytes().to_vec()),
            _ => None,
        },
        // Printable text (already layout/shift-resolved by winit).
        Key::Character(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

/// The kind of mouse event to report to a program (see [`encode_mouse`]).
#[derive(Clone, Copy)]
enum MouseKind {
    Press,
    Release,
    Motion,
    WheelUp,
    WheelDown,
}

/// Encode a mouse event as the escape sequence a tracking program expects.
///
/// Two wire formats: **SGR** (`?1006`, preferred) writes decimal coordinates —
/// `ESC[<Cb;Cx;Cy` then `M` for press/motion/wheel or `m` for release — so it
/// isn't limited to 223 columns. The **legacy X10** form packs each field into
/// one byte offset by 32 (`ESC[M <Cb+32><Cx+32><Cy+32>`), which is why old
/// terminals topped out at column 223. `Cb` is the button (0/1/2), OR-ed with
/// modifier bits (shift 4, alt 8, ctrl 16), plus 32 for motion; the wheel is
/// 64 (up) / 65 (down). `col`/`row` are 0-based here and sent 1-based.
fn encode_mouse(
    kind: MouseKind,
    button: u8,
    col: usize,
    row: usize,
    sgr: bool,
    mods: ModifiersState,
) -> Vec<u8> {
    let mut mod_bits: u32 = 0;
    if mods.shift_key() {
        mod_bits |= 4;
    }
    if mods.alt_key() {
        mod_bits |= 8;
    }
    if mods.control_key() {
        mod_bits |= 16;
    }
    let cb = match kind {
        MouseKind::Press | MouseKind::Release => button as u32,
        MouseKind::Motion => button as u32 + 32, // the "motion" bit
        MouseKind::WheelUp => 64,
        MouseKind::WheelDown => 65,
    } | mod_bits;
    let (x, y) = (col as u32 + 1, row as u32 + 1);

    if sgr {
        let final_ch = if matches!(kind, MouseKind::Release) { 'm' } else { 'M' };
        let mut v = ansi::MOUSE_SGR.as_bytes().to_vec();
        v.extend_from_slice(format!("{cb};{x};{y}{final_ch}").as_bytes());
        v
    } else {
        // Legacy can't say *which* button was released, so release is always 3.
        let cb = if matches!(kind, MouseKind::Release) { 3 | mod_bits } else { cb };
        let enc = |n: u32| (n + 32).min(255) as u8;
        let mut v = ansi::MOUSE_X10.as_bytes().to_vec();
        v.extend_from_slice(&[enc(cb), enc(x), enc(y)]);
        v
    }
}

/// Prepare clipboard text for the PTY. With bracketed paste on, wrap it in
/// `ESC[200~ … ESC[201~`; and strip any `ESC[201~` already inside the payload so
/// a crafted clipboard can't close the bracket early and inject running commands.
fn wrap_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let cleaned = text.replace(ansi::PASTE_END, "");
    let mut out = Vec::with_capacity(cleaned.len() + 12);
    out.extend_from_slice(ansi::PASTE_START.as_bytes());
    out.extend_from_slice(cleaned.as_bytes());
    out.extend_from_slice(ansi::PASTE_END.as_bytes());
    out
}

/// Flatten a nested `{label, children:[...]}` JSON tree (or a top-level array of
/// such nodes) into a pre-order `Vec<TreeNode>` with depths.
fn json_flatten_node(node: &Value, depth: usize, out: &mut Vec<TreeNode>) {
    if let Some(arr) = node.as_array() {
        for child in arr {
            json_flatten_node(child, depth, out);
        }
        return;
    }
    let label = node.get("label").and_then(|l| l.as_str()).unwrap_or("").to_string();
    let children = node.get("children").and_then(|c| c.as_array());
    let has_children = children.is_some_and(|c| !c.is_empty());
    out.push(TreeNode { depth, label, has_children, collapsed: false });
    if let Some(ch) = children {
        for c in ch {
            json_flatten_node(c, depth + 1, out);
        }
    }
}

/// Entry point used by `host/showTree` (named to read well at the call site).
fn flatten_tree(node: &Value, depth: usize, out: &mut Vec<TreeNode>) {
    json_flatten_node(node, depth, out);
}

/// Coerce a JSON array (the `headers`, or one `rows` entry) into `Vec<String>`:
/// strings pass through, numbers/bools stringify, null becomes empty.
fn json_str_row(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|c| match c {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_mouse_encoding() {
        let none = ModifiersState::empty();
        // Left press at col 0,row 0 -> button 0, coords 1-based, final 'M'.
        assert_eq!(encode_mouse(MouseKind::Press, 0, 0, 0, true, none), b"\x1b[<0;1;1M");
        // Release keeps the button code but ends in lowercase 'm'.
        assert_eq!(encode_mouse(MouseKind::Release, 0, 4, 2, true, none), b"\x1b[<0;5;3m");
        // Wheel up is button 64; wheel down 65.
        assert_eq!(encode_mouse(MouseKind::WheelUp, 0, 0, 0, true, none), b"\x1b[<64;1;1M");
        // Ctrl adds bit 16: left press becomes button 16.
        assert_eq!(
            encode_mouse(MouseKind::Press, 0, 0, 0, true, ModifiersState::CONTROL),
            b"\x1b[<16;1;1M"
        );
        // A drag (motion with a held button) sets the motion bit (+32).
        assert_eq!(encode_mouse(MouseKind::Motion, 0, 0, 0, true, none), b"\x1b[<32;1;1M");
    }

    #[test]
    fn legacy_mouse_encoding() {
        let none = ModifiersState::empty();
        // Each field is offset by 32: left press at (0,0) -> ESC[M <space><!><!>.
        assert_eq!(encode_mouse(MouseKind::Press, 0, 0, 0, false, none), b"\x1b[M \x21\x21");
        // Legacy release can't name the button -> code 3 (35 = '#').
        assert_eq!(encode_mouse(MouseKind::Release, 2, 0, 0, false, none), b"\x1b[M#\x21\x21");
    }

    #[test]
    fn bracketed_paste_wraps_and_sanitizes() {
        // Off: raw bytes through unchanged.
        assert_eq!(wrap_paste("ls\n", false), b"ls\n");
        // On: wrapped in the 200~/201~ brackets.
        assert_eq!(wrap_paste("ls", true), b"\x1b[200~ls\x1b[201~");
        // A payload that smuggles a closing bracket is stripped, so it can't end
        // the paste early and inject a runnable command.
        assert_eq!(
            wrap_paste("a\x1b[201~rm -rf /", true),
            b"\x1b[200~arm -rf /\x1b[201~"
        );
    }
}

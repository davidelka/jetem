# Project vision & roadmap

## Vision

A terminal emulator in Rust, built study-first, that is **correct, polished, and
daily-usable** — and goes **beyond** existing terminals by being **plugin-first**:
a thin, fast, safe core, where features (AI, history, rich output, panels, themes)
are implemented as plugins. Aimed at a coding + SSH/sysadmin workflow.

## The novel angle

Two reinforcing ideas:

1. **Commands & output as first-class objects** — the **command block** (command +
   output + exit code + timestamp + working dir), captured via **OSC 133** semantic-
   prompt sequences (+ a small zsh/bash hook). This unlocks searchable **time-travel
   history**, **AI** that can see structured blocks, and **rich/structured output**.
2. **Plugin-first** — the core exposes an event bus + registries; almost every feature
   is a plugin. The AI assistant is "just a plugin."

Priorities: correctness/fidelity, UX/features, novelty rank high. **Performance is the
lowest priority for now** (CPU framebuffer; GPU is a later optimization).

## Plugin capability surface (what a plugin can do)

1. **Panels** — own a window region; either run a real program (e.g. htop) *or*
   custom-draw their own content (dashboard, AI chat box).
2. **Commands + keybindings** — a key triggers a named action.
3. **Services** — long-lived providers (AI, history) others can call.
4. **Event hooks** — react to `command_start/end`, exit codes, block events
   (e.g. "command failed → refer to AI").
5. **Block renderers** — claim some output and render it richly (tables, images).
6. **Theming** — colors, fonts, custom glyphs/symbols.

## Architecture

```
CORE (thin, fast, safe):
  terminal engine: PTY · vte parser · grid · render · input · scrollback
  + panes/layout (multiplexing)  + block model (OSC 133)  + Theme
  + PLUGIN HOST = event bus + registries
      (panels / commands+keys / services / block-renderers / theme)

PLUGIN RUNTIME — tiered:
  out-of-process (MCP-style JSON-RPC over stdio)  ← PRIMARY, start here
      best for AI, history, services, integrations; isolated; language-agnostic
  in-process (WASM or Rhai)                        ← added when needed
      for custom-draw panels, glyph/theme providers, hot-path hooks
```

The event-bus + registries layer is runtime-independent; that's the real design work.

## Roadmap

### Foundation (classic terminal)
- [x] **M1** PTY plumbing
- [x] **M2** vte parser + grid model
- [x] **M3** window + software render
- [x] **M4** keyboard input
- [x] **M5** scrollback, cursor visibility, color polish
- [x] **M6** window resize (reflow + PTY `SIGWINCH`)
- [x] **M7** alternate screen (`?1049h/l`) so vim/tmux/less work
- [x] **M8** panes / layout (multiplexing) — Surface/compositor seam + Ctrl-A keys

### Novel core
- [x] **M9** command blocks + persistent searchable history (OSC 133 capture)
  - M9a: block capture + JSONL persistence; M9b: Ctrl-A r recall overlay
- [x] **M10** plugin host — registries + MCP-style out-of-process JSON-RPC
  - M10a: host core (spawn, initialize, command/invoke, host actions); example
    `hello.py`. M10b: event bus (`command_end`) + `host/notify` toast; `oops.py`.
  - **M10c dogfood (done):** multiplexing now lives in `examples/plugins/mux.py`
    — it registers the `Ctrl-A` split/focus/close chords and calls host actions.
    Core `window.rs` no longer hardcodes them (keeps only `r` recall + `a`
    literal Ctrl-A). Proves the host drives real UX; the compositor stays core.
  - Config: `~/.config/jetem/plugins.toml` lists plugins (opt-in).

### Plugins (dogfood the host)
- [x] AI assistant (`examples/plugins/ai.py`) — `Ctrl-A i` explains the last command via
  Claude (`claude-opus-4-8`), shown as a multi-line toast. Subscribes to `command_end`,
  invokes asynchronously. Two backends (`JETEM_AI_BACKEND`): **cli** (Claude
  subscription via the `claude` CLI, no key) or **api** (`anthropic` SDK + key).
- [~] **M11 Rich / structured output renderers** — *tables done*: `host/showTable` +
  `TextPanel` table mode (core primitive: aligned, zebra-striped, TSV copy), driven by
  `examples/plugins/richout.py` (`Ctrl-A t` — detects JSON or whitespace-aligned columns;
  detection/parsing is plugin policy). The plugin contract is now published
  (`docs/plugin-api.md`) with a Python SDK (`sdk/jetem_plugin.py`), so anyone can write
  a renderer without touching core. **Foldable JSON tree done** too: `host/showTree` +
  a navigable tree panel mode, with `richout.py` routing nested JSON to it (`Ctrl-A t`).
  Deferred: images (sixel/kitty), inline-in-scrollback rendering (a larger render-model change).
- [ ] Custom-draw panels / widgets (needs in-process tier)
- [x] **M13 Plugin-driven theming** — `host/setTheme` lets a plugin swap the whole
  theme by `preset` name or deep-merge a partial color `patch` onto the live theme
  (runtime only). Built-in presets `default`/`light`/`solarized-dark` (+ user files
  at `~/.config/jetem/themes/<name>.toml`); demoed by `examples/plugins/theme.py`
  (`Ctrl-A y` cycle, `Ctrl-A p` bg-flip). Custom symbols/glyphs still need the in-process tier.

### Cross-cutting / later
- [x] **M19 Config-driven keybindings** — a unified binding table (`plugin::Registry`:
  canonical chord → `Core(action)` | `Plugin{command}`) so `~/.config/jetem/keys.toml`
  remaps the prefix, the core actions, and plugin commands (by id). Precedence:
  core defaults → plugin manifest → user overrides. `keys.rs` owns parsing/`KeyConfig`.
  Also: **font fallback** (`font.rs`) so glyphs the primary lacks (e.g. Hebrew) render.
- [x] **M18 Scrollback text search** (`Ctrl-A /`) — incremental `less`/`vim`-style
  search over scrollback + live screen. Matches tint in place (current one brighter),
  the view auto-scrolls to the nearest, ↑/↓/Enter cycle, Esc closes. Pure `Search`
  (`src/search.rs`) + `Grid::all_lines_text`/`scroll_to_line`; highlight in `render::paint`;
  themable colors (`theme.search`). Core, like recall (reads in-process grid state).
- [x] **M15 Parser correctness — cursor/edit CSIs + text attributes.** Added the
  sequences shell line editors and colored prompts rely on: `G/`` ` ``/d` (CHA/HPA/VPA),
  `E/F` (CNL/CPL), `@/P/X` (ICH/DCH/ECH), and save/restore cursor (`ESC 7/8`, `CSI s/u`).
  Plus the rest of the SGR attribute set (2 dim, 5 blink, 8 conceal, 9 strike-through +
  resets) — dim/conceal/underline/strike-through now render; blink is parsed only.
- [x] **M16 Scroll regions** — `DECSTBM` margins + region-aware scrolling: `SU`/`SD`
  (`S`/`T`), `IL`/`DL` (`L`/`M`), `RI` (`ESC M`), and margin-respecting line feeds.
  A full-screen scroll still archives to scrollback; a partial region discards its
  scrolled-out lines. Completes the parser-correctness arc.
- [x] **M14 Mouse reporting + bracketed paste** — core input-side DEC modes. Mouse
  tracking `?1000/1002/1003` with SGR `?1006` (legacy X10 too) so vim/tmux/htop/less
  get clicks, drags, and wheel; **Shift** bypasses to keep local text selection.
  Bracketed paste `?2004` wraps pastes in `ESC[200~ … ESC[201~` (with `201~`
  sanitized). Modes live on `Screen`; encoding in `window::encode_mouse`/`wrap_paste`.
- [x] **M12 Extract a `Theme`** — all paint colors live in `src/theme.rs` (`Theme`),
  built-in default = the original look, overridable via `~/.config/jetem/theme.toml`
  (hex strings, partial override). Threaded through `render::paint`, the text panel,
  and the recall overlay. Plugin-driven theming (`host/setTheme`, named presets) landed
  in **M13**; `host/getTheme` (read the live theme — the first request/reply host action)
  in **M17**. Next here: a preset-picker UI and font/glyph providers (the in-process tier).
- Config file (TOML), copy/paste
- Performance: GPU rendering (wgpu), glyph atlas, damage tracking

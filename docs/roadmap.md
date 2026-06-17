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
- [ ] **M6** window resize (reflow + PTY `SIGWINCH`)
- [ ] **M7** alternate screen (`?1049h/l`) so vim/tmux/less work
- [ ] **M8** panes / layout (multiplexing) — enables panels-as-programs

### Novel core
- [ ] **M9** command blocks + persistent searchable history (OSC 133 capture)
- [ ] **M10** plugin host — event bus + registries + MCP-style out-of-process protocol

### Plugins (dogfood the host)
- [ ] AI assistant (explain failures, suggest commands, summarize) — via Claude
- [ ] Rich / structured output renderers (tables, images, foldable)
- [ ] Custom-draw panels / widgets (needs in-process tier)
- [ ] Themes & custom symbols

### Cross-cutting / later
- Extract a `Theme` (palette/font currently hardcoded in render.rs)
- Config file (TOML), copy/paste
- Performance: GPU rendering (wgpu), glyph atlas, damage tracking
